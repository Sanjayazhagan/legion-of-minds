//! Team training: pick 2 to 5 troops of one side and train them together.
//! Each keeps its own brain, sees what its teammates are doing, and is
//! rewarded partly for the team's result, so they learn to cover each other.

use crate::barracks::fmt_steps;
use crate::render::{draw_ground, draw_world, Look, View};
use crate::ui::*;
use crate::{Game, Screen};
use godcore::classes::Role;
use godcore::env::{EpisodeInfo, Opponents};
use godcore::nn::{Cache, Mlp};
use godcore::ppo::{greedy_action, PpoConfig};
use godcore::rng::Rng;
use godcore::scenario::{add_raiders, build_team_defense, build_team_drill, MAX_TEAM, MIN_TEAM};
use godcore::sim::{Outcome, World};
use godcore::team::{team_problem, TeamTrainer, TEAM_SPIRIT};
use godcore::troop::{Skills, Troop};
use macroquad::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Shared {
    /// Training steps each member has taken this session.
    steps: Vec<u64>,
    actors: Vec<Mlp>,
    trainer: Option<TeamTrainer>,
    recent: Vec<EpisodeInfo>,
    rate: f32,
    done: bool,
}

struct Job {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    target: u64,
}

struct Preview {
    world: World,
    actors: Vec<Mlp>,
    cache: Cache,
    acc: f32,
    end_hold: f32,
}

pub struct TeamState {
    side: Role,
    members: Vec<usize>,
    budget: u64,
    job: Option<Job>,
    preview: Option<Preview>,
    seed: u64,
    finished: Option<String>,
}

impl TeamState {
    pub fn new(g: &Game) -> Self {
        let side = g.selected.map_or(Role::Attack, |i| g.troops[i].spec.role);
        TeamState { side, members: Vec::new(), budget: 100_000, job: None, preview: None, seed: 1, finished: None }
    }

    /// Demo for screenshots: the first troops of the most common side.
    pub fn demo(g: &Game) -> Self {
        let mut s = Self::new(g);
        let atk: Vec<usize> = (0..g.troops.len()).filter(|&i| g.troops[i].spec.role == Role::Attack).collect();
        let def: Vec<usize> = (0..g.troops.len()).filter(|&i| g.troops[i].spec.role == Role::Defend).collect();
        let (side, pick) = if atk.len() >= def.len() { (Role::Attack, atk) } else { (Role::Defend, def) };
        s.side = side;
        s.members = pick.into_iter().take(3).collect();
        s
    }
}

fn spawn(troops: &[Troop], budget: u64, pool: Option<Opponents>) -> Job {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let (sh, st) = (shared.clone(), stop.clone());
    let seed = troops.iter().fold(17u64, |a, t| a.wrapping_mul(31).wrapping_add(t.seed ^ t.steps));
    let troops = troops.to_vec();
    std::thread::spawn(move || {
        let mut tr = TeamTrainer::for_troops(&troops, seed, PpoConfig::default(), pool);
        let t0 = std::time::Instant::now();
        let mut recent: Vec<EpisodeInfo> = Vec::new();
        while tr.members.iter().map(|m| m.steps).min().unwrap_or(0) < budget && !st.load(Ordering::Relaxed) {
            let rep = tr.iterate();
            recent.extend_from_slice(&rep.episodes);
            let cut = recent.len().saturating_sub(300);
            recent.drain(..cut);
            let mut s = sh.lock().unwrap();
            s.steps = tr.members.iter().map(|m| m.steps).collect();
            s.actors = tr.members.iter().map(|m| m.actor.clone()).collect();
            s.recent = recent.clone();
            s.rate = s.steps.iter().sum::<u64>() as f32 / t0.elapsed().as_secs_f32();
        }
        let mut s = sh.lock().unwrap();
        s.trainer = Some(tr);
        s.recent = recent;
        s.done = true;
    });
    Job { shared, stop, target: budget }
}

impl Game {
    /// Your trained troops of the other side spar with the team.
    fn team_opponents(&self, side: Role) -> Option<Opponents> {
        let v: Vec<_> = self
            .troops
            .iter()
            .filter(|t| t.spec.role != side && t.steps > 0)
            .map(|t| (t.spec.clone(), t.actor.clone()))
            .collect();
        (!v.is_empty()).then(|| Arc::new(v))
    }

    fn finish_team(&mut self, s: &mut TeamState) {
        let Some(job) = s.job.take() else { return };
        let mut sh = job.shared.lock().unwrap();
        let Some(tr) = sh.trainer.take() else { return };
        let recent = std::mem::take(&mut sh.recent);
        drop(sh);
        let mut team: Vec<Troop> = s.members.iter().map(|&i| self.troops[i].clone()).collect();
        let steps: Vec<u64> = tr.members.iter().map(|m| m.steps).collect();
        tr.finish(&mut team, &recent);
        for (k, &i) in s.members.iter().enumerate() {
            self.troops[i] = team[k].clone();
            self.store.save_troop(&self.troops[i]);
        }
        let won = Skills::from_episodes(&recent).win;
        let names: Vec<&str> = team.iter().map(|t| t.name.as_str()).collect();
        s.finished = Some(format!(
            "{} trained together, about {} steps each. Team drill wins in the last {} drills: {:.0}%",
            names.join(", "),
            fmt_steps(steps.iter().sum::<u64>() / steps.len().max(1) as u64),
            recent.len(),
            won * 100.0
        ));
        s.preview = None;
    }

    pub fn team_training(&mut self, mut s: TeamState) -> Screen {
        let busy = s.job.is_some();
        if self.header("Team training", !busy) {
            return Screen::Barracks;
        }
        s.members.retain(|&i| i < self.troops.len());
        let specs: Vec<_> = s.members.iter().map(|&i| self.troops[i].spec.clone()).collect();
        let ok = team_problem(&specs).is_none();

        // ---- Live team drill (left)
        let view = View::new(Rect::new(20.0, 66.0, 714.0, 714.0));
        let latest: Option<Vec<Mlp>> = s.job.as_ref().map(|j| j.shared.lock().unwrap().actors.clone()).filter(|a| !a.is_empty());
        if ok {
            if s.preview.is_none() {
                s.seed = s.seed.wrapping_add(1);
                let mut rng = Rng::new(s.seed ^ 0x7ea);
                let world = if s.side == Role::Defend {
                    let (mut w, from) = build_team_defense(&specs, &mut rng);
                    let n = (specs.len() - 1 + rng.below(2)).max(2);
                    add_raiders(&mut w, n, from, 1.0, &mut rng);
                    w
                } else {
                    build_team_drill(&specs, &mut rng)
                };
                let actors = latest.clone().unwrap_or_else(|| s.members.iter().map(|&i| self.troops[i].actor.clone()).collect());
                s.preview = Some(Preview { world, actors, cache: Cache::default(), acc: 0.0, end_hold: 0.0 });
                self.fx.clear();
            }
            let n = specs.len();
            let p = s.preview.as_mut().unwrap();
            let live = |p: &Preview| p.world.outcome() == Outcome::Running && (0..n).any(|k| p.world.units[k].alive);
            let running = live(p);
            let dt = crate::record::dt().min(0.1);
            if running {
                p.acc += dt * 30.0;
                while p.acc >= 1.0 && live(p) {
                    p.acc -= 1.0;
                    let mut actions = vec![0u8; p.world.units.len()];
                    for k in 0..n {
                        if p.world.units[k].alive {
                            actions[k] = greedy_action(&p.actors[k], &p.world.observe(k), &p.world.mask(k), &mut p.cache);
                        }
                    }
                    p.world.step(&actions);
                    self.fx.absorb(&p.world);
                }
            } else {
                p.end_hold += dt;
            }
            self.fx.age(dt);
            draw_ground(view, false, true, &p.world);
            let looks: Vec<(crate::art::Figure, String)> =
                s.members.iter().map(|&i| (crate::art::Figure::of(&self.troops[i].spec), self.troops[i].name.clone())).collect();
            let mine = p.world.units[0].team;
            let all: Vec<usize> = vec![usize::MAX];
            draw_world(view, &p.world, &self.fx, &|i, u| {
                if i < looks.len() {
                    Look { fig: looks[i].0, name: Some(looks[i].1.clone()), hostile: false }
                } else {
                    crate::art::drill_look(i, u, &all, mine, looks[0].0, "")
                }
            });
            let label = format!("Live team drill #{}  tick {}", s.seed % 1000, p.world.tick);
            draw_rectangle(view.r.x, view.r.y, text_width(&label, 20.0) + 20.0, 30.0, Color::new(0.0, 0.0, 0.0, 0.55));
            text(&label, view.r.x + 10.0, view.r.y + 21.0, 20.0, TEXT);
            if !running {
                let won = (p.world.outcome() == Outcome::Victory) != (s.side == Role::Defend);
                let msg = match (s.side == Role::Defend, won) {
                    (true, true) => "Base held",
                    (true, false) => "Base fallen",
                    (false, true) => "Drill won",
                    (false, false) => "Team defeated",
                };
                text_centered(msg, view.r.x + view.r.w / 2.0, view.r.y + 60.0, 40.0, if won { GOOD } else { BAD });
                if p.end_hold > 1.2 {
                    s.preview = None;
                }
            }
        } else {
            panel(view.r);
            let (x, y, w) = (view.r.x + 30.0, view.r.y + 60.0, view.r.w - 60.0);
            text("Train troops as a team", x, y, 30.0, TEXT);
            crate::recruit::wrap(
                &format!(
                    "Pick 2 to {MAX_TEAM} troops from the same side. They train together in the same drills, each with its own brain. Every troop sees its nearest teammates' roles (tank, striker, ranged, healer), whether they are under attack, and whether they are fighting or healing. Rewards are {:.0}% the troop's own and {:.0}% the team's average, so protecting a teammate pays off. Training a team of 3 takes about 3 times as long as training one troop, and every troop's steps count towards its own level cap, weapon mastery and moves.",
                    (1.0 - TEAM_SPIRIT) * 100.0,
                    TEAM_SPIRIT * 100.0
                ),
                x,
                y + 40.0,
                w,
                18.0,
                MUTED,
            );
        }

        // ---- Team picker and controls (right)
        let rx = 754.0;
        let rw = W - rx - 20.0;
        let mut y = 70.0;
        if !busy {
            for (k, (side, label)) in [(Role::Attack, "Attackers"), (Role::Defend, "Defenders")].into_iter().enumerate() {
                if self.ui.button_ex(Rect::new(rx + k as f32 * (rw / 2.0 + 4.0), y, rw / 2.0 - 4.0, 34.0), label, true, s.side == side) && s.side != side {
                    s.side = side;
                    s.members.clear();
                    s.preview = None;
                }
            }
        }
        y += 48.0;
        text(&format!("Team ({}/{MAX_TEAM})", s.members.len()), rx, y, 20.0, TEXT);
        y += 10.0;
        let side_troops: Vec<usize> = (0..self.troops.len()).filter(|&i| self.troops[i].spec.role == s.side).collect();
        let row_h = 40.0;
        let max_rows = 9;
        for &i in side_troops.iter().take(max_rows) {
            let t = &self.troops[i];
            let on = s.members.contains(&i);
            let r = Rect::new(rx, y, rw, row_h - 4.0);
            draw_rectangle(r.x, r.y, r.w, r.h, if on { PANEL_HI } else { PANEL });
            draw_rectangle_lines(r.x, r.y, r.w, r.h, if on { 2.0 } else { 1.0 }, if on { ACCENT } else { LINE });
            crate::art::draw_portrait(vec2(r.x + 18.0, r.y + r.h / 2.0), 14.0, &t.spec);
            text(&t.name, r.x + 40.0, r.y + 16.0, 18.0, TEXT);
            let role = format!("{:?}", godcore::sim::Role::of_spec(&t.spec));
            text(&format!("Lv {} {}  ·  {}  ·  {} left", t.level, t.spec.name, role, fmt_steps(t.steps_left())), r.x + 40.0, r.y + 31.0, 14.0, MUTED);
            if !busy && self.ui.hover(r) && self.ui.clicked {
                if on {
                    s.members.retain(|&m| m != i);
                } else if s.members.len() < MAX_TEAM {
                    s.members.push(i);
                }
                s.preview = None;
            }
            y += row_h;
        }
        if side_troops.len() > max_rows {
            text(&format!("+{} more not shown", side_troops.len() - max_rows), rx, y + 12.0, 15.0, MUTED);
            y += 20.0;
        }
        if side_troops.len() < MIN_TEAM {
            text("Recruit at least 2 troops of this side first.", rx, y + 14.0, 17.0, BAD);
            y += 24.0;
        }
        y += 14.0;

        let left = s.members.iter().map(|&i| self.troops[i].steps_left()).min().unwrap_or(0);
        if let Some(j) = &s.job {
            let sh = j.shared.lock().unwrap();
            let each = sh.steps.iter().copied().min().unwrap_or(0);
            text(&format!("{} / {} steps each", fmt_steps(each), fmt_steps(j.target)), rx, y + 8.0, 20.0, TEXT);
            bar(Rect::new(rx, y + 16.0, rw, 12.0), each as f32 / j.target.max(1) as f32, ACCENT);
            let won = Skills::from_episodes(&sh.recent).win;
            let line = if sh.rate > 0.0 {
                format!("{:.0} steps/s for the team · team wins {:.0}% (last {} drills)", sh.rate, won * 100.0, sh.recent.len())
            } else {
                "Collecting the first batch of team drills...".to_string()
            };
            text(&line, rx, y + 48.0, 16.0, MUTED);
            let done = sh.done;
            drop(sh);
            let stop = Rect::new(rx, H - 80.0, rw, 50.0);
            if self.ui.button(stop, "Stop and keep progress") {
                s.job.as_ref().unwrap().stop.store(true, Ordering::Relaxed);
            }
            if done {
                self.finish_team(&mut s);
            }
        } else {
            if let Some(msg) = &s.finished {
                crate::recruit::wrap(msg, rx, y + 8.0, rw, 17.0, GOOD);
                y += 60.0;
            }
            if let Some(p) = team_problem(&specs) {
                text(p, rx, y + 10.0, 17.0, MUTED);
            } else if left == 0 {
                text("A team member is at its level cap. Level it up first.", rx, y + 10.0, 17.0, ACCENT);
            } else {
                text("Steps for each troop", rx, y + 10.0, 18.0, TEXT);
                s.budget = s.budget.min(left);
                let mut opts: Vec<u64> = [50_000u64, 100_000, 200_000].iter().map(|o| (*o).min(left)).collect();
                opts.dedup();
                let mut bx = rx;
                for o in opts {
                    if self.ui.button_ex(Rect::new(bx, y + 20.0, 106.0, 34.0), &format!("+{}", fmt_steps(o)), true, s.budget == o) {
                        s.budget = o;
                    }
                    bx += 112.0;
                }
                if self.ui.button(Rect::new(rx, H - 80.0, rw, 50.0), &format!("Train together (+{} each)", fmt_steps(s.budget))) {
                    let team: Vec<Troop> = s.members.iter().map(|&i| self.troops[i].clone()).collect();
                    let pool = self.team_opponents(s.side);
                    s.job = Some(spawn(&team, s.budget, pool));
                    s.finished = None;
                    s.preview = None;
                }
            }
        }
        // Keep the preview on the newest brains once a drill ends.
        if let (Some(p), Some(a)) = (&mut s.preview, latest) {
            if p.world.tick == 0 && a.len() == p.actors.len() {
                p.actors = a;
            }
        }
        Screen::Team(s)
    }
}
