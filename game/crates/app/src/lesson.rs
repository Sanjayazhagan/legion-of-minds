//! Before and after: the same troop in the same drill, side by side. On the
//! left it has the brain it was born with; on the right, the brain training
//! gave it. Same body, same drill, same luck: only the brain differs, so
//! the difference is what it learned.
//!
//! Part of the new player tour (a troop visibly learning within the first
//! few minutes), and open any time from a troop's service record.

use crate::overlay::{card, Flow, Overlay};
use crate::render::{draw_ground, draw_world, Effects, View};
use crate::ui::*;
use crate::Game;
use godcore::classes::{ClassSpec, Role};
use godcore::nn::{Cache, Mlp};
use godcore::ppo::greedy_action;
use godcore::rng::Rng;
use godcore::scenario::{add_raiders, build_defense_drill, build_drill, Drill, DEFENSE_SLOTS};
use godcore::sim::{Outcome, World};
use godcore::troop::Troop;
use macroquad::prelude::*;

/// Drills shown before the summary.
const ROUNDS: u32 = 3;
/// Sim ticks shown per second.
const TPS: f32 = 30.0;

struct Arena {
    world: World,
    controlled: Vec<usize>,
    defense: bool,
    cache: Cache,
    acc: f32,
    fx: Effects,
}

impl Arena {
    fn new(spec: &ClassSpec, seed: u64) -> Arena {
        let mut rng = Rng::new(seed);
        let (world, controlled, defense) = if spec.role == Role::Defend {
            let (mut w, from) = build_defense_drill(spec, &mut rng);
            let n = 2 + rng.below(2);
            add_raiders(&mut w, n, from, 1.0, &mut rng);
            (w, (0..DEFENSE_SLOTS).collect(), true)
        } else {
            let drill = Drill::pick(spec, &mut rng);
            (build_drill(spec, drill, &mut rng), vec![0], false)
        };
        Arena { world, controlled, defense, cache: Cache::default(), acc: 0.0, fx: Effects::new() }
    }

    fn running(&self) -> bool {
        self.world.outcome() == Outcome::Running && (self.defense || self.world.units[0].alive)
    }

    fn won(&self) -> bool {
        (self.world.outcome() == Outcome::Victory) != self.defense
    }

    /// What the troop got done: damage to enemies and buildings, and
    /// healing. A blank brain mostly wanders, so this shows learning even
    /// when neither side wins the drill.
    fn done(&self) -> f32 {
        self.controlled
            .iter()
            .map(|&i| {
                let s = &self.world.units[i].stats;
                s.unit_dmg + s.building_dmg + s.heal
            })
            .sum()
    }

    fn step(&mut self, brain: &Mlp, dt: f32) {
        if !self.running() {
            self.fx.age(dt);
            return;
        }
        self.acc += dt * TPS;
        while self.acc >= 1.0 && self.running() {
            self.acc -= 1.0;
            let mut actions = vec![0u8; self.world.units.len()];
            for &i in &self.controlled {
                if self.world.units[i].alive {
                    actions[i] = greedy_action(brain, &self.world.observe(i), &self.world.mask(i), &mut self.cache);
                }
            }
            self.world.step(&actions);
            self.fx.absorb(&self.world);
        }
        self.fx.age(dt);
    }
}

pub struct Lesson {
    name: String,
    spec: ClassSpec,
    steps: u64,
    born: Mlp,
    now: Mlp,
    seed: u64,
    round: u32,
    arenas: [Arena; 2],
    wins: [u32; 2],
    /// Damage and healing over the drills so far.
    work: [f32; 2],
    /// Seconds both drills have been over.
    hold: f32,
    counted: bool,
}

impl Lesson {
    pub fn new(t: &Troop) -> Lesson {
        // An untrained brain from the troop's own seed: how it started out
        // (a brain grown since is wider, but just as blank).
        let born = Troop::new(t.id, t.name.clone(), t.spec.clone(), t.seed).actor;
        let seed = t.seed ^ 0x1E55_0000 ^ t.steps;
        Lesson {
            name: t.name.trim().to_string(),
            spec: t.spec.clone(),
            steps: t.steps,
            born,
            now: t.actor.clone(),
            seed,
            round: 0,
            arenas: [Arena::new(&t.spec, seed), Arena::new(&t.spec, seed)],
            wins: [0, 0],
            work: [0.0, 0.0],
            hold: 0.0,
            counted: false,
        }
    }

    fn over(&self) -> bool {
        self.round >= ROUNDS
    }
}

impl Game {
    pub(crate) fn lesson(&mut self, mut l: Box<Lesson>) -> Flow {
        let r = card(1240.0, 712.0);
        let dt = crate::record::dt().min(0.1);
        text(&format!("Before and after: {}", l.name), r.x + 24.0, r.y + 40.0, 30.0, ACCENT);
        text("Same body, same drill, same luck. Only the brain is different.", r.x + 24.0, r.y + 64.0, 17.0, MUTED);
        if self.ui.button(Rect::new(r.x + r.w - 140.0, r.y + 16.0, 116.0, 36.0), if l.over() { "Done" } else { "Close" }) {
            return Flow::close();
        }
        if !l.over() {
            let (born, now) = (l.born.clone(), l.now.clone());
            l.arenas[0].step(&born, dt);
            l.arenas[1].step(&now, dt);
        }
        let side = 590.0;
        let fig = crate::art::Figure::of(&l.spec);
        let labels = [
            "The brain it was born with".to_string(),
            format!("Now, after {} steps of practice", crate::barracks::fmt_steps(l.steps)),
        ];
        for k in 0..2 {
            let v = View::new(Rect::new(r.x + 20.0 + k as f32 * (side + 20.0), r.y + 80.0, side, side));
            let a = &l.arenas[k];
            draw_ground(v, false, true, &a.world);
            let controlled = a.controlled.clone();
            let mine = controlled.first().map_or(godcore::sim::Team::Attack, |&i| a.world.units[i].team);
            let name = l.name.clone();
            draw_world(v, &a.world, &a.fx, &|i, u| crate::art::drill_look(i, u, &controlled, mine, fig, &name));
            // Labels along the bottom (the top corner shows the light).
            let lw = text_width(&labels[k], 20.0) + 20.0;
            draw_rectangle(v.r.x, v.r.y + side - 62.0, lw, 30.0, Color::new(0.0, 0.0, 0.0, 0.6));
            text(&labels[k], v.r.x + 10.0, v.r.y + side - 41.0, 20.0, if k == 0 { MUTED } else { GOOD });
            if !a.running() && !l.over() {
                let (msg, good) = if a.won() {
                    (if a.defense { "Base held" } else { "Drill won" }, true)
                } else if a.defense {
                    ("Base fallen", false)
                } else if !a.world.units[0].alive {
                    ("Fell", false)
                } else {
                    ("Out of time", false)
                };
                text_centered(msg, v.r.x + side / 2.0, v.r.y + 70.0, 38.0, if good { GOOD } else { BAD });
            }
            let work = l.work[k] + if l.over() { 0.0 } else { a.done() };
            let score = format!("Damage and healing: {work:.0}   Drills won: {} of {}", l.wins[k], l.round.min(ROUNDS));
            draw_rectangle(v.r.x, v.r.y + side - 30.0, side, 30.0, Color::new(0.0, 0.0, 0.0, 0.6));
            text(&score, v.r.x + 10.0, v.r.y + side - 9.0, 18.0, TEXT);
        }
        // Both drills over: count them, then the next pair.
        if !l.over() && !l.arenas[0].running() && !l.arenas[1].running() {
            l.hold += dt;
            if l.hold > 1.5 {
                for k in 0..2 {
                    l.wins[k] += l.arenas[k].won() as u32;
                    l.work[k] += l.arenas[k].done();
                }
                l.round += 1;
                l.hold = 0.0;
                if !l.counted {
                    l.counted = true;
                    self.journal.lessons_watched += 1;
                    self.journal.touch();
                }
                if !l.over() {
                    let seed = l.seed.wrapping_add(l.round as u64 * 7919);
                    l.arenas = [Arena::new(&l.spec, seed), Arena::new(&l.spec, seed)];
                }
            }
        }
        let by = r.y + r.h - 22.0;
        if l.over() {
            let [b, n] = l.wins;
            let [wb, wn] = l.work;
            let better = n > b || (n == b && wn > wb * 1.25 + 5.0);
            let verdict = if n > b {
                format!("Training worked: {} won {n} of {ROUNDS} drills, against {b} with the brain it was born with.", l.name)
            } else if better {
                format!("Training worked: {} got {wn:.0} damage and healing done, against {wb:.0} with the brain it was born with.", l.name)
            } else {
                format!("{} hasn't pulled ahead in these drills yet. A bit more training usually makes it click.", l.name)
            };
            let w = text_width(&verdict, 19.0) + 40.0;
            let br = Rect::new(r.x + (r.w - w) / 2.0, r.y + 300.0, w, 60.0);
            draw_rectangle(br.x, br.y, br.w, br.h, Color::new(0.04, 0.05, 0.08, 0.95));
            draw_rectangle_lines(br.x, br.y, br.w, br.h, 2.0, if better { GOOD } else { ACCENT });
            text(&verdict, br.x + 20.0, br.y + 37.0, 19.0, TEXT);
            if self.ui.button(Rect::new(r.x + r.w / 2.0 - 100.0, r.y + 380.0, 200.0, 44.0), "Watch again") {
                return Flow::stay(Overlay::Lesson(Box::new(Lesson {
                    round: 0,
                    wins: [0, 0],
                    work: [0.0, 0.0],
                    hold: 0.0,
                    seed: l.seed.wrapping_add(1),
                    arenas: [Arena::new(&l.spec, l.seed.wrapping_add(1)), Arena::new(&l.spec, l.seed.wrapping_add(1))],
                    born: l.born.clone(),
                    now: l.now.clone(),
                    name: l.name.clone(),
                    spec: l.spec.clone(),
                    steps: l.steps,
                    counted: true,
                })));
            }
        } else {
            text(&format!("Drill {} of {ROUNDS}", l.round + 1), r.x + 24.0, by, 17.0, MUTED);
        }
        Flow::stay(Overlay::Lesson(l))
    }
}
