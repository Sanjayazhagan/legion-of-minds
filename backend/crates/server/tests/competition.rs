//! Leagues, seasons, clans, lent troops and clan wars, end to end against a
//! real Postgres (TEST_DATABASE_URL, default postgres://bag:bag@localhost/bag).

use bag_client::bag_protocol::{
    loot, BaseSnapshot, ClanRole, League, Placement, WarResult, WarState, MAX_LOANS,
};
use bag_client::{army_spots, defender_spots, snapshot_from, Client, Error};
use bag_server::config::Config;
use bag_server::{router, AppState};
use godcore::scenario::HomeBase;
use godcore::troop::Troop;
use std::time::Duration;

fn fixture(name: &str, id: u64) -> Troop {
    let p = format!("{}/tests/fixtures/{name}.troop.json", env!("CARGO_MANIFEST_DIR"));
    let mut t = Troop::load_json(&std::fs::read_to_string(p).unwrap()).unwrap();
    t.id = id;
    t
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

fn config(db: String, public_url: String) -> Config {
    Config {
        database_url: db,
        bind: "127.0.0.1:0".into(),
        public_url,
        dev_login: true,
        providers: vec![],
        session_ttl: Duration::from_secs(3600),
        login_ttl: Duration::from_secs(600),
        attack_window: Duration::from_secs(300),
        shield: Duration::from_secs(3600),
        start_gold: 500,
        match_range: 200,
        max_body_bytes: 4 << 20,
        season: Duration::from_secs(28 * 86_400),
        // Attacks open as soon as a war is made.
        war_prep: Duration::ZERO,
        war_battle: Duration::from_secs(3600),
        war_min_size: 2,
    }
}

async fn serve(db: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let cfg = config(db, url.clone());
    let pool = bag_server::connect(&cfg.database_url).await.unwrap();
    let app = router(AppState::new(pool, cfg));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

/// Run SQL (with values written in) against the test database.
fn sql(db: &str, q: &str) {
    let (db, q) = (db.to_string(), q.to_string());
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            let pool = sqlx::PgPool::connect(&db).await.unwrap();
            sqlx::query(&q).execute(&pool).await.unwrap();
        })
    })
    .join()
    .unwrap();
}

/// Run some server background work against the test database.
fn server_work<F, Fut>(db: &str, f: F)
where
    F: FnOnce(sqlx::PgPool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let db = db.to_string();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
            f(sqlx::PgPool::connect(&db).await.unwrap()).await
        })
    })
    .join()
    .unwrap();
}

fn api_code<T: std::fmt::Debug>(r: Result<T, Error>) -> String {
    match r {
        Err(Error::Api { code, .. }) => code,
        other => panic!("expected an API error, got {other:?}"),
    }
}

fn login(url: &str, name: &str) -> Client {
    let mut c = Client::new(url);
    c.dev_login(name).unwrap();
    c
}

/// Publish a Town Hall 3 base with two guards, buying the Town Hall online.
fn publish(db: &str, c: &Client) {
    let id = c.me().unwrap().id;
    sql(db, &format!("UPDATE users SET town_hall_level = 3 WHERE id = '{id}'"));
    let defs: Vec<_> = vec![fixture("guard", 1), fixture("guard", 2)]
        .into_iter()
        .zip(defender_spots(2))
        .collect();
    let snap: BaseSnapshot = snapshot_from(&HomeBase::reference(), &defs);
    c.publish_base(&snap).unwrap();
}

fn army() -> Vec<Placement> {
    let troops = vec![fixture("fighter", 1), fixture("fighter", 2), fixture("healer", 3)];
    let spots = army_spots(troops.len());
    bag_client::army_from(&troops.into_iter().zip(spots).collect::<Vec<_>>())
}

#[tokio::test(flavor = "multi_thread")]
async fn seasons_pay_by_league_and_reset_trophies() {
    let db = fresh_db().await;
    let url = serve(db.clone()).await;
    blocking(move || {
        let alice = login(&url, "Alice");
        let bob = login(&url, "Bob");
        let _quiet = login(&url, "Quiet");
        let (a, b) = (alice.me().unwrap().id, bob.me().unwrap().id);
        sql(&db, &format!("UPDATE users SET trophies = 1000 WHERE id = '{a}'"));
        sql(&db, &format!("UPDATE users SET trophies = 300 WHERE id = '{b}'"));

        let s1 = alice.season().unwrap();
        assert_eq!(s1.number, 1);
        assert_eq!(alice.season().unwrap().number, 1, "one season at a time");
        assert!(alice.my_seasons().unwrap().is_empty());

        // Not due yet: nothing happens.
        server_work(&db, |p| async move {
            assert_eq!(bag_server::league::roll(&p, Duration::from_secs(86_400)).await.unwrap(), None);
        });
        sql(&db, "UPDATE seasons SET ends_at = now() - interval '1 second'");
        server_work(&db, |p| async move {
            assert_eq!(bag_server::league::roll(&p, Duration::from_secs(86_400)).await.unwrap(), Some(1));
            assert_eq!(bag_server::league::roll(&p, Duration::from_secs(86_400)).await.unwrap(), None);
        });

        let r = alice.my_seasons().unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].season, r[0].rank, r[0].league), (1, 1, League::Gold));
        assert_eq!(r[0].reward, League::Gold.season_reward());
        let me = alice.me().unwrap();
        assert_eq!(me.trophies, 900, "trophies above 800 are halved");
        assert_eq!(me.gold, 500 + League::Gold.season_reward());

        let r = bob.my_seasons().unwrap();
        assert_eq!((r[0].rank, r[0].league), (2, League::Bronze));
        assert_eq!(bob.me().unwrap().trophies, 300);
        assert_eq!(bob.me().unwrap().gold, 500 + League::Bronze.season_reward());

        assert_eq!(alice.season().unwrap().number, 2);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn winning_raids_pay_a_league_bonus() {
    let db = fresh_db().await;
    let url = serve(db.clone()).await;
    blocking(move || {
        let alice = login(&url, "Alice");
        publish(&db, &alice);
        let bob = login(&url, "Bob");
        let b = bob.me().unwrap().id;
        sql(&db, &format!("UPDATE users SET trophies = 1700 WHERE id = '{b}'"));
        let m = bob.find_match().unwrap();
        let (_, out) = bob.attack(&m, army()).unwrap();
        let bonus = League::Master.win_bonus(out.result.stars);
        assert_eq!(out.league_bonus, bonus);
        assert_eq!(out.attacker.gold, 500 + loot(500, out.result.destruction) + bonus);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clans_have_roles_and_chat() {
    let db = fresh_db().await;
    let url = serve(db.clone()).await;
    blocking(move || {
        let alice = login(&url, "Alice");
        let bob = login(&url, "Bob");
        let carol = login(&url, "Carol");

        assert_eq!(api_code(alice.my_clan()), "not_in_clan");
        assert_eq!(api_code(alice.create_clan("ab", "", true)), "invalid");
        let v = alice.create_clan("  Iron   Wolves ", "We raid at dawn", true).unwrap();
        assert_eq!(v.clan.name, "Iron Wolves");
        assert_eq!(v.members.len(), 1);
        assert_eq!(v.members[0].role, ClanRole::Leader);
        let id = v.clan.id;
        assert_eq!(api_code(bob.create_clan("iron wolves", "", true)), "name_taken");
        assert_eq!(api_code(alice.create_clan("Another", "", true)), "in_clan");

        bob.join_clan(id).unwrap();
        carol.join_clan(id).unwrap();
        assert_eq!(api_code(carol.join_clan(id)), "in_clan");
        let tag = bob.me().unwrap().clan.unwrap();
        assert_eq!((tag.id, tag.role), (id, ClanRole::Member));
        assert_eq!(bob.my_clan().unwrap().members.len(), 3);

        alice.say("hello  wolves").unwrap();
        let chat = carol.chat(0).unwrap();
        let lines: Vec<&str> = chat.iter().map(|m| m.body.as_str()).collect();
        assert!(lines.contains(&"Alice founded Iron Wolves"), "{lines:?}");
        assert!(lines.contains(&"Carol joined"));
        assert_eq!(chat.last().unwrap().body, "hello wolves");
        assert_eq!(chat.last().unwrap().from, "Alice");
        let after = chat.last().unwrap().id;
        assert!(carol.chat(after).unwrap().is_empty());
        assert_eq!(api_code(carol.say("   ")), "invalid");

        // Members can't kick; elders can kick members; only the leader promotes.
        let (b, c) = (bob.me().unwrap().id, carol.me().unwrap().id);
        assert_eq!(api_code(bob.kick(c)), "forbidden");
        assert_eq!(api_code(bob.promote(c)), "forbidden");
        alice.promote(b).unwrap();
        let v = bob.kick(c).unwrap();
        assert_eq!(v.members.len(), 2);
        assert!(carol.me().unwrap().clan.is_none());

        // Promoting an elder hands over the lead.
        alice.promote(b).unwrap();
        assert_eq!(bob.me().unwrap().clan.unwrap().role, ClanRole::Leader);
        assert_eq!(alice.me().unwrap().clan.unwrap().role, ClanRole::Elder);
        // The leader leaving hands it back; the last one out closes the clan.
        bob.leave_clan().unwrap();
        assert_eq!(alice.me().unwrap().clan.unwrap().role, ClanRole::Leader);
        assert_eq!(carol.clans("wolves").unwrap().len(), 1);
        alice.leave_clan().unwrap();
        assert!(carol.clans("wolves").unwrap().is_empty());

        // Clanmates aren't matched against each other.
        let v = alice.create_clan("Pack", "", true).unwrap();
        bob.join_clan(v.clan.id).unwrap();
        publish(&db, &alice);
        assert_eq!(api_code(bob.find_match()), "no_opponent");
        assert_eq!(carol.find_match().unwrap().defender.display_name, "Alice");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lent_troops_fight_in_raids() {
    let db = fresh_db().await;
    let url = serve(db.clone()).await;
    blocking(move || {
        let alice = login(&url, "Alice");
        let bob = login(&url, "Bob");
        let carol = login(&url, "Carol");
        publish(&db, &carol);
        let v = alice.create_clan("Lenders", "", true).unwrap();
        bob.join_clan(v.clan.id).unwrap();

        let three = vec![fixture("fighter", 7), fixture("fighter", 8), fixture("fighter", 9)];
        assert_eq!(api_code(bob.lend(three)), "invalid");
        assert_eq!(api_code(bob.lend(vec![fixture("guard", 7)])), "invalid", "attackers only");
        let mine = bob.lend(vec![fixture("fighter", 7)]).unwrap();
        assert_eq!(mine.len(), 1);
        assert!(MAX_LOANS >= 1);
        assert!(bob.loans().unwrap().is_empty(), "you can't borrow your own");
        assert_eq!(alice.my_clan().unwrap().members.iter().map(|m| m.lending).sum::<u32>(), 1);

        let loans = alice.loans().unwrap();
        assert_eq!(loans.len(), 1);
        assert_eq!(loans[0].owner.display_name, "Bob");
        let spot = army_spots(4)[3];
        let m = alice.find_match().unwrap();
        let (local, out) = alice
            .attack_with(&m, army(), &[(loans[0].clone(), spot)])
            .unwrap();
        assert_eq!(out.client_matched, Some(true));
        assert_eq!(local, out.result);
        let r = alice.replay(out.attack_id).unwrap();
        assert_eq!(r.army.len(), 4);
        assert_eq!(r.army[3].troop.id, 7);

        // A loan disappears when its owner leaves the clan.
        bob.leave_clan().unwrap();
        assert!(alice.loans().unwrap().is_empty());
        let dave = login(&url, "Dave");
        publish(&db, &dave);
        let m = alice.find_match().unwrap();
        // The game still has the old loan; the server no longer lends it.
        let err = alice.attack_with(&m, army(), &[(loans[0].clone(), spot)]);
        assert_eq!(api_code(err), "invalid");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clan_wars_are_fought_and_scored() {
    let db = fresh_db().await;
    let url = serve(db.clone()).await;
    blocking(move || {
        let alice = login(&url, "Alice");
        let bob = login(&url, "Bob");
        let carol = login(&url, "Carol");
        let dave = login(&url, "Dave");
        let wolves = alice.create_clan("Wolves", "", true).unwrap().clan.id;
        bob.join_clan(wolves).unwrap();
        let bears = carol.create_clan("Bears", "", true).unwrap().clan.id;
        dave.join_clan(bears).unwrap();

        assert_eq!(api_code(alice.war()), "no_war");
        assert_eq!(api_code(alice.search_war()), "too_few");
        for c in [&alice, &bob, &carol, &dave] {
            publish(&db, c);
        }
        assert_eq!(api_code(bob.search_war()), "forbidden");
        let s = alice.search_war().unwrap();
        assert!(s.searching && s.war.is_none());
        assert!(alice.my_clan().unwrap().war_searching);
        let s = carol.search_war().unwrap();
        let w = s.war.expect("matched with the searching clan");
        assert_eq!(w.state, WarState::Battle);
        assert_eq!((w.us.name.as_str(), w.them.name.as_str()), ("Bears", "Wolves"));
        assert_eq!((w.us.bases.len(), w.them.bases.len()), (2, 2));
        assert!(!alice.my_clan().unwrap().war_searching);
        assert_eq!(api_code(alice.search_war()), "in_war");

        let w = alice.war().unwrap();
        assert!(w.in_war);
        assert_eq!(w.my_attacks_left, 2);
        let carol_id = carol.me().unwrap().id;
        let bob_id = bob.me().unwrap().id;
        assert_eq!(api_code(alice.war_target(bob_id)), "not_found", "not your own side");

        let m = alice.war_target(carol_id).unwrap();
        assert_eq!(m.max_loot, 0);
        let gold_before = alice.me().unwrap().gold;
        let (local, out) = alice.war_attack(&m, army(), &[]).unwrap();
        assert_eq!(out.client_matched, Some(true));
        assert_eq!(local, out.result);
        assert_eq!(out.new_stars, out.result.stars);
        assert_eq!(out.war.us.stars, out.result.stars);
        assert_eq!(out.war.my_attacks_left, 1);
        let first = out.result.clone();

        // The same attack again adds no new stars.
        let m = alice.war_target(carol_id).unwrap();
        let (_, out) = alice.war_attack(&m, army(), &[]).unwrap();
        assert_eq!(out.result, first, "same army, same frozen base, same battle");
        assert_eq!(out.new_stars, 0);
        assert_eq!(api_code(alice.war_target(carol_id)), "no_attacks");
        // The defender sees the stars against them.
        let cw = carol.war().unwrap();
        assert_eq!(cw.them.stars, first.stars);
        assert_eq!(cw.us.bases.iter().find(|b| b.player.id == carol_id).unwrap().best_stars, first.stars);

        // End the war.
        sql(&db, "UPDATE wars SET ends_at = now() - interval '1 second'");
        server_work(&db, |p| async move { bag_server::wars::settle(&p).await.unwrap() });
        let w = alice.war().unwrap();
        assert_eq!(w.state, WarState::Done);
        let expect = if first.stars > 0 || first.destruction > 0.0 {
            WarResult::Won
        } else {
            WarResult::Draw
        };
        assert_eq!(w.result, Some(expect));
        assert_eq!(alice.me().unwrap().gold, gold_before + expect.reward());
        let wolves_view = alice.clan(wolves).unwrap();
        assert_eq!(wolves_view.clan.war_wins + wolves_view.clan.war_draws, 1);
        // A new war can be searched for now.
        assert!(alice.search_war().unwrap().searching);
    })
    .await;
}
