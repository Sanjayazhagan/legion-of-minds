//! Clan wars. An officer starts a search; when two searching clans meet,
//! each side's strongest members with a published base are locked in, with
//! their bases frozen as they are. After a preparation window, every member
//! in the war gets `WAR_ATTACKS` raids on the other side's bases. A base's
//! best result counts, so the side with the most stars wins (destruction
//! breaks a tie). The server replays every attack, as for normal raids.

use crate::auth::{AuthUser, SameRules, SameRulesJson};
use crate::clans::{membership, raid_army, require_clan};
use crate::error::{ApiError, ApiResult};
use crate::players::load_user;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use bag_protocol::{
    run_raid, war_order, AttackSubmit, BaseSnapshot, Match, PublicPlayer, WarBase, WarOutcome,
    WarResult, WarSearch, WarSide, WarState, WarView, SIM_VERSION, WAR_ATTACKS, WAR_MAX_SIZE,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use std::cmp::Ordering;
use uuid::Uuid;

const GRACE_SECS: i64 = 60;

fn no_war() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no_war", "your clan isn't in a war")
}

/// Members of `clan` who can fight: a base published under these rules.
/// Strongest first.
async fn eligible(db: &mut PgConnection, clan: Uuid) -> ApiResult<Vec<(Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.current_snapshot FROM clan_members m
         JOIN users u ON u.id = m.user_id
         JOIN snapshots s ON s.id = u.current_snapshot AND s.sim_version = $2
         WHERE m.clan_id = $1
         ORDER BY u.trophies DESC, m.joined_at ASC",
    )
    .bind(clan)
    .bind(SIM_VERSION as i32)
    .fetch_all(&mut *db)
    .await?)
}

/// The clan's war that isn't scored yet, if any.
async fn open_war(db: &mut PgConnection, clan: Uuid) -> ApiResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM wars WHERE (clan_a = $1 OR clan_b = $1) AND settled_at IS NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(clan)
    .fetch_optional(&mut *db)
    .await?)
}

async fn latest_war(db: &mut PgConnection, clan: Uuid) -> ApiResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM wars WHERE clan_a = $1 OR clan_b = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(clan)
    .fetch_optional(&mut *db)
    .await?)
}

#[derive(sqlx::FromRow)]
struct WarRow {
    clan_a: Uuid,
    clan_b: Uuid,
    battle_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    winner: Option<Uuid>,
    settled_at: Option<DateTime<Utc>>,
}

fn state_of(w: &WarRow) -> WarState {
    let now = Utc::now();
    if w.settled_at.is_some() || now >= w.ends_at {
        WarState::Done
    } else if now < w.battle_at {
        WarState::Preparation
    } else {
        WarState::Battle
    }
}

/// One side of a war: its bases with the best result the other side got on
/// each, and its own total.
async fn side(db: &mut PgConnection, war: Uuid, clan: Uuid) -> ApiResult<WarSide> {
    let name: String = sqlx::query_scalar("SELECT name FROM clans WHERE id = $1")
        .bind(clan)
        .fetch_optional(&mut *db)
        .await?
        .unwrap_or_else(|| "A disbanded clan".into());
    let rows: Vec<(Uuid, String, i32, i32, i32, Option<i32>, Option<f32>, i64)> = sqlx::query_as(
        "SELECT b.user_id, u.display_name, u.trophies, b.position, s.town_hall,
                (SELECT MAX(stars) FROM war_attacks a WHERE a.war_id = b.war_id
                    AND a.defender_id = b.user_id AND a.status = 'done'),
                (SELECT MAX(destruction) FROM war_attacks a WHERE a.war_id = b.war_id
                    AND a.defender_id = b.user_id AND a.status = 'done'),
                (SELECT COUNT(*) FROM war_attacks a WHERE a.war_id = b.war_id
                    AND a.attacker_id = b.user_id
                    AND (a.status = 'done' OR (a.status = 'pending' AND a.expires_at > now())))
         FROM war_bases b
         JOIN users u ON u.id = b.user_id
         JOIN snapshots s ON s.id = b.snapshot_id
         WHERE b.war_id = $1 AND b.clan_id = $2
         ORDER BY b.position",
    )
    .bind(war)
    .bind(clan)
    .fetch_all(&mut *db)
    .await?;
    let bases: Vec<WarBase> = rows
        .into_iter()
        .map(|(id, name, trophies, pos, th, stars, dest, used)| WarBase {
            player: PublicPlayer {
                id,
                display_name: name,
                trophies,
                town_hall: Some(th),
            },
            position: pos as u32,
            town_hall: th as u32,
            best_stars: stars.unwrap_or(0) as u32,
            best_destruction: dest.unwrap_or(0.0),
            attacks_used: used as u32,
        })
        .collect();
    Ok(WarSide {
        clan_id: clan,
        name,
        // Filled in by `view`: a side's score is what it took from the other.
        stars: 0,
        destruction: 0.0,
        bases,
    })
}

fn taken(from: &WarSide) -> (u32, f32) {
    let stars = from.bases.iter().map(|b| b.best_stars).sum();
    let n = from.bases.len().max(1) as f32;
    let dest = from.bases.iter().map(|b| b.best_destruction).sum::<f32>() / n;
    (stars, dest)
}

pub async fn view(db: &mut PgConnection, war: Uuid, me: Uuid, my_clan: Uuid) -> ApiResult<WarView> {
    let w: WarRow = sqlx::query_as(
        "SELECT clan_a, clan_b, battle_at, ends_at, winner, settled_at FROM wars WHERE id = $1",
    )
    .bind(war)
    .fetch_one(&mut *db)
    .await?;
    let (ours, theirs) = if w.clan_a == my_clan {
        (w.clan_a, w.clan_b)
    } else {
        (w.clan_b, w.clan_a)
    };
    let mut us = side(db, war, ours).await?;
    let mut them = side(db, war, theirs).await?;
    (us.stars, us.destruction) = taken(&them);
    (them.stars, them.destruction) = taken(&us);
    let mine = us.bases.iter().find(|b| b.player.id == me);
    let state = state_of(&w);
    let result = w.settled_at.map(|_| match w.winner {
        Some(c) if c == ours => WarResult::Won,
        Some(_) => WarResult::Lost,
        None => WarResult::Draw,
    });
    Ok(WarView {
        id: war,
        state,
        battle_at: w.battle_at.to_rfc3339(),
        ends_at: w.ends_at.to_rfc3339(),
        in_war: mine.is_some(),
        my_attacks_left: match (state, mine) {
            (WarState::Done, _) | (_, None) => 0,
            (_, Some(b)) => WAR_ATTACKS.saturating_sub(b.attacks_used),
        },
        us,
        them,
        result,
    })
}

/// Make a war between two clans: each side's strongest eligible members,
/// the same number on both sides, bases frozen as published now.
async fn make_war(
    db: &mut PgConnection,
    st: &AppState,
    a: Uuid,
    b: Uuid,
    ea: &[(Uuid, Uuid)],
    eb: &[(Uuid, Uuid)],
) -> ApiResult<Uuid> {
    let size = ea.len().min(eb.len()).min(WAR_MAX_SIZE);
    let id = Uuid::new_v4();
    let battle_at = Utc::now() + chrono::Duration::from_std(st.cfg.war_prep).expect("fits");
    let ends_at = battle_at + chrono::Duration::from_std(st.cfg.war_battle).expect("fits");
    sqlx::query(
        "INSERT INTO wars (id, clan_a, clan_b, size, sim_version, battle_at, ends_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(a)
    .bind(b)
    .bind(size as i32)
    .bind(SIM_VERSION as i32)
    .bind(battle_at)
    .bind(ends_at)
    .execute(&mut *db)
    .await?;
    for (clan, members) in [(a, ea), (b, eb)] {
        for (k, (user, snap)) in members.iter().take(size).enumerate() {
            sqlx::query(
                "INSERT INTO war_bases (war_id, user_id, clan_id, position, snapshot_id)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(user)
            .bind(clan)
            .bind(k as i32 + 1)
            .bind(snap)
            .execute(&mut *db)
            .await?;
        }
        sqlx::query("UPDATE clans SET war_searching = false WHERE id = $1")
            .bind(clan)
            .execute(&mut *db)
            .await?;
    }
    for (clan, other) in [(a, b), (b, a)] {
        let name: String = sqlx::query_scalar("SELECT name FROM clans WHERE id = $1")
            .bind(other)
            .fetch_one(&mut *db)
            .await?;
        sqlx::query(
            "INSERT INTO clan_messages (clan_id, user_id, author, body) VALUES ($1, NULL, '', $2)",
        )
        .bind(clan)
        .bind(format!("War against {name}: {size} bases a side. Attacks open soon."))
        .execute(&mut *db)
        .await?;
    }
    Ok(id)
}

/// Look for a war. Pairs with another searching clan straight away if
/// there is one, otherwise waits for one.
pub async fn search(
    State(st): State<AppState>,
    _v: SameRules,
    user: AuthUser,
) -> ApiResult<Json<WarSearch>> {
    let mut tx = st.db.begin().await?;
    let (clan, role) = membership(&mut tx, user.id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_in_clan", "you're not in a clan"))?;
    if !role.officer() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only the leader or an elder can start a war",
        ));
    }
    sqlx::query("SELECT id FROM clans WHERE id = $1 FOR UPDATE")
        .bind(clan)
        .execute(&mut *tx)
        .await?;
    if open_war(&mut tx, clan).await?.is_some() {
        return Err(ApiError::conflict("in_war", "your clan is already in a war"));
    }
    let mine = eligible(&mut tx, clan).await?;
    if mine.len() < st.cfg.war_min_size {
        return Err(ApiError::conflict(
            "too_few",
            format!(
                "a war needs {} members with a published base; you have {}",
                st.cfg.war_min_size,
                mine.len()
            ),
        ));
    }
    let others: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM clans WHERE war_searching AND id <> $1 ORDER BY random()
         FOR UPDATE SKIP LOCKED",
    )
    .bind(clan)
    .fetch_all(&mut *tx)
    .await?;
    for other in others {
        if open_war(&mut tx, other).await?.is_some() {
            continue;
        }
        let theirs = eligible(&mut tx, other).await?;
        if theirs.len() < st.cfg.war_min_size {
            continue;
        }
        let war = make_war(&mut tx, &st, other, clan, &theirs, &mine).await?;
        let v = view(&mut tx, war, user.id, clan).await?;
        tx.commit().await?;
        return Ok(Json(WarSearch {
            searching: false,
            war: Some(v),
        }));
    }
    sqlx::query("UPDATE clans SET war_searching = true WHERE id = $1")
        .bind(clan)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(WarSearch {
        searching: true,
        war: None,
    }))
}

pub async fn cancel_search(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<WarSearch>> {
    let (clan, role) = require_clan(&st.db, user.id).await?;
    if !role.officer() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "forbidden", "only officers can do that"));
    }
    sqlx::query("UPDATE clans SET war_searching = false WHERE id = $1")
        .bind(clan)
        .execute(&st.db)
        .await?;
    Ok(Json(WarSearch {
        searching: false,
        war: None,
    }))
}

/// Your clan's current war, or the last one.
pub async fn current(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<WarView>> {
    settle(&st.db).await?;
    let (clan, _) = require_clan(&st.db, user.id).await?;
    let mut conn = st.db.acquire().await?;
    let war = latest_war(&mut conn, clan).await?.ok_or_else(no_war)?;
    Ok(Json(view(&mut conn, war, user.id, clan).await?))
}

/// Start a war attack on `target`'s frozen base. Submit it like a raid.
pub async fn attack(
    State(st): State<AppState>,
    _v: SameRules,
    user: AuthUser,
    Path(target): Path<Uuid>,
) -> ApiResult<Json<Match>> {
    let (clan, _) = require_clan(&st.db, user.id).await?;
    let mut tx = st.db.begin().await?;
    let war = open_war(&mut tx, clan).await?.ok_or_else(no_war)?;
    let w: WarRow = sqlx::query_as(
        "SELECT clan_a, clan_b, battle_at, ends_at, winner, settled_at FROM wars WHERE id = $1 FOR UPDATE",
    )
    .bind(war)
    .fetch_one(&mut *tx)
    .await?;
    match state_of(&w) {
        WarState::Preparation => {
            return Err(ApiError::conflict("war_not_started", "war attacks haven't opened yet"))
        }
        WarState::Done => return Err(ApiError::conflict("war_over", "this war is over")),
        WarState::Battle => {}
    }
    let in_war: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM war_bases WHERE war_id = $1 AND user_id = $2 AND clan_id = $3)",
    )
    .bind(war)
    .bind(user.id)
    .bind(clan)
    .fetch_one(&mut *tx)
    .await?;
    if !in_war {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you weren't in the clan when this war started",
        ));
    }
    // A war attack started and never sent is dropped (and its slot freed).
    sqlx::query(
        "UPDATE war_attacks SET status = 'expired' WHERE war_id = $1 AND attacker_id = $2 AND status = 'pending'",
    )
    .bind(war)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    let used: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM war_attacks WHERE war_id = $1 AND attacker_id = $2 AND status = 'done'",
    )
    .bind(war)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    if used as u32 >= WAR_ATTACKS {
        return Err(ApiError::conflict("no_attacks", "you've used all your war attacks"));
    }
    let base: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT b.snapshot_id, s.data FROM war_bases b JOIN snapshots s ON s.id = b.snapshot_id
         WHERE b.war_id = $1 AND b.user_id = $2 AND b.clan_id <> $3",
    )
    .bind(war)
    .bind(target)
    .bind(clan)
    .fetch_optional(&mut *tx)
    .await?;
    let (snapshot_id, data) = base.ok_or_else(|| ApiError::not_found("enemy base"))?;
    let id = Uuid::new_v4();
    let expires = (Utc::now() + chrono::Duration::from_std(st.cfg.attack_window).expect("fits"))
        .min(w.ends_at);
    sqlx::query(
        "INSERT INTO war_attacks (id, war_id, attacker_id, defender_id, status, expires_at)
         VALUES ($1, $2, $3, $4, 'pending', $5)",
    )
    .bind(id)
    .bind(war)
    .bind(user.id)
    .bind(target)
    .bind(expires)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let defender = load_user(&st.db, target).await?;
    let snapshot: BaseSnapshot = serde_json::from_str(&data).map_err(ApiError::internal)?;
    Ok(Json(Match {
        attack_id: id,
        sim_version: SIM_VERSION,
        defender: PublicPlayer {
            id: defender.id,
            display_name: defender.display_name,
            trophies: defender.trophies,
            town_hall: Some(snapshot.base.town_hall as i32),
        },
        snapshot_id,
        snapshot,
        expires_at: expires.to_rfc3339(),
        max_loot: 0,
    }))
}

pub async fn submit(
    State(st): State<AppState>,
    user: AuthUser,
    Path(attack_id): Path<Uuid>,
    SameRulesJson(req): SameRulesJson<AttackSubmit>,
) -> ApiResult<Json<WarOutcome>> {
    let row: Option<(Uuid, Uuid, String, DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT a.war_id, a.defender_id, a.status, a.expires_at, s.data
         FROM war_attacks a
         JOIN war_bases b ON b.war_id = a.war_id AND b.user_id = a.defender_id
         JOIN snapshots s ON s.id = b.snapshot_id
         WHERE a.id = $1 AND a.attacker_id = $2",
    )
    .bind(attack_id)
    .bind(user.id)
    .fetch_optional(&st.db)
    .await?;
    let (war, defender, status, expires, data) = row.ok_or_else(|| ApiError::not_found("war attack"))?;
    if status != "pending" || expires + chrono::Duration::seconds(GRACE_SECS) < Utc::now() {
        return Err(ApiError::conflict(
            "raid_closed",
            "this war attack already finished or timed out",
        ));
    }
    let me = load_user(&st.db, user.id).await?;
    let army = raid_army(&st.db, &me, &req).await?;
    let snapshot: BaseSnapshot = serde_json::from_str(&data).map_err(ApiError::internal)?;
    let (result, army) = tokio::task::spawn_blocking(move || (run_raid(&snapshot, &army), army))
        .await
        .map_err(ApiError::internal)?;
    let client_matched = req
        .client_result
        .as_ref()
        .map(|c| c.fingerprint == result.fingerprint && c.stars == result.stars);

    let mut tx = st.db.begin().await?;
    let pending: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM war_attacks WHERE id = $1 AND status = 'pending' FOR UPDATE",
    )
    .bind(attack_id)
    .fetch_optional(&mut *tx)
    .await?;
    if pending.is_none() {
        return Err(ApiError::conflict("raid_closed", "this war attack was already submitted"));
    }
    // Serialise attacks on the same base so new stars are counted once.
    sqlx::query("SELECT 1 FROM war_bases WHERE war_id = $1 AND user_id = $2 FOR UPDATE")
        .bind(war)
        .bind(defender)
        .execute(&mut *tx)
        .await?;
    let best: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(stars) FROM war_attacks WHERE war_id = $1 AND defender_id = $2 AND status = 'done'",
    )
    .bind(war)
    .bind(defender)
    .fetch_one(&mut *tx)
    .await?;
    let new_stars = result.stars.saturating_sub(best.unwrap_or(0) as u32);
    sqlx::query(
        "UPDATE war_attacks SET status = 'done', army = $2, stars = $3, destruction = $4,
             new_stars = $5, fingerprint = $6, client_matched = $7, finished_at = now()
         WHERE id = $1",
    )
    .bind(attack_id)
    .bind(serde_json::to_string(&army).map_err(ApiError::internal)?)
    .bind(result.stars as i32)
    .bind(result.destruction)
    .bind(new_stars as i32)
    .bind(&result.fingerprint)
    .bind(client_matched)
    .execute(&mut *tx)
    .await?;
    let (clan, _) = membership(&mut tx, user.id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_in_clan", "you're not in a clan"))?;
    let v = view(&mut tx, war, user.id, clan).await?;
    tx.commit().await?;
    Ok(Json(WarOutcome {
        attack_id,
        result,
        new_stars,
        client_matched,
        war: v,
    }))
}

/// Score every war whose battle day is over and pay the members who were
/// in it. Safe to call often and from several servers.
pub async fn settle(db: &PgPool) -> ApiResult<()> {
    let due: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM wars WHERE settled_at IS NULL AND ends_at <= now()",
    )
    .fetch_all(db)
    .await?;
    for war in due {
        let mut tx = db.begin().await?;
        let row: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT clan_a, clan_b FROM wars WHERE id = $1 AND settled_at IS NULL FOR UPDATE SKIP LOCKED",
        )
        .bind(war)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((a, b)) = row else { continue };
        sqlx::query("UPDATE war_attacks SET status = 'expired' WHERE war_id = $1 AND status = 'pending'")
            .bind(war)
            .execute(&mut *tx)
            .await?;
        let side_a = side(&mut tx, war, a).await?;
        let side_b = side(&mut tx, war, b).await?;
        let (sa, da) = taken(&side_b);
        let (sb, db_) = taken(&side_a);
        let (winner, ra, rb) = match war_order(sa, da, sb, db_) {
            Ordering::Greater => (Some(a), WarResult::Won, WarResult::Lost),
            Ordering::Less => (Some(b), WarResult::Lost, WarResult::Won),
            Ordering::Equal => (None, WarResult::Draw, WarResult::Draw),
        };
        for (clan, side, r) in [(a, &side_a, ra), (b, &side_b, rb)] {
            let col = match r {
                WarResult::Won => "war_wins",
                WarResult::Lost => "war_losses",
                WarResult::Draw => "war_draws",
            };
            sqlx::query(&format!("UPDATE clans SET {col} = {col} + 1 WHERE id = $1"))
                .bind(clan)
                .execute(&mut *tx)
                .await?;
            let ids: Vec<Uuid> = side.bases.iter().map(|b| b.player.id).collect();
            sqlx::query("UPDATE users SET gold = gold + $1 WHERE id = ANY($2)")
                .bind(r.reward())
                .bind(&ids[..])
                .execute(&mut *tx)
                .await?;
            let (ours, theirs) = if clan == a { (sa, sb) } else { (sb, sa) };
            let line = match r {
                WarResult::Won => format!("War won, {ours} stars to {theirs}!"),
                WarResult::Lost => format!("War lost, {ours} stars to {theirs}."),
                WarResult::Draw => format!("War drawn, {ours} stars each."),
            };
            let line = format!("{line} Everyone who fought gets {} gold.", r.reward());
            sqlx::query(
                "INSERT INTO clan_messages (clan_id, user_id, author, body) VALUES ($1, NULL, '', $2)",
            )
            .bind(clan)
            .bind(line)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE wars SET winner = $2, settled_at = now() WHERE id = $1")
            .bind(war)
            .bind(winner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(%war, stars_a = sa, stars_b = sb, "war settled");
    }
    Ok(())
}
