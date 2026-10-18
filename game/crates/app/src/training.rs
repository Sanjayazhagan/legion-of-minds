//! Training: PPO runs on a background thread while the screen shows a live
//! drill played by the newest brain, the learning curve and skill badges.

use crate::barracks::fmt_steps;
use crate::render::{draw_ground, draw_world, View};
use crate::ui::*;
use crate::{Game, Screen};
use godcore::classes::{ClassSpec, Role, TrainPlan};
use godcore::env::EpisodeInfo;
use godcore::env::Opponents;
use godcore::nn::{Cache, Mlp};
use godcore::ppo::{greedy_action, PpoConfig, Trainer};
use godcore::rng::Rng;
use godcore::scenario::{add_raiders, build_defense_drill, build_drill, Drill, DEFENSE_SLOTS};
use godcore::sim::{Control, Outcome, World};
use godcore::troop::{CurvePoint, Skills, Troop};
use macroquad::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const WINDOW: usize = 300;

#[derive(Default)]
struct Shared {
    steps: u64,
    actor: Option<Mlp>,
    critic: Option<Mlp>,
    curve: Vec<CurvePoint>,
    skills: Skills,
    rate: f32,
    done: bool,
}

struct Job {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    target: u64,
    start: u64,
}

struct Preview {
    world: World,
    label: &'static str,
    /// Units driven by the troop's brain.
    controlled: Vec<usize>,
    defense: bool,
    seed: u64,
    actor: Mlp,
    cache: Cache,
    acc: f32,
    end_hold: f32,
}

pub struct TrainState {
    idx: usize,
    budget: u64,
    job: Option<Job>,
    preview: Option<Preview>,
    finished: Option<String>,
    preview_seed: u64,
    /// The plan being edited in the overlay, if open.
    plan: Option<godcore::classes::TrainPlan>,
    ed: crate::plan::NumEdit,
}

impl TrainState {
    pub fn new(idx: usize, t: &Troop) -> Self {
        let left = t.steps_left();
        TrainState {
            idx,
            budget: left.min(200_000).max(left.min(50_000)),
            job: None,
            preview: None,
            finished: None,
            preview_seed: t.seed ^ 77,
            plan: None,
            ed: Default::default(),
        }
    }
}

impl TrainState {
    pub fn demo(idx: usize, t: &Troop) -> Self {
        let mut s = Self::new(idx, t);
        s.job = Some(spawn(t, t.steps + 200_000, None));
        s
    }
}

fn spawn(t: &Troop, target: u64, pool: Option<Opponents>) -> Job {
    let shared = Arc::new(Mutex::new(Shared {
        steps: t.steps,
        curve: t.curve.clone(),
        skills: t.skills,
        ..Default::default()
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let (sh, st) = (shared.clone(), stop.clone());
    let spec = t.spec.clone();
    let (actor, critic) = (t.actor.clone(), t.critic.clone());
    let start = t.steps;
    let seed = t
        .seed
        .wrapping_add(t.steps)
        .wrapping_add(t.sessions as u64 * 7919);
    std::thread::spawn(move || {
        let mut tr =
            Trainer::with_opponents(&spec, actor, critic, seed, PpoConfig::default(), pool);
        tr.steps_done = start;
        let mut recent: Vec<EpisodeInfo> = Vec::new();
        let t0 = std::time::Instant::now();
        while tr.steps_done < target && !st.load(Ordering::Relaxed) {
            let rep = tr.iterate();
            recent.extend_from_slice(&rep.episodes);
            let cut = recent.len().saturating_sub(WINDOW);
            recent.drain(..cut);
            let skills = Skills::from_episodes(&recent);
            let n = recent.len().max(1) as f32;
            let point = CurvePoint {
                steps: rep.steps_done,
                win: skills.win,
                died: 1.0 - skills.survival,
                timeout: recent.iter().filter(|e| e.timeout).count() as f32 / n,
                ret: recent.iter().map(|e| e.ret).sum::<f32>() / n,
            };
            let mut s = sh.lock().unwrap();
            s.steps = rep.steps_done;
            s.actor = Some(tr.actor.clone());
            s.critic = Some(tr.critic.clone());
            s.curve.push(point);
            s.skills = skills;
            s.rate = (rep.steps_done - start) as f32 / t0.elapsed().as_secs_f32();
        }
        sh.lock().unwrap().done = true;
    });
    Job {
        shared,
        stop,
        target,
        start,
    }
}

impl Game {
    /// Self-play: your trained troops of the other role become sparring partners.
    fn opponents_for(&self, idx: usize) -> Option<Opponents> {
        let role = self.troops[idx].spec.role;
        let v: Vec<_> = self
            .troops
            .iter()
            .filter(|t| t.spec.role != role && t.steps > 0)
            .map(|t| (t.spec.clone(), t.actor.clone()))
            .collect();
        (!v.is_empty()).then(|| Arc::new(v))
    }

    fn finish_training(&mut self, s: &mut TrainState) {
        let Some(job) = s.job.take() else { return };
        let sh = job.shared.lock().unwrap();
        let t = &mut self.troops[s.idx];
        let before = t.skills;
        let mut mastery_up = None;
        if let (Some(a), Some(c)) = (&sh.actor, &sh.critic) {
            t.actor = a.clone();
            t.critic = c.clone();
            mastery_up = t.add_training(sh.steps - t.steps);
            t.curve = thin(&sh.curve);
            t.skills = sh.skills;
            t.sessions += 1;
        }
        let gained = sh.steps - job.start;
        let mut msg = format!(
            "{} trained {} steps. Drill wins {:.0}% -> {:.0}%",
            t.name,
            fmt_steps(gained),
            before.win * 100.0,
            t.skills.win * 100.0
        );
        let new: Vec<&str> = t
            .skills
            .badges(&t.spec)
            .into_iter()
            .zip(before.badges(&t.spec))
            .filter(|((_, now), (_, was))| *now && !*was)
            .map(|((l, _), _)| l)
            .collect();
        if !new.is_empty() {
            msg += &format!(". New: {}", new.join(", "));
        }
        if let Some(l) = mastery_up {
            msg += &format!(". {} mastery: {}", t.weapon().name(), godcore::weapons::MASTERY_TITLES[l as usize]);
        }
        drop(sh);
        self.store.save_troop(&self.troops[s.idx]);
        s.finished = Some(msg);
    }

    pub fn train(&mut self, mut s: TrainState) -> Screen {
        let busy = s.job.is_some();
        if self.header("Training", !busy) {
            return Screen::Barracks;
        }
        if s.job.is_none() {
            let label = if s.plan.is_some() { "Close plan" } else { "Training plan" };
            if self.ui.button(Rect::new(760.0, 10.0, 200.0, 32.0), label) {
                s.plan = match s.plan {
                    Some(_) => None,
                    None => Some(self.troops[s.idx].spec.plan.unwrap_or_else(|| TrainPlan::default_for(&self.troops[s.idx].spec))),
                };
            }
        }
        let t = &self.troops[s.idx];
        let spec = t.spec.clone();

        // ---- Live drill preview (left)
        let view = View::new(Rect::new(20.0, 66.0, 714.0, 714.0));
        let latest = s
            .job
            .as_ref()
            .and_then(|j| j.shared.lock().unwrap().actor.clone());
        if s.preview.is_none() {
            s.preview_seed = s.preview_seed.wrapping_add(1);
            let mut rng = Rng::new(s.preview_seed);
            let (world, label, controlled, defense) = if spec.role == Role::Defend {
                let (mut w, from) = build_defense_drill(&spec, &mut rng);
                let n = 2 + rng.below(2);
                add_raiders(&mut w, n, from, 1.0, &mut rng);
                (w, "defence drill", (0..DEFENSE_SLOTS).collect(), true)
            } else {
                let drill = Drill::pick(&spec, &mut rng);
                (
                    build_drill(&spec, drill, &mut rng),
                    drill.name(),
                    vec![0],
                    false,
                )
            };
            let actor = latest.clone().unwrap_or_else(|| t.actor.clone());
            s.preview = Some(Preview {
                world,
                label,
                controlled,
                defense,
                seed: s.preview_seed,
                actor,
                cache: Cache::default(),
                acc: 0.0,
                end_hold: 0.0,
            });
            self.fx.clear();
        }
        let p = s.preview.as_mut().unwrap();
        let live = |p: &Preview| {
            p.world.outcome() == Outcome::Running && (p.defense || p.world.units[0].alive)
        };
        let running = live(p);
        let dt = crate::record::dt().min(0.1);
        if running {
            p.acc += dt * 30.0;
            while p.acc >= 1.0 && live(p) {
                p.acc -= 1.0;
                let mut actions = vec![0u8; p.world.units.len()];
                for &i in &p.controlled {
                    if p.world.units[i].alive {
                        actions[i] = greedy_action(
                            &p.actor,
                            &p.world.observe(i),
                            &p.world.mask(i),
                            &mut p.cache,
                        );
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
        let fig = crate::art::Figure::of(&spec);
        let name = t.name.clone();
        let controlled = p.controlled.clone();
        let mine = controlled
            .first()
            .map_or(godcore::sim::Team::Attack, |&i| p.world.units[i].team);
        draw_world(view, &p.world, &self.fx, &|i, u| {
            crate::art::drill_look(i, u, &controlled, mine, fig, &name)
        });
        let label = format!(
            "Live drill: {} #{}  tick {}",
            p.label,
            p.seed % 1000,
            p.world.tick
        );
        draw_rectangle(
            view.r.x,
            view.r.y,
            text_width(&label, 20.0) + 20.0,
            30.0,
            Color::new(0.0, 0.0, 0.0, 0.55),
        );
        text(&label, view.r.x + 10.0, view.r.y + 21.0, 20.0, TEXT);
        if !running {
            let won = (p.world.outcome() == Outcome::Victory) != p.defense;
            let msg = if p.defense {
                if won {
                    "Base held"
                } else {
                    "Base fallen"
                }
            } else if won {
                "Drill won"
            } else if !p.world.units[0].alive {
                "Fell in battle"
            } else {
                "Out of time"
            };
            text_centered(
                msg,
                view.r.x + view.r.w / 2.0,
                view.r.y + 60.0,
                40.0,
                if won { GOOD } else { BAD },
            );
        }
        if p.world.units.iter().any(|u| u.control == Control::Scripted) {
            text(
                if p.defense {
                    "horned = scripted raiders, blue ring = your side"
                } else {
                    "red ring = enemies, blue ring = your side"
                },
                view.r.x + 10.0,
                view.r.y + view.r.h - 10.0,
                16.0,
                MUTED,
            );
        }
        if !running && p.end_hold > 1.2 {
            s.preview = None; // next drill with the newest brain
        }

        // ---- Right panel
        let rx = 754.0;
        let rw = W - rx - 20.0;
        let t = &self.troops[s.idx];
        crate::art::draw_portrait(vec2(rx + 24.0, 96.0), 22.0, &t.spec);
        text(&t.name, rx + 56.0, 104.0, 30.0, TEXT);
        text(
            &format!("Lv {} {}", t.level, t.spec.name),
            rx + 56.0,
            126.0,
            18.0,
            MUTED,
        );

        let (steps, curve, skills, rate, done) = match &s.job {
            Some(j) => {
                let sh = j.shared.lock().unwrap();
                (sh.steps, sh.curve.clone(), sh.skills, sh.rate, sh.done)
            }
            None => (t.steps, t.curve.clone(), t.skills, 0.0, false),
        };
        let mut y = 160.0;
        text(
            &format!("{} / {} steps", fmt_steps(steps), fmt_steps(t.cap())),
            rx,
            y,
            22.0,
            TEXT,
        );
        bar(
            Rect::new(rx, y + 10.0, rw, 16.0),
            steps as f32 / t.cap() as f32,
            SKY,
        );
        if let Some(j) = &s.job {
            let f = (steps - j.start) as f32 / (j.target - j.start).max(1) as f32;
            bar(Rect::new(rx, y + 30.0, rw, 8.0), f, ACCENT);
            if rate > 0.0 {
                let eta = (j.target.saturating_sub(steps)) as f32 / rate;
                text(
                    &format!("{:.0} steps/s on your CPU, about {:.0}s left", rate, eta),
                    rx,
                    y + 58.0,
                    18.0,
                    MUTED,
                );
            } else {
                text(
                    "Collecting the first batch of drills...",
                    rx,
                    y + 58.0,
                    18.0,
                    MUTED,
                );
            }
        }
        y += 84.0;

        let win: Vec<f32> = curve.iter().map(|p| p.win).collect();
        let surv: Vec<f32> = curve.iter().map(|p| 1.0 - p.died).collect();
        let tout: Vec<f32> = curve.iter().map(|p| p.timeout).collect();
        chart(
            Rect::new(rx, y, rw, 190.0),
            &[
                (
                    &win,
                    GOOD,
                    if t.spec.role == Role::Defend {
                        "held"
                    } else {
                        "wins"
                    },
                ),
                (&surv, SKY, "survived"),
                (&tout, MUTED, "ran out of time"),
            ],
            "last 300 drills",
        );
        y += 206.0;

        text("Skills", rx, y + 4.0, 20.0, TEXT);
        y += 14.0;
        for (label, ok) in skills.badges(&t.spec) {
            draw_rectangle(
                rx,
                y,
                22.0,
                22.0,
                if ok {
                    Color::new(0.15, 0.35, 0.2, 1.0)
                } else {
                    BG
                },
            );
            draw_rectangle_lines(rx, y, 22.0, 22.0, 1.0, if ok { GOOD } else { LINE });
            if ok {
                text("+", rx + 6.0, y + 17.0, 22.0, GOOD);
            }
            text(
                label,
                rx + 32.0,
                y + 17.0,
                19.0,
                if ok { TEXT } else { MUTED },
            );
            y += 28.0;
        }
        y += 6.0;
        text(
            &format!(
                "guards {:.0}%  buildings {:.0}%  healed {:.0}/drill",
                skills.guard_share * 100.0,
                skills.building_share * 100.0,
                skills.healed
            ),
            rx,
            y + 8.0,
            16.0,
            MUTED,
        );
        y += 26.0;

        if s.job.is_none() {
            if let Some(msg) = &s.finished {
                crate::recruit::wrap(msg, rx, y + 10.0, rw, 18.0, GOOD);
                y += 50.0;
            }
            let left = t.steps_left();
            if left == 0 {
                text(
                    "Level cap reached. Level up in the barracks",
                    rx,
                    y + 20.0,
                    18.0,
                    ACCENT,
                );
                text("to train longer.", rx, y + 42.0, 18.0, ACCENT);
            } else {
                text("Training budget", rx, y + 14.0, 20.0, TEXT);
                s.budget = s.budget.min(left);
                let mut opts: Vec<u64> = [50_000u64, 100_000, 200_000, left].iter().map(|o| (*o).min(left)).collect();
                opts.dedup();
                let mut bx = rx;
                for o in opts {
                    let label = if o == left {
                        format!("Max {}", fmt_steps(o))
                    } else {
                        format!("+{}", fmt_steps(o))
                    };
                    if self.ui.button_ex(
                        Rect::new(bx, y + 26.0, 106.0, 36.0),
                        &label,
                        true,
                        s.budget == o,
                    ) {
                        s.budget = o;
                    }
                    bx += 112.0;
                }
                let pool = self.opponents_for(s.idx);
                let t = &self.troops[s.idx];
                let share = (t.spec.drills.self_play * 100.0).round();
                let sp = match &pool {
                    Some(p) if t.spec.role == Role::Defend => format!(
                        "Self-play: {share}% of raids use your {} trained attackers",
                        p.len()
                    ),
                    Some(p) => format!(
                        "Self-play: {share}% of drills use your {} trained defenders",
                        p.len()
                    ),
                    None => {
                        "Opponents: scripted (train the other side to enable self-play)".to_string()
                    }
                };
                text(&sp, rx, y + 142.0, 16.0, MUTED);
                if self.ui.button(
                    Rect::new(rx, y + 74.0, rw, 50.0),
                    &format!("Start training (+{})", fmt_steps(s.budget)),
                ) {
                    let target = t.steps + s.budget.min(left);
                    s.job = Some(spawn(t, target, pool.clone()));
                    s.finished = None;
                }
            }
        } else {
            let stop = Rect::new(rx, H - 80.0, rw, 50.0);
            if self.ui.button(stop, "Stop and keep progress") {
                s.job.as_ref().unwrap().stop.store(true, Ordering::Relaxed);
            }
            if s.job.as_ref().unwrap().stop.load(Ordering::Relaxed) {
                text(
                    "Finishing the current update...",
                    rx,
                    stop.y - 10.0,
                    18.0,
                    MUTED,
                );
            }
            if done {
                self.finish_training(&mut s);
            }
        }
        if s.plan.is_some() && s.job.is_none() {
            if self.plan_overlay(&mut s) {
                s.preview = None;
            }
        }
        // Keep the preview on the newest brain once a drill ends.
        if let (Some(p), Some(a)) = (&mut s.preview, latest) {
            if p.world.tick == 0 {
                p.actor = a;
            }
        }
        Screen::Train(s)
    }
}

impl Game {
    /// The plan editor over the drill view. Returns true once a plan is applied.
    fn plan_overlay(&mut self, s: &mut TrainState) -> bool {
        let r = Rect::new(20.0, 66.0, 714.0, 714.0);
        draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.07, 0.075, 0.11, 0.96));
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
        let t = &self.troops[s.idx];
        let (x, w) = (r.x + 30.0, r.w - 60.0);
        text(&format!("Training plan for {}", t.name), x, r.y + 44.0, 28.0, TEXT);
        let mix = t.spec.mix_label();
        text(if mix.is_empty() { t.spec.name.as_str() } else { &mix }, x, r.y + 72.0, 18.0, ACCENT);
        crate::recruit::wrap(
            "Choose what the next training sessions practise and what the reward cares about. The brain keeps everything it already learned; new sessions push it toward the new plan.",
            x,
            r.y + 100.0,
            w,
            17.0,
            MUTED,
        );
        s.ed.begin();
        let mut plan = s.plan.unwrap();
        let base = ClassSpec::blend(&t.spec.parents);
        let ok = crate::plan::plan_editor(&self.ui, &mut s.ed, Rect::new(x, r.y + 160.0, w, 400.0), &base, &mut plan);
        s.plan = Some(plan);
        let by = r.y + r.h - 80.0;
        let mut applied = false;
        if self.ui.button_ex(Rect::new(x, by, 250.0, 48.0), "Apply plan", ok, false) {
            let t = &mut self.troops[s.idx];
            let mut ns = ClassSpec::planned(&t.spec.parents, plan).with_gear_of(&t.spec);
            ns.name = t.spec.name.clone();
            t.spec = ns;
            s.plan = None;
            applied = true;
            self.save_all();
            self.toast("Plan saved. The next session trains with it.".to_string());
        }
        if self.ui.button(Rect::new(x + 270.0, by, 200.0, 48.0), "Cancel") {
            s.plan = None;
        }
        if self.ui.button(Rect::new(x + 490.0, by, 164.0, 48.0), "Class default") {
            s.plan = Some(TrainPlan::default_for(&base));
        }
        s.ed.end(&self.ui);
        applied
    }
}

/// Keep the stored curve small.
fn thin(c: &[CurvePoint]) -> Vec<CurvePoint> {
    if c.len() <= 400 {
        return c.to_vec();
    }
    let step = c.len() as f32 / 400.0;
    (0..400)
        .map(|i| c[(i as f32 * step) as usize])
        .chain(std::iter::once(*c.last().unwrap()))
        .collect()
}
