//! Legion of Minds online backend.
//!
//! - `auth`: test login, Google and Discord OAuth, bearer sessions
//! - `players`: profiles, leaderboard, publishing a base snapshot
//! - `attacks`: matchmaking, server-checked raids, battle log, replays
//! - `shop`: Town Hall, weapons and troop levels bought with online gold
//! - `league`: seasons (leagues come from trophies)
//! - `clans`: clans, chat and troops lent to clanmates
//! - `wars`: clan wars
//! - `liveops`: events, tuned rewards and gifts, as the game sees them
//! - `admin`: the operator's admin page and API (needs ADMIN_KEY)

pub mod attacks;
pub mod admin;
pub mod auth;
pub mod clans;
pub mod config;
pub mod error;
pub mod league;
pub mod liveops;
pub mod players;
pub mod ratelimit;
pub mod shop;
pub mod wars;

use axum::routing::{get, post, put};
use axum::{Json, Router};
use config::Config;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
    pub limiter: Arc<ratelimit::Limiter>,
}

impl AppState {
    pub fn new(db: PgPool, cfg: Config) -> AppState {
        AppState {
            db,
            cfg: Arc::new(cfg),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("legion-of-minds-server")
                .build()
                .expect("http client"),
            limiter: Default::default(),
        }
    }
}

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let db = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await?;
    MIGRATOR.run(&db).await?;
    Ok(db)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}

pub fn router(st: AppState) -> Router {
    let limit = st.cfg.max_body_bytes;
    Router::new()
        .route("/health", get(health))
        .route("/v1/auth/providers", get(auth::providers))
        .route("/v1/auth/dev", post(auth::dev_login))
        .route("/v1/auth/login", post(auth::login_start))
        .route("/v1/auth/logins/{id}/poll", post(auth::login_poll))
        .route("/v1/auth/{provider}/callback", get(auth::callback))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/me", get(players::me).patch(players::rename))
        .route(
            "/v1/me/base",
            put(players::publish_base).get(players::my_base),
        )
        .route("/v1/me/attacks", get(attacks::my_log))
        .route("/v1/me/town-hall", post(shop::upgrade_town_hall))
        .route("/v1/me/weapons", post(shop::buy_weapon))
        .route("/v1/me/troops/{id}/level", post(shop::level_up))
        .route("/v1/players/{id}", get(players::player))
        .route("/v1/leaderboard", get(players::leaderboard))
        .route("/v1/matchmaking", post(attacks::find_match))
        .route("/v1/attacks/{id}", post(attacks::submit))
        .route("/v1/attacks/{id}/replay", get(attacks::replay))
        .route("/v1/season", get(league::get_season))
        .route("/v1/me/seasons", get(league::my_seasons))
        .route("/v1/live", get(liveops::live))
        .route("/v1/me/gifts/{id}/claim", post(liveops::claim))
        .route("/v1/clans", get(clans::list).post(clans::create))
        .route("/v1/leaderboard/clans", get(clans::list))
        .route("/v1/clans/{id}", get(clans::get))
        .route("/v1/clans/{id}/join", post(clans::join))
        .route("/v1/me/clan", get(clans::mine))
        .route("/v1/me/clan/leave", post(clans::leave))
        .route("/v1/me/clan/members/{id}/kick", post(clans::kick))
        .route("/v1/me/clan/members/{id}/promote", post(clans::promote))
        .route("/v1/me/clan/chat", get(clans::chat).post(clans::post_chat))
        .route("/v1/me/clan/loans", get(clans::loans).put(clans::lend))
        .route("/v1/me/clan/my-loans", get(clans::my_loans))
        .route("/v1/me/clan/war", get(wars::current))
        .route(
            "/v1/me/clan/war/search",
            post(wars::search).delete(wars::cancel_search),
        )
        .route("/v1/me/clan/war/targets/{id}", post(wars::attack))
        .route("/v1/war-attacks/{id}", post(wars::submit))
        .merge(admin::routes())
        .layer(axum::middleware::from_fn_with_state(st.clone(), ratelimit::limit))
        .layer(RequestBodyLimitLayer::new(limit))
        .layer(axum::extract::DefaultBodyLimit::max(limit))
        .layer(TraceLayer::new_for_http())
        .with_state(st)
}

/// Background work, every few minutes: close a finished season, score
/// finished wars, then `cleanup`.
pub async fn tick(st: &AppState) -> Result<(), String> {
    let live = liveops::Live::load(&st.db, &st.cfg).await.map_err(|e| e.to_string())?;
    league::current(&st.db, live.season_length()).await.map_err(|e| e.message)?;
    league::roll(&st.db, &live).await.map_err(|e| e.to_string())?;
    wars::settle(&st.db, &live).await.map_err(|e| e.message)?;
    cleanup(&st.db).await.map_err(|e| e.to_string())
}

/// Delete expired sessions and sign-ins, close timed-out raids, and drop old
/// snapshots nothing points at any more.
pub async fn cleanup(db: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE expires_at < now()")
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM oauth_logins WHERE expires_at < now()")
        .execute(db)
        .await?;
    sqlx::query(
        "UPDATE attacks SET status = 'expired'
         WHERE status = 'pending' AND expires_at < now() - interval '5 minutes'",
    )
    .execute(db)
    .await?;
    sqlx::query(
        "DELETE FROM snapshots s
         WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.current_snapshot = s.id)
           AND NOT EXISTS (SELECT 1 FROM attacks a WHERE a.snapshot_id = s.id)
           AND NOT EXISTS (SELECT 1 FROM war_bases b WHERE b.snapshot_id = s.id)",
    )
    .execute(db)
    .await?;
    Ok(())
}
