//! Battles: pick a base and a squad, place troops, then watch them fight.

use crate::art::Figure;
use crate::render::Look;
use crate::render::{draw_ground, draw_world, View};
use crate::ui::*;
use crate::{Game, Screen};
use godcore::battle::{fingerprint, Battle};
use godcore::classes::Role;
use godcore::scenario::{build_base, can_deploy, BASES, DEPLOY_RADIUS, HOME_NAME, WAVES};
use godcore::sim::{Kind, Outcome, Team, World, ARENA};
use macroquad::prelude::*;

pub const MAX_SQUAD: usize = 5;

pub struct SetupState {
    base: usize,
    picked: Vec<usize>,
}

impl SetupState {
    pub fn new(g: &Game) -> Self {
        let base = g
            .profile
            .best_stars
            .iter()
            .position(|&s| s < 3)
            .unwrap_or(0);
        let mut picked: Vec<usize> = (0..g.troops.len())
            .filter(|&i| g.troops[i].spec.role == Role::Attack)
            .collect();
        picked.sort_by(|&a, &b| {
            g.troops[b]
                .skills
                .win
                .partial_cmp(&g.troops[a].skills.win)
                .unwrap()
        });
        picked.truncate(MAX_SQUAD);
        picked.sort();
        SetupState { base, picked }
    }
}

pub struct DeployState {
    base: usize,
    squad: Vec<usize>,
    spots: Vec<(f32, f32)>,
    preview: World,
}

/// What kind of battle is being watched.
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// Attacking one of the AI bases.
    Raid(usize),
    /// Holding the home base against a scripted wave, or (None) your own army.
    Defend(Option<usize>),
}

pub struct BattleState {
    mode: Mode,
    battle: Battle,
    /// Rebuilds the same battle from scratch, for the replay check.
    remake: Box<dyn Fn() -> Battle>,
    acc: f32,
    speed: f32,
    result: Option<(u32, u32, bool)>,
    replay_ok: Option<bool>,
}

impl BattleState {
    pub fn start(mode: Mode, remake: Box<dyn Fn() -> Battle>) -> Self {
        BattleState {
            mode,
            battle: remake(),
            remake,
            acc: 0.0,
            speed: 1.0,
            result: None,
            replay_ok: None,
        }
    }
}

impl DeployState {
    pub fn demo(_g: &Game, base: usize, squad: Vec<usize>) -> Self {
        let mut spots = Vec::new();
        for k in 0..squad.len().saturating_sub(1) {
            spots.push((4.0 + 1.5 * k as f32, 4.0));
        }
        DeployState {
            base,
            squad,
            spots,
            preview: build_base(&BASES[base]),
        }
    }
}

impl BattleState {
    pub fn demo(g: &Game, base: usize, squad: Vec<usize>) -> Self {
        let deploy: Vec<_> = squad
            .iter()
            .enumerate()
            .map(|(k, &i)| (g.troops[i].clone(), (4.0 + 1.5 * k as f32, 4.0)))
            .collect();
        BattleState::start(
            Mode::Raid(base),
            Box::new(move || Battle::new(&BASES[base], deploy.clone())),
        )
    }
}

/// Sim ticks shown per second at 1x. Playing all 20 ticks of a sim second
/// made troops sprint and hit five times a second; at 6 they move at a
/// Clash-of-Clans-like pace, a fighter strikes about every 0.7 s, and a full
/// battle (900 ticks) lasts 2.5 minutes. Only playback changes: results are
/// the same.
pub const WATCH_TPS: f32 = 6.0;

fn base_view() -> View {
    View::new(Rect::new(20.0, 66.0, 714.0, 714.0))
}

impl Game {
    pub fn battle_setup(&mut self, mut s: SetupState) -> Screen {
        if self.header("Battle: choose a base and a squad", true) {
            return Screen::Barracks;
        }
        for (k, b) in BASES.iter().enumerate() {
            let r = Rect::new(20.0 + k as f32 * 310.0, 70.0, 300.0, 170.0);
            let sel = s.base == k;
            draw_rectangle(r.x, r.y, r.w, r.h, if sel { PANEL_HI } else { PANEL });
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                if sel { 3.0 } else { 1.0 },
                if sel { ACCENT } else { LINE },
            );
            text(b.name, r.x + 16.0, r.y + 34.0, 28.0, TEXT);
            for st in 0..3 {
                star(
                    r.x + r.w - 80.0 + st as f32 * 26.0,
                    r.y + 26.0,
                    11.0,
                    st < self.profile.best_stars[k],
                );
            }
            text(
                &format!("{} towers, {} guards", b.towers.len(), b.guards.len()),
                r.x + 16.0,
                r.y + 60.0,
                18.0,
                MUTED,
            );
            crate::recruit::wrap(b.blurb, r.x + 16.0, r.y + 90.0, r.w - 32.0, 18.0, MUTED);
            text(
                &format!("{} gold per new star", b.reward_per_star),
                r.x + 16.0,
                r.y + 155.0,
                18.0,
                ACCENT,
            );
            if self.ui.hover(r) && self.ui.clicked {
                s.base = k;
            }
        }

        text(
            &format!("Squad ({}/{MAX_SQUAD})", s.picked.len()),
            20.0,
            280.0,
            24.0,
            TEXT,
        );
        let cols = 4;
        let mut full = false;
        let attackers: Vec<usize> = (0..self.troops.len())
            .filter(|&i| self.troops[i].spec.role == Role::Attack)
            .collect();
        for (slot, &i) in attackers.iter().enumerate() {
            let t = &self.troops[i];
            let r = Rect::new(
                20.0 + (slot % cols) as f32 * 310.0,
                296.0 + (slot / cols) as f32 * 70.0,
                300.0,
                62.0,
            );
            if r.y > H - 120.0 {
                break;
            }
            let on = s.picked.contains(&i);
            draw_rectangle(r.x, r.y, r.w, r.h, if on { PANEL_HI } else { PANEL });
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                if on { 2.0 } else { 1.0 },
                if on { GOOD } else { LINE },
            );
            crate::art::draw_portrait(vec2(r.x + 30.0, r.y + 31.0), 20.0, &t.spec);
            text(&t.name, r.x + 58.0, r.y + 28.0, 22.0, TEXT);
            text(
                &format!(
                    "Lv{} {}  {:.0}% wins",
                    t.level,
                    t.spec.name,
                    t.skills.win * 100.0
                ),
                r.x + 58.0,
                r.y + 50.0,
                16.0,
                MUTED,
            );
            if self.ui.hover(r) && self.ui.clicked {
                if on {
                    s.picked.retain(|&x| x != i);
                } else if s.picked.len() < MAX_SQUAD {
                    s.picked.push(i);
                } else {
                    full = true;
                }
            }
        }
        if full {
            self.toast(format!("A squad holds at most {MAX_SQUAD} troops"));
        }
        if self.ui.button_ex(
            Rect::new(W - 320.0, H - 80.0, 300.0, 56.0),
            "Deploy troops >",
            !s.picked.is_empty(),
            false,
        ) {
            let preview = build_base(&BASES[s.base]);
            return Screen::Deploy(DeployState {
                base: s.base,
                squad: s.picked.clone(),
                spots: Vec::new(),
                preview,
            });
        }
        text(
            "Click troops to add or remove them.",
            20.0,
            H - 44.0,
            18.0,
            MUTED,
        );
        Screen::BattleSetup(s)
    }

    pub fn deploy(&mut self, mut s: DeployState) -> Screen {
        let base = &BASES[s.base];
        if self.header(&format!("Deploy against {}", base.name), true) {
            return Screen::BattleSetup(SetupState {
                base: s.base,
                picked: s.squad,
            });
        }
        let v = base_view();
        draw_ground(v, true, true, &s.preview);
        self.fx.clear();
        draw_world(v, &s.preview, &self.fx, &|_, _| unreachable!());
        // Placed troops
        for (k, &(x, y)) in s.spots.iter().enumerate() {
            let t = &self.troops[s.squad[k]];
            crate::render::draw_standing(v, x, y, &crate::art::Figure::of(&t.spec), Some(&t.name));
        }
        let next = s.spots.len();
        if v.r.contains(self.ui.mouse) && next < s.squad.len() {
            let (wx, wy) = v.to_world(self.ui.mouse);
            let ok = can_deploy(wx, wy);
            let col = if ok {
                rgb(self.troops[s.squad[next]].spec.color)
            } else {
                Color::new(0.9, 0.2, 0.2, 0.6)
            };
            draw_circle(
                self.ui.mouse.x,
                self.ui.mouse.y,
                0.5 * v.s(),
                Color::new(col.r, col.g, col.b, 0.6),
            );
            if self.ui.clicked {
                if ok {
                    s.spots.push((wx, wy));
                } else {
                    self.toast("Deploy outside the red zone");
                }
            }
        }
        if self.ui.right_clicked {
            s.spots.pop();
        }

        let rx = 754.0;
        let rw = W - rx - 20.0;
        text("Click the map to place each troop", rx, 96.0, 22.0, TEXT);
        text(
            "outside the red zone. Right-click undoes.",
            rx,
            120.0,
            18.0,
            MUTED,
        );
        text(
            "Where and together or apart is up to you.",
            rx,
            142.0,
            18.0,
            MUTED,
        );
        for (k, &i) in s.squad.iter().enumerate() {
            let t = &self.troops[i];
            let y = 170.0 + k as f32 * 56.0;
            let r = Rect::new(rx, y, rw, 48.0);
            let placed = k < s.spots.len();
            draw_rectangle(r.x, r.y, r.w, r.h, if k == next { PANEL_HI } else { PANEL });
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                1.0,
                if k == next { ACCENT } else { LINE },
            );
            crate::art::draw_portrait(vec2(r.x + 24.0, r.y + 24.0), 16.0, &t.spec);
            text(
                &format!("{}  (Lv{} {})", t.name, t.level, t.spec.name),
                r.x + 48.0,
                r.y + 30.0,
                20.0,
                TEXT,
            );
            let st = if placed {
                "placed"
            } else if k == next {
                "next"
            } else {
                ""
            };
            text(
                st,
                r.x + r.w - 70.0,
                r.y + 30.0,
                18.0,
                if placed { GOOD } else { ACCENT },
            );
        }
        let by = 170.0 + s.squad.len() as f32 * 56.0 + 20.0;
        if self.ui.button_ex(
            Rect::new(rx, by, rw / 2.0 - 5.0, 44.0),
            "Auto-place rest",
            next < s.squad.len(),
            false,
        ) {
            // Line up the rest on one side, next to the first troop if any.
            let (cx, cy) = (ARENA / 2.0, ARENA / 2.0);
            let (mut ux, mut uy) = match s.spots.first() {
                Some(&(x, y)) => (x - cx, y - cy),
                None => (-1.0, -1.0),
            };
            let d = (ux * ux + uy * uy).sqrt();
            ux /= d;
            uy /= d;
            let r = DEPLOY_RADIUS + 2.0;
            for k in next..s.squad.len() {
                let off = (k as f32 - (s.squad.len() as f32 - 1.0) / 2.0) * 1.4;
                let (x, y) = (cx + ux * r - uy * off, cy + uy * r + ux * off);
                s.spots
                    .push((x.clamp(1.0, ARENA - 1.0), y.clamp(1.0, ARENA - 1.0)));
            }
        }
        if self.ui.button_ex(
            Rect::new(rx + rw / 2.0 + 5.0, by, rw / 2.0 - 5.0, 44.0),
            "Clear",
            !s.spots.is_empty(),
            false,
        ) {
            s.spots.clear();
        }
        let ready = s.spots.len() == s.squad.len();
        if self
            .ui
            .button_ex(Rect::new(rx, H - 80.0, rw, 56.0), "FIGHT!", ready, false)
        {
            let deploy: Vec<_> = s
                .squad
                .iter()
                .zip(&s.spots)
                .map(|(&i, &p)| (self.troops[i].clone(), p))
                .collect();
            self.fx.clear();
            let b = s.base;
            return Screen::Battle(BattleState::start(
                Mode::Raid(b),
                Box::new(move || Battle::new(&BASES[b], deploy.clone())),
            ));
        }
        Screen::Deploy(s)
    }

    pub fn battle(&mut self, mut s: BattleState) -> Screen {
        let title = match s.mode {
            Mode::Raid(b) => format!("Battle: {}", BASES[b].name),
            Mode::Defend(Some(w)) => format!("Defending {} against {}", HOME_NAME, WAVES[w].name),
            Mode::Defend(None) => format!("Defending {} against your own army", HOME_NAME),
        };
        let defending = matches!(s.mode, Mode::Defend(_));
        self.header(&title, false);
        let dt = crate::record::dt().min(0.1);
        if s.result.is_none() {
            s.acc += dt * WATCH_TPS * s.speed;
            while s.acc >= 1.0 && s.battle.outcome() == Outcome::Running {
                s.acc -= 1.0;
                s.battle.step();
                self.fx.absorb(&s.battle.world);
            }
            if s.battle.outcome() != Outcome::Running {
                s.result = Some(self.settle(&s));
                let stars = s.battle.world.stars();
                let won = if defending { stars == 0 } else { stars > 0 };
                crate::sfx::play_ui(if won { crate::sfx::Sfx::Victory } else { crate::sfx::Sfx::Defeat });
                // Determinism check: replay the same battle headless.
                let mut again = (s.remake)();
                again.run();
                s.replay_ok = Some(fingerprint(&again.world) == fingerprint(&s.battle.world));
            }
        }
        self.fx.age(dt);
        let v = base_view();
        let w = &s.battle.world;
        draw_ground(v, false, true, w);
        let troops = &s.battle.troops;
        draw_world(v, w, &self.fx, &|_, u| {
            if u.tag < troops.len() {
                let mut l = Game::look_for(&troops[u.tag], true);
                l.hostile = defending && u.team == Team::Attack;
                l
            } else {
                Look {
                    fig: Figure::raider(),
                    name: None,
                    hostile: true,
                }
            }
        });

        // HUD
        let rx = 754.0;
        let rw = W - rx - 20.0;
        let secs_left = (w.max_ticks - w.tick) as f32 / WATCH_TPS;
        text(
            &format!("Time left {:.0}s", secs_left),
            rx,
            96.0,
            26.0,
            TEXT,
        );
        let dest = w.destruction();
        let dest_label = if defending {
            "of your base lost"
        } else {
            "destroyed"
        };
        text(
            &format!("{:.0}% {dest_label}", dest * 100.0),
            rx + 260.0,
            96.0,
            26.0,
            ACCENT,
        );
        bar(Rect::new(rx, 108.0, rw, 12.0), dest, ACCENT);
        let stars = w.stars();
        for k in 0..3 {
            star(rx + 24.0 + k as f32 * 44.0, 152.0, 18.0, k < stars);
        }
        let star_note = if defending {
            "stars the raiders earn against you"
        } else {
            "50%   Town Hall   100%"
        };
        text(star_note, rx + 6.0, 188.0, 16.0, MUTED);
        if defending {
            let raiders = w.units.iter().filter(|u| u.team == Team::Attack).count();
            let up = w
                .units
                .iter()
                .filter(|u| u.alive && u.team == Team::Attack)
                .count();
            text(
                &format!("Raiders standing: {up}/{raiders}"),
                rx + 200.0,
                160.0,
                20.0,
                BAD,
            );
        }
        let mut y = 216.0;
        for (i, u) in w
            .units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.kind == Kind::Troop && u.tag < troops.len())
        {
            if y > H - 130.0 {
                break;
            }
            let t = &troops[u.tag];
            let r = Rect::new(rx, y, rw, 50.0);
            panel(r);
            if u.alive {
                crate::art::draw_portrait(vec2(r.x + 22.0, r.y + 25.0), 16.0, &t.spec);
            } else {
                draw_circle(r.x + 22.0, r.y + 25.0, 16.0, Color::new(0.3, 0.3, 0.3, 1.0));
            }
            text(
                &t.name,
                r.x + 44.0,
                r.y + 22.0,
                20.0,
                if u.alive { TEXT } else { MUTED },
            );
            let said = self.fx.recent_call(i).filter(|_| u.alive);
            let act = match said {
                Some(c) => format!("says \"{}\"", crate::render::call_words(c.kind)),
                None if !u.alive => "down".to_string(),
                None => action_name(u.last_action).to_string(),
            };
            text(
                &act,
                r.x + r.w - text_width(&act, 16.0) - 10.0,
                r.y + 22.0,
                16.0,
                said.map_or(MUTED, |c| crate::render::call_color(c.kind)),
            );
            bar(
                Rect::new(r.x + 44.0, r.y + 32.0, r.w - 56.0, 8.0),
                u.hp / u.max_hp,
                GOOD,
            );
            y += 56.0;
        }
        // What the team has been telling each other, newest first.
        if !self.fx.calls.is_empty() && y < H - 200.0 {
            y += 6.0;
            text("Team talk", rx, y + 14.0, 18.0, TEXT);
            y += 24.0;
            let who = |k: usize| {
                let u = &w.units[k];
                match u.kind {
                    Kind::Troop if u.team == Team::Attack && u.tag < troops.len() && !defending => troops[u.tag].name.clone(),
                    Kind::Troop if u.team == Team::Defend && u.tag < troops.len() && defending => troops[u.tag].name.clone(),
                    Kind::Troop => if defending { "a raider" } else { "an enemy troop" }.to_string(),
                    Kind::Guard => "the guard".to_string(),
                    Kind::Tower => "the tower".to_string(),
                    Kind::Core => "the Town Hall".to_string(),
                }
            };
            for c in self.fx.calls.iter().rev().take(((H - 110.0 - y) / 22.0).max(0.0) as usize) {
                let line = match c.target {
                    Some(t) => format!("{}: {} ({})", who(c.from), crate::render::call_words(c.kind), who(t)),
                    None => format!("{}: {}", who(c.from), crate::render::call_words(c.kind)),
                };
                text(&line, rx + 6.0, y + 14.0, 16.0, crate::render::call_color(c.kind));
                y += 22.0;
            }
        }
        if s.result.is_none() {
            let by = H - 80.0;
            for (k, sp) in [1.0f32, 2.0, 4.0].into_iter().enumerate() {
                if self.ui.button_ex(
                    Rect::new(rx + k as f32 * 90.0, by, 82.0, 44.0),
                    &format!("{sp:.0}x"),
                    true,
                    s.speed == sp,
                ) {
                    s.speed = sp;
                }
            }
            if self
                .ui
                .button(Rect::new(rx + 280.0, by, rw - 280.0, 44.0), "Skip to end")
            {
                s.speed = 1000.0;
            }
        } else {
            let (stars, gold, new_best) = s.result.unwrap();
            let r = Rect::new(rx, H - 290.0, rw, 270.0);
            draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.06, 0.06, 0.09, 0.95));
            draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
            let (title, good) = if defending {
                match s.battle.outcome() {
                    Outcome::Victory => ("Base fallen", false),
                    _ if stars > 0 => ("Raiders took stars", false),
                    _ if s.battle.world.destruction() < 0.01 => ("FLAWLESS DEFENCE", true),
                    Outcome::Wiped => ("Raid repelled", true),
                    _ => ("Held until time ran out", true),
                }
            } else {
                match s.battle.outcome() {
                    Outcome::Victory => ("TOTAL VICTORY", true),
                    Outcome::Wiped if stars > 0 => ("Squad fell, but scored", true),
                    Outcome::Wiped => ("Defeat", false),
                    _ if stars > 0 => ("Time up", true),
                    _ => ("Defeat: time ran out", false),
                }
            };
            text_centered(
                title,
                r.x + r.w / 2.0,
                r.y + 44.0,
                34.0,
                if good { ACCENT } else { BAD },
            );
            for k in 0..3 {
                star(
                    r.x + r.w / 2.0 - 50.0 + k as f32 * 50.0,
                    r.y + 88.0,
                    20.0,
                    k < stars,
                );
            }
            text_centered(
                &format!(
                    "+{gold} gold{}",
                    if new_best { "  (new best!)" } else { "" }
                ),
                r.x + r.w / 2.0,
                r.y + 140.0,
                24.0,
                ACCENT,
            );
            if let Some(ok) = s.replay_ok {
                let m = if ok {
                    "Replay check: identical outcome when re-simulated"
                } else {
                    "Replay check FAILED"
                };
                text_centered(
                    m,
                    r.x + r.w / 2.0,
                    r.y + 170.0,
                    16.0,
                    if ok { MUTED } else { BAD },
                );
            }
            if self.ui.button(
                Rect::new(r.x + 20.0, r.y + 196.0, r.w / 2.0 - 30.0, 50.0),
                "Barracks",
            ) {
                return Screen::Barracks;
            }
            if self.ui.button(
                Rect::new(r.x + r.w / 2.0 + 10.0, r.y + 196.0, r.w / 2.0 - 30.0, 50.0),
                if defending {
                    "Defend again"
                } else {
                    "Battle again"
                },
            ) {
                if defending {
                    return Screen::Home(crate::home::HomeState::new(self));
                }
                return Screen::BattleSetup(SetupState::new(self));
            }
        }
        Screen::Battle(s)
    }

    /// Record the result and pay out. Returns (stars, gold, new best?).
    fn settle(&mut self, s: &BattleState) -> (u32, u32, bool) {
        let b = match s.mode {
            Mode::Raid(b) => b,
            Mode::Defend(wave) => {
                let stars = s.battle.world.stars();
                // A wave counts as held when the raiders leave with no stars.
                let held = stars == 0;
                let mut gold = 0;
                let mut first = false;
                if let (Some(wi), true) = (wave, held) {
                    first = !self.profile.waves_held[wi];
                    self.profile.waves_held[wi] = true;
                    gold = if first {
                        WAVES[wi].reward
                    } else {
                        WAVES[wi].reward / 8
                    };
                }
                self.profile.gold += gold;
                self.store.save_profile(&self.profile);
                return (stars, gold, first);
            }
        };
        let base = &BASES[b];
        let stars = s.battle.world.stars();
        let best = self.profile.best_stars[b];
        let new_stars = stars.saturating_sub(best);
        let gold = new_stars * base.reward_per_star + stars * base.reward_per_star / 8;
        self.profile.gold += gold;
        self.profile.best_stars[b] = best.max(stars);
        self.profile.battles += 1;
        if s.battle.outcome() == Outcome::Victory {
            self.profile.victories += 1;
        }
        self.store.save_profile(&self.profile);
        (stars, gold, new_stars > 0)
    }
}

pub fn action_name(a: u8) -> &'static str {
    match a {
        0 => "waiting",
        1..=8 => "moving",
        9 => "fighting",
        10 => "hitting a building",
        11 => "healing",
        _ => "",
    }
}
