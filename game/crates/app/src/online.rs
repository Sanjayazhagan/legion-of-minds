//! Online play through the server in backend/ (bag-client): the sign-in
//! screen the game opens on, and the Online screen where you publish your
//! base, raid other players and watch your battle log.
//!
//! Every server call runs on a background thread so the window keeps
//! drawing; screens poll a `Task` each frame.

use crate::art::Figure;
use crate::battle_screen::MAX_SQUAD;
use crate::home::MAX_DEFENDERS;
use crate::render::{draw_ground, draw_world, Look, View};
use crate::ui::*;
use crate::{Game, Screen};
use bag_client::bag_protocol::{
    AttackLog, LoginStart, Match, Providers, PublicPlayer, Published, Replay, Upgrades, User,
};
use bag_client::{
    army_from, army_spots, defender_spots, replay_battle, snapshot_from, Client, Uuid,
};
use godcore::battle::Battle;
use godcore::classes::Role;
use godcore::scenario::{max_troop_level, town_hall_cost};
use godcore::sim::{Outcome, Team};
use godcore::troop::{level_up_cost, Troop};
use godcore::weapons::Weapon;
use macroquad::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the server is. `BAG_SERVER` when the game runs, else `BAG_SERVER`
/// when it was built (set it for release builds), else a server on this
/// machine.
pub fn server_url() -> String {
    std::env::var("BAG_SERVER")
        .ok()
        .or_else(|| option_env!("BAG_SERVER").map(String::from))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://localhost:8080".into())
        .trim_end_matches('/')
        .to_string()
}

/// The signed-in session, kept in `account.json` next to the save so the
/// player stays signed in between runs. A token only works on the server
/// that issued it.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub token: Option<String>,
    #[serde(skip)]
    pub user: Option<User>,
}

impl Account {
    pub fn load(dir: &std::path::Path) -> Account {
        let server = server_url();
        let mut a: Account = std::fs::read_to_string(dir.join("account.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if a.server != server {
            a = Account {
                server,
                ..Default::default()
            };
        }
        a
    }

    pub fn save(&self, dir: &std::path::Path) {
        let p = dir.join("account.json");
        let _ = std::fs::write(&p, serde_json::to_string_pretty(self).unwrap());
        // The token is a password for this account: only this user may read it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
    }

    pub fn signed_in(&self) -> bool {
        self.token.is_some() && self.user.is_some()
    }

    fn client(&self) -> Client {
        let c = Client::new(&self.server);
        match &self.token {
            Some(t) => c.with_token(t.clone()),
            None => c,
        }
    }
}

/// A server call running on its own thread.
pub struct Task<T>(Receiver<Result<T, String>>);

impl<T: Send + 'static> Task<T> {
    fn spawn(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        Task(rx)
    }

    /// The answer, once it has arrived.
    fn poll(&self) -> Option<Result<T, String>> {
        match self.0.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("something went wrong talking to the server".into()))
            }
        }
    }
}

/// What a failed call shows when the server no longer knows the session.
const SIGNED_OUT: &str = "Your session ran out: sign in again";

fn err(e: bag_client::Error) -> String {
    match e {
        bag_client::Error::Api { status: 401, .. } => SIGNED_OUT.into(),
        e => e.to_string(),
    }
}

fn unauthorized(e: &str) -> bool {
    e == SIGNED_OUT
}

/// True when the server address points at this computer (no hosted
/// server set), so a failed connection means none is running here.
fn is_this_computer(server: &str) -> bool {
    let host = server.split("://").nth(1).unwrap_or(server);
    let host = host.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0") || server.contains("[::1]")
}

/// Open a page in the player's browser.
fn open_browser(url: &str) -> bool {
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

// ------------------------------------------------------------ sign-in

enum Phase {
    /// Checking a saved session.
    Checking(Task<User>),
    /// Showing the sign-in buttons.
    Choose,
    /// Asking the server for a browser sign-in link.
    Starting(String, Task<LoginStart>),
    /// The browser is open; waiting for the player to finish there.
    Browser {
        provider: String,
        url: String,
        cancel: Arc<AtomicBool>,
        task: Task<(String, User)>,
    },
    /// Test login in flight.
    Dev(Task<(String, User)>),
    /// The server can't be reached: telling the player, then going offline
    /// by itself at `at` (seconds, `record::now`).
    GoingOffline { at: f64 },
}

/// How long the "server is down" alert stays up before the game goes
/// offline by itself.
const OFFLINE_AFTER_SECS: f64 = 4.0;

pub struct SignInState {
    phase: Phase,
    providers: Option<Result<Providers, String>>,
    providers_task: Option<Task<Providers>>,
    name: String,
    error: Option<String>,
    /// Where to go once signed in: the Online screen when the player came
    /// from there, else the barracks.
    then_online: bool,
}

impl SignInState {
    /// The screen the game opens on. A saved session is checked first.
    pub fn startup(a: &Account) -> Self {
        let mut s = Self::choose(a, false);
        if a.token.is_some() {
            let c = a.client();
            s.phase = Phase::Checking(Task::spawn(move || c.me().map_err(err)));
        }
        s
    }

    /// Asked for from inside the game (the Online button while signed out).
    pub fn from_game(a: &Account) -> Self {
        Self::choose(a, true)
    }

    fn choose(a: &Account, then_online: bool) -> Self {
        let c = a.client();
        SignInState {
            phase: Phase::Choose,
            providers: None,
            providers_task: Some(Task::spawn(move || c.providers().map_err(err))),
            name: String::new(),
            error: None,
            then_online,
        }
    }
}

fn going_offline() -> Phase {
    Phase::GoingOffline {
        at: crate::record::now() + OFFLINE_AFTER_SECS,
    }
}

fn provider_label(p: &str) -> String {
    match p {
        "google" => "Sign in with Google".into(),
        "discord" => "Sign in with Discord".into(),
        other => format!("Sign in with {other}"),
    }
}

/// Run the browser sign-in: poll until the player finishes, gives up, or
/// the login expires.
fn browser_login(
    mut c: Client,
    l: LoginStart,
    cancel: Arc<AtomicBool>,
) -> Result<(String, User), String> {
    let until = Instant::now() + Duration::from_secs(l.expires_in_secs.max(60));
    while Instant::now() < until {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if let Some(u) = c.poll_login(&l).map_err(err)? {
            return Ok((c.token().unwrap_or_default().to_string(), u));
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
    Err("the sign-in took too long; try again".into())
}

impl Game {
    /// Forget the session here and end it on the server (in the background).
    pub fn sign_out(&mut self) {
        let mut c = self.account.client();
        std::thread::spawn(move || {
            let _ = c.logout();
        });
        self.account.token = None;
        self.account.user = None;
        self.account.save(&self.store.home);
    }

    fn signed_in(&mut self, token: String, user: User) {
        self.account.token = Some(token);
        self.account.user = Some(user);
        self.account.save(&self.store.home);
    }

    pub fn sign_in(&mut self, mut s: SignInState) -> Screen {
        draw_rectangle(0.0, 0.0, W, H, BG);
        text_centered("LEGION OF MINDS", W / 2.0, 170.0, 64.0, ACCENT);
        text_centered(
            "Train tiny minds. Raid other players' bases.",
            W / 2.0,
            212.0,
            24.0,
            MUTED,
        );
        let box_r = Rect::new(W / 2.0 - 240.0, 250.0, 480.0, 470.0);
        panel(box_r);
        let x = box_r.x + 40.0;
        let bw = box_r.w - 80.0;
        let mut y = box_r.y + 40.0;

        if let Some(t) = &s.providers_task {
            if let Some(r) = t.poll() {
                s.providers = Some(r);
                s.providers_task = None;
            }
        }
        if matches!(s.providers, Some(Err(_))) && matches!(s.phase, Phase::Choose) {
            // Down, or not deployed yet: say so, then play offline.
            s.phase = going_offline();
        }

        match &s.phase {
            Phase::Checking(task) => {
                if let Some(r) = task.poll() {
                    match r {
                        Ok(u) => {
                            self.account.user = Some(u.clone());
                            self.toast(format!("Signed in as {}", u.display_name));
                            return self.after_sign_in(&s);
                        }
                        Err(e) => {
                            if unauthorized(&e) {
                                // The saved session ran out: sign in again.
                                self.account.token = None;
                                self.account.save(&self.store.home);
                                s.phase = Phase::Choose;
                            } else {
                                // Server down: keep the saved session for
                                // next time and play offline.
                                s.phase = going_offline();
                            }
                        }
                    }
                } else {
                    text_centered("Signing in...", W / 2.0, y + 40.0, 26.0, TEXT);
                    if self.ui.button(
                        Rect::new(x, box_r.y + box_r.h - 84.0, bw, 48.0),
                        "Play offline",
                    ) {
                        return Screen::Barracks;
                    }
                    return Screen::SignIn(s);
                }
            }
            Phase::Browser {
                provider,
                url,
                cancel,
                task,
            } => {
                if let Some(r) = task.poll() {
                    match r {
                        Ok((token, u)) => {
                            self.toast(format!("Signed in as {}", u.display_name));
                            self.signed_in(token, u);
                            return self.after_sign_in(&s);
                        }
                        Err(e) => {
                            if e != "cancelled" {
                                s.error = Some(e);
                            }
                            s.phase = Phase::Choose;
                            return Screen::SignIn(s);
                        }
                    }
                }
                let name = provider_label(provider).replace("Sign in with ", "");
                text_centered(
                    &format!("Finish signing in with {name}"),
                    W / 2.0,
                    y + 20.0,
                    26.0,
                    TEXT,
                );
                text_centered(
                    "in the browser window that just opened.",
                    W / 2.0,
                    y + 50.0,
                    22.0,
                    MUTED,
                );
                text_centered(
                    "The game carries on by itself when you're done.",
                    W / 2.0,
                    y + 78.0,
                    20.0,
                    MUTED,
                );
                y += 120.0;
                if self
                    .ui
                    .button(Rect::new(x, y, bw, 44.0), "Open the browser again")
                {
                    open_browser(url);
                }
                y += 56.0;
                if self
                    .ui
                    .button(Rect::new(x, y, bw, 44.0), "Copy the sign-in link")
                {
                    macroquad::miniquad::window::clipboard_set(url);
                    self.toast("Link copied: paste it into your browser");
                }
                y += 56.0;
                if self.ui.button(Rect::new(x, y, bw, 44.0), "Cancel") {
                    cancel.store(true, Ordering::Relaxed);
                    s.phase = Phase::Choose;
                }
                return Screen::SignIn(s);
            }
            Phase::Dev(task) => {
                if let Some(r) = task.poll() {
                    match r {
                        Ok((token, u)) => {
                            self.toast(format!("Signed in as {}", u.display_name));
                            self.signed_in(token, u);
                            return self.after_sign_in(&s);
                        }
                        Err(e) => {
                            s.error = Some(e);
                            s.phase = Phase::Choose;
                        }
                    }
                } else {
                    text_centered("Signing in...", W / 2.0, y + 40.0, 26.0, TEXT);
                    return Screen::SignIn(s);
                }
            }
            Phase::Starting(prov, task) => {
                if let Some(r) = task.poll() {
                    match r {
                        Ok(l) => {
                            if !open_browser(&l.url) {
                                self.toast("Couldn't open the browser: use Copy the sign-in link");
                            }
                            let cancel = Arc::new(AtomicBool::new(false));
                            let (c, cl, cc) = (self.account.client(), l.clone(), cancel.clone());
                            s.phase = Phase::Browser {
                                provider: prov.clone(),
                                url: l.url,
                                cancel,
                                task: Task::spawn(move || browser_login(c, cl, cc)),
                            };
                        }
                        Err(e) => {
                            s.error = Some(e);
                            s.phase = Phase::Choose;
                        }
                    }
                    return Screen::SignIn(s);
                }
                text_centered("Opening your browser...", W / 2.0, y + 40.0, 26.0, TEXT);
                return Screen::SignIn(s);
            }
            Phase::GoingOffline { at } => {
                let left = at - crate::record::now();
                if left <= 0.0 {
                    return self.go_offline();
                }
                let (head, hint) = if is_this_computer(&self.account.server) {
                    (
                        "No game server is running on this computer.",
                        "Start one (see backend/README.md) to play online.",
                    )
                } else {
                    (
                        "Can't reach the game server.",
                        "It may be down, or your internet may be off.",
                    )
                };
                text_centered(head, W / 2.0, y + 30.0, 24.0, BAD);
                text_centered(hint, W / 2.0, y + 60.0, 18.0, MUTED);
                text_centered(
                    &format!("Switching to offline play in {}...", left.ceil() as i32),
                    W / 2.0,
                    y + 120.0,
                    22.0,
                    TEXT,
                );
                text_centered(
                    "Your troops and base are saved on this computer.",
                    W / 2.0,
                    y + 150.0,
                    16.0,
                    MUTED,
                );
                y += 200.0;
                if self.ui.button(Rect::new(x, y, bw, 44.0), "Try again") {
                    let then_online = s.then_online;
                    let mut s = SignInState::startup(&self.account);
                    s.then_online = then_online;
                    return Screen::SignIn(s);
                }
                if self.ui.button(
                    Rect::new(x, box_r.y + box_r.h - 84.0, bw, 48.0),
                    "Play offline now",
                ) {
                    return self.go_offline();
                }
                return Screen::SignIn(s);
            }
            Phase::Choose => {}
        }

        // The sign-in choices.
        text_centered("Sign in to play online", W / 2.0, y + 6.0, 26.0, TEXT);
        y += 30.0;
        match &s.providers {
            None => {
                text_centered(
                    "Connecting to the server...",
                    W / 2.0,
                    y + 30.0,
                    20.0,
                    MUTED,
                );
                y += 60.0;
            }
            // A failed check turns into GoingOffline above.
            Some(Err(_)) => {}
            Some(Ok(p)) => {
                y += 16.0;
                if p.oauth.is_empty() && !p.dev {
                    text_centered(
                        "This server has no sign-in turned on yet.",
                        W / 2.0,
                        y + 20.0,
                        20.0,
                        MUTED,
                    );
                    y += 40.0;
                }
                for prov in p.oauth.clone() {
                    if self
                        .ui
                        .button(Rect::new(x, y, bw, 48.0), &provider_label(&prov))
                    {
                        s.error = None;
                        let c = self.account.client();
                        let p = prov.clone();
                        s.phase = Phase::Starting(
                            prov,
                            Task::spawn(move || c.start_login(&p).map_err(err)),
                        );
                        return Screen::SignIn(s);
                    }
                    y += 60.0;
                }
                if p.dev {
                    // Test login: for a server on your own machine.
                    for c in crate::record::chars() {
                        if !c.is_control() && s.name.chars().count() < 24 {
                            s.name.push(c);
                        }
                    }
                    if crate::record::backspace() {
                        s.name.pop();
                    }
                    text("Test login (local server)", x, y + 18.0, 18.0, MUTED);
                    y += 26.0;
                    let field = Rect::new(x, y, bw - 130.0, 44.0);
                    draw_rectangle(
                        field.x,
                        field.y,
                        field.w,
                        field.h,
                        Color::new(0.08, 0.09, 0.12, 1.0),
                    );
                    draw_rectangle_lines(field.x, field.y, field.w, field.h, 2.0, LINE);
                    let shown = if s.name.is_empty() {
                        "type a name"
                    } else {
                        s.name.as_str()
                    };
                    let col = if s.name.is_empty() { MUTED } else { TEXT };
                    text(shown, field.x + 12.0, field.y + 29.0, 22.0, col);
                    let go = self.ui.button_ex(
                        Rect::new(x + bw - 120.0, y, 120.0, 44.0),
                        "Sign in",
                        !s.name.trim().is_empty(),
                        false,
                    ) || (is_key_pressed(KeyCode::Enter) && !s.name.trim().is_empty());
                    if go {
                        let mut c = self.account.client();
                        let name = s.name.trim().to_string();
                        s.error = None;
                        s.phase = Phase::Dev(Task::spawn(move || {
                            let u = c.dev_login(&name).map_err(err)?;
                            Ok((c.token().unwrap_or_default().to_string(), u))
                        }));
                        return Screen::SignIn(s);
                    }
                    y += 56.0;
                }
            }
        }
        if let Some(e) = &s.error {
            let short: String = e.chars().take(64).collect();
            text_centered(&short, W / 2.0, y + 10.0, 18.0, BAD);
        }

        if self.ui.button(
            Rect::new(x, box_r.y + box_r.h - 84.0, bw, 48.0),
            "Play offline",
        ) {
            return Screen::Barracks;
        }
        text_centered(
            "Offline: train, battle AI bases and run tournaments. Online raids need an account.",
            W / 2.0,
            box_r.y + box_r.h + 30.0,
            16.0,
            MUTED,
        );
        text(
            &format!("Server: {}", self.account.server),
            20.0,
            H - 8.0,
            14.0,
            MUTED,
        );
        Screen::SignIn(s)
    }

    fn go_offline(&mut self) -> Screen {
        self.toast("Playing offline: online raids come back once the server is up");
        Screen::Barracks
    }

    fn after_sign_in(&mut self, s: &SignInState) -> Screen {
        if s.then_online {
            Screen::Online(OnlineState::new(self))
        } else {
            Screen::Barracks
        }
    }
}

// ------------------------------------------------------------ online screen

/// What the Online screen is waiting on.
enum Busy {
    Refresh(Task<(User, Vec<AttackLog>, Vec<PublicPlayer>)>),
    Publish(Task<(Published, User)>),
    Find(Task<Match>),
    Raid(Task<(User, Replay)>),
    Replay(Task<Replay>),
    /// An online upgrade, and what to say once it's paid.
    Buy(Task<User>, String),
}

pub struct OnlineState {
    log: Vec<AttackLog>,
    board: Vec<PublicPlayer>,
    found: Option<Match>,
    busy: Option<Busy>,
    msg: Option<(String, Color)>,
}

impl OnlineState {
    /// Test hook (BAG_SCREEN=online): the Online screen with the saved
    /// session, loaded before the first frame.
    pub fn demo(g: &mut Game) -> Screen {
        g.account.user = g.account.client().me().ok();
        Screen::Online(OnlineState::new(g))
    }

    pub fn new(g: &Game) -> Self {
        let mut s = OnlineState {
            log: Vec::new(),
            board: Vec::new(),
            found: None,
            busy: None,
            msg: None,
        };
        s.refresh(g);
        s
    }

    fn refresh(&mut self, g: &Game) {
        let c = g.account.client();
        self.busy = Some(Busy::Refresh(Task::spawn(move || {
            Ok((
                c.me().map_err(err)?,
                c.log("all").map_err(err)?,
                c.leaderboard(10).map_err(err)?,
            ))
        })));
    }
}

/// The defenders a published base gets: the first defenders in the roster
/// that this Town Hall allows, as on the Home Base screen, and that are
/// paid for online.
fn base_defenders(g: &Game, paid: &Upgrades) -> (Vec<(Troop, (f32, f32))>, usize) {
    let cap = max_troop_level(g.profile.base.town_hall);
    let all: Vec<&Troop> = g
        .troops
        .iter()
        .filter(|t| t.spec.role == Role::Defend)
        .collect();
    let ok: Vec<Troop> = all
        .iter()
        .filter(|t| t.level <= cap && paid.blocker(t).is_none())
        .take(MAX_DEFENDERS)
        .map(|t| (*t).clone())
        .collect();
    let skipped = all.len().min(MAX_DEFENDERS).saturating_sub(ok.len());
    let spots = defender_spots(ok.len());
    (ok.into_iter().zip(spots).collect(), skipped)
}

/// The army for an online raid: your best attackers that your online
/// Town Hall allows and that are paid for online.
fn raid_army(g: &Game, paid: &Upgrades) -> Vec<(Troop, (f32, f32))> {
    let cap = max_troop_level(paid.town_hall);
    let mut v: Vec<&Troop> = g
        .troops
        .iter()
        .filter(|t| t.spec.role == Role::Attack && t.level <= cap && paid.blocker(t).is_none())
        .collect();
    v.sort_by(|a, b| {
        b.skills
            .win
            .partial_cmp(&a.skills.win)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v.truncate(MAX_SQUAD);
    let spots = army_spots(v.len());
    v.into_iter().cloned().zip(spots).collect()
}

/// Something the player has offline that doesn't count online yet.
struct Need {
    label: String,
    price: i64,
    buy: Buy,
    /// Why it can't be bought yet, short enough for the button.
    blocked: Option<String>,
}

#[derive(Clone, Copy)]
enum Buy {
    TownHall,
    Weapon(Weapon),
    Level(u64, u32),
}

/// What to pay for online so the Town Hall, weapons and troop levels in the
/// save count online too: Town Hall first, then weapons, then levels.
fn online_needs(g: &Game, paid: &Upgrades) -> Vec<Need> {
    let mut v = Vec::new();
    if g.profile.base.town_hall > paid.town_hall {
        v.push(Need {
            label: format!("Town Hall {}", paid.town_hall + 1),
            price: town_hall_cost(paid.town_hall) as i64,
            buy: Buy::TownHall,
            blocked: None,
        });
    }
    let mut weapons: Vec<Weapon> = Vec::new();
    for t in &g.troops {
        let w = t.spec.weapon();
        if !paid.owns_weapon(w, &t.spec) && !weapons.contains(&w) {
            weapons.push(w);
        }
    }
    for w in weapons {
        v.push(Need {
            label: format!("Unlock the {}", w.name()),
            price: w.stats().cost as i64,
            buy: Buy::Weapon(w),
            blocked: None,
        });
    }
    for t in &g.troops {
        let from = paid.level_of(t.id);
        if t.level > from {
            let name: String = t.name.trim().chars().take(12).collect();
            v.push(Need {
                label: format!("{name} to level {}", from + 1),
                price: level_up_cost(from) as i64,
                buy: Buy::Level(t.id, from),
                blocked: (from >= max_troop_level(paid.town_hall)).then(|| "Town Hall".into()),
            });
        }
    }
    v
}

fn stars_text(n: u32) -> String {
    format!(
        "{}{}",
        "*".repeat(n as usize),
        "-".repeat(3 - n.min(3) as usize)
    )
}

impl Game {
    pub fn online(&mut self, mut s: OnlineState) -> Screen {
        if self.header("Online", true) {
            return Screen::Barracks;
        }
        let Some(me) = self.account.user.clone() else {
            return Screen::SignIn(SignInState::from_game(&self.account));
        };

        // Finished server calls.
        if let Some(b) = &s.busy {
            let done = match b {
                Busy::Refresh(t) => t.poll().map(|r| {
                    r.map(|(u, log, board)| {
                        self.account.user = Some(u);
                        s.log = log;
                        s.board = board;
                    })
                }),
                Busy::Publish(t) => t.poll().map(|r| {
                    r.map(|(p, u)| {
                        self.account.user = Some(u);
                        s.msg = Some((
                            format!(
                                "Base published: Town Hall {} with {} defenders",
                                p.town_hall, p.defenders
                            ),
                            GOOD,
                        ));
                    })
                }),
                Busy::Find(t) => t.poll().map(|r| r.map(|m| s.found = Some(m))),
                Busy::Raid(t) => {
                    if let Some(r) = t.poll() {
                        match r {
                            Ok((u, replay)) => {
                                self.account.user = Some(u);
                                s.busy = None;
                                s.found = None;
                                return Screen::OnlineWatch(WatchState::new(replay, me.id, s));
                            }
                            Err(e) => Some(Err(e)),
                        }
                    } else {
                        None
                    }
                }
                Busy::Replay(t) => {
                    if let Some(r) = t.poll() {
                        match r {
                            Ok(replay) => {
                                s.busy = None;
                                return Screen::OnlineWatch(WatchState::new(replay, me.id, s));
                            }
                            Err(e) => Some(Err(e)),
                        }
                    } else {
                        None
                    }
                }
                Busy::Buy(t, what) => t.poll().map(|r| {
                    r.map(|u| {
                        self.account.user = Some(u);
                        s.msg = Some((format!("Paid online: {what}"), GOOD));
                    })
                }),
            };
            match done {
                Some(Ok(())) => s.busy = None,
                Some(Err(e)) => {
                    s.busy = None;
                    if unauthorized(&e) {
                        self.account.token = None;
                        self.account.user = None;
                        self.account.save(&self.store.home);
                        self.toast(SIGNED_OUT);
                        return Screen::SignIn(SignInState::from_game(&self.account));
                    }
                    s.msg = Some((e, BAD));
                }
                None => {}
            }
        }
        let me = self.account.user.clone().unwrap_or(me);
        let idle = s.busy.is_none();

        // Left: you.
        let left = Rect::new(20.0, 66.0, 400.0, H - 86.0);
        panel(left);
        let x = left.x + 20.0;
        text(&me.display_name, x, left.y + 40.0, 30.0, TEXT);
        text(
            &format!("Trophies {}", me.trophies),
            x,
            left.y + 78.0,
            24.0,
            ACCENT,
        );
        text(
            &format!("Online gold {}", me.gold),
            x,
            left.y + 106.0,
            20.0,
            MUTED,
        );
        let base = match (me.has_base, me.town_hall) {
            (true, Some(th)) => format!("Published base: Town Hall {th}"),
            _ => "No base published yet".into(),
        };
        text(&base, x, left.y + 132.0, 20.0, MUTED);
        if let Some(sh) = &me.shield_until {
            text(
                &format!("Shield until {}", short_time(sh)),
                x,
                left.y + 156.0,
                18.0,
                SKY,
            );
        }

        let mut y = left.y + 186.0;
        let paid = me.upgrades.clone();
        let (defenders, skipped) = base_defenders(self, &paid);
        let th_paid = self.profile.base.town_hall <= paid.town_hall;
        text(
            &format!(
                "Your base: Town Hall {}, {} defenders",
                self.profile.base.town_hall,
                defenders.len()
            ),
            x,
            y,
            18.0,
            TEXT,
        );
        if !th_paid {
            text(
                &format!(
                    "Buy Town Hall {} in Online upgrades first",
                    self.profile.base.town_hall
                ),
                x,
                y + 20.0,
                15.0,
                BAD,
            );
        } else if skipped > 0 {
            text(
                &format!("{skipped} defender(s) left out: over the level cap or not paid online"),
                x,
                y + 20.0,
                15.0,
                MUTED,
            );
        }
        y += 34.0;
        if self.ui.button_ex(
            Rect::new(x, y, left.w - 40.0, 46.0),
            "Publish my base",
            idle && th_paid,
            false,
        ) {
            let snap = snapshot_from(&self.profile.base, &defenders);
            let c = self.account.client();
            s.msg = None;
            s.busy = Some(Busy::Publish(Task::spawn(move || {
                let p = c.publish_base(&snap).map_err(err)?;
                Ok((p, c.me().map_err(err)?))
            })));
        }
        text(
            "Others raid this copy while you're away.",
            x,
            y + 66.0,
            16.0,
            MUTED,
        );
        text(
            "Publish again after training or building.",
            x,
            y + 86.0,
            16.0,
            MUTED,
        );

        if self.ui.button_ex(
            Rect::new(x, left.y + left.h - 66.0, left.w - 40.0, 46.0),
            "Sign out",
            idle,
            false,
        ) {
            self.sign_out();
            return Screen::SignIn(SignInState::from_game(&self.account));
        }

        // Middle: raid.
        let mid = Rect::new(440.0, 66.0, 420.0, 300.0);
        panel(mid);
        let mx = mid.x + 20.0;
        text("Raid", mx, mid.y + 34.0, 26.0, TEXT);
        let army = raid_army(self, &paid);
        match &s.found {
            None => {
                text(
                    &format!("Your army: your best {} attackers", army.len()),
                    mx,
                    mid.y + 70.0,
                    18.0,
                    MUTED,
                );
                if self.ui.button_ex(
                    Rect::new(mx, mid.y + 96.0, mid.w - 40.0, 50.0),
                    "Find an opponent",
                    idle && !army.is_empty(),
                    false,
                ) {
                    let c = self.account.client();
                    s.msg = None;
                    s.busy = Some(Busy::Find(Task::spawn(move || c.find_match().map_err(err))));
                }
                if army.is_empty() {
                    let none = !self.troops.iter().any(|t| t.spec.role == Role::Attack);
                    text(
                        if none {
                            "Recruit and train an attacker first."
                        } else {
                            "Your attackers need online upgrades first."
                        },
                        mx,
                        mid.y + 172.0,
                        16.0,
                        MUTED,
                    );
                }
            }
            Some(m) => {
                let d = &m.defender;
                text(&d.display_name, mx, mid.y + 72.0, 24.0, ACCENT);
                text(
                    &format!(
                        "Trophies {}  Town Hall {}  {} defenders",
                        d.trophies,
                        m.snapshot.base.town_hall,
                        m.snapshot.defenders.len()
                    ),
                    mx,
                    mid.y + 98.0,
                    18.0,
                    TEXT,
                );
                text(
                    &format!("Up to {} gold to loot", m.max_loot),
                    mx,
                    mid.y + 122.0,
                    18.0,
                    MUTED,
                );
                let names: Vec<&str> = army.iter().map(|(t, _)| t.name.as_str()).collect();
                text(
                    &format!("Sending: {}", names.join(", ")),
                    mx,
                    mid.y + 150.0,
                    16.0,
                    MUTED,
                );
                if self.ui.button_ex(
                    Rect::new(mx, mid.y + 170.0, mid.w - 40.0, 50.0),
                    "Attack!",
                    idle && !army.is_empty(),
                    false,
                ) {
                    let c = self.account.client();
                    let m = m.clone();
                    let army = army_from(&army);
                    s.msg = None;
                    s.busy = Some(Busy::Raid(Task::spawn(move || {
                        let (_, out) = c.attack(&m, army).map_err(err)?;
                        let replay = c.replay(out.attack_id).map_err(err)?;
                        Ok((out.attacker, replay))
                    })));
                }
                if self.ui.button_ex(
                    Rect::new(mx, mid.y + 230.0, mid.w - 40.0, 40.0),
                    "Find someone else",
                    idle,
                    false,
                ) {
                    let c = self.account.client();
                    s.busy = Some(Busy::Find(Task::spawn(move || c.find_match().map_err(err))));
                }
            }
        }

        // Status line.
        let status = match &s.busy {
            Some(Busy::Refresh(_)) => Some("Loading...".to_string()),
            Some(Busy::Publish(_)) => Some("Publishing your base...".into()),
            Some(Busy::Find(_)) => Some("Looking for an opponent...".into()),
            Some(Busy::Raid(_)) => Some("Raiding: the server is checking the battle...".into()),
            Some(Busy::Replay(_)) => Some("Loading the replay...".into()),
            Some(Busy::Buy(..)) => Some("Paying with online gold...".into()),
            None => None,
        };
        if let Some(t) = status {
            text(&t, 440.0, 392.0, 18.0, SKY);
        } else if let Some((m, c)) = &s.msg {
            let short: String = m.chars().take(70).collect();
            text(&short, 440.0, 392.0, 18.0, *c);
        }

        // Battle log.
        let logr = Rect::new(440.0, 410.0, 420.0, H - 430.0);
        panel(logr);
        text("Battle log", logr.x + 20.0, logr.y + 30.0, 22.0, TEXT);
        if self.ui.button_ex(
            Rect::new(logr.x + logr.w - 110.0, logr.y + 8.0, 96.0, 30.0),
            "Refresh",
            idle,
            false,
        ) {
            s.refresh(self);
        }
        if s.log.is_empty() {
            text("No raids yet.", logr.x + 20.0, logr.y + 64.0, 18.0, MUTED);
        }
        let mut ly = logr.y + 50.0;
        for l in s.log.iter().take(((logr.h - 60.0) / 46.0) as usize) {
            let mine = l.attacker.id == me.id;
            let (who, swing) = if mine {
                (
                    format!("You raided {}", l.defender.display_name),
                    l.trophies,
                )
            } else {
                (
                    format!("{} raided you", l.attacker.display_name),
                    -l.trophies,
                )
            };
            text(&who, logr.x + 20.0, ly + 18.0, 18.0, TEXT);
            let detail = format!(
                "{}  {:.0}%  {:+} trophies",
                stars_text(l.result.stars),
                l.result.destruction * 100.0,
                swing
            );
            text(
                &detail,
                logr.x + 20.0,
                ly + 38.0,
                16.0,
                if swing >= 0 { GOOD } else { BAD },
            );
            if self.ui.button_ex(
                Rect::new(logr.x + logr.w - 100.0, ly + 6.0, 86.0, 30.0),
                "Watch",
                idle,
                false,
            ) {
                let c = self.account.client();
                let id = l.attack_id;
                s.busy = Some(Busy::Replay(Task::spawn(move || c.replay(id).map_err(err))));
            }
            ly += 46.0;
        }

        // Right: leaderboard.
        let right = Rect::new(880.0, 66.0, W - 900.0, 360.0);
        panel(right);
        text("Top players", right.x + 20.0, right.y + 34.0, 22.0, TEXT);
        for (k, p) in s.board.iter().enumerate() {
            let yy = right.y + 70.0 + k as f32 * 30.0;
            let col = if p.id == me.id { ACCENT } else { TEXT };
            text(
                &format!("{}. {}", k + 1, p.display_name),
                right.x + 20.0,
                yy,
                18.0,
                col,
            );
            let t = p.trophies.to_string();
            text(
                &t,
                right.x + right.w - 20.0 - text_width(&t, 18.0),
                yy,
                18.0,
                col,
            );
        }

        // Right, below: online upgrades.
        let up = Rect::new(880.0, 446.0, W - 900.0, H - 466.0);
        panel(up);
        let ux = up.x + 20.0;
        text("Online upgrades", ux, up.y + 30.0, 22.0, TEXT);
        text("Bought offline only counts offline.", ux, up.y + 52.0, 15.0, MUTED);
        let needs = online_needs(self, &paid);
        if needs.is_empty() {
            text("Everything you have counts online.", ux, up.y + 84.0, 16.0, GOOD);
        }
        let rows = ((up.h - 80.0) / 42.0) as usize;
        let mut ny = up.y + 64.0;
        for n in needs.iter().take(rows) {
            text(&n.label, ux, ny + 22.0, 16.0, TEXT);
            let b = Rect::new(up.x + up.w - 124.0, ny + 2.0, 110.0, 32.0);
            let label = n.blocked.clone().unwrap_or_else(|| format!("{}g", n.price));
            let ok = idle && n.blocked.is_none() && me.gold >= n.price;
            if self.ui.button_ex(b, &label, ok, false) {
                let c = self.account.client();
                let buy = n.buy;
                s.msg = None;
                s.busy = Some(Busy::Buy(
                    Task::spawn(move || {
                        match buy {
                            Buy::TownHall => c.upgrade_town_hall(),
                            Buy::Weapon(w) => c.buy_weapon(w),
                            Buy::Level(id, from) => c.level_up(id, from),
                        }
                        .map_err(err)
                    }),
                    n.label.clone(),
                ));
            }
            ny += 42.0;
        }
        if needs.len() > rows {
            text(&format!("and {} more", needs.len() - rows), ux, ny + 18.0, 15.0, MUTED);
        }

        Screen::Online(s)
    }
}

/// "2026-10-21T10:38:26.123Z" -> "10:38 UTC".
fn short_time(ts: &str) -> String {
    ts.split('T')
        .nth(1)
        .map(|t| format!("{} UTC", &t[..t.len().min(5)]))
        .unwrap_or_else(|| ts.to_string())
}

// ------------------------------------------------------------ watching

/// Watching an online raid, from the server's replay.
pub struct WatchState {
    replay: Replay,
    battle: Battle,
    /// True when the player was the attacker.
    attacking: bool,
    acc: f32,
    speed: f32,
    back: Option<OnlineState>,
}

impl WatchState {
    fn new(replay: Replay, me: Uuid, back: OnlineState) -> Self {
        WatchState {
            battle: replay_battle(&replay),
            attacking: replay.log.attacker.id == me,
            replay,
            acc: 0.0,
            speed: 1.0,
            back: Some(back),
        }
    }
}

impl Game {
    pub fn online_watch(&mut self, mut s: WatchState) -> Screen {
        let l = &s.replay.log;
        let title = format!(
            "{} raids {}",
            l.attacker.display_name, l.defender.display_name
        );
        self.header(&title, false);
        let dt = crate::record::dt().min(0.1);
        let running = s.battle.outcome() == Outcome::Running;
        if running {
            s.acc += dt * 20.0 * s.speed;
            while s.acc >= 1.0 && s.battle.outcome() == Outcome::Running {
                s.acc -= 1.0;
                s.battle.step();
                self.fx.absorb(&s.battle.world);
            }
        }
        self.fx.age(dt);
        let v = View::new(Rect::new(20.0, 66.0, 714.0, 714.0));
        let w = &s.battle.world;
        draw_ground(v, false, true, w);
        let troops = &s.battle.troops;
        let attacking = s.attacking;
        draw_world(v, w, &self.fx, &|_, u| {
            if u.tag < troops.len() {
                let mut look = Game::look_for(&troops[u.tag], true);
                look.hostile = (u.team == Team::Attack) != attacking;
                look
            } else {
                Look {
                    fig: Figure::raider(),
                    name: None,
                    hostile: true,
                }
            }
        });

        let rx = 754.0;
        let rw = W - rx - 20.0;
        let secs_left = (w.max_ticks - w.tick) as f32 / 20.0;
        text(&format!("Time left {secs_left:.0}s"), rx, 96.0, 26.0, TEXT);
        let dest = w.destruction();
        text(
            &format!("{:.0}% destroyed", dest * 100.0),
            rx + 260.0,
            96.0,
            26.0,
            ACCENT,
        );
        bar(Rect::new(rx, 108.0, rw, 12.0), dest, ACCENT);
        for k in 0..3 {
            star(rx + 24.0 + k as f32 * 44.0, 152.0, 18.0, k < w.stars());
        }
        for (k, sp) in [1.0f32, 2.0, 4.0].into_iter().enumerate() {
            if self.ui.button_ex(
                Rect::new(rx + 200.0 + k as f32 * 70.0, 134.0, 60.0, 36.0),
                &format!("{sp}x"),
                running,
                s.speed == sp,
            ) {
                s.speed = sp;
            }
        }

        if !running {
            let r = Rect::new(rx, 200.0, rw, 250.0);
            panel(r);
            let res = &l.result;
            let swing = if s.attacking { l.trophies } else { -l.trophies };
            let head = match (s.attacking, res.stars) {
                (true, 0) => "Raid failed",
                (true, _) => "Raid won",
                (false, 0) => "Your base held",
                (false, _) => "Your base was raided",
            };
            text_centered(
                head,
                r.x + r.w / 2.0,
                r.y + 44.0,
                32.0,
                if swing >= 0 { GOOD } else { BAD },
            );
            text_centered(
                &format!(
                    "{} stars, {:.0}% destroyed, {:+} trophies",
                    res.stars,
                    res.destruction * 100.0,
                    swing
                ),
                r.x + r.w / 2.0,
                r.y + 84.0,
                22.0,
                TEXT,
            );
            let gold = if s.attacking {
                format!("{} gold looted", l.gold_looted)
            } else {
                format!("{} gold lost", l.gold_looted)
            };
            text_centered(&gold, r.x + r.w / 2.0, r.y + 114.0, 20.0, ACCENT);
            // The server's result counts; this checks the replay agrees.
            let same = godcore::battle::fingerprint(w);
            let ok = format!("{same:016x}") == res.fingerprint;
            text_centered(
                if ok {
                    "Same result as the server's check"
                } else {
                    "This replay differs from the server's result"
                },
                r.x + r.w / 2.0,
                r.y + 146.0,
                16.0,
                if ok { MUTED } else { BAD },
            );
        }
        let back_label = if running {
            "Skip to the end"
        } else {
            "Back to Online"
        };
        if self
            .ui
            .button(Rect::new(rx, H - 90.0, rw, 50.0), back_label)
        {
            if running {
                s.battle.run();
                return Screen::OnlineWatch(s);
            }
            let mut back = s.back.take().unwrap_or_else(|| OnlineState::new(self));
            back.refresh(self);
            return Screen::Online(back);
        }
        Screen::OnlineWatch(s)
    }
}

#[cfg(test)]
mod tests {
    use super::is_this_computer;

    #[test]
    fn local_server_addresses() {
        for s in [
            "http://localhost:8080",
            "http://127.0.0.1:8080/",
            "http://[::1]:8080",
            "localhost",
        ] {
            assert!(is_this_computer(s), "{s}");
        }
        for s in [
            "https://bag.fly.dev",
            "https://localhost.example.com",
            "http://10.0.0.5:8080",
        ] {
            assert!(!is_this_computer(s), "{s}");
        }
    }
}
