//! Matchmaking and raids. The attacker gets a defender's snapshot, fights it
//! on their own machine, and sends their army and placement. The server
//! re-runs the raid with the same code; its result is the one that counts.

use crate::auth::{AuthUser, SameRules, SameRulesJson};
use crate::error::{ApiError, ApiResult};
use crate::players::{load_user, public};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bag_protocol::{
    canonical_army, loot, run_raid, trophy_swing, AttackLog, AttackOutcome, AttackSubmit,
    BaseSnapshot, Match, Placement, PublicPlayer, RaidResult, Replay, SIM_VERSION,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> ApiResult<T> {
    serde_json::from_str(s).map_err(ApiError::internal)
}

/// Find an opponent near the attacker's trophies and reserve their base for
/// one raid. Any earlier unfinished match of this attacker is dropped.
pub async fn find_match(
    State(st): State<AppState>,
    _v: SameRules,
    user: AuthUser,
) -> ApiResult<Json<Match>> {
    let me = load_user(&st.db, user.id).await?;
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
             WHERE u.id <> $1
               AND (u.shield_until IS NULL OR u.shield_until < now())
               AND abs(u.trophies::bigint - $2::bigint) <= $3::bigint
               AND NOT EXISTS (SELECT 1 FROM attacks a WHERE a.defender_id = u.id
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
    let Some((defender_id, snapshot_id)) = found else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_opponent",
            "no base to raid right now, try again soon",
        ));
    };
    let attack_id = Uuid::new_v4();
    let expires: DateTime<Utc> =
        Utc::now() + chrono::Duration::from_std(st.cfg.attack_window).expect("fits");
    sqlx::query(
        "INSERT INTO attacks (id, attacker_id, defender_id, snapshot_id, status, expires_at, sim_version)
         VALUES ($1, $2, $3, $4, 'pending', $5, $6)",
    )
    .bind(attack_id)
    .bind(user.id)
    .bind(defender_id)
    .bind(snapshot_id)
    .bind(expires)
    .bind(SIM_VERSION as i32)
    .execute(&mut *tx)
    .await?;
    let data: String = sqlx::query_scalar("SELECT data FROM snapshots WHERE id = $1")
        .bind(snapshot_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let defender = load_user(&st.db, defender_id).await?;
    Ok(Json(Match {
        attack_id,
        sim_version: SIM_VERSION,
        max_loot: loot(defender.gold, 1.0),
        defender: public(&defender),
        snapshot_id,
        snapshot: parse(&data)?,
        expires_at: expires.to_rfc3339(),
    }))
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
    let row: Option<(Uuid, String, String, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT a.defender_id, a.status, s.data, a.expires_at, a.sim_version
         FROM attacks a JOIN snapshots s ON s.id = a.snapshot_id
         WHERE a.id = $1 AND a.attacker_id = $2",
    )
    .bind(attack_id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let (defender_id, status, data, expires, sim) =
        row.ok_or_else(|| ApiError::not_found("raid"))?;
    if status != "pending" || sim != SIM_VERSION as i32 || expires + chrono::Duration::seconds(GRACE_SECS) < Utc::now() {
        return Err(ApiError::conflict(
            "raid_closed",
            "this raid already finished or timed out; find a new match",
        ));
    }
    let me = load_user(&st.db, user.id).await?;
    let army = canonical_army(&req.army, me.upgrades.town_hall)?;
    me.upgrades.check(&army)?;
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
    // Lock both players in id order so two raids can't deadlock.
    let mut ids = [user.id, defender_id];
    ids.sort();
    let locked: Vec<(Uuid, i32, i64)> =
        sqlx::query_as("SELECT id, trophies, gold FROM users WHERE id = ANY($1) ORDER BY id FOR UPDATE")
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
    let (_, att_trophies, _) = get(user.id)?;
    let (_, def_trophies, def_gold) = get(defender_id)?;
    let swing = trophy_swing(result.stars);
    // Nobody drops below zero, so the real change can differ per side.
    let att_new = (att_trophies + swing).max(0);
    let def_new = (def_trophies - swing).max(0);
    let gold = loot(def_gold, result.destruction);
    let shield = (result.stars > 0)
        .then(|| Utc::now() + chrono::Duration::from_std(st.cfg.shield).expect("fits"));

    sqlx::query("UPDATE users SET trophies = $1, gold = gold + $2 WHERE id = $3")
        .bind(att_new)
        .bind(gold)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE users SET trophies = $1, gold = gold - $2,
             shield_until = COALESCE($3, shield_until) WHERE id = $4",
    )
    .bind(def_new)
    .bind(gold)
    .bind(shield)
    .bind(defender_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE attacks SET status = 'done', army = $2, stars = $3, destruction = $4,
             town_hall_destroyed = $5, ticks = $6, fingerprint = $7, client_fingerprint = $8,
             client_matched = $9, trophies = $10, gold_looted = $11, finished_at = now()
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
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(AttackOutcome {
        attack_id,
        trophies: att_new - att_trophies,
        gold_looted: gold,
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
}

const LOG_SELECT: &str = "SELECT a.id,
        a.attacker_id, ua.display_name AS a_name, ua.trophies AS a_trophies, sa.town_hall AS a_th,
        a.defender_id, ud.display_name AS d_name, ud.trophies AS d_trophies, sd.town_hall AS d_th,
        a.stars, a.destruction, a.town_hall_destroyed, a.ticks, a.fingerprint, a.trophies,
        a.gold_looted, a.client_matched, a.finished_at
    FROM attacks a
    JOIN users ua ON ua.id = a.attacker_id
    LEFT JOIN snapshots sa ON sa.id = ua.current_snapshot
    JOIN users ud ON ud.id = a.defender_id
    LEFT JOIN snapshots sd ON sd.id = ud.current_snapshot
    WHERE a.status = 'done'";

impl LogRow {
    fn into_log(self) -> AttackLog {
        AttackLog {
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
        Some("defense") => "a.defender_id = $1",
        Some("attack") => "a.attacker_id = $1",
        None | Some("all") => "(a.attacker_id = $1 OR a.defender_id = $1)",
        Some(_) => return Err(ApiError::bad_request("role is defense, attack or all")),
    };
    let rows: Vec<LogRow> = sqlx::query_as(&format!(
        "{LOG_SELECT} AND {filter} ORDER BY a.finished_at DESC LIMIT $2"
    ))
    .bind(user.id)
    .bind(q.limit.unwrap_or(50).clamp(1, 200))
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.into_iter().map(LogRow::into_log).collect()))
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
    let log = row.ok_or_else(|| ApiError::not_found("raid"))?.into_log();
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
