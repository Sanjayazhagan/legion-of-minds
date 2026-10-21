//! Home base: build defences around your Town Hall, place your trained
//! defenders, then hold off a raid wave or an attack by your own army.

use crate::battle_screen::{BattleState, Mode};
use crate::render::{draw_building_ghost, draw_ground, draw_world, View};
use crate::ui::*;
use crate::{Game, Screen};
use godcore::battle::Battle;
use godcore::classes::Role;
use godcore::scenario::{
    build_home_base, can_place_defender, home_spots, max_troop_level, town_hall_cost, BuildKind,
    HomeBase, Placed, HOME_NAME, HOME_RADIUS, MAX_TOWN_HALL, RAID_ENTRY, WAVES,
};
use godcore::sim::{World, ARENA};
use macroquad::prelude::*;

pub const MAX_DEFENDERS: usize = 4;
const MAX_ARMY: usize = 5;

#[derive(Clone, Copy, PartialEq)]
enum Opponent {
    Wave(usize),
    Army,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Defend,
    Build,
}

/// What the cursor is carrying in build mode.
#[derive(Clone, Copy, PartialEq)]
enum Carry {
    New(BuildKind),
    /// An existing building picked up to move, with where it came from.
    Moving(Placed),
}

pub struct HomeState {
    tab: Tab,
    picked: Vec<usize>,
    spots: Vec<(f32, f32)>,
    opp: Opponent,
    carry: Option<Carry>,
    preview: World,
}

impl HomeState {
    pub fn new(g: &Game) -> Self {
        let mut picked: Vec<usize> = (0..g.troops.len())
            .filter(|&i| g.troops[i].spec.role == Role::Defend)
            .collect();
        picked.truncate(MAX_DEFENDERS);
        let wave = g.profile.waves_held.iter().position(|&h| !h).unwrap_or(0);
        HomeState {
            tab: Tab::Defend,
            picked,
            spots: Vec::new(),
            opp: Opponent::Wave(wave),
            carry: None,
            preview: build_home_base(&g.profile.base),
        }
    }

    /// Test hook: everything placed in a ring.
    pub fn demo(g: &Game) -> Self {
        let mut s = Self::new(g);
        s.auto_place();
        s
    }

    /// Test hook: the build tab with a cannon on the cursor.
    pub fn demo_build(g: &Game) -> Self {
        let mut s = Self::new(g);
        s.tab = Tab::Build;
        s.carry = Some(Carry::New(BuildKind::Cannon));
        s
    }

    /// Test hook: a placed home base already fighting a wave.
    pub fn demo_battle(g: &Game, wave: usize) -> BattleState {
        let s = Self::demo(g);
        let defenders: Vec<_> = s
            .picked
            .iter()
            .zip(&s.spots)
            .map(|(&i, &p)| (g.troops[i].clone(), p))
            .collect();
        let base = g.profile.base.clone();
        BattleState::start(
            Mode::Defend(Some(wave)),
            Box::new(move || Battle::defense(&base, defenders.clone(), Some(&WAVES[wave]), vec![])),
        )
    }

    fn auto_place(&mut self) {
        let all = home_spots(self.picked.len());
        let n = self.spots.len();
        self.spots.extend_from_slice(&all[n..]);
    }
}

fn army_positions(n: usize) -> Vec<(f32, f32)> {
    (0..n)
        .map(|k| {
            (
                RAID_ENTRY.0 + 1.4 * k as f32 - 1.0,
                RAID_ENTRY.1 - 1.0 + 0.6 * k as f32,
            )
        })
        .collect()
}

impl Game {
    pub fn home(&mut self, mut s: HomeState) -> Screen {
        if self.header(&format!("{HOME_NAME}: your base"), true) {
            if let Some(Carry::Moving(p)) = s.carry.take() {
                self.profile.base.buildings.push(p);
            }
            self.save_all();
            return Screen::Barracks;
        }
        let v = View::home(Rect::new(20.0, 66.0, 714.0, 714.0));
        draw_ground(v, false, true, &s.preview);
        v.disc(ARENA / 2.0, ARENA / 2.0, HOME_RADIUS, Color::new(0.3, 0.5, 1.0, 0.14));
        v.ring(ARENA / 2.0, ARENA / 2.0, HOME_RADIUS, 2.0, Color::new(0.6, 0.8, 1.0, 0.8));
        self.fx.clear();
        draw_world(v, &s.preview, &self.fx, &|_, _| unreachable!());
        // The entry can sit past the edge of the zoomed-in map: keep the
        // marker and its label inside the map panel.
        let e = v.p(RAID_ENTRY.0, RAID_ENTRY.1);
        let (m, rad) = (34.0, 26.0);
        let c = vec2(e.x.clamp(v.r.x + m, v.r.x + v.r.w - m), e.y.clamp(v.r.y + m, v.r.y + v.r.h - m));
        draw_circle_lines(c.x, c.y, rad, 3.0, BAD);
        let label = if c.distance(e) > 1.0 { "raiders come from this corner" } else { "raiders enter here" };
        let lw = measure_text(label, None, 16, 1.0).width;
        let lx = (c.x - lw / 2.0).clamp(v.r.x + 6.0, v.r.x + v.r.w - lw - 6.0);
        draw_rectangle(lx - 4.0, c.y - rad - 24.0, lw + 8.0, 22.0, Color::new(0.05, 0.05, 0.08, 0.75));
        text(label, lx, c.y - rad - 8.0, 16.0, BAD);

        let rx = 754.0;
        let rw = W - rx - 20.0;
        let tw = rw / 2.0 - 5.0;
        if self.ui.button_ex(
            Rect::new(rx, 66.0, tw, 40.0),
            "Defend",
            true,
            s.tab == Tab::Defend,
        ) {
            if let Some(Carry::Moving(p)) = s.carry.take() {
                self.profile.base.buildings.push(p);
                s.preview = build_home_base(&self.profile.base);
            }
            s.carry = None;
            s.tab = Tab::Defend;
        }
        if self.ui.button_ex(
            Rect::new(rx + rw / 2.0 + 5.0, 66.0, tw, 40.0),
            "Build",
            true,
            s.tab == Tab::Build,
        ) {
            s.tab = Tab::Build;
            s.spots.clear();
        }
        match s.tab {
            Tab::Defend => self.home_defend(v, s),
            Tab::Build => self.home_build(v, s),
        }
    }

    fn home_build(&mut self, v: View, mut s: HomeState) -> Screen {
        let rx = 754.0;
        let rw = W - rx - 20.0;
        let mut changed = false;

        // ---- Map: carry, place or pick up buildings
        if v.r.contains(self.ui.mouse) {
            let (wx, wy) = v.to_world(self.ui.mouse);
            let (ox, oy) = (wx - ARENA / 2.0, wy - ARENA / 2.0);
            match s.carry {
                Some(carry) => {
                    let kind = match carry {
                        Carry::New(k) => k,
                        Carry::Moving(p) => p.kind,
                    };
                    let ok = self.profile.base.fits(ox, oy);
                    draw_building_ghost(v, kind, wx, wy, ok);
                    // Dragging works too: letting go away from where it was
                    // picked up sets it down.
                    let dropped = match carry {
                        Carry::Moving(p) => self.ui.released && ok && ((p.x - ox).powi(2) + (p.y - oy).powi(2)).sqrt() > 0.6,
                        Carry::New(_) => false,
                    };
                    if self.ui.clicked || dropped {
                        if !ok {
                            self.toast(
                                "Buildings go inside the blue ring, clear of other buildings",
                            );
                        } else {
                            if let Carry::New(k) = carry {
                                self.profile.gold -= k.cost();
                            }
                            self.profile
                                .base
                                .buildings
                                .push(Placed { kind, x: ox, y: oy });
                            changed = true;
                            s.carry = None;
                        }
                    }
                    if self.ui.right_clicked {
                        if let Carry::Moving(p) = carry {
                            self.profile.base.buildings.push(p);
                            changed = true;
                        }
                        s.carry = None;
                    }
                }
                None => {
                    if let Some(k) = self.profile.base.at(ox, oy) {
                        let b = self.profile.base.buildings[k];
                        v.ring(ARENA / 2.0 + b.x, ARENA / 2.0 + b.y, 1.3, 2.5, ACCENT);
                        if self.ui.clicked {
                            let p = self.profile.base.buildings.remove(k);
                            s.carry = Some(Carry::Moving(p));
                            changed = true;
                        }
                    }
                }
            }
        }

        // ---- Town Hall
        let th = self.profile.base.town_hall;
        let r = Rect::new(rx, 122.0, rw, 132.0);
        panel(r);
        text(
            &format!("Town Hall level {th}"),
            r.x + 14.0,
            r.y + 30.0,
            24.0,
            ACCENT,
        );
        text(
            &format!(
                "Troops up to level {}.  Towers {}, cannons {}.",
                max_troop_level(th),
                BuildKind::Tower.max_at(th),
                BuildKind::Cannon.max_at(th)
            ),
            r.x + 14.0,
            r.y + 56.0,
            16.0,
            MUTED,
        );
        text(
            "Raiders who destroy it earn a star.",
            r.x + 14.0,
            r.y + 76.0,
            16.0,
            MUTED,
        );
        if th < MAX_TOWN_HALL {
            let cost = town_hall_cost(th);
            let afford = self.profile.gold >= cost;
            let label = format!("Upgrade to level {} ({cost}g)", th + 1);
            if self.ui.button_ex(
                Rect::new(r.x + 14.0, r.y + 86.0, rw - 28.0, 36.0),
                &label,
                afford,
                false,
            ) {
                self.profile.gold -= cost;
                self.profile.base.town_hall += 1;
                changed = true;
                self.toast(format!(
                    "Town Hall {}: more defences and higher troop levels",
                    th + 1
                ));
            }
        } else {
            text("Max level", r.x + 14.0, r.y + 110.0, 18.0, GOOD);
        }

        // ---- Shop
        text("Defences", rx, 290.0, 22.0, TEXT);
        let mut y = 304.0;
        let blurb = |k: BuildKind| match k {
            BuildKind::Tower => "Long range, light arrows",
            BuildKind::Cannon => "Short range, heavy shots",
        };
        let mut buy = None;
        for k in BuildKind::ALL {
            let r = Rect::new(rx, y, rw, 70.0);
            let carrying = s.carry == Some(Carry::New(k));
            draw_rectangle(r.x, r.y, r.w, r.h, if carrying { PANEL_HI } else { PANEL });
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                if carrying { 2.0 } else { 1.0 },
                if carrying { ACCENT } else { LINE },
            );
            crate::render::draw_building_icon(vec2(r.x + 34.0, r.y + 58.0), 26.0, k);
            let base = &self.profile.base;
            let (n, max) = (base.count(k), k.max_at(th));
            text(k.name(), r.x + 70.0, r.y + 28.0, 20.0, TEXT);
            text(blurb(k), r.x + 70.0, r.y + 52.0, 15.0, MUTED);
            text(
                &format!("{n}/{max}"),
                r.x + r.w - 170.0,
                r.y + 28.0,
                18.0,
                if n < max { TEXT } else { MUTED },
            );
            let ok = base.can_add(k) && self.profile.gold >= k.cost() && s.carry.is_none();
            if self.ui.button_ex(
                Rect::new(r.x + r.w - 110.0, r.y + 14.0, 96.0, 40.0),
                &format!("{}g", k.cost()),
                ok,
                carrying,
            ) {
                buy = Some(k);
            }
            y += 80.0;
        }
        if let Some(k) = buy {
            s.carry = Some(Carry::New(k));
        }
        let hint = match s.carry {
            Some(Carry::New(_)) => "Click the map to build it. Right-click to cancel.",
            Some(Carry::Moving(_)) => "Click to set it down. Right-click to put it back.",
            None => "Click or drag a tower or cannon on the map to move it.",
        };
        text(hint, rx, y + 14.0, 16.0, MUTED);
        text("The Town Hall always stays in the middle.", rx, y + 36.0, 16.0, MUTED);
        if BuildKind::Cannon.max_at(th) == 0 {
            text("Cannons unlock at Town Hall 2.", rx, y + 58.0, 16.0, MUTED);
        }

        if changed {
            s.preview = build_home_base(&self.profile.base);
            self.save_all();
        }
        Screen::Home(s)
    }

    fn home_defend(&mut self, v: View, mut s: HomeState) -> Screen {
        s.picked.retain(|&i| i < self.troops.len());
        s.spots.truncate(s.picked.len());
        for (k, &(x, y)) in s.spots.iter().enumerate() {
            let t = &self.troops[s.picked[k]];
            crate::render::draw_standing(v, x, y, &crate::art::Figure::of(&t.spec), Some(&t.name));
        }
        let next = s.spots.len();
        if v.r.contains(self.ui.mouse) && next < s.picked.len() {
            let (wx, wy) = v.to_world(self.ui.mouse);
            let ok = can_place_defender(wx, wy);
            let col = if ok {
                rgb(self.troops[s.picked[next]].spec.color)
            } else {
                Color::new(0.9, 0.2, 0.2, 1.0)
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
                    self.toast("Place defenders inside the blue ring");
                }
            }
        }
        if self.ui.right_clicked {
            s.spots.pop();
        }

        // ---- Right panel
        let rx = 754.0;
        let rw = W - rx - 20.0;
        let defenders: Vec<usize> = (0..self.troops.len())
            .filter(|&i| self.troops[i].spec.role == Role::Defend)
            .collect();
        text(
            &format!("Defenders ({}/{MAX_DEFENDERS})", s.picked.len()),
            rx,
            136.0,
            22.0,
            TEXT,
        );
        if defenders.is_empty() {
            text(
                "No defenders yet. Recruit a Guard, Sentry",
                rx,
                164.0,
                18.0,
                MUTED,
            );
            text("or Keeper and train it first.", rx, 186.0, 18.0, MUTED);
        }
        let y = 148.0;
        let mut toggled = None;
        for (slot, &i) in defenders.iter().enumerate() {
            let t = &self.troops[i];
            let r = Rect::new(
                rx + (slot % 2) as f32 * (rw / 2.0 + 4.0),
                y + (slot / 2) as f32 * 52.0,
                rw / 2.0 - 4.0,
                46.0,
            );
            if r.y > 300.0 {
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
            crate::art::draw_portrait(vec2(r.x + 22.0, r.y + 23.0), 18.0, &t.spec);
            text(&t.name, r.x + 46.0, r.y + 21.0, 19.0, TEXT);
            text(
                &format!("Lv{} {} {:.0}%", t.level, t.spec.name, t.skills.win * 100.0),
                r.x + 46.0,
                r.y + 39.0,
                14.0,
                MUTED,
            );
            if self.ui.hover(r) && self.ui.clicked {
                toggled = Some(i);
            }
        }
        if let Some(i) = toggled {
            if let Some(pos) = s.picked.iter().position(|&x| x == i) {
                s.picked.remove(pos);
                s.spots.clear();
            } else if s.picked.len() < MAX_DEFENDERS {
                s.picked.push(i);
            } else {
                self.toast(format!("At most {MAX_DEFENDERS} defenders"));
            }
        }
        let y = 330.0;
        if self.ui.button_ex(
            Rect::new(rx, y, rw / 2.0 - 5.0, 40.0),
            "Auto-place",
            next < s.picked.len(),
            false,
        ) {
            s.auto_place();
        }
        if self.ui.button_ex(
            Rect::new(rx + rw / 2.0 + 5.0, y, rw / 2.0 - 5.0, 40.0),
            "Clear",
            !s.spots.is_empty(),
            false,
        ) {
            s.spots.clear();
        }

        text("Who attacks?", rx, 408.0, 22.0, TEXT);
        let y = 420.0;
        for (k, w) in WAVES.iter().enumerate() {
            let r = Rect::new(
                rx + (k % 2) as f32 * (rw / 2.0 + 4.0),
                y + (k / 2) as f32 * 58.0,
                rw / 2.0 - 4.0,
                52.0,
            );
            let sel = s.opp == Opponent::Wave(k);
            draw_rectangle(r.x, r.y, r.w, r.h, if sel { PANEL_HI } else { PANEL });
            draw_rectangle_lines(
                r.x,
                r.y,
                r.w,
                r.h,
                if sel { 2.0 } else { 1.0 },
                if sel { ACCENT } else { LINE },
            );
            text(w.name, r.x + 12.0, r.y + 22.0, 20.0, TEXT);
            let held = self.profile.waves_held[k];
            text(
                &format!(
                    "{} raiders x{:.1}  {}",
                    w.raiders,
                    w.strength,
                    if held { "held" } else { "" }
                ),
                r.x + 12.0,
                r.y + 42.0,
                15.0,
                if held { GOOD } else { MUTED },
            );
            text(
                &format!("{}g", w.reward),
                r.x + r.w - 48.0,
                r.y + 22.0,
                16.0,
                ACCENT,
            );
            if self.ui.hover(r) && self.ui.clicked {
                s.opp = Opponent::Wave(k);
            }
        }
        let army: Vec<usize> = (0..self.troops.len())
            .filter(|&i| self.troops[i].spec.role == Role::Attack)
            .take(MAX_ARMY)
            .collect();
        let ar = Rect::new(rx, y + 120.0, rw, 52.0);
        let sel = s.opp == Opponent::Army;
        if self.ui.button_ex(
            ar,
            &format!("Your own army ({} attackers): a self-play test", army.len()),
            !army.is_empty(),
            sel,
        ) {
            s.opp = Opponent::Army;
        }
        text(
            "A wave is held when the raiders leave with no stars.",
            rx,
            y + 196.0,
            15.0,
            MUTED,
        );
        text(
            "To move towers and cannons, open Build and click one on the map.",
            rx,
            y + 218.0,
            15.0,
            MUTED,
        );

        let ready = !s.picked.is_empty() && s.spots.len() == s.picked.len();
        if self
            .ui
            .button_ex(Rect::new(rx, H - 76.0, rw, 52.0), "DEFEND!", ready, false)
        {
            let defenders: Vec<_> = s
                .picked
                .iter()
                .zip(&s.spots)
                .map(|(&i, &p)| (self.troops[i].clone(), p))
                .collect();
            let base: HomeBase = self.profile.base.clone();
            self.fx.clear();
            let (mode, remake): (Mode, Box<dyn Fn() -> Battle>) = match s.opp {
                Opponent::Wave(k) => (
                    Mode::Defend(Some(k)),
                    Box::new(move || {
                        Battle::defense(&base, defenders.clone(), Some(&WAVES[k]), vec![])
                    }),
                ),
                Opponent::Army => {
                    let attackers: Vec<_> = army
                        .iter()
                        .zip(army_positions(army.len()))
                        .map(|(&i, p)| (self.troops[i].clone(), p))
                        .collect();
                    (
                        Mode::Defend(None),
                        Box::new(move || {
                            Battle::defense(&base, defenders.clone(), None, attackers.clone())
                        }),
                    )
                }
            };
            return Screen::Battle(BattleState::start(mode, remake));
        }
        if !ready && !s.picked.is_empty() {
            text(
                "Click inside the blue ring to place each defender.",
                rx,
                H - 88.0,
                16.0,
                MUTED,
            );
        }
        Screen::Home(s)
    }
}
