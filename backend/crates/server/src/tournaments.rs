//! One-off tournaments: the operator creates them on the admin page (see
//! `admin`), players enter one attacker each, and the server scores it
//! like the weekly cup. When a tournament ends, `pay_finished` ranks each
//! brain-size category and hands the prizes out as gifts the player
//! claims in the game (gold, weapons, a troop).

use crate::auth::{AuthUser, SameRulesJson};
use crate::error::{ApiError, ApiResult};
use crate::liveops::weapons_to;
use crate::players::load_user;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use bag_protocol::{
    canonical_troop, cup_score, prize_message, CupEntry, CupReplay, CupRow, PrizeBand, PrizeLine,
    PublicPlayer, TournamentInfo, TournamentStandings, SIM_VERSION,
};
use chrono::{DateTime, Utc};
use godcore::classes::{BaseClass, Role};
use godcore::scenario::max_troop_level;
use sqlx::PgPool;
use uuid::Uuid;

/// Days players have to claim a tournament prize.
pub const PRIZE_CLAIM_DAYS: i64 = 30;

type InfoRow = (
    Uuid,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<i16>,
    Option<String>,
    String,
    Option<DateTime<Utc>>,
    i64,
);

const INFO: &str = "SELECT t.id, t.title, t.message, t.starts_at, t.ends_at, t.category, t.class,
        t.prizes, t.paid_at,
        (SELECT count(*) FROM tournament_entries e WHERE e.tournament_id = t.id)
    FROM tournaments t";

pub fn class_from(s: &str) -> Option<BaseClass> {
    serde_json::from_value(serde_json::Value::String(s.into())).ok()
}

pub fn class_key(c: BaseClass) -> String {
    format!("{c:?}")
}

fn bands_of(json: &str) -> Vec<PrizeBand> {
    serde_json::from_str(json).unwrap_or_default()
}

fn info_of(r: InfoRow) -> TournamentInfo {
    let (id, title, message, s, e, category, class, prizes, paid_at, entries) = r;
    let now = Utc::now();
    TournamentInfo {
        id,
        title,
        message,
        starts_at: s.to_rfc3339(),
        ends_at: e.to_rfc3339(),
        category: category.map(|c| c as u8),
        class: class.as_deref().and_then(class_from),
        prizes: bands_of(&prizes).iter().map(PrizeLine::of).collect(),
        entries,
        active: s <= now && now < e,
        finished: e <= now,
        paid: paid_at.is_some(),
    }
}

/// Tournaments matching a fixed SQL condition on `t`, soonest ending first.
pub async fn infos(db: &PgPool, filter: &str, limit: i64) -> Result<Vec<TournamentInfo>, sqlx::Error> {
    let rows: Vec<InfoRow> = sqlx::query_as(&format!(
        "{INFO} WHERE {filter} ORDER BY t.ends_at, t.starts_at LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(info_of).collect())
}

pub async fn info(db: &PgPool, id: Uuid) -> ApiResult<TournamentInfo> {
    let row: Option<InfoRow> = sqlx::query_as(&format!("{INFO} WHERE t.id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?;
    row.map(info_of).ok_or_else(|| ApiError::not_found("tournament"))
}

#[derive(sqlx::FromRow)]
struct Row {
    rank: i64,
    user_id: Uuid,
    display_name: String,
    trophies: i32,
    town_hall: Option<i32>,
    troop_name: String,
    class_name: String,
    score: f32,
    prize: Option<i64>,
    category: i16,
}

/// Entries ranked within each category, best first; ties go to whoever
/// entered first. `filter` may use $2 (a player) and $3 (a category).
async fn rows(db: &PgPool, id: Uuid, filter: &str, player: Option<Uuid>, category: u8) -> ApiResult<Vec<CupRow>> {
    let sql = format!(
        "SELECT * FROM (
            SELECT row_number() OVER (PARTITION BY e.category ORDER BY e.score DESC, e.entered_at, e.user_id) AS rank,
                e.user_id, u.display_name, u.trophies, s.town_hall, e.troop_name, e.class_name,
                e.score, e.prize, e.category
            FROM tournament_entries e
            JOIN users u ON u.id = e.user_id
            LEFT JOIN snapshots s ON s.id = u.current_snapshot
            WHERE e.tournament_id = $1
        ) r {filter} AND ($2::uuid IS NULL OR TRUE) AND ($3::smallint IS NULL OR TRUE) ORDER BY rank"
    );
    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(id)
        .bind(player)
        .bind(category as i16)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| CupRow {
            rank: r.rank as u32,
            player: PublicPlayer {
                id: r.user_id,
                display_name: r.display_name,
                trophies: r.trophies,
                town_hall: r.town_hall,
            },
            troop_name: r.troop_name,
            class_name: r.class_name,
            score: r.score,
            prize: r.prize,
            category: r.category as u8,
        })
        .collect())
}

async fn standings_for(db: &PgPool, id: Uuid, me: Uuid, category: Option<u8>) -> ApiResult<TournamentStandings> {
    let info = info(db, id).await?;
    let mine = rows(db, id, "WHERE user_id = $2", Some(me), 0).await?.pop();
    let category = category
        .or(info.category)
        .or(mine.as_ref().map(|m| m.category))
        .unwrap_or(0);
    let entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tournament_entries WHERE tournament_id = $1 AND category = $2",
    )
    .bind(id)
    .bind(category as i16)
    .fetch_one(db)
    .await?;
    Ok(TournamentStandings {
        info,
        category,
        entries: entries as u32,
        top: rows(db, id, "WHERE rank <= 50 AND category = $3", None, category).await?,
        mine,
    })
}

/// `GET /v1/tournaments`: running ones, ones starting within a week, and
/// ones that ended in the last three days.
pub async fn list(State(st): State<AppState>, _user: AuthUser) -> ApiResult<Json<Vec<TournamentInfo>>> {
    Ok(Json(
        infos(
            &st.db,
            "t.starts_at < now() + interval '7 days' AND t.ends_at > now() - interval '3 days'",
            20,
        )
        .await?,
    ))
}

#[derive(serde::Deserialize)]
pub struct CategoryQuery {
    category: Option<u8>,
}

/// `GET /v1/tournaments/{id}?category=`: one category's table.
pub async fn standings(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<CategoryQuery>,
) -> ApiResult<Json<TournamentStandings>> {
    Ok(Json(standings_for(&st.db, id, user.id, q.category).await?))
}

/// `POST /v1/tournaments/{id}/entry`: enter a troop, or replace the entry.
/// Checked like a cup entry, plus the tournament's own rules.
pub async fn enter(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    SameRulesJson(req): SameRulesJson<CupEntry>,
) -> ApiResult<Json<TournamentStandings>> {
    let t = info(&st.db, id).await?;
    if !t.active {
        return Err(if t.finished {
            ApiError::conflict("tournament_over", "this tournament is over")
        } else {
            ApiError::conflict("tournament_not_started", "this tournament hasn't started yet")
        });
    }
    let paid = load_user(&st.db, user.id).await?.upgrades;
    let troop = canonical_troop(&req.troop, Role::Attack, max_troop_level(paid.town_hall))?;
    if let Some(why) = paid.blocker(&troop) {
        return Err(bag_protocol::Invalid(why).into());
    }
    let category = bag_protocol::troop_bracket(&troop);
    if let Some(only) = t.category {
        if category != only {
            return Err(bag_protocol::Invalid(format!(
                "this tournament is for \"{}\" brains only; {} has a \"{}\" brain",
                bag_protocol::bracket_name(only),
                troop.name.trim(),
                bag_protocol::bracket_name(category)
            ))
            .into());
        }
    }
    if let Some(class) = t.class {
        if troop.spec.main_class() != class {
            return Err(bag_protocol::Invalid(format!(
                "this tournament is for {class:?} troops only; {} is mostly {:?}",
                troop.name.trim(),
                troop.spec.main_class()
            ))
            .into());
        }
    }
    let (score, troop) = tokio::task::spawn_blocking(move || (cup_score(&troop), troop))
        .await
        .map_err(ApiError::internal)?;
    // Only while it still runs: the end may have passed during scoring.
    let done = sqlx::query(
        "INSERT INTO tournament_entries (tournament_id, user_id, troop, troop_name, class_name, score, category)
         SELECT $1, $2, $3, $4, $5, $6, $7 FROM tournaments
         WHERE id = $1 AND ends_at > now() AND paid_at IS NULL
         ON CONFLICT (tournament_id, user_id) DO UPDATE SET troop = EXCLUDED.troop,
             troop_name = EXCLUDED.troop_name, class_name = EXCLUDED.class_name,
             score = EXCLUDED.score, category = EXCLUDED.category, entered_at = now()",
    )
    .bind(id)
    .bind(user.id)
    .bind(serde_json::to_string(&troop).map_err(ApiError::internal)?)
    .bind(&troop.name)
    .bind(&troop.spec.name)
    .bind(score)
    .bind(category as i16)
    .execute(&st.db)
    .await?
    .rows_affected();
    if done == 0 {
        return Err(ApiError::conflict("tournament_over", "this tournament is over"));
    }
    Ok(Json(standings_for(&st.db, id, user.id, None).await?))
}

/// `GET /v1/tournaments/{id}/entries/{player}`: an entry with its troop, to
/// replay its raids.
pub async fn entry(
    State(st): State<AppState>,
    _user: AuthUser,
    Path((id, player)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<CupReplay>> {
    let row = rows(&st.db, id, "WHERE user_id = $2", Some(player), 0)
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("tournament entry"))?;
    let troop: String = sqlx::query_scalar(
        "SELECT troop FROM tournament_entries WHERE tournament_id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(player)
    .fetch_one(&st.db)
    .await?;
    if troop.is_empty() {
        return Err(ApiError::not_found("tournament entry's troop (it was cleaned up)"));
    }
    Ok(Json(CupReplay {
        row,
        sim_version: SIM_VERSION,
        troop: serde_json::from_str(&troop).map_err(ApiError::internal)?,
    }))
}

/// Hand out the prizes of every tournament that ended and isn't paid yet.
/// Each tournament is paid in one transaction that first marks it paid, so
/// it is never paid twice. Returns how many prizes went out.
pub async fn pay_finished(db: &PgPool) -> ApiResult<u64> {
    let mut sent = 0;
    loop {
        let mut tx = db.begin().await?;
        let due: Option<(Uuid, String, String)> = sqlx::query_as(
            "UPDATE tournaments SET paid_at = now()
             WHERE id = (SELECT id FROM tournaments WHERE ends_at <= now() AND paid_at IS NULL
                         ORDER BY ends_at LIMIT 1 FOR UPDATE SKIP LOCKED)
             RETURNING id, title, prizes",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some((id, title, prizes)) = due else { break };
        let bands = bands_of(&prizes);
        let last = bands.iter().map(|b| b.to).max().unwrap_or(0) as i64;
        let ranked: Vec<(Uuid, i64)> = sqlx::query_as(
            "SELECT user_id, rank FROM (
                 SELECT user_id, row_number() OVER (PARTITION BY category ORDER BY score DESC, entered_at, user_id) AS rank
                 FROM tournament_entries WHERE tournament_id = $1
             ) r ORDER BY rank",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let expires = Utc::now() + chrono::Duration::days(PRIZE_CLAIM_DAYS);
        let mut n = 0;
        for (user, rank) in ranked {
            let band = if rank <= last {
                bands.iter().find(|b| b.from as i64 <= rank && rank <= b.to as i64)
            } else {
                None
            };
            let gold = band.map(|b| b.reward.gold).unwrap_or(0);
            sqlx::query(
                "UPDATE tournament_entries SET rank = $3, prize = $4 WHERE tournament_id = $1 AND user_id = $2",
            )
            .bind(id)
            .bind(user)
            .bind(rank as i32)
            .bind(band.map(|_| gold))
            .execute(&mut *tx)
            .await?;
            let Some(b) = band else { continue };
            let troop = b
                .reward
                .troop
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(ApiError::internal)?;
            sqlx::query(
                "INSERT INTO gifts (id, user_id, gold, message, expires_at, weapons, troop, troop_name)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(Uuid::new_v4())
            .bind(user)
            .bind(gold)
            .bind(prize_message(rank as u32, &title))
            .bind(expires)
            .bind(weapons_to(&b.reward.weapons))
            .bind(troop)
            .bind(b.reward.troop.as_ref().map(|t| t.name.trim().to_string()))
            .execute(&mut *tx)
            .await?;
            n += 1;
        }
        sqlx::query("INSERT INTO admin_log (action, detail) VALUES ('tournament paid', $1)")
            .bind(format!("{title}: {n} prizes sent as gifts"))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        sent += n;
    }
    Ok(sent)
}
