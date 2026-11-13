//! bagsim: a crowd of pretend players for a local server, to find what
//! breaks before launch, plus a balance check of classes and armies.
//!
//! ```text
//! bagsim crowd  --troops DIR [--server URL] [--players 100] [--secs 120]
//!               [--think-ms 800] [--local-check 0.2] [--chaos] [--out report.json]
//! bagsim matrix --troops DIR [--out matrix.json]
//! bagsim economy --troops DIR [--players 200] [--days 30] [--defence-reward 30] [--daily-bonus 100] [--out economy.json]
//! ```
//!
//! `--troops DIR` holds trained troop files (`godctl train ... --level 1`),
//! attackers and defenders. Each pretend player picks a strategy (which
//! classes it raids with and defends with) and plays: publish a base, find a
//! match, raid, buy upgrades, check the log and the leaderboard, and now and
//! then publish a new base. With `--chaos`, extra players misbehave: old game
//! versions, broken uploads, double submits, double purchases, tampered
//! troops, cut-off connections. The server needs `DEV_LOGIN=true`.
//!
//! `matrix` and `economy` run no server: they use the same battle code
//! (`run_raid`) to see which armies beat which bases and where the gold goes.

use bag_client::bag_protocol::{
    canonical_army, canonical_snapshot, loot, run_raid, trophy_swing, BaseSnapshot, Placement,
    SIM_VERSION, SIM_VERSION_HEADER,
};
use bag_client::*;
use godcore::classes::Role;
use godcore::rng::Rng;
use godcore::scenario::{max_troop_level, town_hall_cost};
use godcore::troop::Troop;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const USAGE: &str = "bagsim crowd  --troops DIR [--server URL] [--players 100] [--secs 120] [--think-ms 800] [--local-check 0.2] [--chaos] [--out F]
bagsim matrix --troops DIR [--out F]
bagsim economy --troops DIR [--players 200] [--days 30] [--out F]";

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    flag(args, name)
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| die(&format!("{name} must be a number")))
        })
        .unwrap_or(default)
}

fn die(msg: &str) -> ! {
    eprintln!("bagsim: {msg}\n{USAGE}");
    std::process::exit(1)
}

fn write_out(args: &[String], v: &impl Serialize) {
    if let Some(p) = flag(args, "--out") {
        std::fs::write(&p, serde_json::to_string_pretty(v).unwrap())
            .unwrap_or_else(|e| die(&format!("{p}: {e}")));
        println!("\nwrote {p}");
    }
}

// ---------------------------------------------------------------- troops

#[derive(Clone)]
struct Kind {
    name: String,
    troop: Troop,
}

struct Roster {
    attackers: Vec<Kind>,
    defenders: Vec<Kind>,
}

fn load_roster(dir: &str) -> Roster {
    let mut r = Roster {
        attackers: Vec::new(),
        defenders: Vec::new(),
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| die(&format!("{dir}: {e}")))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".troop.json"))
        .collect();
    paths.sort();
    for p in paths {
        let s = std::fs::read_to_string(&p).unwrap();
        let t = Troop::load_json(&s).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
        let name = p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".troop.json")
            .to_string();
        let k = Kind { name, troop: t };
        match k.troop.spec.role {
            Role::Attack => r.attackers.push(k),
            Role::Defend => r.defenders.push(k),
        }
    }
    if r.attackers.is_empty() || r.defenders.is_empty() {
        die("--troops needs at least one attacker and one defender troop file");
    }
    r
}

/// Copies of troops with ids from `first`, so no troop is sent twice.
fn team(kinds: &[&Kind], first: u64) -> Vec<Troop> {
    kinds
        .iter()
        .enumerate()
        .map(|(k, kind)| {
            let mut t = kind.troop.clone();
            t.id = first + k as u64;
            t.level = 1;
            t
        })
        .collect()
}

/// A way to play: what the army and the defence are made of.
#[derive(Clone)]
struct Strategy {
    army_name: String,
    army: Vec<usize>,
    _base_name: String,
    base: Vec<usize>,
}

/// Armies: five of one class, or a mix. Bases: four of one class, or a mix.
fn armies(r: &Roster) -> Vec<(String, Vec<usize>)> {
    let n = r.attackers.len();
    let mut v: Vec<(String, Vec<usize>)> = (0..n)
        .map(|i| (format!("all {}", r.attackers[i].name), vec![i; 5]))
        .collect();
    v.push(("mixed".into(), (0..5).map(|k| k % n).collect()));
    v
}

fn bases(r: &Roster) -> Vec<(String, Vec<usize>)> {
    let n = r.defenders.len();
    let mut v: Vec<(String, Vec<usize>)> = (0..n)
        .map(|i| (format!("all {}", r.defenders[i].name), vec![i; 4]))
        .collect();
    v.push(("mixed".into(), (0..4).map(|k| k % n).collect()));
    v
}

fn army_of(r: &Roster, idx: &[usize]) -> Vec<Placement> {
    let kinds: Vec<&Kind> = idx.iter().map(|&i| &r.attackers[i]).collect();
    let troops = team(&kinds, 1);
    army_from(
        &troops
            .into_iter()
            .zip(army_spots(idx.len()))
            .collect::<Vec<_>>(),
    )
}

fn base_of(r: &Roster, idx: &[usize], town_hall: u32) -> BaseSnapshot {
    let kinds: Vec<&Kind> = idx.iter().map(|&i| &r.defenders[i]).collect();
    let troops = team(&kinds, 100);
    snapshot_from(
        &starter_layout(town_hall),
        &troops
            .into_iter()
            .zip(defender_spots(idx.len()))
            .collect::<Vec<_>>(),
    )
}

// ---------------------------------------------------------------- stats

#[derive(Default, Serialize, Clone)]
struct CallStats {
    count: u64,
    /// Answer (e.g. "ok", "409 update_required", "network") -> count.
    answers: BTreeMap<String, u64>,
    #[serde(skip)]
    ms: Vec<f64>,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

#[derive(Serialize, Clone)]
struct RaidRow {
    /// "raid", "practice" or "revenge".
    kind: String,
    bonus_gold: i64,
    army: String,
    base: String,
    stars: u32,
    destruction: f32,
    gold: i64,
    trophies: i32,
    client_matched: Option<bool>,
}

#[derive(Serialize, Clone)]
struct Problem {
    test: String,
    expected: String,
    got: String,
}

#[derive(Default)]
struct Stats {
    calls: BTreeMap<String, CallStats>,
    raids: Vec<RaidRow>,
    problems: Vec<Problem>,
    checks_passed: BTreeMap<String, u64>,
}

type Shared = Arc<Mutex<Stats>>;

fn answer<T>(r: &Result<T>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(Error::Api { status, code, .. }) => format!("{status} {code}"),
        Err(Error::Network(e)) => {
            if e.contains("timed out") || e.contains("timeout") {
                "network: timed out".into()
            } else {
                "network".into()
            }
        }
        Err(Error::Invalid(_)) => "refused before sending".into(),
        Err(Error::LoginTimedOut) => "login timed out".into(),
    }
}

/// Time one call and count its answer.
fn timed<T>(st: &Shared, name: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let t0 = Instant::now();
    let r = f();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut s = st.lock().unwrap();
    let c = s.calls.entry(name.to_string()).or_default();
    c.count += 1;
    c.ms.push(ms);
    *c.answers.entry(answer(&r)).or_default() += 1;
    r
}

fn check(st: &Shared, test: &str, ok: bool, expected: &str, got: impl Into<String>) {
    let mut s = st.lock().unwrap();
    if ok {
        *s.checks_passed.entry(test.to_string()).or_default() += 1;
    } else {
        s.problems.push(Problem {
            test: test.into(),
            expected: expected.into(),
            got: got.into(),
        });
    }
}

fn is_code<T>(r: &Result<T>, codes: &[&str]) -> bool {
    matches!(r, Err(Error::Api { code, .. }) if codes.contains(&code.as_str()))
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    (v[i] * 10.0).round() / 10.0
}

// ---------------------------------------------------------------- crowd

struct Crowd {
    server: String,
    roster: Roster,
    stop: AtomicBool,
    think_ms: u64,
    local_check: f32,
    seq: AtomicU64,
}

impl Crowd {
    fn running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed)
    }

    fn nap(&self, rng: &mut Rng) {
        let ms = (self.think_ms as f32 * rng.range(0.5, 1.5)) as u64;
        std::thread::sleep(Duration::from_millis(ms));
    }

    fn login(&self, st: &Shared, name: &str) -> Option<Client> {
        let mut c = Client::new(&self.server);
        for _ in 0..5 {
            if timed(st, "sign in", || c.dev_login(name)).is_ok() {
                return Some(c);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        None
    }
}

/// An honest player: plays the game the way the desktop game would.
fn player(crowd: &Crowd, st: &Shared, id: usize, s: Strategy) {
    let mut rng = Rng::new(id as u64 * 7919 + 13);
    let Some(c) = crowd.login(st, &format!("sim{id}")) else {
        return;
    };
    let r = &crowd.roster;
    let mut th = 1;
    let _ = timed(st, "publish base", || {
        c.publish_base(&base_of(r, &s.base, th))
    });
    while crowd.running() {
        crowd.nap(&mut rng);
        let roll = rng.f32();
        if roll < 0.55 {
            let m = timed(st, "find match", || c.find_match());
            let Ok(m) = m else { continue };
            // The player watches the battle before it is sent.
            std::thread::sleep(Duration::from_millis(rng.range(200.0, 1500.0) as u64));
            let army = army_of(r, &s.army);
            let local = rng
                .chance(crowd.local_check)
                .then(|| run_raid(&m.snapshot, &army));
            let out = timed(st, "submit raid", || c.submit(m.attack_id, army, local));
            if let Ok(o) = out {
                let base = describe_base(r, &m.snapshot);
                st.lock().unwrap().raids.push(RaidRow {
                    kind: m.kind.clone(),
                    bonus_gold: o.bonus_gold,
                    army: s.army_name.clone(),
                    base,
                    stars: o.result.stars,
                    destruction: o.result.destruction,
                    gold: o.gold_looted,
                    trophies: o.trophies,
                    client_matched: o.client_matched,
                });
                if o.client_matched == Some(false) {
                    check(
                        st,
                        "server and player agree on a raid",
                        false,
                        "same result",
                        "different result",
                    );
                }
            }
        } else if roll < 0.70 {
            let _ = timed(st, "me", || c.me());
        } else if roll < 0.80 {
            let _ = timed(st, "battle log", || c.log("all"));
        } else if roll < 0.88 {
            let _ = timed(st, "leaderboard", || c.leaderboard(50));
        } else if roll < 0.96 {
            // Spend gold: the next Town Hall if affordable, then publish a
            // base that uses it.
            let Ok(me) = timed(st, "me", || c.me()) else {
                continue;
            };
            if th < 5 && me.gold >= town_hall_cost(th) as i64 {
                if timed(st, "buy town hall", || c.upgrade_town_hall()).is_ok() {
                    th += 1;
                    let _ = timed(st, "publish base", || {
                        c.publish_base(&base_of(r, &s.base, th))
                    });
                }
            }
        } else {
            let _ = timed(st, "publish base", || {
                c.publish_base(&base_of(r, &s.base, th))
            });
        }
    }
}

fn describe_base(r: &Roster, snap: &BaseSnapshot) -> String {
    let names: Vec<&str> = snap
        .defenders
        .iter()
        .map(|p| {
            r.defenders
                .iter()
                .find(|k| k.troop.actor.params == p.troop.actor.params)
                .map_or("?", |k| k.name.as_str())
        })
        .collect();
    if names.windows(2).all(|w| w[0] == w[1]) && !names.is_empty() {
        format!("all {}", names[0])
    } else {
        "mixed".into()
    }
}

/// Raw HTTP, for requests the client library would never send.
fn raw(
    server: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String) {
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let mut r = http.request(method.parse().unwrap(), format!("{server}{path}"));
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    match r.body(body.to_vec()).send() {
        Ok(resp) => {
            let s = resp.status().as_u16();
            (s, resp.text().unwrap_or_default())
        }
        Err(e) => (0, format!("network: {e}")),
    }
}

/// Players who misbehave on purpose. Each test says what a healthy server
/// should answer; anything else is a problem.
fn chaos(crowd: &Crowd, st: &Shared, id: usize) {
    let server = crowd.server.clone();
    let r = &crowd.roster;
    let n = crowd.seq.fetch_add(1, Ordering::Relaxed);
    let Some(c) = crowd.login(st, &format!("chaos{id}_{n}")) else {
        return;
    };
    let token = c.token().unwrap().to_string();
    let auth = format!("Bearer {token}");
    let ver = SIM_VERSION.to_string();
    let old = (SIM_VERSION - 1).to_string();
    let _ = c.publish_base(&base_of(r, &[0], 1));

    // 1. An old game version.
    let (s, _) = raw(
        &server,
        "POST",
        "/v1/matchmaking",
        &[
            ("authorization", &auth),
            (SIM_VERSION_HEADER, &old),
            ("content-type", "application/json"),
        ],
        b"{}",
    );
    check(
        st,
        "old game version is told to update",
        s == 409,
        "409",
        s.to_string(),
    );
    let big = serde_json::to_vec(&base_of(r, &[0, 0, 0, 0], 1)).unwrap();
    let (s, b) = raw(
        &server,
        "PUT",
        "/v1/me/base",
        &[
            ("authorization", &auth),
            (SIM_VERSION_HEADER, &old),
            ("content-type", "application/json"),
        ],
        &big,
    );
    check(
        st,
        "old game version publishing a base is told to update",
        s == 409 && b.contains("update_required"),
        "409 update_required",
        format!("{s} {}", short(&b)),
    );

    // 2. Broken and odd uploads.
    let (s, _) = raw(
        &server,
        "PUT",
        "/v1/me/base",
        &[
            ("authorization", &auth),
            (SIM_VERSION_HEADER, &ver),
            ("content-type", "application/json"),
        ],
        b"{\"base\": [1,2",
    );
    check(
        st,
        "half-written base is refused",
        (400..500).contains(&s),
        "4xx",
        s.to_string(),
    );
    let (s, _) = raw(
        &server,
        "PUT",
        "/v1/me/base",
        &[
            ("authorization", &auth),
            (SIM_VERSION_HEADER, &ver),
            ("content-type", "application/json"),
        ],
        b"null",
    );
    check(
        st,
        "empty base is refused",
        (400..500).contains(&s),
        "4xx",
        s.to_string(),
    );
    let huge = vec![b' '; 5 << 20];
    let (s, b) = raw(
        &server,
        "PUT",
        "/v1/me/base",
        &[
            ("authorization", &auth),
            (SIM_VERSION_HEADER, &ver),
            ("content-type", "application/json"),
        ],
        &huge,
    );
    check(
        st,
        "5 MB upload gets a clear 'too big' answer",
        s == 413,
        "413",
        format!("{s} {}", short(&b)),
    );
    let (s, _) = raw(
        &server,
        "GET",
        "/v1/me",
        &[("authorization", "Bearer bag_nottherealtoken")],
        b"",
    );
    check(
        st,
        "made-up token is refused",
        s == 401,
        "401",
        s.to_string(),
    );
    let (s, _) = raw(&server, "GET", "/v1/leaderboard?limit=-5", &[], b"");
    check(
        st,
        "odd leaderboard size still works",
        s == 200,
        "200",
        s.to_string(),
    );
    let (s, _) = raw(&server, "GET", "/v1/leaderboard?limit=abc", &[], b"");
    check(
        st,
        "nonsense leaderboard size is refused",
        s == 400,
        "400",
        s.to_string(),
    );
    let (s, _) = raw(
        &server,
        "GET",
        "/v1/players/not-a-uuid",
        &[("authorization", &auth)],
        b"",
    );
    check(
        st,
        "nonsense player id is refused",
        (400..500).contains(&s),
        "4xx",
        s.to_string(),
    );
    let name = "\u{202e}ADMIN\u{0000}\u{200b}".to_string() + &"x".repeat(5000);
    let renamed = c.rename(&name);
    check(
        st,
        "very long or strange names are cleaned",
        matches!(&renamed, Ok(u) if u.display_name.chars().count() <= 24 && !u.display_name.contains('\0')),
        "short clean name",
        match &renamed {
            Ok(u) => format!("{:?}", u.display_name),
            Err(e) => e.to_string(),
        },
    );

    // 3. Tampered troops.
    let mut army = army_of(r, &[0]);
    army[0].troop.steps = 50_000_000;
    let t = c.find_match();
    if let Ok(m) = &t {
        let x = c.submit(m.attack_id, army.clone(), None);
        check(
            st,
            "over-trained troop is refused",
            is_code(&x, &["invalid"]),
            "422 invalid",
            answer(&x),
        );
        let mut lvl = army_of(r, &[0]);
        lvl[0].troop.level = 5;
        lvl[0].troop.steps = 0;
        let x = c.submit(m.attack_id, lvl, None);
        check(
            st,
            "level 5 troop without paying online is refused",
            is_code(&x, &["invalid"]),
            "422 invalid",
            answer(&x),
        );
        let mut twice = army_of(r, &[0, 0]);
        twice[1].troop.id = twice[0].troop.id;
        let x = c.submit(m.attack_id, twice, None);
        check(
            st,
            "same troop sent twice is refused",
            is_code(&x, &["invalid"]),
            "422 invalid",
            answer(&x),
        );
        let mut def = army_of(r, &[0]);
        def[0].troop = r.defenders[0].troop.clone();
        let x = c.submit(m.attack_id, def, None);
        check(
            st,
            "defender sent to attack is refused",
            is_code(&x, &["invalid"]),
            "422 invalid",
            answer(&x),
        );
        let mut inside = army_of(r, &[0]);
        inside[0].x = 12.0;
        inside[0].y = 12.0;
        let x = c.submit(m.attack_id, inside, None);
        check(
            st,
            "troop dropped inside the base is refused",
            is_code(&x, &["invalid"]),
            "422 invalid",
            answer(&x),
        );
        // A lie about the result is caught and the server's result counts.
        let mut fake = run_raid(&m.snapshot, &canonical_army(&army_of(r, &[0]), 1).unwrap());
        fake.stars = 3;
        fake.fingerprint = "0000000000000000".into();
        let x = c.submit(m.attack_id, army_of(r, &[0]), Some(fake));
        check(
            st,
            "lying about the raid result is caught",
            matches!(&x, Ok(o) if o.client_matched == Some(false)),
            "client_matched false",
            match &x {
                Ok(o) => format!("client_matched {:?}", o.client_matched),
                Err(e) => format!("{e}"),
            },
        );
        // Sending the same raid again.
        let x = c.submit(m.attack_id, army_of(r, &[0]), None);
        check(
            st,
            "raid sent a second time is refused",
            is_code(&x, &["raid_closed"]),
            "409 raid_closed",
            answer(&x),
        );
    }

    // 4. Double submit at the same instant: gold and trophies change once.
    if let Ok(m) = c.find_match() {
        let (seen, before) = steady(&c);
        let army = army_of(r, &[0, 0, 0]);
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let (srv, tok, army, id) =
                    (server.clone(), token.clone(), army.clone(), m.attack_id);
                std::thread::spawn(move || Client::new(&srv).with_token(tok).submit(id, army, None))
            })
            .collect();
        let res: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        let oks: Vec<_> = res.iter().filter_map(|r| r.as_ref().ok()).collect();
        let after = c.me().map(|u| u.gold);
        let all_known = res
            .iter()
            .all(|r| r.is_ok() || is_code(r, &["raid_closed"]));
        check(
            st,
            "same raid sent 4 times at once counts once",
            oks.len() == 1 && all_known,
            "1 ok, 3 raid_closed",
            res.iter().map(answer).collect::<Vec<_>>().join(", "),
        );
        if let (Ok(b), Ok(a), Some(o)) = (before, after, oks.first()) {
            // Other players may raid this one meanwhile; count those too.
            let want = b + o.gold_looted + o.bonus_gold - defended_since(&c, &seen).0;
            check(
                st,
                "gold from a raid sent 4 times at once is paid once",
                a == want,
                &want.to_string(),
                a.to_string(),
            );
        }
    }

    // 5. Double purchases at the same instant.
    let (seen, me) = steady_user(&c);
    if let Ok(me) = me {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let (srv, tok) = (server.clone(), token.clone());
                std::thread::spawn(move || {
                    Client::new(&srv)
                        .with_token(tok)
                        .buy_weapon(godcore::weapons::Weapon::Daggers)
                })
            })
            .collect();
        let res: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        let oks = res.iter().filter(|r| r.is_ok()).count();
        let after = c.me().map(|u| u.gold).unwrap_or(-1);
        let price = godcore::weapons::Weapon::Daggers.stats().cost as i64;
        let afford = me.gold >= price
            && !me
                .upgrades
                .weapons
                .contains(&godcore::weapons::Weapon::Daggers);
        let lost = defended_since(&c, &seen).0;
        let ok = if afford {
            oks == 1 && after == me.gold - price - lost
        } else {
            oks == 0 && after == me.gold - lost
        };
        check(
            st,
            "same weapon bought 4 times at once is paid once",
            ok,
            "paid once",
            format!(
                "{oks} bought, gold {} -> {after}; answers: {}",
                me.gold,
                res.iter().map(answer).collect::<Vec<_>>().join(", ")
            ),
        );
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let (srv, tok) = (server.clone(), token.clone());
                std::thread::spawn(move || Client::new(&srv).with_token(tok).level_up(7, 1))
            })
            .collect();
        let res: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        let oks = res.iter().filter(|r| r.is_ok()).count();
        check(
            st,
            "same level-up sent 4 times at once is paid at most once",
            oks <= 1
                && res
                    .iter()
                    .all(|r| r.is_ok() || is_code(r, &["invalid", "not_enough_gold"])),
            "at most 1 ok, the rest refused",
            res.iter().map(answer).collect::<Vec<_>>().join(", "),
        );
    }

    // 6. Two first sign-ins with the same new account at once.
    let fresh = format!("twin{id}_{n}");
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let (srv, nm) = (server.clone(), fresh.clone());
            std::thread::spawn(move || Client::new(&srv).dev_login(&nm).map(|u| u.id))
        })
        .collect();
    let res: Vec<_> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    let ids: std::collections::BTreeSet<_> = res.iter().filter_map(|r| r.as_ref().ok()).collect();
    check(
        st,
        "same new account signing in 4 times at once gets one account",
        ids.len() == 1 && res.iter().all(|r| r.is_ok()),
        "4 ok, 1 account",
        res.iter().map(answer).collect::<Vec<_>>().join(", "),
    );

    // 7. A connection cut off half way through an upload.
    if let Some(addr) = server.strip_prefix("http://") {
        if let Ok(mut s) = std::net::TcpStream::connect(addr) {
            let head = format!("PUT /v1/me/base HTTP/1.1\r\nhost: {addr}\r\nauthorization: {auth}\r\n{SIM_VERSION_HEADER}: {ver}\r\ncontent-type: application/json\r\ncontent-length: 900000\r\n\r\n");
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&big[..big.len().min(20_000)]);
            drop(s);
        }
        // A connection that says nothing at all, held open a while.
        let idle = std::net::TcpStream::connect(addr).ok();
        std::thread::sleep(Duration::from_millis(300));
        let (s, _) = raw(&server, "GET", "/health", &[], b"");
        check(
            st,
            "server still answers after cut-off uploads",
            s == 200,
            "200",
            s.to_string(),
        );
        drop(idle);
    }

    // 8. Raiding without publishing a base: a fresh account.
    if let Some(ghost) = crowd.login(st, &format!("ghost{id}_{n}")) {
        let x = ghost.find_match();
        check(
            st,
            "a player with no base of their own can't raid",
            is_code(&x, &["no_base"]),
            "409 no_base",
            answer(&x),
        );
    }

    // 9. Old raid after finding a new match.
    if let (Ok(a), Ok(_b)) = (c.find_match(), c.find_match()) {
        let x = c.submit(a.attack_id, army_of(r, &[0]), None);
        check(
            st,
            "raid on a base given up by finding another is refused",
            is_code(&x, &["raid_closed"]),
            "409 raid_closed",
            answer(&x),
        );
    }

    // 10. Someone else's raid.
    if let Some(other) = crowd.login(st, &format!("other{id}_{n}")) {
        let _ = other.publish_base(&base_of(r, &[0], 1));
        if let Ok(m) = other.find_match() {
            let x = c.submit(m.attack_id, army_of(r, &[0]), None);
            check(
                st,
                "sending someone else's raid is refused",
                is_code(&x, &["not_found"]),
                "404 not_found",
                answer(&x),
            );
        }
    }
}

/// Raids on this player so far.
fn defended(c: &Client) -> std::collections::BTreeSet<Uuid> {
    c.log("defense")
        .unwrap_or_default()
        .iter()
        .map(|l| l.attack_id)
        .collect()
}

/// The raids on this player and their account, read while no raid on them
/// finished in between.
fn steady_user(
    c: &Client,
) -> (
    std::collections::BTreeSet<Uuid>,
    Result<bag_client::bag_protocol::User>,
) {
    loop {
        let seen = defended(c);
        let me = c.me();
        if defended(c) == seen {
            return (seen, me);
        }
    }
}

fn steady(c: &Client) -> (std::collections::BTreeSet<Uuid>, Result<i64>) {
    let (seen, me) = steady_user(c);
    (seen, me.map(|u| u.gold))
}

/// Gold lost to raids not in `seen`.
fn defended_since(c: &Client, seen: &std::collections::BTreeSet<Uuid>) -> (i64, ()) {
    let g = c
        .log("defense")
        .unwrap_or_default()
        .iter()
        .filter(|l| !seen.contains(&l.attack_id))
        .map(|l| l.gold_looted)
        .sum();
    (g, ())
}

fn short(s: &str) -> String {
    s.chars().take(120).collect()
}

fn crowd(args: &[String]) {
    let roster = load_roster(&flag(args, "--troops").unwrap_or_else(|| die("--troops DIR")));
    let server = flag(args, "--server").unwrap_or_else(|| "http://127.0.0.1:8080".into());
    let players: usize = num(args, "--players", 100);
    let secs: u64 = num(args, "--secs", 120);
    let with_chaos = args.iter().any(|a| a == "--chaos");
    let crowd = Arc::new(Crowd {
        server: server.trim_end_matches('/').into(),
        roster,
        stop: AtomicBool::new(false),
        think_ms: num(args, "--think-ms", 800),
        local_check: num(args, "--local-check", 0.2),
        seq: AtomicU64::new(0),
    });
    let st: Shared = Arc::default();
    let all_armies = armies(&crowd.roster);
    let all_bases = bases(&crowd.roster);
    println!(
        "{players} players for {secs}s against {server}{}; {} armies x {} bases",
        if with_chaos {
            " plus misbehaving players"
        } else {
            ""
        },
        all_armies.len(),
        all_bases.len()
    );
    let t0 = Instant::now();
    let mut hs = Vec::new();
    for id in 0..players {
        let (a, b) = (
            &all_armies[id % all_armies.len()],
            &all_bases[(id / all_armies.len()) % all_bases.len()],
        );
        let s = Strategy {
            army_name: a.0.clone(),
            army: a.1.clone(),
            _base_name: b.0.clone(),
            base: b.1.clone(),
        };
        let (cr, st) = (crowd.clone(), st.clone());
        hs.push(std::thread::spawn(move || player(&cr, &st, id, s)));
        // People don't all arrive in the same millisecond.
        std::thread::sleep(Duration::from_millis(10));
    }
    if with_chaos {
        for id in 0..4 {
            let (cr, st) = (crowd.clone(), st.clone());
            hs.push(std::thread::spawn(move || {
                while cr.running() {
                    chaos(&cr, &st, id);
                }
            }));
        }
    }
    while t0.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_secs(10));
        let s = st.lock().unwrap();
        let calls: u64 = s.calls.values().map(|c| c.count).sum();
        eprintln!(
            "  {:>4}s  {calls} calls, {} raids, {} problems",
            t0.elapsed().as_secs(),
            s.raids.len(),
            s.problems.len()
        );
    }
    crowd.stop.store(true, Ordering::Relaxed);
    for h in hs {
        let _ = h.join();
    }
    let secs = t0.elapsed().as_secs_f64();
    let mut s = st.lock().unwrap();
    for c in s.calls.values_mut() {
        c.p50_ms = pct(&mut c.ms, 0.5);
        c.p95_ms = pct(&mut c.ms, 0.95);
        c.p99_ms = pct(&mut c.ms, 0.99);
        c.max_ms = pct(&mut c.ms, 1.0);
    }
    let total: u64 = s.calls.values().map(|c| c.count).sum();
    println!(
        "\n{total} calls in {secs:.0}s ({:.1}/s), {} raids",
        total as f64 / secs,
        s.raids.len()
    );
    println!(
        "{:<16} {:>7} {:>8} {:>8} {:>8}  answers",
        "call", "count", "p50 ms", "p95 ms", "p99 ms"
    );
    for (k, c) in &s.calls {
        println!(
            "{k:<16} {:>7} {:>8} {:>8} {:>8}  {:?}",
            c.count, c.p50_ms, c.p95_ms, c.p99_ms, c.answers
        );
    }
    let mut grouped: BTreeMap<String, (u32, Vec<String>)> = BTreeMap::new();
    for p in &s.problems {
        let e = grouped.entry(p.test.clone()).or_default();
        e.0 += 1;
        if e.1.len() < 3 {
            e.1.push(format!("expected {}, got {}", p.expected, p.got));
        }
    }
    println!("\nchecks passed: {:?}", s.checks_passed);
    println!(
        "problems: {}",
        if grouped.is_empty() {
            "none".into()
        } else {
            String::new()
        }
    );
    for (k, (n, ex)) in &grouped {
        println!("  {n}x {k}");
        for e in ex {
            println!("       {e}");
        }
    }
    let mut by_army: BTreeMap<String, (u32, u32, f32)> = BTreeMap::new();
    for r in &s.raids {
        let e = by_army.entry(r.army.clone()).or_default();
        e.0 += 1;
        e.1 += r.stars;
        e.2 += r.destruction;
    }
    let practice = s.raids.iter().filter(|r| r.kind == "practice").count();
    println!(
        "\n{} raids on players, {practice} practice raids, {} bonus gold paid by the server",
        s.raids.len() - practice,
        s.raids.iter().map(|r| r.bonus_gold).sum::<i64>()
    );
    println!("\narmy                      raids  avg stars  avg destruction");
    for (k, (n, stars, d)) in &by_army {
        println!(
            "{k:<25} {n:>6} {:>10.2} {:>15.0}%",
            *stars as f32 / *n as f32,
            100.0 * d / *n as f32
        );
    }
    #[derive(Serialize)]
    struct Report<'a> {
        players: usize,
        seconds: f64,
        calls: &'a BTreeMap<String, CallStats>,
        checks_passed: &'a BTreeMap<String, u64>,
        problems: &'a [Problem],
        raids: &'a [RaidRow],
    }
    write_out(
        args,
        &Report {
            players,
            seconds: secs,
            calls: &s.calls,
            checks_passed: &s.checks_passed,
            problems: &s.problems,
            raids: &s.raids,
        },
    );
}

// ---------------------------------------------------------------- matrix

#[derive(Serialize)]
struct Cell {
    army: String,
    base: String,
    town_hall: u32,
    stars: u32,
    destruction: f32,
    ticks: u32,
}

/// Every army against every base, at each Town Hall level.
fn matrix(args: &[String]) {
    let r = Arc::new(load_roster(
        &flag(args, "--troops").unwrap_or_else(|| die("--troops DIR")),
    ));
    let a = armies(&r);
    let b = bases(&r);
    let mut jobs = Vec::new();
    for th in 1..=5u32 {
        for x in &a {
            for y in &b {
                jobs.push((x.clone(), y.clone(), th));
            }
        }
    }
    let jobs = Arc::new(Mutex::new(jobs));
    let out = Arc::new(Mutex::new(Vec::new()));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let hs: Vec<_> = (0..threads)
        .map(|_| {
            let (jobs, out, r) = (jobs.clone(), out.clone(), r.clone());
            std::thread::spawn(move || loop {
                let Some(((an, ai), (bn, bi), th)) = jobs.lock().unwrap().pop() else {
                    break;
                };
                let snap = canonical_snapshot(&base_of(&r, &bi, th)).unwrap();
                let army = canonical_army(&army_of(&r, &ai), 1).unwrap();
                let res = run_raid(&snap, &army);
                out.lock().unwrap().push(Cell {
                    army: an,
                    base: bn,
                    town_hall: th,
                    stars: res.stars,
                    destruction: res.destruction,
                    ticks: res.ticks,
                });
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    let mut cells = Arc::try_unwrap(out).ok().unwrap().into_inner().unwrap();
    cells.sort_by(|x, y| (x.town_hall, &x.army, &x.base).cmp(&(y.town_hall, &y.army, &y.base)));
    for th in 1..=5 {
        println!("\nTown Hall {th}: stars (destruction %) for army (rows) vs base (columns)");
        print!("{:<22}", "");
        for (bn, _) in &b {
            print!("{:>16}", bn);
        }
        println!();
        for (an, _) in &a {
            print!("{an:<22}");
            for (bn, _) in &b {
                let c = cells
                    .iter()
                    .find(|c| c.town_hall == th && &c.army == an && &c.base == bn)
                    .unwrap();
                print!(
                    "{:>16}",
                    format!("{}* ({:.0}%)", c.stars, c.destruction * 100.0)
                );
            }
            println!();
        }
    }
    write_out(args, &cells);
}

// ---------------------------------------------------------------- economy

#[derive(Serialize, Default, Clone)]
struct Day {
    day: u32,
    total_gold: i64,
    median_gold: i64,
    players_at_th: [u32; 5],
    raids: u32,
    avg_stars: f32,
    gold_looted: i64,
    gold_spent: i64,
    top_trophies: i32,
    median_trophies: i32,
}

/// Many players over many days with the server's rules, without a server:
/// each day every player raids a few times near their trophies and buys the
/// next Town Hall when they can. Shows whether the online economy runs dry.
fn economy(args: &[String]) {
    let r = load_roster(&flag(args, "--troops").unwrap_or_else(|| die("--troops DIR")));
    let n: usize = num(args, "--players", 200);
    let days: u32 = num(args, "--days", 30);
    let raids_per_day: usize = num(args, "--raids-per-day", 5);
    let start_gold: i64 = num(args, "--start-gold", 500);
    // The server's gold rewards (0 turns one off, as before they existed).
    let defence_reward: i64 = num(args, "--defence-reward", 30);
    let daily_bonus: i64 = num(args, "--daily-bonus", 100);
    let a = armies(&r);
    let b = bases(&r);
    let mut rng = Rng::new(42);
    struct P {
        gold: i64,
        trophies: i32,
        th: u32,
        army: usize,
        base: usize,
    }
    let mut ps: Vec<P> = (0..n)
        .map(|i| P {
            gold: start_gold,
            trophies: 0,
            th: 1,
            army: i % a.len(),
            base: (i / a.len()) % b.len(),
        })
        .collect();
    // Raids are deterministic: cache each (army, base, attacker TH, defender TH).
    let mut cache: BTreeMap<(usize, usize, u32), (u32, f32)> = BTreeMap::new();
    let mut out = Vec::new();
    let mut wins: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for day in 1..=days {
        let mut d = Day {
            day,
            ..Default::default()
        };
        let mut stars = 0;
        let mut won_today = vec![false; n];
        let mut held_today = vec![0u32; n];
        for _ in 0..raids_per_day {
            for i in 0..n {
                // Closest trophies first, like matchmaking (200 either side).
                let mut cands: Vec<usize> = (0..n)
                    .filter(|&j| j != i && (ps[j].trophies - ps[i].trophies).abs() <= 200)
                    .collect();
                if cands.is_empty() {
                    cands = (0..n).filter(|&j| j != i).collect();
                }
                let j = cands[rng.below(cands.len())];
                let key = (ps[i].army, ps[j].base, ps[j].th);
                let (st, de) = *cache.entry(key).or_insert_with(|| {
                    let snap = canonical_snapshot(&base_of(&r, &b[key.1].1, key.2)).unwrap();
                    let army = canonical_army(&army_of(&r, &a[key.0].1), 1).unwrap();
                    let res = run_raid(&snap, &army);
                    (res.stars, res.destruction)
                });
                let g = loot(ps[j].gold, de);
                let sw = trophy_swing(st);
                ps[i].gold += g;
                ps[j].gold -= g;
                if st > 0 && !won_today[i] {
                    won_today[i] = true;
                    ps[i].gold += daily_bonus;
                }
                if st == 0 && held_today[j] < 5 {
                    held_today[j] += 1;
                    ps[j].gold += defence_reward;
                }
                ps[i].trophies = (ps[i].trophies + sw).max(0);
                ps[j].trophies = (ps[j].trophies - sw).max(0);
                d.raids += 1;
                d.gold_looted += g;
                stars += st;
                let w = wins.entry(a[ps[i].army].0.clone()).or_default();
                w.0 += 1;
                w.1 += (st > 0) as u32;
            }
        }
        for p in ps.iter_mut() {
            while p.th < 5 && p.gold >= town_hall_cost(p.th) as i64 {
                p.gold -= town_hall_cost(p.th) as i64;
                d.gold_spent += town_hall_cost(p.th) as i64;
                p.th += 1;
            }
            debug_assert!(max_troop_level(p.th) >= 1);
        }
        let mut golds: Vec<i64> = ps.iter().map(|p| p.gold).collect();
        golds.sort();
        let mut tro: Vec<i32> = ps.iter().map(|p| p.trophies).collect();
        tro.sort();
        d.total_gold = golds.iter().sum();
        d.median_gold = golds[n / 2];
        d.top_trophies = *tro.last().unwrap();
        d.median_trophies = tro[n / 2];
        for p in &ps {
            d.players_at_th[p.th as usize - 1] += 1;
        }
        d.avg_stars = stars as f32 / d.raids as f32;
        println!(
            "day {:>3}: total gold {:>7}  median {:>5}  looted {:>6}  spent {:>6}  TH1-5 {:?}  trophies median {:>4} top {:>4}",
            day, d.total_gold, d.median_gold, d.gold_looted, d.gold_spent, d.players_at_th, d.median_trophies, d.top_trophies
        );
        out.push(d);
    }
    println!("\nshare of raids that won at least one star, by army:");
    for (k, (n, w)) in &wins {
        println!("  {k:<25} {:>5.1}%", 100.0 * *w as f32 / *n as f32);
    }
    write_out(args, &out);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("crowd") => crowd(&args),
        Some("matrix") => matrix(&args),
        Some("economy") => economy(&args),
        _ => die("pick a command"),
    }
    let _ = std::io::stdout().flush();
}
