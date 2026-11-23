//! Anonymous play data from players who opted in (see
//! `bag_protocol::playdata` for what is sent and why), and the admin page's
//! summary of it.
//!
//! Uploads need no sign-in and store no account or address: only the random
//! id the game made when the player said yes. The game's "Delete my play
//! data" button deletes everything under that id.

use crate::admin::Admin;
use crate::error::{ApiError, ApiResult};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bag_protocol::{bracket_name, check_play_batch, PlayBatch, PLAY_KEEP_DAYS};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// Most events one install may store a day; more are dropped quietly, so a
/// stuck or scripted client can't fill the database.
const PER_INSTALL_DAY: i64 = 5_000;

/// `POST /v1/play-data`: store a batch of events.
pub async fn upload(State(st): State<AppState>, Json(b): Json<PlayBatch>) -> ApiResult<StatusCode> {
    check_play_batch(&b).map_err(ApiError::bad_request)?;
    let today: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM play_events WHERE install_id = $1 AND received_at > now() - interval '1 day'",
    )
    .bind(b.install)
    .fetch_one(&st.db)
    .await?;
    if today > PER_INSTALL_DAY {
        return Ok(StatusCode::NO_CONTENT);
    }
    let names: Vec<String> = b.events.iter().map(|e| e.name.clone()).collect();
    let secs: Vec<i32> = b.events.iter().map(|e| e.secs.min(i32::MAX as u32) as i32).collect();
    let props: Vec<String> = b
        .events
        .iter()
        .map(|e| serde_json::to_string(&e.props).unwrap_or_else(|_| "{}".into()))
        .collect();
    sqlx::query(
        "INSERT INTO play_events (install_id, session_id, name, secs, props, game_version, sim_version, os)
         SELECT $1, $2, n, s, p::jsonb, $6, $7, $8
         FROM UNNEST($3::text[], $4::int[], $5::text[]) AS t(n, s, p)",
    )
    .bind(b.install)
    .bind(b.session)
    .bind(&names)
    .bind(&secs)
    .bind(&props)
    .bind(&b.game_version)
    .bind(b.sim_version as i32)
    .bind(&b.os)
    .execute(&st.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/play-data/{install}`: forget everything sent under this id.
pub async fn forget(State(st): State<AppState>, Path(install): Path<Uuid>) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM play_events WHERE install_id = $1")
        .bind(install)
        .execute(&st.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Called by the server's cleanup: play data is kept for `PLAY_KEEP_DAYS`.
pub async fn cleanup(db: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM play_events WHERE received_at < now() - make_interval(days => $1)")
        .bind(PLAY_KEEP_DAYS as i32)
        .execute(db)
        .await?;
    Ok(())
}

// ------------------------------------------------------------- summary

#[derive(Deserialize)]
pub struct Days {
    days: Option<i32>,
}

/// How a number column reads.
#[derive(Clone, Copy)]
enum Fmt {
    Count,
    Pct,
    OneDp,
}

fn show(v: Option<f64>, f: Fmt) -> String {
    match (v, f) {
        (None, _) => "-".into(),
        (Some(x), Fmt::Count) => format!("{x:.0}"),
        (Some(x), Fmt::Pct) => format!("{:.0}%", x * 100.0),
        (Some(x), Fmt::OneDp) => format!("{x:.1}"),
    }
}

/// The events of the last `$1` days.
const SINCE: &str = "received_at > now() - make_interval(days => $1)";

/// Each player's first ever session, when it was in the last `$1` days.
const FIRST: &str = "first AS (
    SELECT DISTINCT ON (install_id) install_id, session_id FROM play_events
    WHERE name = 'session_start' AND play_num(props, 'day') = 0 AND SINCE
    ORDER BY install_id, received_at)";

/// First sessions that ended within 10 minutes.
const SHORT: &str = "short AS (
    SELECT f.install_id, f.session_id FROM first f
    JOIN play_events e USING (install_id, session_id)
    GROUP BY 1, 2 HAVING max(e.secs) < 600)";

/// One table on the admin page. `sql` gives rows of (label, count, a, b),
/// with `$1` the number of days; `cols` names them ("" hides a column).
async fn table(
    db: &PgPool,
    days: i32,
    title: &str,
    help: &str,
    cols: [&str; 4],
    fmts: [Fmt; 2],
    sql: &str,
) -> ApiResult<Value> {
    let sql = sql.replace("FIRST", FIRST).replace("SHORT", SHORT).replace("SINCE", SINCE);
    let rows: Vec<(Option<String>, i64, Option<f64>, Option<f64>)> =
        sqlx::query_as(&sql).bind(days).fetch_all(db).await?;
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|(l, n, a, b)| {
            let mut r = vec![l.unwrap_or_else(|| "(none)".into()), n.to_string()];
            if !cols[2].is_empty() {
                r.push(show(a, fmts[0]));
            }
            if !cols[3].is_empty() {
                r.push(show(b, fmts[1]));
            }
            r
        })
        .collect();
    let cols: Vec<&str> = cols.into_iter().filter(|c| !c.is_empty()).collect();
    Ok(json!({ "title": title, "help": help, "cols": cols, "rows": rows }))
}

/// `GET /v1/admin/play-data?days=30`: what opted-in players did.
pub async fn summary(State(st): State<AppState>, _: Admin, Query(q): Query<Days>) -> ApiResult<Json<Value>> {
    let days = q.days.unwrap_or(30).clamp(1, PLAY_KEEP_DAYS as i32);
    let db = &st.db;
    use Fmt::*;

    let (players, sessions): (i64, i64) = sqlx::query_as(&format!(
        "SELECT count(DISTINCT install_id), count(DISTINCT session_id) FROM play_events WHERE {SINCE}"
    ))
    .bind(days)
    .fetch_one(db)
    .await?;
    let new_players: i64 = sqlx::query_scalar(&format!("WITH {} SELECT count(*) FROM first", FIRST.replace("SINCE", SINCE)))
        .bind(days)
        .fetch_one(db)
        .await?;
    let minutes: Option<f64> = sqlx::query_scalar(&format!(
        "SELECT (percentile_cont(0.5) WITHIN GROUP (ORDER BY secs) / 60.0)::float8
         FROM play_events WHERE name = 'session_end' AND {SINCE}"
    ))
    .bind(days)
    .fetch_one(db)
    .await?;

    let mut tables = Vec::new();
    // Share of new players who did each thing, so the funnel reads as one.
    let new = (new_players.max(1)) as f64;
    let funnel = table(
        db,
        days,
        "First 10 minutes",
        "New players (first game in this period) who did each thing in the first 10 minutes of their first session, and who came back.",
        ["", "Players", "Share of new players", ""],
        [Pct, Pct],
        &format!(
            "WITH FIRST, SHORT, ev AS (SELECT e.* FROM play_events e JOIN first f USING (install_id, session_id))
             SELECT label, n, (n::float8 / {new:.1}), NULL::float8 FROM (
               SELECT 1 AS k, 'Opened the game' AS label, count(*) AS n FROM first
               UNION ALL SELECT 2, 'Finished the tour', count(DISTINCT install_id) FROM ev WHERE name = 'tour_end' AND play_bool(props, 'finished') AND secs < 600
               UNION ALL SELECT 3, 'Won a raid', count(DISTINCT install_id) FROM ev WHERE name = 'first_raid_won' AND secs < 600
               UNION ALL SELECT 4, 'Started training', count(DISTINCT install_id) FROM ev WHERE name = 'training_start' AND secs < 600
               UNION ALL SELECT 5, 'Played over 10 minutes', count(*) FROM first f WHERE NOT EXISTS (SELECT 1 FROM short s WHERE s.install_id = f.install_id)
               UNION ALL SELECT 6, 'Came back on a later day', count(DISTINCT e.install_id) FROM play_events e JOIN first f USING (install_id)
                 WHERE e.name = 'session_start' AND play_num(e.props, 'day') >= 1
               UNION ALL SELECT 7, 'Came back a week or more later', count(DISTINCT e.install_id) FROM play_events e JOIN first f USING (install_id)
                 WHERE e.name = 'session_start' AND play_num(e.props, 'day') >= 7
             ) x ORDER BY k"
        ),
    )
    .await?;
    tables.push(funnel);
    tables.push(
        table(
            db,
            days,
            "Where short first sessions stopped",
            "New players whose first session ended within 10 minutes: the last screen they were on.",
            ["Screen", "Players", "", ""],
            [Count, Count],
            "WITH FIRST, SHORT, last AS (
               SELECT DISTINCT ON (e.session_id) e.props ->> 'name' AS screen FROM play_events e
               JOIN short s USING (install_id, session_id) WHERE e.name = 'screen'
               ORDER BY e.session_id, e.secs DESC, e.id DESC)
             SELECT screen, count(*), NULL::float8, NULL::float8 FROM last GROUP BY 1 ORDER BY 2 DESC LIMIT 12",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Tour step when short first sessions stopped",
            "The same players: the furthest tour step they reached (of 14).",
            ["Tour step", "Players", "", ""],
            [Count, Count],
            "WITH FIRST, SHORT, last AS (
               SELECT s.session_id, max(play_num(e.props, 'step')) AS step FROM short s
               LEFT JOIN play_events e ON e.session_id = s.session_id AND e.name = 'tour_step'
               GROUP BY 1)
             SELECT coalesce((step::int + 1)::text, 'before the tour'), count(*), NULL::float8, NULL::float8
             FROM last GROUP BY 1 ORDER BY 2 DESC LIMIT 15",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Training by brain size",
            "Finished training sessions: how long one session took and how often it ended with the troop at its plateau.",
            ["Brain (neurons)", "Sessions", "Minutes (average)", "At plateau"],
            [OneDp, Pct],
            "SELECT coalesce(play_num(props, 'brain')::int::text, '?'), count(*),
                    avg(play_num(props, 'mins'))::float8,
                    avg(CASE WHEN play_bool(props, 'plateau') THEN 1.0 ELSE 0.0 END)::float8
             FROM play_events WHERE name = 'training_end' AND SINCE
             GROUP BY 1 ORDER BY min(play_num(props, 'brain'))",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "PC speed test",
            "Results of the speed test that sets how big a brain a PC may train.",
            ["Result", "Tests", "Players", ""],
            [Count, Count],
            "SELECT CASE WHEN play_bool(props, 'ok') THEN 'up to ' || coalesce(play_num(props, 'max_brain')::int::text, '?')
                         ELSE 'failed: ' || left(coalesce(props ->> 'error', '?'), 80) END,
                    count(*), count(DISTINCT install_id)::float8, NULL::float8
             FROM play_events WHERE name = 'speed_test' AND SINCE GROUP BY 1 ORDER BY 2 DESC LIMIT 15",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Raids by class",
            "Offline and online raids with at least one troop of this class (hybrids by their mix).",
            ["Class", "Raids", "Won", "Ran out of time"],
            [Pct, Pct],
            "SELECT c, count(*),
                    avg(CASE WHEN play_bool(props, 'won') THEN 1.0 ELSE 0.0 END)::float8,
                    avg(CASE WHEN play_bool(props, 'timeout') THEN 1.0 ELSE 0.0 END)::float8
             FROM play_events, jsonb_array_elements_text(CASE WHEN jsonb_typeof(props -> 'classes') = 'array' THEN props -> 'classes' ELSE '[]' END) c
             WHERE name = 'battle_end' AND props ->> 'mode' IN ('raid', 'online') AND SINCE
             GROUP BY 1 ORDER BY 2 DESC LIMIT 30",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Battles and tools",
            "Every battle by mode, and how often players looked inside a troop's head during it.",
            ["Mode", "Battles", "Won", "Watched thinking"],
            [Pct, Pct],
            "(SELECT props ->> 'mode', count(*),
                    avg(CASE WHEN play_bool(props, 'won') THEN 1.0 ELSE 0.0 END)::float8,
                    avg(CASE WHEN play_bool(props, 'thinking') THEN 1.0 ELSE 0.0 END)::float8
             FROM play_events WHERE name = 'battle_end' AND SINCE GROUP BY 1 ORDER BY 2 DESC)
             UNION ALL
             SELECT 'Sessions that opened the stat panel', count(DISTINCT session_id),
                    (count(DISTINCT session_id)::float8 / greatest(1, (SELECT count(DISTINCT session_id) FROM play_events WHERE SINCE))), NULL::float8
             FROM play_events WHERE name = 'stat_panel' AND SINCE",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Find opponent",
            "Online searches by brain-size category, and how often nobody was there to raid.",
            ["Category", "Searches", "Came back empty", ""],
            [Pct, Pct],
            "SELECT 'category ' || coalesce(play_num(props, 'category')::int::text, '?'), count(*),
                    avg(CASE WHEN play_bool(props, 'found') THEN 0.0 ELSE 1.0 END)::float8, NULL::float8
             FROM play_events WHERE name = 'find_opponent' AND SINCE GROUP BY 1 ORDER BY 1",
        )
        .await?,
    );
    // Straight from the online tables (all players, not just opted in).
    let cats: Vec<(i16, i64, i64)> = sqlx::query_as(
        "SELECT b.bracket, count(DISTINCT b.user_id), (SELECT count(*) FROM cup_entries c WHERE c.week = $1 AND c.category = b.bracket)
         FROM (SELECT u.id AS user_id, s.bracket FROM users u JOIN snapshots s ON s.id = u.current_snapshot) b
         GROUP BY 1 ORDER BY 1",
    )
    .bind(bag_protocol::cup_week(chrono::Utc::now().timestamp()) as i32)
    .fetch_all(db)
    .await?;
    tables.push(json!({
        "title": "Brain-size categories (all online players)",
        "help": "From the online tables, for every player: published bases per category, and this week's cup entries.",
        "cols": ["Category", "Bases", "Cup entries this week"],
        "rows": cats.iter().map(|(b, n, c)| vec![bracket_name(*b as u8).to_string(), n.to_string(), c.to_string()]).collect::<Vec<_>>(),
    }));
    tables.push(
        table(
            db,
            days,
            "Sign-in and server errors",
            "Failed sign-ins by provider and error, and requests the server refused (update_required = the player's game is older than the server's battle rules).",
            ["What", "Times", "Players", ""],
            [Count, Count],
            "SELECT CASE WHEN name = 'sign_in' THEN 'sign-in failed (' || coalesce(props ->> 'provider', '?') || '): ' || left(coalesce(props ->> 'error', '?'), 60)
                         ELSE 'server refused: ' || coalesce(props ->> 'code', '?') END,
                    count(*), count(DISTINCT install_id)::float8, NULL::float8
             FROM play_events
             WHERE ((name = 'sign_in' AND NOT coalesce(play_bool(props, 'ok'), false)) OR name = 'online_error') AND SINCE
             GROUP BY 1 ORDER BY 2 DESC LIMIT 20",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Crashes",
            "Most common crashes, by message and place in the code.",
            ["Crash", "Times", "Players", ""],
            [Count, Count],
            "SELECT left(coalesce(props ->> 'message', '?'), 120) || ' @ ' || coalesce(props ->> 'at', '?') || ' (v' || game_version || ')',
                    count(*), count(DISTINCT install_id)::float8, NULL::float8
             FROM play_events WHERE name = 'crash' AND SINCE GROUP BY 1 ORDER BY 2 DESC LIMIT 15",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Frame rate",
            "Sessions by operating system: average frames per second and the share of slow frames (over 1/30 s).",
            ["OS", "Sessions", "Frames per second", "Slow frames"],
            [OneDp, Pct],
            "SELECT os, count(*), avg(play_num(props, 'fps'))::float8, avg(play_num(props, 'slow'))::float8
             FROM play_events WHERE name = 'session_end' AND SINCE GROUP BY 1 ORDER BY 2 DESC",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "PCs",
            "Players by graphics chip, with their CPU cores and the biggest brain their PC trains (averages).",
            ["Graphics chip", "Players", "CPU cores", "Biggest brain"],
            [OneDp, Count],
            "SELECT left(coalesce(props ->> 'gpu', '?'), 60), count(DISTINCT install_id),
                    avg(play_num(props, 'cores'))::float8, avg(play_num(props, 'max_brain'))::float8
             FROM play_events WHERE name = 'session_start' AND SINCE GROUP BY 1 ORDER BY 2 DESC LIMIT 15",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Ratings",
            "The one-tap 1 to 5 rating after a player's first raid and first training.",
            ["After", "Ratings", "Average (1-5)", "Gave 4 or 5"],
            [OneDp, Pct],
            "SELECT props ->> 'after', count(*), avg(play_num(props, 'stars'))::float8,
                    avg(CASE WHEN play_num(props, 'stars') >= 4 THEN 1.0 ELSE 0.0 END)::float8
             FROM play_events WHERE name = 'rating' AND SINCE GROUP BY 1 ORDER BY 1",
        )
        .await?,
    );
    tables.push(
        table(
            db,
            days,
            "Game versions",
            "Which builds players run (old ones can't play online after a battle rules change).",
            ["Version (rules)", "Players", "Sessions", ""],
            [Count, Count],
            "SELECT game_version || ' (rules v' || sim_version || ')', count(DISTINCT install_id),
                    count(DISTINCT session_id)::float8, NULL::float8
             FROM play_events WHERE SINCE GROUP BY 1 ORDER BY 2 DESC LIMIT 10",
        )
        .await?,
    );

    Ok(Json(json!({
        "days": days,
        "players": players,
        "new_players": new_players,
        "sessions": sessions,
        "median_minutes": minutes,
        "tables": tables,
    })))
}
