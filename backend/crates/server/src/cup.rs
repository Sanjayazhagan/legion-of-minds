//! The weekly cup: every player enters one attacker, and the server scores
//! it with the game's own Tournament (the same seeded solo and squad raids
//! for everyone). When the week closes, the standings pay gold.

use crate::auth::{AuthUser, SameRulesJson};
use crate::error::{ApiError, ApiResult};
use crate::players::load_user;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use bag_protocol::{
    canonical_troop, cup_prize, cup_score, cup_week, cup_week_end, CupEntry, CupReplay, CupRow,
    CupStandings, PublicPlayer, CUP_PRIZES, SIM_VERSION,
};
use chrono::{DateTime, Utc};
use godcore::classes::Role;
use godcore::scenario::max_troop_level;
use sqlx::PgPool;
use uuid::Uuid;

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

impl Row {
    fn into_row(self) -> CupRow {
        CupRow {
            rank: self.rank as u32,
            player: PublicPlayer {
                id: self.user_id,
                display_name: self.display_name,
                trophies: self.trophies,
                town_hall: self.town_hall,
            },
            troop_name: self.troop_name,
            class_name: self.class_name,
            score: self.score,
            prize: self.prize,
            category: self.category as u8,
        }
    }
}

/// A week's entries, best first, ranked within each brain-size category.
/// Ties go to whoever entered first.
const RANKED: &str = "SELECT * FROM (
        SELECT row_number() OVER (PARTITION BY c.category ORDER BY c.score DESC, c.entered_at, c.user_id) AS rank,
            c.user_id, u.display_name, u.trophies, s.town_hall, c.troop_name, c.class_name,
            c.score, c.prize, c.category
        FROM cup_entries c
        JOIN users u ON u.id = c.user_id
        LEFT JOIN snapshots s ON s.id = u.current_snapshot
        WHERE c.week = $1
    ) r";

/// `filter` may use $2 (a player) and $3 (a category).
async fn rows(db: &PgPool, week: i64, filter: &str, arg: Option<Uuid>, category: u8) -> ApiResult<Vec<CupRow>> {
    // Both parameters are always sent; mention them so Postgres knows their
    // types even when the filter doesn't use one.
    let sql = format!("{RANKED} {filter} AND ($2::uuid IS NULL OR TRUE) AND ($3::smallint IS NULL OR TRUE) ORDER BY rank");
    let q = sqlx::query_as::<_, Row>(&sql).bind(week as i32).bind(arg).bind(category as i16);
    Ok(q.fetch_all(db).await?.into_iter().map(Row::into_row).collect())
}

/// Standings in one category: `category` if asked, else the player's
/// entry's, else the first.
async fn standings_for(st: &AppState, me: Uuid, category: Option<u8>) -> ApiResult<CupStandings> {
    let week = cup_week(Utc::now().timestamp());
    let mine = rows(&st.db, week, "WHERE user_id = $2", Some(me), 0).await?.pop();
    let category = category.or(mine.as_ref().map(|m| m.category)).unwrap_or(0);
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM cup_entries WHERE week = $1 AND category = $2")
        .bind(week as i32)
        .bind(category as i16)
        .fetch_one(&st.db)
        .await?;
    Ok(CupStandings {
        week,
        ends_at: DateTime::<Utc>::from_timestamp(cup_week_end(week), 0)
            .unwrap_or_default()
            .to_rfc3339(),
        entries: entries as u32,
        category,
        top: rows(&st.db, week, "WHERE rank <= 50 AND category = $3", None, category).await?,
        mine,
        last_week: rows(&st.db, week - 1, "WHERE rank <= 3 AND category = $3", None, category).await?,
    })
}

#[derive(serde::Deserialize)]
pub struct CupQuery {
    category: Option<u8>,
}

pub async fn standings(
    State(st): State<AppState>,
    user: AuthUser,
    axum::extract::Query(q): axum::extract::Query<CupQuery>,
) -> ApiResult<Json<CupStandings>> {
    Ok(Json(standings_for(&st, user.id, q.category).await?))
}

/// Enter a troop, or replace this week's entry. The troop is checked like
/// a raiding troop: a real class, paid-for level and weapon.
pub async fn enter(
    State(st): State<AppState>,
    user: AuthUser,
    SameRulesJson(req): SameRulesJson<CupEntry>,
) -> ApiResult<Json<CupStandings>> {
    let paid = load_user(&st.db, user.id).await?.upgrades;
    let troop = canonical_troop(&req.troop, Role::Attack, max_troop_level(paid.town_hall))?;
    crate::check_brains([&troop])?;
    if let Some(why) = paid.blocker(&troop) {
        return Err(bag_protocol::Invalid(why).into());
    }
    // Each brain-size category is its own cup.
    let category = bag_protocol::troop_bracket(&troop);
    let (score, troop) = crate::run_battle(move || (cup_score(&troop), troop)).await?;
    sqlx::query(
        "INSERT INTO cup_entries (week, user_id, troop, troop_name, class_name, score, category)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (week, user_id) DO UPDATE SET troop = EXCLUDED.troop,
             troop_name = EXCLUDED.troop_name, class_name = EXCLUDED.class_name,
             score = EXCLUDED.score, category = EXCLUDED.category, entered_at = now()",
    )
    .bind(cup_week(Utc::now().timestamp()) as i32)
    .bind(user.id)
    .bind(serde_json::to_string(&troop).map_err(ApiError::internal)?)
    .bind(&troop.name)
    .bind(&troop.spec.name)
    .bind(score)
    .bind(category as i16)
    .execute(&st.db)
    .await?;
    Ok(Json(standings_for(&st, user.id, None).await?))
}

/// One entry with its troop, so anyone can replay its cup raids.
pub async fn entry(
    State(st): State<AppState>,
    _user: AuthUser,
    Path((week, player)): Path<(i64, Uuid)>,
) -> ApiResult<Json<CupReplay>> {
    let row = rows(&st.db, week, "WHERE user_id = $2", Some(player), 0)
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("cup entry"))?;
    let troop: String =
        sqlx::query_scalar("SELECT troop FROM cup_entries WHERE week = $1 AND user_id = $2")
            .bind(week as i32)
            .bind(player)
            .fetch_one(&st.db)
            .await?;
    Ok(Json(CupReplay {
        row,
        sim_version: SIM_VERSION,
        troop: serde_json::from_str(&troop).map_err(ApiError::internal)?,
    }))
}

/// Pay every finished week that hasn't been paid: one statement, so a week
/// is never paid twice. Returns how many entries were paid.
pub async fn pay_finished(db: &PgPool, current_week: i64) -> Result<u64, sqlx::Error> {
    debug_assert_eq!(cup_prize(1), CUP_PRIZES[0]);
    let paid = sqlx::query(
        "WITH ranked AS (
             SELECT week, user_id,
                 row_number() OVER (PARTITION BY week, category ORDER BY score DESC, entered_at, user_id) AS r
             FROM cup_entries WHERE week < $1 AND prize IS NULL
         ), paid AS (
             UPDATE cup_entries c SET prize = CASE
                 WHEN k.r = 1 THEN $2 WHEN k.r = 2 THEN $3 WHEN k.r = 3 THEN $4
                 WHEN k.r <= 10 THEN $5 ELSE $6 END
             FROM ranked k WHERE c.week = k.week AND c.user_id = k.user_id
             RETURNING c.user_id, c.prize
         )
         UPDATE users u SET gold = u.gold + p.total
         FROM (SELECT user_id, sum(prize)::bigint AS total FROM paid GROUP BY user_id) p
         WHERE u.id = p.user_id",
    )
    .bind(current_week as i32)
    .bind(CUP_PRIZES[0])
    .bind(CUP_PRIZES[1])
    .bind(CUP_PRIZES[2])
    .bind(CUP_PRIZES[3])
    .bind(CUP_PRIZES[4])
    .execute(db)
    .await?;
    Ok(paid.rows_affected())
}
