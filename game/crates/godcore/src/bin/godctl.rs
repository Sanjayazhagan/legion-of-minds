//! Headless tools: train, evaluate and battle troops from the command line.
//!
//!   godctl train --class fighter --steps 300000 --out fighter.troop.json
//!   godctl train --class fighter+healer --steps 300000 --out priest.troop.json
//!   godctl train --class mage:50,healer:30,fighter:20 --solo 40 --focus 40,30,10,20 --out m.troop.json
//!   godctl eval fighter.troop.json
//!   godctl battle --base 1 a.troop.json b.troop.json
//!   godctl baseline

use godcore::battle::{fingerprint, run_defense_drill, run_drill, tournament, Battle};
use godcore::classes::{BaseClass, ClassSpec, TrainPlan};
use godcore::env::{DrillEnv, EpisodeInfo};
use godcore::ppo::{PpoConfig, Trainer};
use godcore::scenario::{Drill, BASES};
use godcore::sim::{Outcome, ACT_HEAL, ACT_HEAL_2};
use godcore::troop::{CurvePoint, Skills, Troop};

fn parse_class(s: &str) -> ClassSpec {
    let base = |n: &str| match n {
        "fighter" => BaseClass::Fighter,
        "archer" => BaseClass::Archer,
        "healer" => BaseClass::Healer,
        "guard" => BaseClass::Guard,
        "sentry" => BaseClass::Sentry,
        "keeper" => BaseClass::Keeper,
        "mage" => BaseClass::Mage,
        "assassin" => BaseClass::Assassin,
        "paladin" => BaseClass::Paladin,
        "warlock" => BaseClass::Warlock,
        "hunter" => BaseClass::Hunter,
        _ => panic!("unknown class {n}"),
    };
    if s.contains(',') || (s.contains(':') && !s.contains('+')) {
        // Any number of classes: name:percent,name:percent,...
        let parts: Vec<(BaseClass, f32)> = s
            .split(',')
            .map(|p| {
                let (n, w) = p.split_once(':').expect("use name:percent");
                (base(n), w.parse::<f32>().unwrap())
            })
            .collect();
        return ClassSpec::blend(&parts);
    }
    if let Some((a, rest)) = s.split_once('+') {
        let (b, t) = rest
            .split_once(':')
            .map(|(b, t)| (b, t.parse().unwrap()))
            .unwrap_or((rest, 0.5));
        ClassSpec::hybrid(base(a), base(b), t)
    } else {
        base(s).spec()
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .map(|i| args[i + 1].clone())
}

fn summary(eps: &[EpisodeInfo]) -> String {
    let s = Skills::from_episodes(eps);
    let n = eps.len().max(1) as f32;
    let avg = |f: &dyn Fn(&EpisodeInfo) -> f32| eps.iter().map(f).sum::<f32>() / n;
    format!(
        "win {:5.1}%  survive {:5.1}%  guards {:4.2}  buildings {:4.2}  healed {:5.1}  skill {:4.1} dodge {:4.1} evade {:4.1} block {:5.1}  (n={})",
        s.win * 100.0,
        s.survival * 100.0,
        s.guard_share,
        s.building_share,
        s.healed,
        avg(&|e| e.skills),
        avg(&|e| e.dodges),
        avg(&|e| e.evaded),
        avg(&|e| e.blocked),
        s.episodes
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("train") => {
            let spec = parse_class(&arg(&args, "--class").unwrap_or("fighter".into()));
            let steps: u64 = arg(&args, "--steps")
                .map(|s| s.parse().unwrap())
                .unwrap_or(300_000);
            let seed: u64 = arg(&args, "--seed")
                .map(|s| s.parse().unwrap())
                .unwrap_or(1);
            let out = arg(&args, "--out").unwrap_or("troop.troop.json".into());
            let mut troop = match arg(&args, "--from") {
                Some(p) => Troop::load_json(&std::fs::read_to_string(p).unwrap()).unwrap(),
                None => Troop::new(seed, "Cli".into(), spec, seed),
            };
            troop.level = arg(&args, "--level").map(|s| s.parse().unwrap()).unwrap_or(5);
            troop.sync_moves();
            match arg(&args, "--brain") {
                // Neurons per layer; without it a new troop gets the size for its level.
                Some(h) => assert!(troop.set_brain(h.parse().unwrap()), "brain must be one of {:?} and not smaller than now", godcore::ppo::HIDDEN_SIZES),
                None => {
                    if arg(&args, "--from").is_none() {
                        troop.brain = None;
                    }
                    troop.upgrade_brain();
                }
            }
            if let Some(w) = arg(&args, "--weapon") {
                let w = godcore::weapons::Weapon::ALL
                    .into_iter()
                    .find(|x| format!("{x:?}").eq_ignore_ascii_case(&w))
                    .unwrap_or_else(|| panic!("unknown weapon {w}"));
                troop.equip(w);
            }
            println!(
                "{} with {} (mastery {}), brain {:?} = {} numbers",
                troop.spec.name,
                troop.weapon().name(),
                troop.spec.mastery,
                troop.actor.dims,
                troop.brain_params()
            );
            let mut cfg = PpoConfig::default();
            if let Some(v) = arg(&args, "--nsteps") {
                cfg.n_steps = v.parse().unwrap();
            }
            if let Some(v) = arg(&args, "--epochs") {
                cfg.epochs = v.parse().unwrap();
            }
            if let Some(v) = arg(&args, "--mb") {
                cfg.minibatch = v.parse().unwrap();
            }
            if let Some(v) = arg(&args, "--lr") {
                cfg.lr = v.parse().unwrap();
            }
            if let Some(v) = arg(&args, "--envs") {
                cfg.n_envs = v.parse().unwrap();
            }
            if arg(&args, "--solo").is_some() || arg(&args, "--focus").is_some() || arg(&args, "--selfplay").is_some() {
                let mut plan = TrainPlan::default_for(&troop.spec);
                if let Some(v) = arg(&args, "--solo") {
                    plan.solo = v.parse().unwrap();
                }
                if let Some(v) = arg(&args, "--selfplay") {
                    plan.self_play = v.parse().unwrap();
                }
                if let Some(v) = arg(&args, "--focus") {
                    let f: Vec<u32> = v.split(',').map(|x| x.parse().unwrap()).collect();
                    plan.focus = [f[0], f[1], f[2], f[3]];
                }
                // Rebuild from the blend so a plan given again with --from isn't applied twice.
                let name = troop.spec.name.clone();
                troop.spec = ClassSpec::planned(&troop.spec.parents, plan).with_gear_of(&troop.spec);
                troop.spec.name = name;
            }
            if let Some(v) = arg(&args, "--timeout") {
                troop.spec.weights.timeout = v.parse().unwrap();
            }
            if let Some(v) = arg(&args, "--bw") {
                troop.spec.weights.building_dmg = v.parse().unwrap();
            }
            if let Some(v) = arg(&args, "--tick") {
                troop.spec.weights.tick = v.parse().unwrap();
            }
            let pool = arg(&args, "--vs").map(|list| {
                std::sync::Arc::new(
                    list.split(',')
                        .map(|f| {
                            let t = Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap();
                            (t.spec, t.actor)
                        })
                        .collect::<Vec<_>>(),
                )
            });
            let mut tr = Trainer::with_opponents(
                &troop.spec,
                troop.actor.clone(),
                troop.critic.clone(),
                seed,
                cfg,
                pool,
            );
            tr.steps_done = troop.steps;
            let t0 = std::time::Instant::now();
            let mut recent: Vec<EpisodeInfo> = Vec::new();
            while tr.steps_done < troop.steps + steps {
                let rep = tr.iterate();
                recent.extend_from_slice(&rep.episodes);
                let keep = recent.len().saturating_sub(300);
                recent.drain(..keep);
                println!(
                    "{:>8} steps  {:5.1}s/it  ent {:.2}  {}",
                    rep.steps_done,
                    rep.seconds,
                    rep.entropy,
                    summary(&recent)
                );
                let s = Skills::from_episodes(&recent);
                troop.curve.push(CurvePoint {
                    steps: rep.steps_done,
                    win: s.win,
                    died: 1.0 - s.survival,
                    timeout: 0.0,
                    ret: 0.0,
                });
                troop.skills = s;
            }
            if let Some(l) = troop.add_training(tr.steps_done - troop.steps) {
                println!("weapon mastery up: {}", godcore::weapons::MASTERY_TITLES[l as usize]);
            }
            troop.actor = tr.actor;
            troop.critic = tr.critic;
            std::fs::write(&out, troop.save_json()).unwrap();
            let secs = t0.elapsed().as_secs_f32();
            println!(
                "trained {} steps in {:.0}s ({:.0} steps/s) -> {out}",
                steps,
                secs,
                steps as f32 / secs
            );
        }
        Some("team") => {
            // godctl team --troops a.json,b.json --steps N [--seed S] [--vs d.json,..] [--suffix .team]
            let files: Vec<String> = arg(&args, "--troops").expect("--troops a.json,b.json").split(',').map(String::from).collect();
            let mut troops: Vec<Troop> = files.iter().map(|f| Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap()).collect();
            let specs: Vec<_> = troops.iter().map(|t| t.spec.clone()).collect();
            if let Some(p) = godcore::team::team_problem(&specs) {
                panic!("{p}");
            }
            let steps: u64 = arg(&args, "--steps").map(|s| s.parse().unwrap()).unwrap_or(300_000);
            let seed: u64 = arg(&args, "--seed").map(|s| s.parse().unwrap()).unwrap_or(1);
            let suffix = arg(&args, "--suffix").unwrap_or(".team".into());
            let pool = arg(&args, "--vs").map(|list| {
                std::sync::Arc::new(
                    list.split(',')
                        .map(|f| {
                            let t = Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap();
                            (t.spec, t.actor)
                        })
                        .collect::<Vec<_>>(),
                )
            });
            for t in &troops {
                println!("{} ({}), brain {:?}", t.name, t.spec.name, t.actor.dims);
            }
            let mut tr = godcore::team::TeamTrainer::for_troops(&troops, seed, PpoConfig::default(), pool);
            // --hardness 1..5, or auto
            tr.set_hardness(arg(&args, "--hardness").map(|h| if h == "auto" { 0 } else { h.parse().unwrap() }).unwrap_or(1));
            let t0 = std::time::Instant::now();
            let mut recent: Vec<EpisodeInfo> = Vec::new();
            while tr.members.iter().map(|m| m.steps).min().unwrap_or(0) < steps {
                let rep = tr.iterate();
                recent.extend_from_slice(&rep.episodes);
                let keep = recent.len().saturating_sub(300);
                recent.drain(..keep);
                println!("{:>8} steps each  {:5.1}s/it  hardness {}  {}", tr.members[0].steps, rep.seconds, rep.hardness, summary(&recent));
            }
            tr.finish(&mut troops, &recent);
            for (t, f) in troops.iter().zip(&files) {
                let out = format!("{}{}.json", f.trim_end_matches(".json"), suffix);
                std::fs::write(&out, t.save_json()).unwrap();
                println!("{} -> {out}", t.name);
            }
            println!("team trained in {:.0}s", t0.elapsed().as_secs_f32());
        }
        Some("teameval") => {
            // godctl teameval a.json,b.json [n] [--hardness H]: the team plays fixed team drills with no randomness.
            let troops: Vec<Troop> = args[2].split(',').map(|f| Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap()).collect();
            let n: u64 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(200);
            let hardness: u8 = arg(&args, "--hardness").map(|h| h.parse().unwrap()).unwrap_or(1);
            let specs: Vec<_> = troops.iter().map(|t| t.spec.clone()).collect();
            let k = troops.len();
            let (mut wins, mut destr) = (0, 0.0);
            let mut alive = vec![0u32; k];
            let (mut dmg, mut heal, mut blocked) = (vec![0.0f32; k], vec![0.0f32; k], vec![0.0f32; k]);
            let mut calls = vec![0u32; k];
            let mut cache = godcore::nn::Cache::default();
            // Heals where both nearest allies could be healed: how often the
            // more hurt one was picked, and, when only one of them was under
            // attack, how often that one was.
            let (mut choices, mut wise) = (vec![0u32; k], vec![0u32; k]);
            let (mut threat_choices, mut saved) = (vec![0u32; k], vec![0u32; k]);
            // Whom each healer's heals went to: team members by index, then others.
            let mut healed_who = vec![vec![0u32; k + 1]; k];
            for s in 0..n {
                let mut env = godcore::team::TeamEnv::new(specs.clone(), 70_000 + s, None);
                if hardness > 1 {
                    env.hardness = hardness;
                    env.reset();
                }
                loop {
                    let acts: Vec<u8> = (0..k)
                        .map(|j| if env.active(j) { godcore::ppo::greedy_action(&troops[j].actor, &env.obs(j), &env.mask(j), &mut cache) } else { 0 })
                        .collect();
                    for j in 0..k {
                        let (a, m) = (acts[j], env.mask(j));
                        if env.active(j) && (a == ACT_HEAL || a == ACT_HEAL_2) && m[a as usize] {
                            let w = &env.world;
                            let team = w.units[j].team;
                            let mut al: Vec<(usize, f32)> = (0..w.units.len())
                                .filter(|&x| x != j && w.units[x].alive && w.units[x].team == team && !w.units[x].kind.is_building())
                                .map(|x| (x, w.units[j].dist(&w.units[x])))
                                .collect();
                            al.sort_by(|p, q| p.1.partial_cmp(&q.1).unwrap().then(p.0.cmp(&q.0)));
                            let t = al[(a == ACT_HEAL_2) as usize].0;
                            healed_who[j][t.min(k)] += 1;
                        }
                        if env.active(j) && (a == ACT_HEAL || a == ACT_HEAL_2) && m[ACT_HEAL as usize] && m[ACT_HEAL_2 as usize] {
                            let w = &env.world;
                            let team = w.units[j].team;
                            let mut al: Vec<(usize, f32)> = (0..w.units.len())
                                .filter(|&x| x != j && w.units[x].alive && w.units[x].team == team && !w.units[x].kind.is_building())
                                .map(|x| (x, w.units[j].dist(&w.units[x])))
                                .collect();
                            al.sort_by(|p, q| p.1.partial_cmp(&q.1).unwrap().then(p.0.cmp(&q.0)));
                            let frac = |x: usize| w.units[x].hp / w.units[x].max_hp;
                            let (p0, p1) = (frac(al[0].0), frac(al[1].0));
                            if p0 != p1 {
                                choices[j] += 1;
                                wise[j] += ((a == ACT_HEAL) == (p0 < p1)) as u32;
                            }
                            // Threat flags of the two allies in the troop's own view.
                            let o = env.obs(j);
                            let base = godcore::sim::OBS_TEAM;
                            let (t0, t1) = (o[base + 4] > 0.5, o[base + 11] > 0.5);
                            if t0 != t1 {
                                threat_choices[j] += 1;
                                saved[j] += ((a == ACT_HEAL) == t0) as u32;
                            }
                        }
                    }
                    // Read the end state before the env resets itself.
                    let before: Vec<_> = (0..k).map(|j| (env.world.units[j].alive, env.world.units[j].stats)).collect();
                    let out = env.act(&acts);
                    if let Some(e) = out.episode {
                        wins += e.won as u32;
                        destr += e.destruction;
                        for j in 0..k {
                            let (a, st) = before[j];
                            alive[j] += (a && !out.done.is_empty()) as u32;
                            dmg[j] += st.unit_dmg + st.building_dmg;
                            heal[j] += st.heal;
                            blocked[j] += st.blocked;
                            calls[j] += st.calls;
                        }
                        break;
                    }
                }
            }
            let nf = n as f32;
            for j in 0..k {
                let total: u32 = healed_who[j].iter().sum();
                if total > 0 {
                    let parts: Vec<String> = (0..=k)
                        .map(|x| {
                            let who = if x < k { troops[x].spec.name.clone() } else { "others".into() };
                            format!("{who} {:.0}%", healed_who[j][x] as f32 / total as f32 * 100.0)
                        })
                        .collect();
                    println!("  {}'s heals went to: {}", troops[j].spec.name, parts.join(", "));
                }
            }
            println!("team wins {:.1}%  destruction {:.0}%", wins as f32 / nf * 100.0, destr / nf * 100.0);
            for j in 0..k {
                println!(
                    "  {:<28} alive at end {:>5.1}%  damage {:>6.1}  healed {:>5.1}  blocked {:>5.1}  calls {:>4.1}{}",
                    troops[j].spec.name,
                    alive[j] as f32 / nf * 100.0,
                    dmg[j] / nf,
                    heal[j] / nf,
                    blocked[j] / nf,
                    calls[j] as f32 / nf,
                    if choices[j] > 0 {
                        format!(
                            "  picked the more hurt ally {:.0}% of {}, the one under attack {:.0}% of {}",
                            wise[j] as f32 / choices[j] as f32 * 100.0,
                            choices[j],
                            saved[j] as f32 / threat_choices[j].max(1) as f32 * 100.0,
                            threat_choices[j]
                        )
                    } else {
                        String::new()
                    }
                );
            }
        }
        Some("eval") => {
            let t = Troop::load_json(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
            if t.spec.role == godcore::classes::Role::Defend {
                let eps: Vec<EpisodeInfo> = (0..200)
                    .map(|s| run_defense_drill(&t, 50_000 + s).0)
                    .collect();
                let lost = eps.iter().map(|e| e.destruction).sum::<f32>() / eps.len() as f32;
                println!(
                    "defence     {}  base lost {:.0}%",
                    summary(&eps),
                    lost * 100.0
                );
                return;
            }
            for drill in [Drill::Solo, Drill::Squad] {
                let eps: Vec<EpisodeInfo> = (0..200)
                    .map(|s| run_drill(&t, drill, 50_000 + s).0)
                    .collect();
                println!("{:<11} {}", drill.name(), summary(&eps));
            }
        }
        Some("baseline") => {
            // Untrained and scripted reference numbers.
            for name in ["fighter", "archer", "healer", "fighter+healer"] {
                let spec = parse_class(name);
                let t = Troop::new(1, "x".into(), spec.clone(), 1);
                for drill in [Drill::Solo, Drill::Squad] {
                    let eps: Vec<EpisodeInfo> = (0..200)
                        .map(|s| run_drill(&t, drill, 50_000 + s).0)
                        .collect();
                    println!(
                        "untrained {name:<15} {:<11} {}",
                        drill.name(),
                        summary(&eps)
                    );
                }
            }
            for name in ["guard", "sentry", "keeper"] {
                let t = Troop::new(1, "x".into(), parse_class(name), 1);
                let eps: Vec<EpisodeInfo> = (0..200)
                    .map(|s| run_defense_drill(&t, 50_000 + s).0)
                    .collect();
                println!("untrained {name:<15} defence     {}", summary(&eps));
            }
            // Scripted fighter in the trained troop's slot.
            for drill in [Drill::Solo, Drill::Squad] {
                let mut eps = Vec::new();
                for s in 0..200 {
                    let mut env = DrillEnv::fixed(BaseClass::Fighter.spec(), drill, 50_000 + s);
                    loop {
                        let a = env.world.scripted_action(0);
                        if let (_, Some(i)) = env.step(a) {
                            eps.push(i);
                            break;
                        }
                    }
                }
                println!(
                    "scripted  fighter         {:<11} {}",
                    drill.name(),
                    summary(&eps)
                );
            }
        }
        Some("battle") => {
            let base = &BASES[arg(&args, "--base")
                .map(|s| s.parse().unwrap())
                .unwrap_or(0)];
            let files: Vec<&String> = args[2..].iter().filter(|a| a.ends_with(".json")).collect();
            let deploy: Vec<(Troop, (f32, f32))> = files
                .iter()
                .enumerate()
                .map(|(k, f)| {
                    (
                        Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap(),
                        (4.0 + 1.5 * k as f32, 3.0),
                    )
                })
                .collect();
            let mut b = Battle::new(base, deploy.clone());
            b.run();
            let mut b2 = Battle::new(base, deploy);
            b2.run();
            println!(
                "{}: {:?} after {} ticks, {:.0}% destroyed, {} stars, {} troops left, replay identical: {}",
                base.name,
                b.outcome(),
                b.world.tick,
                b.world.destruction() * 100.0,
                b.world.stars(),
                b.world.troops_alive(),
                fingerprint(&b.world) == fingerprint(&b2.world)
            );
            assert!(b.outcome() != Outcome::Running);
        }
        Some("defend") => {
            // godctl defend d1.json d2.json [--wave K | --army a1.json,a2.json]
            let load = |f: &str| Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap();
            let files: Vec<&String> = args[2..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .collect();
            let spots = godcore::scenario::home_spots(files.len());
            let defs: Vec<(Troop, (f32, f32))> =
                files.iter().zip(spots).map(|(f, p)| (load(f), p)).collect();
            let wave =
                arg(&args, "--wave").map(|w| godcore::scenario::WAVES[w.parse::<usize>().unwrap()]);
            let army: Vec<(Troop, (f32, f32))> = arg(&args, "--army")
                .map(|l| {
                    l.split(',')
                        .enumerate()
                        .map(|(k, f)| (load(f), (3.0 + 1.4 * k as f32, 3.0 + 0.6 * k as f32)))
                        .collect()
                })
                .unwrap_or_default();
            let base = arg(&args, "--th")
                .map(|t| godcore::scenario::HomeBase::full(t.parse().unwrap()))
                .unwrap_or_else(godcore::scenario::HomeBase::reference);
            let mut b = Battle::defense(&base, defs, wave.as_ref(), army);
            b.run();
            println!(
                "{:?} after {} ticks: base lost {:.0}%, raiders earned {} stars, {} raiders left",
                b.outcome(),
                b.world.tick,
                b.world.destruction() * 100.0,
                b.world.stars(),
                b.world.troops_alive()
            );
        }
        Some("cup") => {
            let troops: Vec<Troop> = args[2..]
                .iter()
                .map(|f| Troop::load_json(&std::fs::read_to_string(f).unwrap()).unwrap())
                .collect();
            for e in tournament(&troops) {
                println!(
                    "{:<20} total {:6.1}  solo {:5.1} ({}/{} wins)  squad {:5.1} ({}/{} wins)",
                    troops[e.troop_index].spec.name,
                    e.total,
                    e.solo.score,
                    e.solo.wins,
                    e.solo.played,
                    e.squad.score,
                    e.squad.wins,
                    e.squad.played
                );
            }
        }
        _ => eprintln!("usage: godctl train|eval|baseline|battle|cup ..."),
    }
}
