//! Become a God online backend.
//!
//! - `auth`: test login, Google and Discord OAuth, bearer sessions
//! - `players`: profiles, leaderboard, publishing a base snapshot
//! - `attacks`: matchmaking, server-checked raids, battle log, replays

pub mod attacks;
pub mod auth;
pub mod config;
pub mod error;
pub mod players;

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
}

impl AppState {
    pub fn new(db: PgPool, cfg: Config) -> AppState {
        AppState {
            db,
            cfg: Arc::new(cfg),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("become-a-god-server")
                .build()
                .expect("http client"),
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
        .route("/v1/players/{id}", get(players::player))
        .route("/v1/leaderboard", get(players::leaderboard))
        .route("/v1/matchmaking", post(attacks::find_match))
        .route("/v1/attacks/{id}", post(attacks::submit))
        .route("/v1/attacks/{id}/replay", get(attacks::replay))
        .layer(RequestBodyLimitLayer::new(limit))
        .layer(axum::extract::DefaultBodyLimit::max(limit))
        .layer(TraceLayer::new_for_http())
        .with_state(st)
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
           AND NOT EXISTS (SELECT 1 FROM attacks a WHERE a.snapshot_id = s.id)",
    )
    .execute(db)
    .await?;
    Ok(())
}
