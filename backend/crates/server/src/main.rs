use bag_server::{config::Config, connect, router, tick, AppState};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    let cfg = Config::from_env().unwrap_or_else(|e| {
        eprintln!("config error: {e}");
        std::process::exit(2);
    });
    let db = connect(&cfg.database_url).await.unwrap_or_else(|e| {
        eprintln!("database error: {e}");
        std::process::exit(1);
    });
    if cfg.dev_login {
        tracing::warn!("DEV_LOGIN is on: anyone can sign in by name. Turn it off on a public server.");
    }
    let names: Vec<_> = cfg.providers.iter().map(|p| p.name).collect();
    tracing::info!(providers = ?names, public_url = %cfg.public_url, "starting");

    let state = AppState::new(db, cfg.clone());
    let bg = state.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(60));
        loop {
            every.tick().await;
            if let Err(e) = tick(&bg).await {
                tracing::warn!("background work failed: {e}");
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .unwrap_or_else(|e| {
            eprintln!("can't listen on {}: {e}", cfg.bind);
            std::process::exit(1);
        });
    tracing::info!("listening on {}", cfg.bind);
    let app = router(state);
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server");
}
