//! Tournament: your troops run the same seeded proving-grounds raids and get
//! ranked. Any entry can be replayed exactly.

use crate::render::{draw_ground, draw_world, View};
use crate::ui::*;
use crate::{Game, Screen};
use godcore::battle::{run_drill, tournament, TournamentEntry, CUP_ROUNDS, CUP_SEED};
use godcore::nn::Cache;
use godcore::ppo::greedy_action;
use godcore::scenario::Drill;
use godcore::sim::{Outcome, World};
use godcore::troop::Troop;
use macroquad::prelude::*;

pub struct TourState {
    entrants: Vec<usize>,
    results: Option<Vec<TournamentEntry>>,
}

impl TourState {
    pub fn new(g: &Game) -> Self {
        TourState {
            entrants: attackers(g),
            results: None,
        }
    }
}

impl TourState {
    pub fn demo(g: &Game) -> Self {
        let entrants = attackers(g);
        let field: Vec<Troop> = entrants.iter().map(|&i| g.troops[i].clone()).collect();
        let mut res = tournament(&field);
        for e in &mut res {
            e.troop_index = entrants[e.troop_index];
        }
        TourState {
            entrants,
            results: Some(res),
        }
    }
}

/// The cup is for attackers; defenders prove themselves at the Home Base.
fn attackers(g: &Game) -> Vec<usize> {
    (0..g.troops.len())
        .filter(|&i| g.troops[i].spec.role == godcore::classes::Role::Attack)
        .collect()
}

pub struct ReplayState {
    troop: Troop,
    drill: Drill,
    round: u64,
    world: World,
    cache: Cache,
    acc: f32,
    back: TourState,
}

impl ReplayState {
    fn new(troop: Troop, drill: Drill, round: u64, back: TourState) -> Self {
        let (_, env) = run_drill(&troop, drill, CUP_SEED + round);
        ReplayState {
            troop,
            drill,
            round,
            world: env.world,
            cache: Cache::default(),
            acc: 0.0,
            back,
        }
    }
}

impl Game {
    pub fn tournament(&mut self, mut s: TourState) -> Screen {
        if self.header("Tournament: the Proving Grounds", true) {
            return Screen::Barracks;
        }
        text(
            &format!("Every entrant runs the same {CUP_ROUNDS} solo raids and {CUP_ROUNDS} squad raids (with two scripted fighters)."),
            20.0,
            90.0,
            20.0,
            MUTED,
        );
        text("Points: destruction x100, +50 for a win, +20 for surviving. Same seeds for everyone, so it's fair and replayable.", 20.0, 114.0, 20.0, MUTED);

        if s.results.is_none() {
            text("Entrants", 20.0, 160.0, 24.0, TEXT);
            for (slot, i) in attackers(self).into_iter().enumerate() {
                let t = &self.troops[i];
                let r = Rect::new(
                    20.0 + (slot % 4) as f32 * 310.0,
                    176.0 + (slot / 4) as f32 * 64.0,
                    300.0,
                    56.0,
                );
                if r.y > H - 140.0 {
                    break;
                }
                let on = s.entrants.contains(&i);
                draw_rectangle(r.x, r.y, r.w, r.h, if on { PANEL_HI } else { PANEL });
                draw_rectangle_lines(
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    if on { 2.0 } else { 1.0 },
                    if on { GOOD } else { LINE },
                );
                crate::art::draw_portrait(vec2(r.x + 28.0, r.y + 28.0), 18.0, &t.spec);
                text(&t.name, r.x + 54.0, r.y + 26.0, 22.0, TEXT);
                text(
                    &format!("Lv{} {}", t.level, t.spec.name),
                    r.x + 54.0,
                    r.y + 46.0,
                    16.0,
                    MUTED,
                );
                if self.ui.hover(r) && self.ui.clicked {
                    if on {
                        s.entrants.retain(|&x| x != i);
                    } else {
                        s.entrants.push(i);
                    }
                }
            }
            if self.ui.button_ex(
                Rect::new(W - 320.0, H - 80.0, 300.0, 56.0),
                "Run the cup",
                !s.entrants.is_empty(),
                false,
            ) {
                s.entrants.sort();
                let field: Vec<Troop> =
                    s.entrants.iter().map(|&i| self.troops[i].clone()).collect();
                let mut res = tournament(&field);
                for e in &mut res {
                    e.troop_index = s.entrants[e.troop_index];
                }
                s.results = Some(res);
            }
            return Screen::Tournament(s);
        }

        let res = s.results.as_ref().unwrap();
        let x = 20.0;
        let mut y = 160.0;
        let cols = [
            x + 10.0,
            x + 70.0,
            x + 330.0,
            x + 560.0,
            x + 760.0,
            x + 900.0,
        ];
        for (c, h) in cols
            .iter()
            .zip(["#", "Troop", "Solo cup", "Squad cup", "Total", ""])
        {
            text(h, *c, y, 20.0, MUTED);
        }
        y += 12.0;
        let mut watch: Option<(usize, Drill)> = None;
        for (rank, e) in res.iter().enumerate() {
            let t = &self.troops[e.troop_index];
            let r = Rect::new(x, y, W - 40.0, 56.0);
            draw_rectangle(
                r.x,
                r.y,
                r.w,
                r.h,
                if rank == 0 {
                    Color::new(0.25, 0.2, 0.1, 1.0)
                } else {
                    PANEL
                },
            );
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                1.0,
                if rank == 0 { ACCENT } else { LINE },
            );
            text(
                &format!("{}", rank + 1),
                cols[0],
                y + 36.0,
                28.0,
                if rank == 0 { ACCENT } else { TEXT },
            );
            crate::art::draw_portrait(vec2(cols[1] + 14.0, y + 28.0), 16.0, &t.spec);
            text(&t.name, cols[1] + 38.0, y + 26.0, 22.0, TEXT);
            text(
                &format!("Lv{} {}", t.level, t.spec.name),
                cols[1] + 38.0,
                y + 46.0,
                16.0,
                MUTED,
            );
            for (k, cup) in [&e.solo, &e.squad].iter().enumerate() {
                let c = cols[2 + k];
                text(&format!("{:.0} pts", cup.score), c, y + 26.0, 22.0, TEXT);
                text(
                    &format!(
                        "{}/{} wins, {:.0}% dmg",
                        cup.wins,
                        cup.played,
                        cup.destruction * 100.0
                    ),
                    c,
                    y + 46.0,
                    16.0,
                    MUTED,
                );
            }
            text(&format!("{:.0}", e.total), cols[4], y + 36.0, 28.0, ACCENT);
            if self
                .ui
                .button(Rect::new(cols[5], y + 10.0, 150.0, 36.0), "Watch solo")
            {
                watch = Some((e.troop_index, Drill::Solo));
            }
            if self.ui.button(
                Rect::new(cols[5] + 160.0, y + 10.0, 150.0, 36.0),
                "Watch squad",
            ) {
                watch = Some((e.troop_index, Drill::Squad));
            }
            y += 64.0;
            if y > H - 140.0 {
                break;
            }
        }
        if let Some(champ) = res.first() {
            text(
                &format!("Champion: {}", self.troops[champ.troop_index].name),
                x,
                H - 40.0,
                26.0,
                ACCENT,
            );
        }
        if self
            .ui
            .button(Rect::new(W - 320.0, H - 80.0, 300.0, 56.0), "New cup")
        {
            s.results = None;
        }
        if let Some((i, d)) = watch {
            self.fx.clear();
            return Screen::Replay(ReplayState::new(self.troops[i].clone(), d, 0, s));
        }
        Screen::Tournament(s)
    }

    pub fn replay(&mut self, mut s: ReplayState) -> Screen {
        if self.header(
            &format!(
                "Replay: {} in {} round {}",
                s.troop.name,
                s.drill.name(),
                s.round + 1
            ),
            true,
        ) {
            return Screen::Tournament(s.back);
        }
        let dt = crate::record::dt().min(0.1);
        let running = s.world.outcome() == Outcome::Running && s.world.units[0].alive;
        if running {
            s.acc += dt * 30.0;
            while s.acc >= 1.0 && s.world.outcome() == Outcome::Running && s.world.units[0].alive {
                s.acc -= 1.0;
                let mut a = vec![0u8; s.world.units.len()];
                a[0] = greedy_action(
                    &s.troop.actor,
                    &s.world.observe(0),
                    &s.world.mask(0),
                    &mut s.cache,
                );
                s.world.step(&a);
                self.fx.absorb(&s.world);
            }
        }
        self.fx.age(dt);
        let v = View::new(Rect::new(20.0, 66.0, 714.0, 714.0));
        draw_ground(v, false, true, &s.world);
        let look = Game::look_for(&s.troop, true);
        let name = look.name.clone().unwrap_or_default();
        let mine = s.world.units[0].team;
        draw_world(v, &s.world, &self.fx, &|i, u| {
            crate::art::drill_look(i, u, &[0], mine, look.fig, &name)
        });
        let rx = 754.0;
        text(&format!("Tick {}", s.world.tick), rx, 100.0, 24.0, TEXT);
        text(
            &format!("{:.0}% destroyed", s.world.destruction() * 100.0),
            rx,
            130.0,
            22.0,
            ACCENT,
        );
        if !running {
            let won = s.world.outcome() == Outcome::Victory;
            text(
                if won {
                    "Won"
                } else if !s.world.units[0].alive {
                    "Fell"
                } else {
                    "Out of time"
                },
                rx,
                180.0,
                36.0,
                if won { GOOD } else { BAD },
            );
        }
        let next = (s.round + 1) % CUP_ROUNDS;
        if self.ui.button(
            Rect::new(rx, 220.0, 260.0, 44.0),
            &format!("Next round ({})", next + 1),
        ) {
            let back = std::mem::replace(
                &mut s.back,
                TourState {
                    entrants: vec![],
                    results: None,
                },
            );
            self.fx.clear();
            return Screen::Replay(ReplayState::new(s.troop, s.drill, next, back));
        }
        Screen::Replay(s)
    }
}
