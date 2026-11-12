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
        keep_raids: Duration::from_secs(14 * 86_400),
        defence_reward: 30,
        defence_rewards_per_day: 5,
        daily_win_bonus: 100,
        practice_gold: 20,
        practice_paid_per_day: 5,
        season: Duration::from_secs(28 * 86_400),
        war_prep: Duration::from_secs(3600),
        war_battle: Duration::from_secs(86_400),
        war_min_size: 3,
        requests_per_min: 100_000,
        heavy_per_min: 100_000,
        trust_proxy: false,
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

/// A fixture troop with its own id, as the game gives every troop.
fn troop(name: &str, id: u64) -> Troop {
    let mut t = fixture(name);
    t.id = id;
    t
}

fn home_defense() -> BaseSnapshot {
    let defs: Vec<_> = vec![troop("guard", 1), troop("guard", 2)]
        .into_iter()
        .zip(defender_spots(2))
        .collect();
    snapshot_from(&HomeBase::reference(), &defs)
}

/// Run SQL against the test database from blocking code.
fn sql(db: &str, q: &str, gold: i64, id: uuid::Uuid) {
    let (db, q) = (db.to_string(), q.to_string());
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let pool = sqlx::PgPool::connect(&db).await.unwrap();
        sqlx::query(&q).bind(gold).bind(id).execute(&pool).await.unwrap();
    });
}

fn set_gold(db: &str, c: &Client, gold: i64) {
    sql(db, "UPDATE users SET gold = $1 WHERE id = $2", gold, c.me().unwrap().id);
}

/// Buy Town Hall `th` online, leaving the player's gold as it was.
fn buy_town_hall(db: &str, c: &Client, th: u32) {
    let gold = c.me().unwrap().gold;
    set_gold(db, c, 100_000);
    while c.me().unwrap().upgrades.town_hall < th {
        c.upgrade_town_hall().unwrap();
    }
    set_gold(db, c, gold);
}

fn army() -> Vec<Placement> {
    let troops = vec![troop("fighter", 1), troop("fighter", 2), troop("healer", 3)];
    let spots = army_spots(troops.len());
    bag_client::army_from(&troops.into_iter().zip(spots).collect::<Vec<_>>())
}

#[tokio::test(flavor = "multi_thread")]
async fn raid_flow_is_checked_and_scored() {
    let db = fresh_db().await;
    let dbu = db.clone();
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

        // Nobody else has a base yet: Bob gets a practice raid on a
        // computer-made base instead of nothing.
        let p = bob.find_match().unwrap();
        assert_eq!((p.kind.as_str(), p.defender.display_name.as_str()), ("practice", "Training Camp"));

        // The reference base is Town Hall 3, which Alice must buy online.
        assert_eq!(api_code(alice.publish_base(&home_defense())), "invalid");
        buy_town_hall(&dbu, &alice, 3);
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
        // Only players with a base of their own can raid.
        assert_eq!(api_code(carol.find_match()), "no_base");
        carol.publish_base(&snapshot_from(&HomeBase::default(), &[])).unwrap();
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
        // The first win of the day pays a bonus on top of the loot.
        assert_eq!(out.bonus_gold, if out.result.stars > 0 { 100 } else { 0 });
        assert_eq!(out.attacker.gold, 500 + gold + out.bonus_gold);
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
    let dbu = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    blocking(move || {
        let mut alice = Client::new(&url);
        alice.dev_login("Alice").unwrap();
        buy_town_hall(&dbu, &alice, 3);
        alice.publish_base(&home_defense()).unwrap();
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        bob.publish_base(&snapshot_from(&HomeBase::default(), &[])).unwrap();

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
            buy_town_hall(&dbu, &dave, 3);
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
async fn practice_revenge_rewards_and_raid_of_the_day() {
    let db = fresh_db().await;
    let dbu = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    blocking(move || {
        let empty = || snapshot_from(&HomeBase::default(), &[]);
        let mut alice = Client::new(&url);
        alice.dev_login("Alice").unwrap();
        alice.publish_base(&empty()).unwrap();
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        bob.publish_base(&empty()).unwrap();
        assert_eq!(api_code(alice.raid_of_the_day()), "not_found");

        // Bob raids Alice's empty base: stars, loot, the first-win bonus,
        // and a shield for Alice.
        let m = bob.find_match().unwrap();
        assert_eq!(m.kind, "raid");
        let (_, won) = bob.attack(&m, army()).unwrap();
        assert!(won.result.stars > 0);
        assert_eq!(won.bonus_gold, 100);
        assert!(alice.me().unwrap().shield_until.is_some());

        // Alice is shielded, so Bob's next match is practice on a copy of
        // her base: no trophies, nothing taken from her, a little gold from
        // the server, and no second first-win bonus.
        let (a0, b0) = (alice.me().unwrap(), bob.me().unwrap());
        let p = bob.find_match().unwrap();
        assert_eq!((p.kind.as_str(), p.defender.display_name.as_str()), ("practice", "Alice"));
        assert_eq!(p.max_loot, 20);
        let (_, out) = bob.attack(&p, army()).unwrap();
        let paid = (20.0 * out.result.destruction as f64).floor() as i64;
        assert_eq!((out.trophies, out.gold_looted, out.bonus_gold), (0, 0, paid));
        assert_eq!(out.attacker.gold, b0.gold + paid);
        let a1 = alice.me().unwrap();
        assert_eq!((a1.gold, a1.trophies), (a0.gold, a0.trophies));
        // Alice's log doesn't show practice on her base.
        assert_eq!(alice.log("defense").unwrap().len(), 1);
        // Only the first five practice raids in a day pay.
        for k in 1..6 {
            let p = bob.find_match().unwrap();
            let (_, out) = bob.attack(&p, army()).unwrap();
            assert_eq!(out.bonus_gold > 0, k < 5, "practice raid {}", k + 1);
        }

        // Revenge: Alice raids Bob back from her log, once.
        let log = alice.log("defense").unwrap();
        assert!(log[0].can_revenge);
        assert!(!bob.log("attack").unwrap().iter().any(|l| l.can_revenge));
        assert_eq!(api_code(bob.revenge(log[0].attack_id)), "not_found");
        let r = alice.revenge(log[0].attack_id).unwrap();
        assert_eq!((r.kind.as_str(), r.defender.display_name.as_str()), ("revenge", "Bob"));
        // A one-troop army of a healer can't hurt the base: Bob holds, and
        // the server pays him for the defence.
        let weak = bag_client::army_from(&[(fixture("healer"), army_spots(1)[0])]);
        let b0 = bob.me().unwrap();
        let (_, lost) = alice.attack(&r, weak).unwrap();
        assert_eq!(lost.result.stars, 0);
        let b1 = bob.me().unwrap();
        assert_eq!(b1.gold, b0.gold - lost.gold_looted + 30);
        assert!(b1.shield_until.is_none(), "no shield for a held raid");
        let entry = bob.log("defense").unwrap();
        assert_eq!((entry[0].kind.as_str(), entry[0].defence_reward), ("revenge", 30));
        assert!(!alice.log("defense").unwrap()[0].can_revenge);
        assert_eq!(api_code(alice.revenge(log[0].attack_id)), "revenged");

        // Anyone can watch the best raid of the day: Bob's win, not practice.
        let mut carol = Client::new(&url);
        carol.dev_login("Carol").unwrap();
        let best = carol.raid_of_the_day().unwrap();
        assert_eq!(best.log.attack_id, won.attack_id);
        assert_eq!(bag_client::bag_protocol::run_raid(&best.snapshot, &best.army), won.result);
        // The Training Camp is not a player.
        assert!(carol.leaderboard(10).unwrap().iter().all(|p| p.display_name != "Training Camp"));
        let _ = &dbu;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn weekly_cup_ranks_and_pays() {
    use bag_client::bag_protocol::{cup_score, cup_week};
    let db = fresh_db().await;
    let db2 = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    let pool = sqlx::PgPool::connect(&db2).await.unwrap();
    let (week, alice_id) = blocking(move || {
        let mut alice = Client::new(&url);
        let a = alice.dev_login("Alice").unwrap();
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        assert!(alice.cup().unwrap().top.is_empty());

        // Defenders can't enter; attackers are scored like the game's own
        // Tournament, the same on any machine.
        assert!(matches!(alice.enter_cup(&fixture("guard")), Err(Error::Invalid(_))));
        let fighter = fixture("fighter");
        let s = alice.enter_cup(&fighter).unwrap();
        let mine = s.mine.unwrap();
        assert_eq!((s.entries, mine.rank), (1, 1));
        assert_eq!(mine.score, cup_score(&fighter));
        assert!(mine.prize.is_none());
        let s = bob.enter_cup(&fixture("healer")).unwrap();
        assert_eq!(s.entries, 2);
        assert!(s.top[0].score >= s.top[1].score);
        // Entering again replaces the entry.
        assert_eq!(bob.enter_cup(&fighter).unwrap().entries, 2);
        // Anyone can fetch an entry's troop to replay it.
        let r = bob.cup_replay(s.week, a.id).unwrap();
        assert_eq!(r.troop.actor.params, fighter.actor.params);
        (s.week, a.id)
    })
    .await;
    assert_eq!(week, cup_week(chrono::Utc::now().timestamp()));
    // The week is still open: nothing is paid.
    assert_eq!(bag_server::cup::pay_finished(&pool, week).await.unwrap(), 0);
    // Once it closes, everyone is paid once.
    assert_eq!(bag_server::cup::pay_finished(&pool, week + 1).await.unwrap(), 2);
    assert_eq!(bag_server::cup::pay_finished(&pool, week + 1).await.unwrap(), 0);
    let golds: Vec<i64> = sqlx::query_scalar("SELECT gold FROM users WHERE NOT is_bot ORDER BY gold DESC")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(golds, vec![500 + 500, 500 + 300]);
    let prize: Option<i64> = sqlx::query_scalar("SELECT prize FROM cup_entries WHERE user_id = $1")
        .bind(alice_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(matches!(prize, Some(500 | 300)));
}

#[tokio::test(flavor = "multi_thread")]
async fn old_raids_and_their_bases_are_cleaned_up() {
    let db = fresh_db().await;
    let db2 = db.clone();
    // No shields, so Bob can raid Alice twice.
    let url = serve(|u| Config {
        shield: Duration::ZERO,
        ..test_config(db, u, vec![])
    })
    .await;
    let pool = sqlx::PgPool::connect(&db2).await.unwrap();
    let dbu = db2.clone();
    let (old, new) = blocking(move || {
        let mut alice = Client::new(&url);
        alice.dev_login("Alice").unwrap();
        buy_town_hall(&dbu, &alice, 3);
        alice.publish_base(&home_defense()).unwrap();
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        bob.publish_base(&snapshot_from(&HomeBase::default(), &[]))
            .unwrap();
        let m = bob.find_match().unwrap();
        let (_, old) = bob.attack(&m, army()).unwrap();
        // Alice publishes a new base; the old one stays while a raid needs it.
        alice.publish_base(&home_defense()).unwrap();
        bob.find_match().unwrap(); // given up: expired
        let m = bob.find_match().unwrap();
        let (_, new) = bob.attack(&m, army()).unwrap();
        (old.attack_id, new.attack_id)
    })
    .await;
    let count = |q: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(q)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count("SELECT count(*) FROM snapshots").await, 3);
    sqlx::query("UPDATE attacks SET finished_at = now() - interval '15 days' WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE attacks SET expires_at = now() - interval '2 days' WHERE status = 'expired'",
    )
    .execute(&pool)
    .await
    .unwrap();
    bag_server::cleanup(&pool, Duration::from_secs(14 * 86_400))
        .await
        .unwrap();
    let left: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM attacks")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![new], "old and given-up raids are gone");
    // Alice's first base went with its raid; her current one and Bob's stay.
    assert_eq!(count("SELECT count(*) FROM snapshots").await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn bases_from_older_rules_are_not_matched() {
    let db = fresh_db().await;
    let db2 = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    let pool = sqlx::PgPool::connect(&db2).await.unwrap();
    let u2 = url.clone();
    let dbu = db2.clone();
    blocking(move || {
        let mut alice = Client::new(&u2);
        alice.dev_login("Alice").unwrap();
        buy_town_hall(&dbu, &alice, 3);
        alice.publish_base(&home_defense()).unwrap();
    })
    .await;
    sqlx::query("UPDATE snapshots SET sim_version = 1").execute(&pool).await.unwrap();
    blocking(move || {
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        bob.publish_base(&snapshot_from(&HomeBase::default(), &[])).unwrap();
        // Alice's old-rules base is never offered, not even for practice.
        assert_eq!(bob.find_match().unwrap().defender.display_name, "Training Camp");
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
async fn upgrades_only_count_once_paid_online() {
    use godcore::weapons::Weapon;
    let db = fresh_db().await;
    let dbu = db.clone();
    let url = serve(|u| test_config(db, u, vec![])).await;
    blocking(move || {
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap();
        bob.publish_base(&snapshot_from(&HomeBase::default(), &[])).unwrap();

        let mut alice = Client::new(&url);
        let me = alice.dev_login("Alice").unwrap();
        assert_eq!(me.upgrades.town_hall, 1);
        assert!(me.upgrades.weapons.is_empty() && me.upgrades.levels.is_empty());
        alice.publish_base(&snapshot_from(&HomeBase::default(), &[])).unwrap();

        // A level 2 troop (allowed by Town Hall 1) that was only levelled offline.
        let m = alice.find_match().unwrap();
        let mut a = army();
        a[0].troop.level = 2;
        assert_eq!(api_code(alice.submit(m.attack_id, a.clone(), None)), "invalid");

        // Levelling online takes its online level, so a repeat doesn't pay twice.
        assert_eq!(api_code(alice.level_up(1, 2)), "invalid");
        let me = alice.level_up(1, 1).unwrap();
        assert_eq!(me.gold, 500 - 100);
        assert_eq!(me.upgrades.level_of(1), 2);
        assert_eq!(me.upgrades.level_of(2), 1);
        assert_eq!(api_code(alice.level_up(1, 1)), "invalid");
        // Level 3 needs Town Hall 2.
        assert_eq!(api_code(alice.level_up(1, 2)), "invalid");

        // A weapon other than the class's own must be unlocked online too.
        a[1].troop.equip(Weapon::Daggers);
        assert_eq!(api_code(alice.submit(m.attack_id, a.clone(), None)), "invalid");
        let cost = Weapon::Daggers.stats().cost as i64;
        let me = alice.buy_weapon(Weapon::Daggers).unwrap();
        assert_eq!(me.gold, 400 - cost);
        assert_eq!(me.upgrades.weapons, vec![Weapon::Daggers]);
        assert_eq!(api_code(alice.buy_weapon(Weapon::Daggers)), "invalid");
        // The class's own weapon is always free.
        a[2].troop.equip(Weapon::HolyWand);

        // The same troop can't be sent twice.
        let mut twice = a.clone();
        twice[2].troop.id = 1;
        assert_eq!(api_code(alice.submit(m.attack_id, twice, None)), "invalid");

        // Now it counts.
        let out = alice.submit(m.attack_id, a, None).unwrap();
        assert_eq!(out.attacker.upgrades.level_of(1), 2);

        // A base above the online Town Hall is refused; buying it fixes that.
        let base = snapshot_from(&bag_client::starter_layout(2), &[]);
        assert_eq!(api_code(alice.publish_base(&base)), "invalid");
        set_gold(&dbu, &alice, 0);
        assert_eq!(api_code(alice.upgrade_town_hall()), "not_enough_gold");
        set_gold(&dbu, &alice, 200);
        let me = alice.upgrade_town_hall().unwrap();
        assert_eq!((me.gold, me.upgrades.town_hall), (0, 2));
        alice.publish_base(&base).unwrap();

        // Defenders follow the same rules.
        let mut g = troop("guard", 9);
        g.level = 2;
        let defs = vec![(g, defender_spots(1)[0])];
        assert_eq!(api_code(alice.publish_base(&snapshot_from(&base.base, &defs))), "invalid");
        set_gold(&dbu, &alice, 100);
        alice.level_up(9, 1).unwrap();
        alice.publish_base(&snapshot_from(&base.base, &defs)).unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn raids_are_rate_limited() {
    let db = fresh_db().await;
    let url = serve(|u| Config { heavy_per_min: 2, ..test_config(db, u, vec![]) }).await;
    blocking(move || {
        let mut bob = Client::new(&url);
        bob.dev_login("Bob").unwrap(); // sign-ins have their own, larger allowance
        assert_eq!(api_code(bob.find_match()), "no_opponent");
        assert_eq!(api_code(bob.find_match()), "no_opponent");
        assert_eq!(api_code(bob.find_match()), "slow_down");
        // Another player on the same address has their own allowance.
        let mut amy = Client::new(&url);
        amy.dev_login("Amy").unwrap();
        assert_eq!(api_code(amy.find_match()), "no_opponent");
        // Light requests still go through.
        assert_eq!(bob.me().unwrap().display_name, "Bob");
        let r = reqwest::blocking::Client::new()
            .post(format!("{url}/v1/matchmaking"))
            .bearer_auth(bob.token().unwrap())
            .header("x-sim-version", godcore::sim::SIM_VERSION.to_string())
            .json(&serde_json::json!({}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 429);
        assert!(r.headers().get("retry-after").is_some());
        // A big upload that is refused still gets its answer, not a dropped
        // connection: the server reads the body before saying slow down.
        let big = serde_json::json!({ "pad": "x".repeat(3_500_000) });
        for _ in 0..10 {
            let r = reqwest::blocking::Client::new()
                .put(format!("{url}/v1/me/base"))
                .bearer_auth(bob.token().unwrap())
                .header("x-sim-version", godcore::sim::SIM_VERSION.to_string())
                .json(&big)
                .send()
                .expect("an answer, not a broken connection");
            assert_eq!(r.status(), 429);
            assert!(r.text().unwrap().contains("slow_down"));
        }
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
