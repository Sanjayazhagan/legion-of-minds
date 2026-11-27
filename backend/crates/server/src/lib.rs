//! Legion of Minds online backend.
//!
//! - `auth`: test login, Google and Discord OAuth, bearer sessions
//! - `players`: profiles, leaderboard, publishing a base snapshot
//! - `attacks`: matchmaking, server-checked raids, battle log, replays
//! - `shop`: Town Hall, weapons and troop levels bought with online gold
//! - `cup`: the weekly cup, one troop per player, paid in gold
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
pub mod cup;
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
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
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

/// How many battles the server re-runs at once (raid and war checks, cup
/// scores). Each takes a whole CPU core for up to several seconds, so
/// without a limit a burst of raids takes every core and every other
/// request waits; and each holds its brains in memory. The rest queue here.
/// BATTLE_THREADS sets it; by default all cores but one, at least one.
pub fn battle_slots() -> usize {
    std::env::var("BATTLE_THREADS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1)))
        .max(1)
}

/// Run a battle (CPU-heavy, blocking) on its own thread, waiting for a free
/// battle slot first (see [`battle_slots`]).
pub async fn run_battle<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> error::ApiResult<T> {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let slots = SLOTS.get_or_init(|| tokio::sync::Semaphore::new(battle_slots()));
    let _slot = slots.acquire().await.map_err(error::ApiError::internal)?;
    tokio::task::spawn_blocking(f).await.map_err(error::ApiError::internal)
}

/// The biggest brain this server takes online. MAX_ONLINE_BRAIN lowers it
/// below the game's own limit (1024): a small server (1 GB of memory, one or
/// two cores) should take 256, because one raid with 1024 brains needs about
/// 10 s of CPU and a few hundred MB of memory to check.
pub fn max_online_brain() -> usize {
    std::env::var("MAX_ONLINE_BRAIN")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(godcore::ppo::ONLINE_BRAIN_MAX)
        .min(godcore::ppo::ONLINE_BRAIN_MAX)
}

/// Refuse troops with brains bigger than [`max_online_brain`].
pub fn check_brains<'a>(troops: impl IntoIterator<Item = &'a godcore::troop::Troop>) -> error::ApiResult<()> {
    let max = max_online_brain();
    for t in troops {
        let h = t.actor.dims.get(1).copied().unwrap_or(0);
        if h > max {
            return Err(bag_protocol::Invalid(format!(
                "this server takes brains up to {max} neurons online ({} has {h}); use a smaller-brained troop",
                t.name.trim()
            ))
            .into());
        }
    }
    Ok(())
}

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    // Each connection keeps buffers as big as the biggest row it has moved
    // (a base or an army: megabytes with big brains), so the pool size is
    // also a memory limit. DB_POOL sets it (default 8, plenty for hundreds of
    // players: queries take milliseconds).
    let size = std::env::var("DB_POOL").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(8u32).max(2);
    let db = PgPoolOptions::new()
        .max_connections(size)
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
    let (wire, json) = (st.cfg.max_body_bytes, st.cfg.max_json_bytes);
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
        .route("/v1/attacks/{id}/revenge", post(attacks::revenge))
        .route("/v1/raid-of-the-day", get(attacks::raid_of_the_day))
        .route("/v1/cup", get(cup::standings))
        .route("/v1/cup/entry", post(cup::enter))
        .route("/v1/cup/{week}/{player}", get(cup::entry))
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
        // Layers run bottom to top on the way in: the body is size-checked as
        // sent, rate-limited (a refused upload is drained while still
        // compressed, so the size check bounds it), then unzipped (game
        // clients gzip big uploads), then read with a limit on the unzipped
        // size, so a small zip can't unpack into gigabytes. Big replies
        // (bases, replays) go out gzipped.
        .layer(axum::extract::DefaultBodyLimit::max(json))
        .layer(RequestDecompressionLayer::new())
        .layer(axum::middleware::from_fn_with_state(st.clone(), ratelimit::limit))
        .layer(RequestBodyLimitLayer::new(wire))
        .layer(CompressionLayer::new())
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
    cleanup(&st.db, st.cfg.keep_raids).await.map_err(|e| e.to_string())
}

/// Delete expired sessions and sign-ins, close timed-out raids, forget raids
/// older than `keep_raids` (their replays hold whole armies, about 1 MB each),
/// and drop old snapshots nothing points at any more.
pub async fn cleanup(db: &PgPool, keep_raids: Duration) -> Result<(), sqlx::Error> {
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
    // Finished raids leave the battle log after `keep_raids`; given-up and
    // timed-out ones after a day.
    sqlx::query(
        "DELETE FROM attacks
         WHERE (status = 'done' AND finished_at < now() - $1)
            OR (status = 'expired' AND expires_at < now() - interval '1 day')",
    )
    .bind(keep_raids)
    .execute(db)
    .await?;
    cup::pay_finished(db, bag_protocol::cup_week(chrono::Utc::now().timestamp())).await?;
    // Cup entries stay for last week's podium, then go (each holds a troop).
    sqlx::query("DELETE FROM cup_entries WHERE week < $1")
        .bind(bag_protocol::cup_week(chrono::Utc::now().timestamp()) as i32 - 4)
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
