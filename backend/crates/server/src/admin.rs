//! The operator's controls: an admin page at `/admin` and the API behind it
//! under `/v1/admin`. Every call needs the `x-admin-key` header to match
//! the server's ADMIN_KEY. Without ADMIN_KEY set, all of it is switched off.
//!
//! From here the operator can start and stop events (double gold weekend),
//! give gold to everyone or one player, rename or end the season or move
//! its end, and change rewards and timings. Battle rules are not here on
//! purpose: changing them needs a new game build (a SIM_VERSION bump).

use crate::error::{ApiError, ApiResult};
use crate::league;
use crate::liveops::{self, default_of, knob_of, Live, KNOBS};
use crate::AppState;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use bag_protocol::{
    clean_text, AdminGift, AdminLogEntry, AdminOverview, LiveEvent, NewEvent, NewGift, Season,
    SeasonEdit, SettingView, EVENT_GOLD_PCT_MAX, EVENT_MAX_DAYS, EVENT_MESSAGE_MAX,
    EVENT_TITLE_MAX, EVENT_WIN_GOLD_MAX, GIFT_GOLD_MAX, SEASON_NAME_MAX,
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// How times read in the admin log.
const LOG_TIME: &str = "%a %e %b %H:%M UTC";

/// Shortest ADMIN_KEY the server accepts.
pub const ADMIN_KEY_MIN: usize = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin", get(page))
        .route("/v1/admin/overview", get(overview))
        .route("/v1/admin/events", post(start_event))
        .route("/v1/admin/events/{id}/stop", post(stop_event))
        .route("/v1/admin/gifts", post(give))
        .route("/v1/admin/gifts/{id}/cancel", post(cancel_gift))
        .route("/v1/admin/season", get(season).patch(edit_season))
        .route("/v1/admin/season/end", post(end_season))
        .route("/v1/admin/settings", get(settings).put(change_settings))
}

/// The admin page. It holds no data or secrets itself: it asks for the key
/// and then calls the API.
async fn page(State(st): State<AppState>) -> impl IntoResponse {
    if st.cfg.admin_key.is_none() {
        return (StatusCode::NOT_FOUND, Html("Admin is off. Set ADMIN_KEY on the server to turn it on.")).into_response();
    }
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'",
            ),
        ],
        Html(include_str!("admin.html")),
    )
        .into_response()
}

/// Proof that the request carries the admin key.
pub struct Admin;

fn digest(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let Some(key) = &st.cfg.admin_key else {
            return Err(ApiError::not_found("admin"));
        };
        let given = parts
            .headers
            .get("x-admin-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim();
        // Compare hashes byte by byte without stopping early, so timing
        // gives nothing away.
        let (a, b) = (digest(given), digest(key));
        let same = a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0;
        if same && !given.is_empty() {
            return Ok(Admin);
        }
        tracing::warn!("admin call with a wrong key");
        // Slow down anyone guessing.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "wrong admin key",
        ))
    }
}

async fn log(db: &PgPool, action: &str, detail: String) -> ApiResult<()> {
    tracing::info!(action, %detail, "admin");
    sqlx::query("INSERT INTO admin_log (action, detail) VALUES ($1, $2)")
        .bind(action)
        .bind(detail)
        .execute(db)
        .await?;
    Ok(())
}

fn time(s: &str, what: &str) -> ApiResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| ApiError::bad_request(format!("{what} is not a valid date and time")))
}

async fn current_season(st: &AppState) -> ApiResult<Season> {
    let live = Live::load(&st.db, &st.cfg).await?;
    league::current(&st.db, live.season_length()).await
}

// ------------------------------------------------------------- overview

async fn overview(State(st): State<AppState>, _: Admin) -> ApiResult<Json<AdminOverview>> {
    let season = current_season(&st).await?;
    let events = liveops::events(
        &st.db,
        "ends_at > now() - interval '14 days'",
        50,
    )
    .await?;
    let players: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&st.db)
        .await?;
    let active_today: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT attacker_id) FROM attacks WHERE created_at > now() - interval '1 day'",
    )
    .fetch_one(&st.db)
    .await?;
    let log: Vec<(DateTime<Utc>, String, String)> = sqlx::query_as(
        "SELECT at, action, detail FROM admin_log ORDER BY id DESC LIMIT 30",
    )
    .fetch_all(&st.db)
    .await?;
    Ok(Json(AdminOverview {
        season,
        events,
        gifts: gifts(&st.db).await?,
        players,
        active_today,
        log: log
            .into_iter()
            .map(|(at, action, detail)| AdminLogEntry {
                at: at.to_rfc3339(),
                action,
                detail,
            })
            .collect(),
    }))
}

type GiftRow = (Uuid, i64, String, Option<String>, DateTime<Utc>, DateTime<Utc>, i64);

async fn gifts(db: &PgPool) -> ApiResult<Vec<AdminGift>> {
    let rows: Vec<GiftRow> = sqlx::query_as(
        "SELECT g.id, g.gold, g.message, u.display_name, g.created_at, g.expires_at,
                (SELECT count(*) FROM gift_claims c WHERE c.gift_id = g.id)
         FROM gifts g LEFT JOIN users u ON u.id = g.user_id
         WHERE g.expires_at > now() - interval '30 days'
         ORDER BY g.created_at DESC LIMIT 30",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, gold, message, player, c, e, claims)| AdminGift {
            id,
            gold,
            message,
            player: player.unwrap_or_default(),
            created_at: c.to_rfc3339(),
            expires_at: e.to_rfc3339(),
            claims,
        })
        .collect())
}

// --------------------------------------------------------------- events

async fn start_event(
    State(st): State<AppState>,
    _: Admin,
    Json(req): Json<NewEvent>,
) -> ApiResult<Json<LiveEvent>> {
    let title = clean_text(&req.title, 3, EVENT_TITLE_MAX, "the event name")?;
    let message = clean_text(&req.message, 0, EVENT_MESSAGE_MAX, "the event message")?;
    let now = Utc::now();
    let starts = match req.starts_at.as_deref().map(str::trim) {
        None | Some("") => now,
        Some(s) => time(s, "the start")?.max(now - chrono::Duration::minutes(5)),
    };
    let ends = time(&req.ends_at, "the end")?;
    if ends <= starts || ends <= now {
        return Err(ApiError::bad_request("the event must end after it starts, in the future"));
    }
    if ends - starts > chrono::Duration::days(EVENT_MAX_DAYS) {
        return Err(ApiError::bad_request(format!(
            "an event can last at most {EVENT_MAX_DAYS} days"
        )));
    }
    if !(100..=EVENT_GOLD_PCT_MAX).contains(&req.gold_pct) {
        return Err(ApiError::bad_request(format!(
            "gold multiplier must be 100% to {EVENT_GOLD_PCT_MAX}%"
        )));
    }
    if !(0..=EVENT_WIN_GOLD_MAX).contains(&req.win_gold) {
        return Err(ApiError::bad_request(format!(
            "gold per win must be 0 to {EVENT_WIN_GOLD_MAX}"
        )));
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, title, message, starts_at, ends_at, gold_pct, win_gold)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(&title)
    .bind(&message)
    .bind(starts)
    .bind(ends)
    .bind(req.gold_pct)
    .bind(req.win_gold)
    .execute(&st.db)
    .await?;
    let ev = LiveEvent {
        id,
        title,
        message,
        starts_at: starts.to_rfc3339(),
        ends_at: ends.to_rfc3339(),
        gold_pct: req.gold_pct,
        win_gold: req.win_gold,
        active: starts <= now,
    };
    log(
        &st.db,
        "event started",
        format!(
            "{} ({}) {} to {}",
            ev.title,
            if ev.perks().is_empty() { "announcement".into() } else { ev.perks() },
            starts.format(LOG_TIME),
            ends.format(LOG_TIME)
        ),
    )
    .await?;
    Ok(Json(ev))
}

/// End a running event now, or drop one that hasn't started.
async fn stop_event(
    State(st): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let title: Option<String> = sqlx::query_scalar(
        "DELETE FROM events WHERE id = $1 AND starts_at > now() RETURNING title",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?;
    let title = match title {
        Some(t) => Some(t),
        None => sqlx::query_scalar(
            "UPDATE events SET ends_at = now() WHERE id = $1 AND ends_at > now() RETURNING title",
        )
        .bind(id)
        .fetch_optional(&st.db)
        .await?,
    };
    let title = title.ok_or_else(|| ApiError::not_found("running or upcoming event"))?;
    log(&st.db, "event stopped", title).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- gifts

async fn give(
    State(st): State<AppState>,
    _: Admin,
    Json(req): Json<NewGift>,
) -> ApiResult<Json<AdminGift>> {
    if !(1..=GIFT_GOLD_MAX).contains(&req.gold) {
        return Err(ApiError::bad_request(format!("gold must be 1 to {GIFT_GOLD_MAX}")));
    }
    let message = clean_text(&req.message, 0, EVENT_MESSAGE_MAX, "the message")?;
    let days = req.days.unwrap_or(14);
    if !(1..=90).contains(&days) {
        return Err(ApiError::bad_request("players get 1 to 90 days to claim a gift"));
    }
    let player = match req.player.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(p) => {
            // An id, or an exact name (ignoring case). Names aren't unique,
            // so refuse when several players share it.
            let found: Vec<(Uuid, String)> = match Uuid::parse_str(p) {
                Ok(id) => sqlx::query_as("SELECT id, display_name FROM users WHERE id = $1")
                    .bind(id)
                    .fetch_all(&st.db)
                    .await?,
                Err(_) => sqlx::query_as(
                    "SELECT id, display_name FROM users WHERE lower(display_name) = lower($1) LIMIT 3",
                )
                .bind(p)
                .fetch_all(&st.db)
                .await?,
            };
            match found.len() {
                0 => return Err(ApiError::not_found("player")),
                1 => Some(found.into_iter().next().expect("one")),
                _ => {
                    return Err(ApiError::bad_request(
                        "several players have that name; use their player id",
                    ))
                }
            }
        }
    };
    let id = Uuid::new_v4();
    let expires = Utc::now() + chrono::Duration::days(days);
    sqlx::query(
        "INSERT INTO gifts (id, user_id, gold, message, expires_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(player.as_ref().map(|p| p.0))
    .bind(req.gold)
    .bind(&message)
    .bind(expires)
    .execute(&st.db)
    .await?;
    let who = player.as_ref().map(|p| p.1.clone()).unwrap_or_default();
    log(
        &st.db,
        "gift",
        format!(
            "{} gold to {}{}",
            req.gold,
            if who.is_empty() { "everyone" } else { &who },
            if message.is_empty() { String::new() } else { format!(": {message}") }
        ),
    )
    .await?;
    Ok(Json(AdminGift {
        id,
        gold: req.gold,
        message,
        player: who,
        created_at: Utc::now().to_rfc3339(),
        expires_at: expires.to_rfc3339(),
        claims: 0,
    }))
}

/// Stop a gift being claimed. Gold already claimed stays with the players.
async fn cancel_gift(
    State(st): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let gold: Option<i64> = sqlx::query_scalar(
        "UPDATE gifts SET expires_at = now() WHERE id = $1 AND expires_at > now() RETURNING gold",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?;
    let gold = gold.ok_or_else(|| ApiError::not_found("open gift"))?;
    log(&st.db, "gift cancelled", format!("{gold} gold")).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------- season

async fn season(State(st): State<AppState>, _: Admin) -> ApiResult<Json<Season>> {
    Ok(Json(current_season(&st).await?))
}

/// Rename the running season or move its end.
async fn edit_season(
    State(st): State<AppState>,
    _: Admin,
    Json(req): Json<SeasonEdit>,
) -> ApiResult<Json<Season>> {
    let s = current_season(&st).await?;
    if let Some(name) = &req.name {
        let name = clean_text(name, 0, SEASON_NAME_MAX, "the season name")?;
        sqlx::query("UPDATE seasons SET name = $2 WHERE number = $1 AND closed_at IS NULL")
            .bind(s.number)
            .bind(&name)
            .execute(&st.db)
            .await?;
        log(&st.db, "season renamed", format!("Season {}: {name}", s.number)).await?;
    }
    if let Some(ends) = &req.ends_at {
        let ends = time(ends, "the season end")?;
        if ends < Utc::now() + chrono::Duration::minutes(1) {
            return Err(ApiError::bad_request(
                "the end must be in the future; use End season now to end it",
            ));
        }
        if ends > Utc::now() + chrono::Duration::days(365) {
            return Err(ApiError::bad_request("a season can end at most a year from now"));
        }
        sqlx::query("UPDATE seasons SET ends_at = $2 WHERE number = $1 AND closed_at IS NULL")
            .bind(s.number)
            .bind(ends)
            .execute(&st.db)
            .await?;
        log(
            &st.db,
            "season end moved",
            format!("Season {} now ends {}", s.number, ends.format(LOG_TIME)),
        )
        .await?;
    }
    Ok(Json(current_season(&st).await?))
}

/// End the running season now: pay rewards, reset trophies, start the
/// next. Returns the new season.
async fn end_season(State(st): State<AppState>, _: Admin) -> ApiResult<Json<Season>> {
    let s = current_season(&st).await?;
    sqlx::query("UPDATE seasons SET ends_at = now() WHERE number = $1 AND closed_at IS NULL")
        .bind(s.number)
        .execute(&st.db)
        .await?;
    let live = Live::load(&st.db, &st.cfg).await?;
    league::roll(&st.db, &live).await?;
    log(&st.db, "season ended", format!("Season {} ended early", s.number)).await?;
    Ok(Json(current_season(&st).await?))
}

// ------------------------------------------------------------- settings

async fn settings_view(st: &AppState) -> ApiResult<Vec<SettingView>> {
    let live = Live::load(&st.db, &st.cfg).await?;
    Ok(KNOBS
        .iter()
        .map(|k| SettingView {
            key: k.key.into(),
            group: k.group.into(),
            label: k.label.into(),
            help: k.help.into(),
            value: live.get(k.key),
            default: default_of(&st.cfg, k.key),
            min: k.min,
            max: k.max,
        })
        .collect())
}

async fn settings(State(st): State<AppState>, _: Admin) -> ApiResult<Json<Vec<SettingView>>> {
    Ok(Json(settings_view(&st).await?))
}

/// Change some settings: `{"season_days": 14, "war_reward_won": 800}`.
/// Setting one back to its default removes the override.
async fn change_settings(
    State(st): State<AppState>,
    _: Admin,
    Json(req): Json<HashMap<String, i64>>,
) -> ApiResult<Json<Vec<SettingView>>> {
    for (key, value) in &req {
        let knob = knob_of(key).ok_or_else(|| ApiError::bad_request(format!("unknown setting {key}")))?;
        if !(knob.min..=knob.max).contains(value) {
            return Err(ApiError::bad_request(format!(
                "{} must be {} to {}",
                knob.label, knob.min, knob.max
            )));
        }
    }
    let mut tx = st.db.begin().await?;
    let mut changed = Vec::new();
    for (key, value) in &req {
        if *value == default_of(&st.cfg, key) {
            sqlx::query("DELETE FROM settings WHERE key = $1")
                .bind(key)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
            )
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        }
        changed.push(format!("{key} = {value}"));
    }
    tx.commit().await?;
    if !changed.is_empty() {
        changed.sort();
        log(&st.db, "settings", changed.join(", ")).await?;
    }
    Ok(Json(settings_view(&st).await?))
}
