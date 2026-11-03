//! bagctl: try the server from the command line with troop files from the
//! game or godctl.
//!
//! ```text
//! bagctl [--server URL] [--token-file F] <command>
//!   providers
//!   login-dev NAME
//!   login PROVIDER                      open the URL it prints, then wait
//!   me | rename NAME | leaderboard
//!   publish --th N def1.troop.json ...  base = every building Town Hall N allows
//!   town-hall                           buy the next Town Hall level online
//!   weapon NAME                         unlock a weapon online, e.g. Daggers
//!   level TROOP.json                    level a troop up online by one
//!   find                                matchmaking only
//!   raid army1.troop.json ...           find a match and raid it
//!   log [defense|attack|all]
//!   replay ATTACK_ID [--out DIR]        re-run locally; --out writes the raiders
//!   logout
//! ```

use bag_client::bag_protocol::run_raid;
use bag_client::*;
use godcore::troop::Troop;
use std::path::PathBuf;

const USAGE: &str = "bagctl [--server URL] [--token-file F] <command>
  providers
  login-dev NAME
  login PROVIDER                      open the URL it prints, then wait
  me | rename NAME | leaderboard
  publish --th N def1.troop.json ...  base = every building Town Hall N allows
  town-hall                           buy the next Town Hall level online
  weapon NAME                         unlock a weapon online, e.g. Daggers
  level TROOP.json                    level a troop up online by one
  find                                matchmaking only
  raid army1.troop.json ...           find a match and raid it
  log [defense|attack|all]
  replay ATTACK_ID [--out DIR]        re-run locally; --out writes the raiders
  logout";

fn flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn load(path: &str) -> Troop {
    let s = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    Troop::load_json(&s).unwrap_or_else(|e| die(&format!("{path}: {e}")))
}

fn die(msg: &str) -> ! {
    eprintln!("bagctl: {msg}");
    std::process::exit(1)
}

fn ok<T>(r: Result<T>) -> T {
    r.unwrap_or_else(|e| die(&e.to_string()))
}

fn json(v: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let server = flag(&mut args, "--server")
        .or_else(|| std::env::var("BAG_SERVER").ok())
        .unwrap_or_else(|| "http://localhost:8080".into());
    let token_file = PathBuf::from(
        flag(&mut args, "--token-file").unwrap_or_else(|| "bagctl.token".into()),
    );
    let mut c = Client::new(&server);
    if let Ok(t) = std::fs::read_to_string(&token_file) {
        c = c.with_token(t.trim());
    }
    let save = |c: &Client| {
        if let Some(t) = c.token() {
            std::fs::write(&token_file, t).unwrap_or_else(|e| die(&e.to_string()));
        }
    };
    let cmd = if args.is_empty() { "help".to_string() } else { args.remove(0) };
    match cmd.as_str() {
        "providers" => json(&ok(c.providers())),
        "login-dev" => {
            let name = args.first().cloned().unwrap_or_else(|| die("login-dev NAME"));
            json(&ok(c.dev_login(&name)));
            save(&c);
        }
        "login" => {
            let p = args.first().cloned().unwrap_or_else(|| die("login google|discord"));
            let l = ok(c.start_login(&p));
            println!("Open this in your browser:\n{}\n", l.url);
            json(&ok(c.wait_for_login(&l)));
            save(&c);
        }
        "logout" => {
            ok(c.logout());
            let _ = std::fs::remove_file(&token_file);
        }
        "me" => json(&ok(c.me())),
        "rename" => json(&ok(c.rename(&args.join(" ")))),
        "leaderboard" => json(&ok(c.leaderboard(50))),
        "publish" => {
            let th: u32 = flag(&mut args, "--th")
                .map(|s| s.parse().unwrap_or_else(|_| die("--th is 1 to 5")))
                .unwrap_or(1);
            let defs: Vec<Troop> = args.iter().map(|p| load(p)).collect();
            let spots = defender_spots(defs.len());
            let defs: Vec<_> = defs.into_iter().zip(spots).collect();
            json(&ok(c.publish_base(&snapshot_from(&starter_layout(th), &defs))));
        }
        "town-hall" => json(&ok(c.upgrade_town_hall())),
        "weapon" => {
            let name = args.first().unwrap_or_else(|| die(USAGE));
            let w = serde_json::from_value(serde_json::Value::String(name.clone()))
                .unwrap_or_else(|_| die("weapon names: SwordShield Greataxe Greatsword Daggers Crossbow FireStaff HolyWand"));
            json(&ok(c.buy_weapon(w)));
        }
        "level" => {
            let t = load(args.first().unwrap_or_else(|| die(USAGE)));
            let from = ok(c.me()).upgrades.level_of(t.id);
            json(&ok(c.level_up(t.id, from)));
        }
        "find" => {
            let m = ok(c.find_match());
            println!(
                "raid {} on {} ({} trophies, Town Hall {:?}, {} defenders), up to {} gold, submit before {}",
                m.attack_id,
                m.defender.display_name,
                m.defender.trophies,
                m.defender.town_hall,
                m.snapshot.defenders.len(),
                m.max_loot,
                m.expires_at
            );
        }
        "raid" => {
            if args.is_empty() {
                die("raid army1.troop.json ...");
            }
            let army: Vec<Troop> = args.iter().map(|p| load(p)).collect();
            let spots = army_spots(army.len());
            let army: Vec<_> = army.into_iter().zip(spots).collect();
            let m = ok(c.find_match());
            println!("raiding {} ...", m.defender.display_name);
            let (local, out) = ok(c.attack(&m, army_from(&army)));
            println!(
                "local:  {} stars, {:.0}% destroyed, fingerprint {}",
                local.stars,
                local.destruction * 100.0,
                local.fingerprint
            );
            println!(
                "server: {} stars, {:.0}% destroyed, fingerprint {} (match: {:?})",
                out.result.stars,
                out.result.destruction * 100.0,
                out.result.fingerprint,
                out.client_matched
            );
            println!("trophies {:+}, gold looted {}", out.trophies, out.gold_looted);
        }
        "log" => {
            let role = args.first().map(String::as_str).unwrap_or("all");
            for l in ok(c.log(role)) {
                println!(
                    "{}  {} -> {}: {} stars, {:.0}%, trophies {:+}, gold {}  [{}]",
                    l.finished_at,
                    l.attacker.display_name,
                    l.defender.display_name,
                    l.result.stars,
                    l.result.destruction * 100.0,
                    l.trophies,
                    l.gold_looted,
                    l.attack_id
                );
            }
        }
        "replay" => {
            let out = flag(&mut args, "--out");
            let id = args
                .first()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| die("replay ATTACK_ID"));
            let r = ok(c.replay(id));
            let again = run_raid(&r.snapshot, &r.army);
            println!(
                "{} stars, {:.0}% destroyed; replay check: {}",
                again.stars,
                again.destruction * 100.0,
                if again == r.log.result { "identical" } else { "DIFFERENT" }
            );
            if let Some(dir) = out {
                let _ = std::fs::create_dir_all(&dir);
                for (k, t) in raiders(&r).iter().enumerate() {
                    let p = PathBuf::from(&dir).join(format!("raider{k}.troop.json"));
                    std::fs::write(&p, t.save_json()).unwrap();
                    println!("wrote {} (train against it with godctl --vs)", p.display());
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
        }
    }
}
