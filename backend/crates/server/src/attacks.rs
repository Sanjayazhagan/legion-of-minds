//! Matchmaking and raids. The attacker gets a defender's snapshot, fights it
//! on their own machine, and sends their army and placement. The server
//! re-runs the raid with the same code; its result is the one that counts.

use crate::auth::{AuthUser, SameRules, SameRulesJson};
use crate::error::{ApiError, ApiResult};
use crate::players::{load_user, public};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use bag_protocol::{
    loot, practice_gold, run_raid, shield_share, starter_layout, trophy_swing,
    AttackLog, AttackOutcome, AttackSubmit, BaseSnapshot, Match, Placement, PublicPlayer,
    RaidResult, Replay, User, KIND_PRACTICE, KIND_RAID, KIND_REVENGE, SIM_VERSION,
    League,
};
use sqlx::{Postgres, Transaction};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> ApiResult<T> {
    serde_json::from_str(s).map_err(ApiError::internal)
}

/// Owner of the computer-made practice bases (see migration 0007).
pub const CAMP: Uuid = Uuid::from_u128(1);

/// The Training Camp's base for a Town Hall level, made on first use.
async fn camp_snapshot(tx: &mut Transaction<'_, Postgres>, town_hall: u32) -> ApiResult<Uuid> {
    let have: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM snapshots WHERE user_id = $1 AND town_hall = $2 AND sim_version = $3 LIMIT 1",
    )
    .bind(CAMP)
    .bind(town_hall as i32)
    .bind(SIM_VERSION as i32)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = have {
        return Ok(id);
    }
    let snap = BaseSnapshot {
        base: starter_layout(town_hall),
        defenders: Vec::new(),
    };
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO snapshots (id, user_id, town_hall, data, sim_version) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(CAMP)
        .bind(town_hall as i32)
        .bind(serde_json::to_string(&snap).map_err(ApiError::internal)?)
        .bind(SIM_VERSION as i32)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}

/// Reserve `snapshot_id` for one raid by `me` and build the reply.
async fn open_raid(
    st: &AppState,
    mut tx: Transaction<'_, Postgres>,
    me: &User,
    defender_id: Uuid,
    snapshot_id: Uuid,
    kind: &str,
    revenge_of: Option<Uuid>,
) -> ApiResult<Json<Match>> {
    let attack_id = Uuid::new_v4();
    let expires: DateTime<Utc> =
        Utc::now() + chrono::Duration::from_std(st.cfg.attack_window).expect("fits");
    sqlx::query(
        "INSERT INTO attacks (id, attacker_id, defender_id, snapshot_id, status, expires_at, sim_version, kind, revenge_of)
         VALUES ($1, $2, $3, $4, 'pending', $5, $6, $7, $8)",
    )
    .bind(attack_id)
    .bind(me.id)
    .bind(defender_id)
    .bind(snapshot_id)
    .bind(expires)
    .bind(SIM_VERSION as i32)
    .bind(kind)
    .bind(revenge_of)
    .execute(&mut *tx)
    .await?;
    let data: String = sqlx::query_scalar("SELECT data FROM snapshots WHERE id = $1")
        .bind(snapshot_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let defender = load_user(&st.db, defender_id).await?;
    let max_loot = if kind == KIND_PRACTICE {
        practice_gold(st.cfg.practice_gold, me.upgrades.town_hall, 1.0)
    } else {
        loot(defender.gold, 1.0)
    };
    Ok(Json(Match {
        attack_id,
        sim_version: SIM_VERSION,
        max_loot,
        defender: public(&defender),
        snapshot_id,
        snapshot: parse(&data)?,
        expires_at: expires.to_rfc3339(),
        kind: kind.into(),
    }))
}

/// Find an opponent near the attacker's trophies and reserve their base for
/// one raid. Any earlier unfinished match of this attacker is dropped.
///
/// When nobody can be raided (everyone shielded or already under attack, or
/// no other player yet), the attacker gets a practice raid instead: a copy
/// of another player's base, or a computer-made one. It costs that player
/// nothing and moves no trophies.
pub async fn find_match(
    State(st): State<AppState>,
    _v: SameRules,
    user: AuthUser,
) -> ApiResult<Json<Match>> {
    let me = load_user(&st.db, user.id).await?;
    // Players who raid can be raided: without this, a player who never
    // publishes keeps every coin and loses no trophies.
    if !me.has_base {
        return Err(ApiError::conflict(
            "no_base",
            "publish your own base first: players who raid can be raided",
        ));
    }
    let mut tx = st.db.begin().await?;
    sqlx::query("UPDATE attacks SET status = 'expired' WHERE attacker_id = $1 AND status = 'pending'")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let mut found: Option<(Uuid, Uuid)> = None;
    // Close matches first, then anyone.
    for range in [st.cfg.match_range, i32::MAX] {
        found = sqlx::query_as(
            "SELECT u.id, u.current_snapshot FROM users u
             JOIN snapshots s ON s.id = u.current_snapshot AND s.sim_version = $4
             WHERE u.id <> $1 AND NOT u.is_bot
               AND (u.shield_until IS NULL OR u.shield_until < now())
               AND abs(u.trophies::bigint - $2::bigint) <= $3::bigint
               AND NOT EXISTS (SELECT 1 FROM clan_members mine JOIN clan_members theirs
                               ON theirs.clan_id = mine.clan_id
                               WHERE mine.user_id = $1 AND theirs.user_id = u.id)
               AND NOT EXISTS (SELECT 1 FROM attacks a WHERE a.defender_id = u.id
                               AND a.kind <> 'practice'
                               AND a.status = 'pending' AND a.expires_at > now())
             ORDER BY random() LIMIT 1
             FOR UPDATE OF u SKIP LOCKED",
        )
        .bind(user.id)
        .bind(me.trophies)
        .bind(range as i64)
        .bind(SIM_VERSION as i32)
        .fetch_optional(&mut *tx)
        .await?;
        if found.is_some() {
            break;
        }
    }
    if let Some((defender_id, snapshot_id)) = found {
        return open_raid(&st, tx, &me, defender_id, snapshot_id, KIND_RAID, None).await;
    }
    let copy: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT u.id, u.current_snapshot FROM users u
         JOIN snapshots s ON s.id = u.current_snapshot AND s.sim_version = $2
         WHERE u.id <> $1 AND NOT u.is_bot
         ORDER BY random() LIMIT 1",
    )
    .bind(user.id)
    .bind(SIM_VERSION as i32)
    .fetch_optional(&mut *tx)
    .await?;
    let (defender_id, snapshot_id) = match copy {
        Some(c) => c,
        None => (CAMP, camp_snapshot(&mut tx, me.upgrades.town_hall).await?),
    };
    open_raid(&st, tx, &me, defender_id, snapshot_id, KIND_PRACTICE, None).await
}

/// Raid back the player who raided you. Their shield doesn't protect them,
/// and each raid can be avenged once.
pub async fn revenge(
    State(st): State<AppState>,
    _v: SameRules,
    user: AuthUser,
    Path(attack_id): Path<Uuid>,
) -> ApiResult<Json<Match>> {
    let me = load_user(&st.db, user.id).await?;
    let mut tx = st.db.begin().await?;
    let row: Option<(Uuid, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT a.attacker_id, a.revenged, s.id FROM attacks a
         JOIN users u ON u.id = a.attacker_id
         LEFT JOIN snapshots s ON s.id = u.current_snapshot AND s.sim_version = $3
         WHERE a.id = $1 AND a.defender_id = $2 AND a.status = 'done' AND a.kind <> 'practice'",
    )
    .bind(attack_id)
    .bind(user.id)
    .bind(SIM_VERSION as i32)
    .fetch_optional(&mut *tx)
    .await?;
    let (target, revenged, snapshot) = row.ok_or_else(|| ApiError::not_found("raid"))?;
    if revenged {
        return Err(ApiError::conflict("revenged", "you already took revenge for this raid"));
    }
    let Some(snapshot) = snapshot else {
        return Err(ApiError::conflict(
            "no_opponent",
            "that player has no base under the current battle rules",
        ));
    };
    sqlx::query("UPDATE attacks SET status = 'expired' WHERE attacker_id = $1 AND status = 'pending'")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    // One raid on a base at a time, as in matchmaking.
    let free: Option<Uuid> = sqlx::query_scalar(
        "SELECT u.id FROM users u WHERE u.id = $1
           AND NOT EXISTS (SELECT 1 FROM attacks a WHERE a.defender_id = u.id AND a.kind <> 'practice'
                           AND a.status = 'pending' AND a.expires_at > now())
         FOR UPDATE OF u",
    )
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    if free.is_none() {
        return Err(ApiError::conflict(
            "busy",
            "that player is being raided right now, try again in a few minutes",
        ));
    }
    open_raid(&st, tx, &me, target, snapshot, KIND_REVENGE, Some(attack_id)).await
}

/// Late submissions are accepted for this long past the window, to allow for
/// a slow connection.
const GRACE_SECS: i64 = 60;

pub async fn submit(
    State(st): State<AppState>,
    user: AuthUser,
    Path(attack_id): Path<Uuid>,
    SameRulesJson(req): SameRulesJson<AttackSubmit>,
) -> ApiResult<Json<AttackOutcome>> {
    type Row = (Uuid, String, String, DateTime<Utc>, i32, String, Option<Uuid>);
    let row: Option<Row> = sqlx::query_as(
        "SELECT a.defender_id, a.status, s.data, a.expires_at, a.sim_version, a.kind, a.revenge_of
         FROM attacks a JOIN snapshots s ON s.id = a.snapshot_id
         WHERE a.id = $1 AND a.attacker_id = $2",
    )
    .bind(attack_id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let (defender_id, status, data, expires, sim, kind, revenge_of) =
        row.ok_or_else(|| ApiError::not_found("raid"))?;
    if status != "pending" || sim != SIM_VERSION as i32 || expires + chrono::Duration::seconds(GRACE_SECS) < Utc::now() {
        return Err(ApiError::conflict(
            "raid_closed",
            "this raid already finished or timed out; find a new match",
        ));
    }
    let me = load_user(&st.db, user.id).await?;
    let army = crate::clans::raid_army(&st.db, &me, &req).await?;
    let snapshot: BaseSnapshot = parse(&data)?;

    let (result, army) = {
        let army = army.clone();
        tokio::task::spawn_blocking(move || (run_raid(&snapshot, &army), army))
            .await
            .map_err(ApiError::internal)?
    };
    let client_matched = req
        .client_result
        .as_ref()
        .map(|c| c.fingerprint == result.fingerprint && c.stars == result.stars);
    if client_matched == Some(false) {
        tracing::warn!(%attack_id, attacker = %user.id, "client raid result differs from server");
    }
    let practice = kind == KIND_PRACTICE;

    let mut tx = st.db.begin().await?;
    let still_pending: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM attacks WHERE id = $1 AND status = 'pending' FOR UPDATE",
    )
    .bind(attack_id)
    .fetch_optional(&mut *tx)
    .await?;
    if still_pending.is_none() {
        return Err(ApiError::conflict("raid_closed", "this raid was already submitted"));
    }
    // Lock both players in id order so two raids can't deadlock. A practice
    // raid changes nothing for the defender, so only the attacker is locked.
    let mut ids = if practice { vec![user.id] } else { vec![user.id, defender_id] };
    ids.sort();
    let locked: Vec<(Uuid, i32, i64, bool)> = sqlx::query_as(
        "SELECT id, trophies, gold, last_win_bonus IS DISTINCT FROM current_date
         FROM users WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&ids[..])
    .fetch_all(&mut *tx)
    .await?;
    let get = |id| {
        locked
            .iter()
            .find(|r| r.0 == id)
            .copied()
            .ok_or_else(|| ApiError::not_found("player"))
    };
    let (_, att_trophies, _, first_win_due) = get(user.id)?;
    let mut att_new = att_trophies;
    let mut gold = 0;
    let mut bonus = 0;
    let mut league_bonus = 0;
    let mut defence_reward = 0;
    if practice {
        // A little gold from the server, for the first few each day.
        let paid: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM attacks WHERE attacker_id = $1 AND kind = 'practice'
               AND status = 'done' AND bonus_gold > 0 AND finished_at > now() - interval '1 day'",
        )
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await?;
        if paid < st.cfg.practice_paid_per_day {
            bonus += practice_gold(st.cfg.practice_gold, me.upgrades.town_hall, result.destruction);
        }
    } else {
        let (_, def_trophies, def_gold, _) = get(defender_id)?;
        let swing = trophy_swing(result.stars);
        // Nobody drops below zero, so the real change can differ per side.
        att_new = (att_trophies + swing).max(0);
        league_bonus = League::of(att_trophies).win_bonus(result.stars);
        bonus += league_bonus;
        let def_new = (def_trophies - swing).max(0);
        gold = loot(def_gold, result.destruction);
        let shield = (result.stars > 0).then(|| {
            Utc::now()
                + chrono::Duration::from_std(st.cfg.shield.mul_f32(shield_share(result.stars)))
                    .expect("fits")
        });
        if result.stars == 0 {
            // Holding a raid to no stars pays, a few times a day.
            let paid: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM attacks WHERE defender_id = $1 AND status = 'done'
                   AND defence_reward > 0 AND finished_at > now() - interval '1 day'",
            )
            .bind(defender_id)
            .fetch_one(&mut *tx)
            .await?;
            if paid < st.cfg.defence_rewards_per_day {
                defence_reward = st.cfg.defence_reward;
            }
        }
        sqlx::query(
            "UPDATE users SET trophies = $1, gold = gold - $2 + $3,
                 shield_until = COALESCE($4, shield_until) WHERE id = $5",
        )
        .bind(def_new)
        .bind(gold)
        .bind(defence_reward)
        .bind(shield)
        .bind(defender_id)
        .execute(&mut *tx)
        .await?;
        if let Some(original) = revenge_of {
            sqlx::query("UPDATE attacks SET revenged = TRUE WHERE id = $1")
                .bind(original)
                .execute(&mut *tx)
                .await?;
        }
    }
    // The first win of the day (UTC) pays a bonus, in any kind of raid.
    let first_win = result.stars > 0 && first_win_due && st.cfg.daily_win_bonus > 0;
    if first_win {
        bonus += st.cfg.daily_win_bonus;
    }
    sqlx::query(
        "UPDATE users SET trophies = $1, gold = gold + $2,
             last_win_bonus = CASE WHEN $3 THEN current_date ELSE last_win_bonus END
         WHERE id = $4",
    )
    .bind(att_new)
    .bind(gold + bonus)
    .bind(first_win)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE attacks SET status = 'done', army = $2, stars = $3, destruction = $4,
             town_hall_destroyed = $5, ticks = $6, fingerprint = $7, client_fingerprint = $8,
             client_matched = $9, trophies = $10, gold_looted = $11, bonus_gold = $12,
             defence_reward = $13, finished_at = now()
         WHERE id = $1",
    )
    .bind(attack_id)
    .bind(serde_json::to_string(&army).map_err(ApiError::internal)?)
    .bind(result.stars as i32)
    .bind(result.destruction)
    .bind(result.town_hall_destroyed)
    .bind(result.ticks as i32)
    .bind(&result.fingerprint)
    .bind(req.client_result.as_ref().map(|c| c.fingerprint.clone()))
    .bind(client_matched)
    .bind(att_new - att_trophies)
    .bind(gold)
    .bind(bonus)
    .bind(defence_reward)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(AttackOutcome {
        attack_id,
        trophies: att_new - att_trophies,
        gold_looted: gold,
        bonus_gold: bonus,
        league_bonus,
        client_matched,
        result,
        attacker: load_user(&st.db, user.id).await?,
    }))
}

#[derive(sqlx::FromRow)]
struct LogRow {
    id: Uuid,
    attacker_id: Uuid,
    a_name: String,
    a_trophies: i32,
    a_th: Option<i32>,
    defender_id: Uuid,
    d_name: String,
    d_trophies: i32,
    d_th: Option<i32>,
    stars: i32,
    destruction: f32,
    town_hall_destroyed: bool,
    ticks: i32,
    fingerprint: String,
    trophies: i32,
    gold_looted: i64,
    client_matched: Option<bool>,
    finished_at: DateTime<Utc>,
    kind: String,
    bonus_gold: i64,
    defence_reward: i64,
    revenged: bool,
}

const LOG_SELECT: &str = "SELECT a.id,
        a.attacker_id, ua.display_name AS a_name, ua.trophies AS a_trophies, sa.town_hall AS a_th,
        a.defender_id, ud.display_name AS d_name, ud.trophies AS d_trophies, sd.town_hall AS d_th,
        a.stars, a.destruction, a.town_hall_destroyed, a.ticks, a.fingerprint, a.trophies,
        a.gold_looted, a.client_matched, a.finished_at, a.kind, a.bonus_gold, a.defence_reward,
        a.revenged
    FROM attacks a
    JOIN users ua ON ua.id = a.attacker_id
    LEFT JOIN snapshots sa ON sa.id = ua.current_snapshot
    JOIN users ud ON ud.id = a.defender_id
    LEFT JOIN snapshots sd ON sd.id = ud.current_snapshot
    WHERE a.status = 'done'";

impl LogRow {
    /// `viewer` is the player reading the log.
    fn into_log(self, viewer: Uuid) -> AttackLog {
        AttackLog {
            can_revenge: viewer == self.defender_id && !self.revenged && self.kind != KIND_PRACTICE,
            kind: self.kind,
            bonus_gold: self.bonus_gold,
            defence_reward: self.defence_reward,
            attack_id: self.id,
            attacker: PublicPlayer {
                id: self.attacker_id,
                display_name: self.a_name,
                trophies: self.a_trophies,
                town_hall: self.a_th,
            },
            defender: PublicPlayer {
                id: self.defender_id,
                display_name: self.d_name,
                trophies: self.d_trophies,
                town_hall: self.d_th,
            },
            result: RaidResult {
                stars: self.stars as u32,
                destruction: self.destruction,
                town_hall_destroyed: self.town_hall_destroyed,
                ticks: self.ticks as u32,
                fingerprint: self.fingerprint,
            },
            trophies: self.trophies,
            gold_looted: self.gold_looted,
            client_matched: self.client_matched,
            finished_at: self.finished_at.to_rfc3339(),
        }
    }
}

#[derive(Deserialize)]
pub struct LogQuery {
    /// "defense", "attack" or both when absent.
    role: Option<String>,
    limit: Option<i64>,
}

pub async fn my_log(
    State(st): State<AppState>,
    user: AuthUser,
    Query(q): Query<LogQuery>,
) -> ApiResult<Json<Vec<AttackLog>>> {
    let filter = match q.role.as_deref() {
        Some("defense") => "a.defender_id = $1 AND a.kind <> 'practice'",
        Some("attack") => "a.attacker_id = $1",
        // A practice raid on a copy of your base isn't your battle.
        None | Some("all") => {
            "(a.attacker_id = $1 OR (a.defender_id = $1 AND a.kind <> 'practice'))"
        }
        Some(_) => return Err(ApiError::bad_request("role is defense, attack or all")),
    };
    let rows: Vec<LogRow> = sqlx::query_as(&format!(
        "{LOG_SELECT} AND {filter} ORDER BY a.finished_at DESC LIMIT $2"
    ))
    .bind(user.id)
    .bind(q.limit.unwrap_or(50).clamp(1, 200))
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.into_iter().map(|r| r.into_log(user.id)).collect()))
}

/// The defender's snapshot and the attacker's army, for watching the raid
/// again or training against it. Only the two players can open it.
pub async fn replay(
    State(st): State<AppState>,
    user: AuthUser,
    Path(attack_id): Path<Uuid>,
) -> ApiResult<Json<Replay>> {
    let row: Option<LogRow> = sqlx::query_as(&format!(
        "{LOG_SELECT} AND a.id = $1 AND (a.attacker_id = $2 OR a.defender_id = $2)"
    ))
    .bind(attack_id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let log = row.ok_or_else(|| ApiError::not_found("raid"))?.into_log(user.id);
    load_replay(&st, log).await
}

/// The best raid of the last 24 hours: most stars, then most destruction,
/// then fastest. Anyone signed in can watch it.
pub async fn raid_of_the_day(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Replay>> {
    let row: Option<LogRow> = sqlx::query_as(&format!(
        "{LOG_SELECT} AND a.kind <> 'practice' AND a.sim_version = $1
           AND a.finished_at > now() - interval '1 day'
         ORDER BY a.stars DESC, a.destruction DESC, a.ticks ASC, a.finished_at ASC LIMIT 1"
    ))
    .bind(SIM_VERSION as i32)
    .fetch_optional(&st.db)
    .await?;
    let mut log = row.ok_or_else(|| ApiError::not_found("raid of the day"))?.into_log(user.id);
    // Revenge is offered from the player's own log, not from here.
    log.can_revenge = false;
    load_replay(&st, log).await
}

async fn load_replay(st: &AppState, log: AttackLog) -> ApiResult<Json<Replay>> {
    let attack_id = log.attack_id;
    let (data, army, sim): (String, String, i32) = sqlx::query_as(
        "SELECT s.data, a.army, a.sim_version FROM attacks a JOIN snapshots s ON s.id = a.snapshot_id
         WHERE a.id = $1",
    )
    .bind(attack_id)
    .fetch_one(&st.db)
    .await?;
    let army: Vec<Placement> = parse(&army)?;
    Ok(Json(Replay {
        log,
        sim_version: sim as u32,
        snapshot: parse(&data)?,
        army,
    }))
}
