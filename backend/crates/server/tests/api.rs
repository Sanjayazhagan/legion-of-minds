//! End-to-end tests against a real Postgres. Each test gets its own fresh
//! database. Set TEST_DATABASE_URL to a server where the user can create
//! databases (default postgres://bag:bag@localhost/bag).
//!
//! The game side is driven through bag-client, exactly as the desktop game
//! would use it.

use axum::extract::{Form, State as AxState};
use axum::routing::{get, post};
use axum::{Json, Router};
use bag_client::bag_protocol::{loot, trophy_swing, BaseSnapshot, Placement};
use bag_client::{army_spots, defender_spots, snapshot_from, Client, Error};
use bag_server::config::{Config, OAuthProvider};
use bag_server::{router, AppState};
use base64::Engine;
use godcore::classes::BaseClass;
use godcore::scenario::HomeBase;
use godcore::troop::Troop;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn fixture(name: &str) -> Troop {
    let p = format!("{}/tests/fixtures/{name}.troop.json", env!("CARGO_MANIFEST_DIR"));
    Troop::load_json(&std::fs::read_to_string(p).unwrap()).unwrap()
}

async fn fresh_db() -> String {
    let admin = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://bag:bag@localhost/bag".into());
    let name = format!("bag_test_{}", uuid::Uuid::new_v4().simple());
    let pool = sqlx::PgPool::connect(&admin).await.expect("test database");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&pool)
        .await
        .unwrap();
    let mut url = url::Url::parse(&admin).unwrap();
    url.set_path(&name);
    url.to_string()
}

fn test_config(db: String, public_url: String, providers: Vec<OAuthProvider>) -> Config {
    Config {
        database_url: db,
        bind: "127.0.0.1:0".into(),
        public_url,
        dev_login: true,
        providers,
        session_ttl: Duration::from_secs(3600),
        login_ttl: Duration::from_secs(600),
        attack_window: Duration::from_secs(300),
        shield: Duration::from_secs(3600),
        start_gold: 500,
        match_range: 200,
        max_body_bytes: 4 << 20,
    }
}

/// Start a server; returns its base URL.
async fn serve(cfg: impl FnOnce(String) -> Config) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let cfg = cfg(url.clone());
    let db = bag_server::connect(&cfg.database_url).await.unwrap();
    let app = router(AppState::new(db, cfg));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// Run blocking client code off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn api_code<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
    match r {
        Err(Error::Api { code, .. }) => code,
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn home_defense() -> BaseSnapshot {
    let g = fixture("guard");
    let defs: Vec<_> = vec![g.clone(), g].into_iter().zip(defender_spots(2)).collect();
    snapshot_from(&HomeBase::reference(), &defs)
}

fn army() -> Vec<Placement> {
    let (f, h) = (fixture("fighter"), fixture("healer"));
    let troops = vec![f.clone(), f, h];
    let spots = army_spots(troops.len());
    bag_client::army_from(&troops.into_iter().zip(spots).collect::<Vec<_>>())
}

#[tokio::test(flavor = "multi_thread")]
async fn raid_flow_is_checked_and_scored() {
    let db = fresh_db().await;
    let url = serve(|u| test_config(db, u, vec![])).await;
    blocking(move || {
        let mut alice = Client::new(&url);
        let a = alice.dev_login("Alice").unwrap();
        assert_eq!(a.gold, 500);
        assert!(!a.has_base);

        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        // Bob's own base sets his Town Hall (troop level cap) for raids.
        bob.publish_base(&snapshot_from(&HomeBase::default(), &[])).unwrap();

        // Nobody else has a base yet.
        assert_eq!(api_code(bob.find_match()), "no_opponent");

        let p = alice.publish_base(&home_defense()).unwrap();
        assert_eq!((p.town_hall, p.defenders), (3, 2));
        assert_eq!(alice.me().unwrap().town_hall, Some(3));
        let back = alice.my_base().unwrap();
        assert_eq!(back.defenders.len(), 2);
        assert!(back.defenders[0].troop.curve.is_empty(), "history stripped");

        let m = bob.find_match().unwrap();
        assert_eq!(m.defender.display_name, "Alice");
        assert_eq!(m.max_loot, 100);
        // Alice is reserved while Bob's raid is open, so Carol gets Bob.
        let mut carol = Client::new(&url);
        carol.dev_login("Carol").unwrap();
        assert_eq!(carol.find_match().unwrap().defender.display_name, "Bob");

        let (local, out) = bob.attack(&m, army()).unwrap();
        assert_eq!(out.client_matched, Some(true), "server re-run matches the client");
        assert_eq!(local, out.result);
        println!(
            "raid: {} stars, {:.0}% in {} ticks, fp {}",
            local.stars,
            local.destruction * 100.0,
            local.ticks,
            local.fingerprint
        );

        let swing = trophy_swing(out.result.stars);
        let gold = loot(500, out.result.destruction);
        assert_eq!(out.gold_looted, gold);
        assert_eq!(out.attacker.gold, 500 + gold);
        assert_eq!(out.attacker.trophies, swing.max(0));
        let a = alice.me().unwrap();
        assert_eq!(a.gold, 500 - gold);
        assert_eq!(a.trophies, (-swing).max(0));
        assert_eq!(a.shield_until.is_some(), out.result.stars > 0);

        // Can't submit twice.
        assert_eq!(api_code(bob.submit(m.attack_id, army(), None)), "raid_closed");

        // Both sides see it in their log; Alice can replay it exactly.
        let log = alice.log("defense").unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].attacker.display_name, "Bob");
        assert_eq!(bob.log("attack").unwrap().len(), 1);
        assert!(bob.log("defense").unwrap().is_empty());
        let r = alice.replay(m.attack_id).unwrap();
        assert_eq!(bag_client::bag_protocol::run_raid(&r.snapshot, &r.army), out.result);
        assert_eq!(bag_client::raiders(&r).len(), 3);
        assert_eq!(api_code(carol.replay(m.attack_id)), "not_found");

        let board = carol.leaderboard(10).unwrap();
        assert_eq!(board.len(), 3);
        assert!(board.windows(2).all(|w| w[0].trophies >= w[1].trophies));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_raids_are_caught() {
    let db = fresh_db().await;
    let url = serve(|u| test_config(db, u, vec![])).await;
    blocking(move || {
        let mut alice = Client::new(&url);
        alice.dev_login("Alice").unwrap();
        alice.publish_base(&home_defense()).unwrap();
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();

        // Honest local result for reference.
        let m = bob.find_match().unwrap();
        let honest = bag_client::bag_protocol::run_raid(
            &m.snapshot,
            &bag_client::bag_protocol::canonical_army(&army(), 1).unwrap(),
        );

        // A hand-edited army (10x HP and damage) plus a fake 3-star claim.
        let mut cheat = army();
        for p in &mut cheat {
            p.troop.spec.hp *= 10.0;
            p.troop.spec.dmg *= 10.0;
        }
        let mut claim = honest.clone();
        claim.stars = 3;
        claim.destruction = 1.0;
        claim.fingerprint = "0000000000000000".into();
        let out = bob.submit(m.attack_id, cheat, Some(claim)).unwrap();
        assert_eq!(out.client_matched, Some(false));
        assert_eq!(out.result, honest, "server ignores edited stats and the claim");

        // Invalid armies and bases are refused outright.
        let m = {
            let mut dave = Client::new(&url);
            dave.dev_login("Dave").unwrap();
            dave.publish_base(&home_defense()).unwrap();
            bob.find_match().unwrap()
        };
        let mut a = army();
        a[0].troop = fixture("guard");
        assert_eq!(api_code(bob.submit(m.attack_id, a, None)), "invalid");
        let mut a = army();
        a[0].troop.level = 5;
        assert_eq!(api_code(bob.submit(m.attack_id, a, None)), "invalid");
        let mut a = army();
        (a[0].x, a[0].y) = (16.0, 16.0);
        assert_eq!(api_code(bob.submit(m.attack_id, a, None)), "invalid");

        // The local check in publish_base is skipped by sending raw JSON.
        let mut s = home_defense();
        let g = s.defenders[0].clone();
        s.defenders.extend([g.clone(), g.clone(), g]);
        let raw = |version: &str| {
            reqwest::blocking::Client::new()
                .put(format!("{url}/v1/me/base"))
                .bearer_auth(bob.token().unwrap())
                .header("x-sim-version", version)
                .json(&s)
                .send()
                .unwrap()
        };
        assert_eq!(raw(&godcore::sim::SIM_VERSION.to_string()).status(), 422);

        // A game on different battle rules can't publish or raid.
        // Every time, not just when the server happens to read the upload
        // before answering.
        for _ in 0..20 {
            let old = raw("1");
            assert_eq!(old.status(), 409);
            assert!(old.text().unwrap().contains("update_required"));
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bases_from_older_rules_are_not_matched() {
    let db = fresh_db().await;
    let db2 = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    let pool = sqlx::PgPool::connect(&db2).await.unwrap();
    let u2 = url.clone();
    blocking(move || {
        let mut alice = Client::new(&u2);
        alice.dev_login("Alice").unwrap();
        alice.publish_base(&home_defense()).unwrap();
    })
    .await;
    sqlx::query("UPDATE snapshots SET sim_version = 1").execute(&pool).await.unwrap();
    blocking(move || {
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        assert_eq!(api_code(bob.find_match()), "no_opponent");
        // Re-publishing under the current rules makes Alice raidable again.
        let mut alice = Client::new(&url);
        alice.dev_login("Alice").unwrap();
        alice.publish_base(&home_defense()).unwrap();
        let m = bob.find_match().unwrap();
        assert_eq!(m.sim_version, godcore::sim::SIM_VERSION);
        let (_, out) = bob.attack(&m, army()).unwrap();
        assert_eq!(alice.replay(out.attack_id).unwrap().sim_version, godcore::sim::SIM_VERSION);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_and_dev_login_switch() {
    let db = fresh_db().await;
    let db2 = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    let off = serve(|u| Config {
        dev_login: false,
        ..test_config(db2, u, vec![])
    })
    .await;
    blocking(move || {
        let anon = Client::new(&url);
        assert_eq!(api_code(anon.me()), "unauthorized");
        assert_eq!(api_code(Client::new(&url).with_token("bag_nope").me()), "unauthorized");

        let mut a = Client::new(&url);
        let u1 = a.dev_login("  Zed   the\tGreat ").unwrap();
        assert_eq!(u1.display_name, "Zed the Great");
        let mut again = Client::new(&url);
        assert_eq!(again.dev_login("zed THE great").unwrap().id, u1.id, "same account");
        assert_eq!(a.rename("Zeddy").unwrap().display_name, "Zeddy");

        let token = a.token().unwrap().to_string();
        a.logout().unwrap();
        assert_eq!(api_code(Client::new(&url).with_token(token).me()), "unauthorized");
        assert!(again.me().is_ok(), "other sessions stay signed in");

        let p = Client::new(&off).providers().unwrap();
        assert!(!p.dev && p.oauth.is_empty());
        assert_eq!(api_code(Client::new(&off).dev_login("x")), "not_found");
    })
    .await;
}

// ------------------------------------------------------------ OAuth

#[derive(Clone, Default)]
struct MockProvider {
    /// code -> PKCE challenge the "browser" saw.
    codes: Arc<Mutex<HashMap<String, String>>>,
}

async fn mock_token(
    AxState(m): AxState<MockProvider>,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let challenge = m
        .codes
        .lock()
        .unwrap()
        .remove(&f["code"])
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let got = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(f["code_verifier"].as_bytes()));
    if got != challenge || f["client_secret"] != "s3cret" || f["grant_type"] != "authorization_code"
    {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }
    Ok(Json(serde_json::json!({ "access_token": "at-1", "token_type": "Bearer" })))
}

async fn mock_userinfo(headers: axum::http::HeaderMap) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer at-1") {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }
    Ok(Json(serde_json::json!({ "sub": "g-123", "given_name": "Gina", "name": "Gina G" })))
}

#[tokio::test(flavor = "multi_thread")]
async fn oauth_browser_login() {
    let mock = MockProvider::default();
    let ml = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let murl = format!("http://{}", ml.local_addr().unwrap());
    let app = Router::new()
        .route("/token", post(mock_token))
        .route("/userinfo", get(mock_userinfo))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(ml, app).await.unwrap() });

    let provider = OAuthProvider {
        auth_url: format!("{murl}/authorize"),
        token_url: format!("{murl}/token"),
        userinfo_url: format!("{murl}/userinfo"),
        ..OAuthProvider::google("cid".into(), "s3cret".into())
    };
    let db = fresh_db().await;
    let url = serve(|u| test_config(db, u, vec![provider])).await;
    blocking(move || {
        let mut c = Client::new(&url);
        assert_eq!(c.providers().unwrap().oauth, vec!["google"]);
        assert_eq!(api_code(c.start_login("discord")), "not_found");
        let l = c.start_login("google").unwrap();
        let au = url::Url::parse(&l.url).unwrap();
        let q: HashMap<_, _> = au.query_pairs().into_owned().collect();
        assert!(l.url.starts_with(&format!("{murl}/authorize?")));
        assert_eq!(q["client_id"], "cid");
        assert_eq!(q["redirect_uri"], format!("{url}/v1/auth/google/callback"));
        assert_eq!(q["code_challenge_method"], "S256");

        assert!(c.poll_login(&l).unwrap().is_none(), "pending before the browser returns");
        let mut wrong = l.clone();
        wrong.poll_secret = "guess".into();
        assert_eq!(api_code(c.poll_login(&wrong)), "not_found");

        // The provider sends the browser back with a code.
        mock.codes
            .lock()
            .unwrap()
            .insert("code-1".into(), q["code_challenge"].clone());
        let browser = reqwest::blocking::Client::new();
        let bad = browser
            .get(format!("{url}/v1/auth/google/callback?code=code-1&state=forged"))
            .send()
            .unwrap();
        assert_eq!(bad.status(), 404);
        let page = browser
            .get(format!("{url}/v1/auth/google/callback?code=code-1&state={}", q["state"]))
            .send()
            .unwrap();
        assert_eq!(page.status(), 200);
        assert!(page.text().unwrap().contains("signed in"));

        let user = c.poll_login(&l).unwrap().expect("signed in");
        assert_eq!(user.display_name, "Gina");
        assert_eq!(c.me().unwrap().id, user.id);
        assert_eq!(api_code(Client::new(&url).poll_login(&l)), "not_found", "token handed out once");

        // Signing in again with the same Google account finds the same user.
        let l2 = c.start_login("google").unwrap();
        let q2: HashMap<_, _> = url::Url::parse(&l2.url).unwrap().query_pairs().into_owned().collect();
        mock.codes.lock().unwrap().insert("code-2".into(), q2["code_challenge"].clone());
        browser
            .get(format!("{url}/v1/auth/google/callback?code=code-2&state={}", q2["state"]))
            .send()
            .unwrap();
        let mut c2 = Client::new(&url);
        assert_eq!(c2.poll_login(&l2).unwrap().unwrap().id, user.id);
    })
    .await;
}

#[test]
fn every_class_fixture_is_valid() {
    for (n, class) in [("fighter", BaseClass::Fighter), ("guard", BaseClass::Guard), ("healer", BaseClass::Healer)] {
        assert_eq!(fixture(n).spec.parents[0].0, class);
    }
}

#[test]
fn starter_layouts_are_full_and_valid() {
    use godcore::scenario::BuildKind;
    for th in 1..=5 {
        let base = bag_client::starter_layout(th);
        for k in BuildKind::ALL {
            assert_eq!(base.count(k), k.max_at(th), "Town Hall {th}");
        }
        let s = snapshot_from(&base, &[]);
        assert!(bag_client::bag_protocol::canonical_snapshot(&s).is_ok(), "Town Hall {th}");
    }
}
