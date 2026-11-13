//! Seasons. Leagues come straight from trophies (`League::of`), so the
//! only stored state is the season calendar and each player's results.
//!
//! When a season ends, every player who raided or was raided gets a result
//! row and online gold by the league they finished in, and trophies above
//! `SEASON_FLOOR` are halved.

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::liveops::Live;
use crate::AppState;
use axum::extract::State;
use axum::Json;
use bag_protocol::{season_reset, League, Season, SeasonResult, SEASON_FLOOR};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::time::Duration;

/// Key for the Postgres advisory lock that keeps two servers from closing
/// the same season twice.
const SEASON_LOCK: i64 = 0x5ea5_0001;

fn season(number: i32, name: String, started: DateTime<Utc>, ends: DateTime<Utc>) -> Season {
    Season {
        number,
        name,
        started_at: started.to_rfc3339(),
        ends_at: ends.to_rfc3339(),
    }
}

/// The season running now, opening the first one if there is none.
pub async fn current(db: &PgPool, length: Duration) -> ApiResult<Season> {
    let row: Option<(i32, String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT number, name, started_at, ends_at FROM seasons WHERE closed_at IS NULL
         ORDER BY number DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if let Some((n, name, s, e)) = row {
        return Ok(season(n, name, s, e));
    }
    let now = Utc::now();
    let ends = now + chrono::Duration::from_std(length).expect("fits");
    sqlx::query(
        "INSERT INTO seasons (number, started_at, ends_at)
         SELECT COALESCE(MAX(number), 0) + 1, $1, $2 FROM seasons
         WHERE NOT EXISTS (SELECT 1 FROM seasons WHERE closed_at IS NULL)",
    )
    .bind(now)
    .bind(ends)
    .execute(db)
    .await?;
    let (n, name, s, e): (i32, String, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT number, name, started_at, ends_at FROM seasons WHERE closed_at IS NULL
         ORDER BY number DESC LIMIT 1",
    )
    .fetch_one(db)
    .await?;
    Ok(season(n, name, s, e))
}

/// Close the season if it has ended: record results, pay rewards, reset
/// trophies and open the next one. Returns the number of the season it
/// closed, if any. Safe to call from several servers at once. Rewards and
/// the next season's length come from the operator's settings.
pub async fn roll(db: &PgPool, live: &Live) -> Result<Option<i32>, sqlx::Error> {
    let length = live.season_length();
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SEASON_LOCK)
        .execute(&mut *tx)
        .await?;
    let due: Option<(i32, DateTime<Utc>)> = sqlx::query_as(
        "SELECT number, ends_at FROM seasons WHERE closed_at IS NULL AND ends_at <= now()
         ORDER BY number DESC LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some((number, ended)) = due else {
        return Ok(None);
    };

    // Everyone who played online this season, ranked by trophies.
    let players: Vec<(uuid::Uuid, i32)> = sqlx::query_as(
        "SELECT u.id, u.trophies FROM users u
         WHERE u.trophies > 0 OR EXISTS (
             SELECT 1 FROM attacks a WHERE a.status = 'done'
               AND (a.attacker_id = u.id OR a.defender_id = u.id))
         ORDER BY u.trophies DESC, u.created_at ASC",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (k, (id, trophies)) in players.iter().enumerate() {
        let league = League::of(*trophies);
        let reward = live.season_reward(league);
        sqlx::query(
            "INSERT INTO season_results (season, user_id, trophies, league, rank, reward)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(number)
        .bind(id)
        .bind(trophies)
        .bind(league_key(league))
        .bind(k as i64 + 1)
        .bind(reward)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE users SET gold = gold + $1, trophies = $2 WHERE id = $3")
            .bind(reward)
            .bind(season_reset(*trophies))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE seasons SET closed_at = now() WHERE number = $1")
        .bind(number)
        .execute(&mut *tx)
        .await?;
    // The next season starts when the last one was due, so a server that was
    // down for a while doesn't shift the calendar.
    let mut start = ended;
    let step = chrono::Duration::from_std(length).expect("fits");
    while start + step <= Utc::now() {
        start += step;
    }
    sqlx::query("INSERT INTO seasons (number, started_at, ends_at) VALUES ($1, $2, $3)")
        .bind(number + 1)
        .bind(start)
        .bind(start + step)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(season = number, players = players.len(), floor = SEASON_FLOOR, "season closed");
    Ok(Some(number))
}

pub fn league_key(l: League) -> String {
    match serde_json::to_value(l) {
        Ok(serde_json::Value::String(s)) => s,
        _ => unreachable!("leagues serialise as strings"),
    }
}

fn parse_league(s: &str) -> League {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or(League::Bronze)
}

pub async fn get_season(State(st): State<AppState>) -> ApiResult<Json<Season>> {
    let live = Live::load(&st.db, &st.cfg).await?;
    Ok(Json(current(&st.db, live.season_length()).await?))
}

/// Your results from past seasons, newest first.
pub async fn my_seasons(
    State(st): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Vec<SeasonResult>>> {
    let rows: Vec<(i32, i32, String, i64, i64)> = sqlx::query_as(
        "SELECT season, trophies, league, rank, reward FROM season_results
         WHERE user_id = $1 ORDER BY season DESC LIMIT 24",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(season, trophies, league, rank, reward)| SeasonResult {
                season,
                trophies,
                league: parse_league(&league),
                rank,
                reward,
            })
            .collect(),
    ))
}
