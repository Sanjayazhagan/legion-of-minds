//! Many players at once against a running server (DEV_LOGIN=1, and
//! REQUESTS_PER_MINUTE / RAIDS_PER_MINUTE raised: every player here comes
//! from one address). Worst-case brains: untrained ones of the given size,
//! so every raid runs the full 3 minutes.
//!
//! cargo run --release -p bag-server --example loadtest -- \
//!   <url> <database_url> <brain size> <players> <raiders> <secs> <browsers> [cup]
//!
//! Raiders find a match and send a 5-troop army over and over (no local
//! fight, to leave this machine's CPU to the server); browsers open the
//! light screens every 2 s. `cup` ends with every raider entering the cup.
use bag_client::bag_protocol::canonical_army;
use bag_client::{army_from, army_spots, defender_spots, snapshot_from, Client};
use godcore::nn::Mlp;
use godcore::ppo::{actor_dims_h, brain_bracket};
use godcore::rng::Rng;
use godcore::scenario::HomeBase;
use godcore::troop::{Troop, MAX_LEVEL};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn troop(name: &str, id: u64, h: usize) -> Troop {
    let p = format!(
        "{}/tests/fixtures/{name}.troop.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut t = Troop::load_json(&std::fs::read_to_string(p).unwrap()).unwrap();
    t.id = id;
    if h != t.actor.dims[1] {
        t.level = MAX_LEVEL;
        t.actor = Mlp::new(&actor_dims_h(h), 0.01, &mut Rng::new(id * 7 + h as u64));
        t.brain = Some(h);
    }
    t
}

#[derive(Default)]
struct Stats {
    lat: std::collections::BTreeMap<&'static str, Vec<f64>>,
    errs: std::collections::BTreeMap<String, u32>,
}
impl Stats {
    fn add(&mut self, k: &'static str, s: f64) {
        self.lat.entry(k).or_default().push(s);
    }
    fn err(&mut self, k: &str, e: String) {
        let short: String = e.chars().take(70).collect();
        *self.errs.entry(format!("{k}: {short}")).or_default() += 1;
    }
}

fn timed<T>(
    st: &Mutex<Stats>,
    k: &'static str,
    f: impl FnOnce() -> Result<T, bag_client::Error>,
) -> Option<T> {
    let t0 = Instant::now();
    match f() {
        Ok(v) => {
            st.lock().unwrap().add(k, t0.elapsed().as_secs_f64());
            Some(v)
        }
        Err(e) => {
            st.lock().unwrap().err(k, e.to_string());
            None
        }
    }
}

fn pct(v: &mut Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (url, db) = (a[0].clone(), a[1].clone());
    let h: usize = a[2].parse().unwrap();
    let players: usize = a[3].parse().unwrap();
    let raiders: usize = a[4].parse().unwrap();
    let secs: u64 = a[5].parse().unwrap();
    let browsers: usize = a[6].parse().unwrap();
    let cup: bool = a.get(7).is_some_and(|s| s == "cup");
    let bracket = brain_bracket(h).unwrap();
    let run = std::process::id();

    // Players, all at Town Hall 5 with levelled troops, each with a base.
    let t0 = Instant::now();
    let mut clients: Vec<Client> = (0..players)
        .map(|i| {
            let mut c = Client::new(&url);
            c.dev_login(&format!("p{run}_{i}")).expect("dev login");
            c
        })
        .collect();
    let sql = format!(
        "UPDATE users SET town_hall_level = 5 WHERE display_name LIKE 'p{run}\\_%';
         INSERT INTO troop_levels SELECT u.id, t, 5 FROM users u, unnest(ARRAY[1,2,3,4,5,100,101,102,103]::bigint[]) t
           WHERE u.display_name LIKE 'p{run}\\_%' ON CONFLICT DO NOTHING;"
    );
    let ok = std::process::Command::new("psql")
        .arg(&db)
        .arg("-qc")
        .arg(&sql)
        .status()
        .unwrap();
    assert!(ok.success());
    let defs: Vec<_> = (0..4)
        .map(|k| troop("guard", 100 + k as u64, h))
        .zip(defender_spots(4))
        .collect();
    let mut home = HomeBase::reference();
    home.town_hall = 5;
    let snap = snapshot_from(&home, &defs);
    let names = ["fighter", "healer"];
    let army = army_from(
        &(0..5)
            .map(|k| troop(names[k % 2], 1 + k as u64, h))
            .zip(army_spots(5))
            .collect::<Vec<_>>(),
    );
    let army = canonical_army(&army, 5).unwrap();
    let st = Arc::new(Mutex::new(Stats::default()));
    // Publish bases a few at a time.
    let clients_arc: Vec<Arc<Client>> = clients.drain(..).map(Arc::new).collect();
    std::thread::scope(|s| {
        for chunk in clients_arc.chunks(players.div_ceil(8)) {
            let (st, snap) = (st.clone(), &snap);
            s.spawn(move || {
                for c in chunk {
                    timed(&st, "publish base", || c.publish_base(snap));
                }
            });
        }
    });
    println!(
        "setup: {players} players with bases in {:.1}s",
        t0.elapsed().as_secs_f64()
    );

    // Raiders: find a match, send the army (no local fight, to keep this
    // machine's CPU for the server), repeat. Browsers: the light screens
    // one request every ~2 s each.
    let deadline = Instant::now() + Duration::from_secs(secs);
    let raids = Arc::new(Mutex::new(0u32));
    std::thread::scope(|s| {
        for i in 0..raiders {
            let (c, st, army, raids) = (
                clients_arc[i % players].clone(),
                st.clone(),
                army.clone(),
                raids.clone(),
            );
            s.spawn(move || {
                while Instant::now() < deadline {
                    let Some(m) = timed(&st, "find match", || c.find_match_in(bracket)) else {
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    };
                    if timed(&st, "raid check", || {
                        c.submit(m.attack_id, army.clone(), None)
                    })
                    .is_some()
                    {
                        *raids.lock().unwrap() += 1;
                    }
                }
            });
        }
        for i in 0..browsers {
            let (c, st) = (clients_arc[(raiders + i) % players].clone(), st.clone());
            s.spawn(move || {
                let mut k = 0u32;
                while Instant::now() < deadline {
                    match k % 6 {
                        0 => {
                            timed(&st, "me", || c.me());
                        }
                        1 => {
                            timed(&st, "leaderboard", || c.leaderboard(50));
                        }
                        2 => {
                            timed(&st, "live (events)", || c.live());
                        }
                        3 => {
                            timed(&st, "battle log", || c.log("all"));
                        }
                        4 => {
                            timed(&st, "cup standings", || c.cup_in(bracket));
                        }
                        _ => {
                            timed(&st, "season", || c.season());
                        }
                    }
                    k += 1;
                    std::thread::sleep(Duration::from_millis(2000));
                }
            });
        }
    });
    let n = *raids.lock().unwrap();
    println!(
        "raids checked: {n} in {secs}s = {:.2}/s ({:.0}/min)",
        n as f64 / secs as f64,
        n as f64 * 60.0 / secs as f64
    );

    if cup {
        let t = troop("fighter", 1, h);
        let t0 = Instant::now();
        std::thread::scope(|s| {
            for c in clients_arc.iter().take(raiders.max(1)) {
                let (st, t) = (st.clone(), &t);
                s.spawn(move || {
                    timed(&st, "cup entry", || c.enter_cup(t));
                });
            }
        });
        println!(
            "{} cup entries at once took {:.1}s",
            raiders.max(1),
            t0.elapsed().as_secs_f64()
        );
    }

    let mut s = st.lock().unwrap();
    for (k, v) in s.lat.iter_mut() {
        println!(
            "  {k:<14} n={:<5} p50={:.3}s p95={:.3}s max={:.3}s",
            v.len(),
            pct(v, 0.5),
            pct(v, 0.95),
            pct(v, 1.0)
        );
    }
    for (k, n) in &s.errs {
        println!("  ERR x{n}: {k}");
    }
}
