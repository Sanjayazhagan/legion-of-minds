//! Become a God: desktop app. Recruit troops, train their tiny brains on your
//! own CPU, then send them against AI-built bases.

mod armoury;
mod art;
mod plan;
mod record;
mod barracks;
mod battle_screen;
mod home;
mod recruit;
mod render;
mod scene;
mod teamtrain;
mod save;
mod tournament;
mod training;
mod ui;

use godcore::rng::Rng;
use godcore::troop::Troop;
use macroquad::prelude::*;
use render::{Effects, Look};
use save::{Profile, Store};
use ui::*;

pub enum Screen {
    Barracks,
    Recruit(recruit::RecruitState),
    Train(training::TrainState),
    BattleSetup(battle_screen::SetupState),
    Deploy(battle_screen::DeployState),
    Battle(battle_screen::BattleState),
    Tournament(tournament::TourState),
    Home(home::HomeState),
    Replay(tournament::ReplayState),
    Armoury(armoury::ArmouryState),
    Team(teamtrain::TeamState),
}

pub struct Game {
    pub store: Store,
    pub profile: Profile,
    pub troops: Vec<Troop>,
    pub selected: Option<usize>,
    pub ui: Ui,
    pub fx: Effects,
    pub rng: Rng,
    pub toast: Option<(String, f32)>,
    pub roster_scroll: f32,
    pub confirm_release: bool,
}

impl Game {
    pub fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), 3.0));
    }

    pub fn save_all(&self) {
        self.store.save_profile(&self.profile);
        for t in &self.troops {
            self.store.save_troop(t);
        }
    }

    /// How a troop is drawn: its class colour and initial.
    pub fn look_for(t: &Troop, show_name: bool) -> Look {
        Look {
            fig: art::Figure::of(&t.spec),
            name: show_name.then(|| t.name.clone()),
            hostile: false,
        }
    }

    /// Header bar shared by every screen. Returns true if "back" was clicked.
    pub fn header(&mut self, title: &str, back: bool) -> bool {
        draw_rectangle(0.0, 0.0, W, 52.0, Color::new(0.1, 0.11, 0.155, 1.0));
        draw_line(0.0, 52.0, W, 52.0, 1.0, LINE);
        text("BECOME A GOD", 20.0, 34.0, 28.0, ACCENT);
        text(title, 230.0, 34.0, 24.0, TEXT);
        let g = format!("Gold {}", self.profile.gold);
        text(&g, W - 20.0 - text_width(&g, 24.0), 34.0, 24.0, ACCENT);
        back && self
            .ui
            .button(Rect::new(W - 300.0, 10.0, 120.0, 32.0), "< Back")
    }

    fn draw_toast(&mut self) {
        if let Some((msg, t)) = &mut self.toast {
            *t -= crate::record::dt();
            let w = text_width(msg, 20.0) + 40.0;
            let r = Rect::new((W - w) / 2.0, H - 70.0, w, 40.0);
            draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.05, 0.05, 0.08, 0.92));
            draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ACCENT);
            text(msg, r.x + 20.0, r.y + 26.0, 20.0, TEXT);
            if *t <= 0.0 {
                self.toast = None;
            }
        }
    }
}

/// Test hook: BAG_SCREEN opens a screen directly (used for headless screenshots).
fn demo_screen(g: &mut Game) -> Option<Screen> {
    let which = std::env::var("BAG_SCREEN").ok()?;
    let all: Vec<usize> = (0..g.troops.len().min(5)).collect();
    Some(match which.as_str() {
        "recruit" => Screen::Recruit(recruit::RecruitState::new(g.fresh_name())),
        "train" => Screen::Train(training::TrainState::demo(0, &g.troops[0])),
        "setup" => Screen::BattleSetup(battle_screen::SetupState::new(g)),
        "deploy" => Screen::Deploy(battle_screen::DeployState::demo(g, 1, all)),
        "battle" => Screen::Battle(battle_screen::BattleState::demo(g, 1, all)),
        "tournament" => Screen::Tournament(tournament::TourState::demo(g)),
        "home" => Screen::Home(home::HomeState::demo(g)),
        "defend" => Screen::Battle(home::HomeState::demo_battle(g, 1)),
        "build" => Screen::Home(home::HomeState::demo_build(g)),
        "armoury" => Screen::Armoury(armoury::ArmouryState { idx: 0 }),
        "team" => Screen::Team(teamtrain::TeamState::demo(g)),
        _ => return None,
    })
}

fn window_conf() -> macroquad::conf::Conf {
    macroquad::conf::Conf {
        miniquad_conf: Conf {
            window_title: "Become a God".to_owned(),
            window_width: 1280,
            window_height: 800,
            high_dpi: true,
            window_resizable: true,
            sample_count: 4,
            ..Default::default()
        },
        // 3D characters are sent as one mesh per body part.
        draw_call_vertex_capacity: 60_000,
        draw_call_index_capacity: 120_000,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    // Leave one core for the window while troops train.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(cores.saturating_sub(1).max(1))
        .build_global();

    scene::load_library();
    record::start();
    let store = Store::open();
    let profile = store.load_profile();
    let troops = store.load_troops();
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let mut game = Game {
        selected: if troops.is_empty() { None } else { Some(0) },
        store,
        profile,
        troops,
        ui: Ui::new(),
        fx: Effects::new(),
        rng: Rng::new(seed),
        toast: None,
        roster_scroll: 0.0,
        confirm_release: false,
    };
    let mut screen = demo_screen(&mut game).unwrap_or(Screen::Barracks);
    let autoshot = std::env::var("BAG_SCREENSHOT").ok();
    let shot_frame: u32 = std::env::var("BAG_SHOT_FRAME")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let mut frame = 0u32;
    loop {
        game.ui.begin();
        if std::env::var("BAG_SCREEN").as_deref() == Ok("gallery") {
            render::draw_gallery();
            frame += 1;
            if let Some(p) = &autoshot {
                if frame == shot_frame {
                    get_screen_data().export_png(p);
                    break;
                }
            }
            next_frame().await;
            continue;
        }
        screen = match screen {
            Screen::Barracks => game.barracks(),
            Screen::Recruit(s) => game.recruit(s),
            Screen::Train(s) => game.train(s),
            Screen::BattleSetup(s) => game.battle_setup(s),
            Screen::Deploy(s) => game.deploy(s),
            Screen::Battle(s) => game.battle(s),
            Screen::Tournament(s) => game.tournament(s),
            Screen::Replay(s) => game.replay(s),
            Screen::Home(s) => game.home(s),
            Screen::Armoury(s) => game.armoury(s),
            Screen::Team(s) => game.team_training(s),
        };
        game.draw_toast();
        frame += 1;
        if record::active() {
            if record::end_frame() {
                break;
            }
            next_frame().await;
            continue;
        }
        // Test hook: BAG_SCREENSHOT=path saves one frame and exits.
        if let Some(p) = &autoshot {
            if frame == shot_frame {
                get_screen_data().export_png(p);
                break;
            }
        }
        next_frame().await;
    }
}
