//! The Clan screen: find or start a clan, members, chat, troops lent to
//! clanmates, and clan wars.

use crate::online::{
    league_color, raid_army, time_left, OnlineState, Task, WatchState,
};
use crate::ui::*;
use crate::{Game, Screen};
use bag_client::bag_protocol::{
    canonical_army, with_borrowed, AttackLog, Borrowed, ChatMessage, ClanRole, ClanSummary,
    ClanView, League, Loan, Match, Replay, User, WarResult, WarState, WarView, CHAT_MAX,
    CLAN_MAX_MEMBERS, CLAN_NAME_MAX, MAX_BORROWED, MAX_LOANS, SIM_VERSION, WAR_ATTACKS,
};
use bag_client::{army_from, army_spots, Client, Uuid};
use macroquad::prelude::*;

fn err(e: bag_client::Error) -> String {
    e.to_string()
}

/// No clan war yet is not an error.
fn no_war(e: &bag_client::Error) -> bool {
    matches!(e, bag_client::Error::Api { code, .. } if code == "no_war")
}

/// Everything the screen shows, loaded together.
struct Loaded {
    user: User,
    view: Option<ClanView>,
    list: Vec<ClanSummary>,
    chat: Vec<ChatMessage>,
    loans: Vec<Loan>,
    mine: Vec<Loan>,
    war: Option<WarView>,
}

fn load(c: &Client, search: &str) -> Result<Loaded, String> {
    let user = c.me().map_err(err)?;
    if user.clan.is_none() {
        return Ok(Loaded {
            list: c.clans(search).map_err(err)?,
            user,
            view: None,
            chat: Vec::new(),
            loans: Vec::new(),
            mine: Vec::new(),
            war: None,
        });
    }
    let war = match c.war() {
        Ok(w) => Some(w),
        Err(e) if no_war(&e) => None,
        Err(e) => return Err(err(e)),
    };
    Ok(Loaded {
        view: Some(c.my_clan().map_err(err)?),
        chat: c.chat(0).map_err(err)?,
        loans: c.loans().map_err(err)?,
        mine: c.my_loans().map_err(err)?,
        list: Vec::new(),
        war,
        user,
    })
}

enum Busy {
    /// Load the screen, or do something and then load it. The string is
    /// what to say once it's done.
    Load(Task<Loaded>, Option<String>),
    Chat(Task<Vec<ChatMessage>>),
    /// A war attack: the replay to watch and the new stars.
    War(Task<(Replay, u32)>),
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    None,
    Search,
    Name,
    Say,
}

pub struct ClanState {
    view: Option<ClanView>,
    list: Vec<ClanSummary>,
    chat: Vec<ChatMessage>,
    loans: Vec<Loan>,
    mine: Vec<Loan>,
    war: Option<WarView>,
    search: String,
    name: String,
    say: String,
    focus: Field,
    /// Member picked in the list, for Promote and Remove.
    picked: Option<Uuid>,
    busy: Option<Busy>,
    msg: Option<(String, Color)>,
    since_chat: f32,
}

impl ClanState {
    pub fn new(g: &Game) -> Self {
        let mut s = ClanState {
            view: None,
            list: Vec::new(),
            chat: Vec::new(),
            loans: Vec::new(),
            mine: Vec::new(),
            war: None,
            search: String::new(),
            name: String::new(),
            say: String::new(),
            focus: Field::None,
            picked: None,
            busy: None,
            msg: None,
            since_chat: 0.0,
        };
        s.reload(g);
        s
    }

    pub fn reload(&mut self, g: &Game) {
        self.act(g, None, |_| Ok(()));
    }

    /// Run `f` on a background thread, then reload everything.
    fn act(
        &mut self,
        g: &Game,
        done: Option<String>,
        f: impl FnOnce(&Client) -> Result<(), bag_client::Error> + Send + 'static,
    ) {
        let c = g.account.client();
        let search = self.search.clone();
        self.msg = None;
        self.busy = Some(Busy::Load(
            Task::spawn(move || {
                f(&c).map_err(err)?;
                load(&c, &search)
            }),
            done,
        ));
    }
}

/// A one-line text box. Returns true when clicked.
fn field(ui: &Ui, r: Rect, value: &str, focused: bool, hint: &str) -> bool {
    draw_rectangle(r.x, r.y, r.w, r.h, BG);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, if focused { ACCENT } else { LINE });
    let size = 18.0;
    let ty = r.y + r.h / 2.0 + 6.0;
    if value.is_empty() && !focused {
        text(hint, r.x + 10.0, ty, size, MUTED);
    } else {
        // Show the end of a long value.
        let mut shown: String = value.to_string();
        while text_width(&shown, size) > r.w - 24.0 && !shown.is_empty() {
            shown.remove(0);
        }
        text(&shown, r.x + 10.0, ty, size, TEXT);
        if focused && (get_time() * 2.0) as i64 % 2 == 0 {
            let cx = r.x + 12.0 + text_width(&shown, size);
            draw_line(cx, r.y + 8.0, cx, r.y + r.h - 8.0, 2.0, ACCENT);
        }
    }
    ui.clicked && ui.hover(r)
}

/// Typing into the focused box. Returns true when Enter was pressed.
fn typing(value: &mut String, max: usize) -> bool {
    for c in crate::record::chars() {
        if !c.is_control() && value.chars().count() < max {
            value.push(c);
        }
    }
    if crate::record::backspace() {
        value.pop();
    }
    is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter)
}

fn stars_line(n: u32) -> String {
    format!("{}{}", "*".repeat(n.min(3) as usize), "-".repeat(3 - n.min(3) as usize))
}

impl Game {
    pub fn clan(&mut self, mut s: ClanState) -> Screen {
        let title = match &s.view {
            Some(v) => format!("Clan: {}", v.clan.name),
            None => "Clans".into(),
        };
        if self.header(&title, true) {
            return Screen::Online(OnlineState::new(self));
        }
        if self.account.user.is_none() {
            return Screen::Online(OnlineState::new(self));
        }

        // Finished server calls.
        let mut watch: Option<(Replay, u32)> = None;
        if let Some(b) = &s.busy {
            let done = match b {
                Busy::Load(t, said) => t.poll().map(|r| {
                    r.map(|l| {
                        self.account.user = Some(l.user);
                        s.view = l.view;
                        s.list = l.list;
                        s.chat = l.chat;
                        s.loans = l.loans;
                        s.mine = l.mine;
                        s.war = l.war;
                        if let Some(m) = said {
                            s.msg = Some((m.clone(), GOOD));
                        }
                    })
                }),
                Busy::Chat(t) => t.poll().map(|r| r.map(|c| s.chat = c)),
                Busy::War(t) => t.poll().map(|r| r.map(|w| watch = Some(w))),
            };
            match done {
                Some(Ok(())) => s.busy = None,
                Some(Err(e)) => {
                    s.busy = None;
                    s.msg = Some((e, BAD));
                }
                None => {}
            }
        }
        if let Some((replay, stars)) = watch {
            return Screen::OnlineWatch(WatchState::war(replay, stars, s));
        }
        let idle = s.busy.is_none();

        // Chat refreshes on its own every few seconds.
        s.since_chat += crate::record::dt();
        if idle && s.view.is_some() && s.since_chat > 6.0 {
            s.since_chat = 0.0;
            let c = self.account.client();
            s.busy = Some(Busy::Chat(Task::spawn(move || c.chat(0).map_err(err))));
        }

        // Typing.
        let enter = match s.focus {
            Field::Search => typing(&mut s.search, 24),
            Field::Name => typing(&mut s.name, CLAN_NAME_MAX),
            Field::Say => typing(&mut s.say, CHAT_MAX),
            Field::None => false,
        };
        if self.ui.clicked {
            s.focus = Field::None;
        }

        match s.view.clone() {
            None => self.find_clan(&mut s, idle, enter),
            Some(v) => self.my_clan(&mut s, &v, idle, enter),
        }

        // Status line.
        let status = match &s.busy {
            Some(Busy::Load(..)) => Some("Talking to the server..."),
            Some(Busy::War(_)) => Some("War attack: the server is checking the battle..."),
            _ => None,
        };
        let sy = H - 14.0;
        if let Some(t) = status {
            text(t, 20.0, sy, 16.0, SKY);
        } else if let Some((m, c)) = &s.msg {
            let short: String = m.chars().take(110).collect();
            text(&short, 20.0, sy, 16.0, *c);
        }
        Screen::Clan(s)
    }

    /// Not in a clan: search and join, or start one.
    fn find_clan(&mut self, s: &mut ClanState, idle: bool, enter: bool) {
        let left = Rect::new(20.0, 66.0, 760.0, H - 100.0);
        panel(left);
        let x = left.x + 20.0;
        text("Find a clan", x, left.y + 36.0, 26.0, TEXT);
        let sr = Rect::new(x, left.y + 54.0, 480.0, 40.0);
        if field(&self.ui, sr, &s.search, s.focus == Field::Search, "Search by name") {
            s.focus = Field::Search;
        }
        let go = self.ui.button_ex(Rect::new(x + 496.0, left.y + 54.0, 120.0, 40.0), "Search", idle, false);
        if (go || (enter && s.focus == Field::Search)) && idle {
            s.reload(self);
        }
        if s.list.is_empty() && idle {
            text("No clans found. Start the first one!", x, left.y + 136.0, 18.0, MUTED);
        }
        let mut y = left.y + 112.0;
        text("Clan", x, y, 15.0, MUTED);
        text("Members", x + 300.0, y, 15.0, MUTED);
        text("Trophies", x + 400.0, y, 15.0, MUTED);
        text("Wars won", x + 500.0, y, 15.0, MUTED);
        y += 10.0;
        let mut join: Option<(Uuid, String)> = None;
        for c in s.list.iter().take(((left.h - 140.0) / 50.0) as usize) {
            draw_line(x, y, left.x + left.w - 20.0, y, 1.0, LINE);
            let name: String = c.name.chars().take(26).collect();
            text(&name, x, y + 24.0, 20.0, TEXT);
            let d: String = c.description.chars().take(40).collect();
            text(&d, x, y + 42.0, 14.0, MUTED);
            text(&format!("{}/{CLAN_MAX_MEMBERS}", c.members), x + 300.0, y + 30.0, 18.0, TEXT);
            text(&c.trophies.to_string(), x + 400.0, y + 30.0, 18.0, ACCENT);
            text(&c.war_wins.to_string(), x + 500.0, y + 30.0, 18.0, TEXT);
            let full = c.members as usize >= CLAN_MAX_MEMBERS;
            let label = if !c.open {
                "Closed"
            } else if full {
                "Full"
            } else {
                "Join"
            };
            let b = Rect::new(left.x + left.w - 130.0, y + 8.0, 110.0, 34.0);
            if self.ui.button_ex(b, label, idle && c.open && !full, false) {
                join = Some((c.id, c.name.clone()));
            }
            y += 50.0;
        }
        if let Some((id, name)) = join {
            s.act(self, Some(format!("You joined {name}")), move |cl| cl.join_clan(id).map(|_| ()));
        }

        let right = Rect::new(800.0, 66.0, W - 820.0, 250.0);
        panel(right);
        let rx = right.x + 20.0;
        text("Start a clan", rx, right.y + 36.0, 26.0, TEXT);
        let nr = Rect::new(rx, right.y + 56.0, right.w - 40.0, 40.0);
        if field(&self.ui, nr, &s.name, s.focus == Field::Name, "Clan name (3 to 24 letters)") {
            s.focus = Field::Name;
        }
        let ok = idle && s.name.trim().chars().count() >= 3;
        let make = self.ui.button_ex(Rect::new(rx, right.y + 110.0, right.w - 40.0, 46.0), "Create clan", ok, false);
        if ok && (make || (enter && s.focus == Field::Name)) {
            let name = s.name.trim().to_string();
            s.name.clear();
            s.act(self, Some(format!("You founded {name}")), move |c| {
                c.create_clan(&name, "", true).map(|_| ())
            });
        }
        text("Anyone can join. You lead it.", rx, right.y + 186.0, 16.0, MUTED);

        let info = Rect::new(800.0, 336.0, W - 820.0, H - 370.0);
        panel(info);
        let ix = info.x + 20.0;
        text("Why join a clan", ix, info.y + 34.0, 22.0, TEXT);
        let lines = [
            "Chat with your clan.",
            &format!("Lend {MAX_LOANS} attackers; borrow up to {MAX_BORROWED}"),
            "for your own raids.",
            "Clan wars: two clans, frozen bases,",
            &format!("{WAR_ATTACKS} attacks each. Most stars wins."),
            "Everyone who fights gets gold:",
            &format!(
                "{} for a win, {} for a draw.",
                WarResult::Won.reward(),
                WarResult::Draw.reward()
            ),
            "Clanmates never raid each other.",
        ];
        for (k, l) in lines.iter().enumerate() {
            text(l, ix, info.y + 70.0 + k as f32 * 26.0, 17.0, MUTED);
        }
    }

    /// In a clan: members, chat, loans and the war.
    fn my_clan(&mut self, s: &mut ClanState, v: &ClanView, idle: bool, enter: bool) {
        let me = self.account.user.clone().expect("signed in");
        let my_role = me.clan.as_ref().map(|c| c.role).unwrap_or(ClanRole::Member);

        // Left: members.
        let left = Rect::new(20.0, 66.0, 400.0, H - 100.0);
        panel(left);
        let x = left.x + 20.0;
        text(&v.clan.name, x, left.y + 34.0, 26.0, ACCENT);
        text(
            &format!(
                "{}/{CLAN_MAX_MEMBERS} members  {} trophies",
                v.clan.members, v.clan.trophies
            ),
            x,
            left.y + 58.0,
            16.0,
            TEXT,
        );
        text(
            &format!(
                "Wars: {} won, {} lost, {} drawn",
                v.clan.war_wins, v.clan.war_losses, v.clan.war_draws
            ),
            x,
            left.y + 80.0,
            16.0,
            MUTED,
        );
        let mut y = left.y + 96.0;
        let rows = ((left.h - 210.0) / 34.0) as usize;
        for m in v.members.iter().take(rows) {
            let r = Rect::new(x - 8.0, y, left.w - 24.0, 32.0);
            let picked = s.picked == Some(m.player.id);
            if picked {
                draw_rectangle(r.x, r.y, r.w, r.h, PANEL_HI);
            }
            if self.ui.hover(r) && self.ui.clicked && m.player.id != me.id {
                s.picked = if picked { None } else { Some(m.player.id) };
            }
            let col = if m.player.id == me.id { ACCENT } else { TEXT };
            let name: String = m.player.display_name.chars().take(16).collect();
            text(&name, x, y + 22.0, 18.0, col);
            let role = match m.role {
                ClanRole::Member => "",
                r => r.name(),
            };
            text(role, x + 160.0, y + 22.0, 14.0, SKY);
            let l = League::of(m.player.trophies);
            text(&m.player.trophies.to_string(), x + 230.0, y + 22.0, 16.0, league_color(l));
            if m.lending > 0 {
                text(&format!("lends {}", m.lending), x + 290.0, y + 22.0, 14.0, GOOD);
            }
            y += 34.0;
        }
        // Officer actions on the picked member.
        let picked = s
            .picked
            .and_then(|id| v.members.iter().find(|m| m.player.id == id))
            .cloned();
        let by = left.y + left.h - 112.0;
        if let Some(m) = picked {
            let can_promote = my_role == ClanRole::Leader;
            let promote = if m.role == ClanRole::Member { "Make elder" } else { "Make leader" };
            let can_kick = match my_role {
                ClanRole::Leader => true,
                ClanRole::Elder => m.role == ClanRole::Member,
                ClanRole::Member => false,
            };
            let half = (left.w - 50.0) / 2.0;
            if self.ui.button_ex(Rect::new(x, by, half, 40.0), promote, idle && can_promote, false) {
                let id = m.player.id;
                s.act(self, Some(format!("{} promoted", m.player.display_name)), move |c| {
                    c.promote(id).map(|_| ())
                });
            }
            if self.ui.button_ex(Rect::new(x + half + 10.0, by, half, 40.0), "Remove", idle && can_kick, false) {
                let id = m.player.id;
                s.picked = None;
                s.act(self, Some(format!("{} removed", m.player.display_name)), move |c| {
                    c.kick(id).map(|_| ())
                });
            }
        } else if my_role.officer() {
            text("Pick a member to promote or remove.", x, by + 26.0, 15.0, MUTED);
        }
        if self.ui.button_ex(
            Rect::new(x, left.y + left.h - 60.0, left.w - 40.0, 42.0),
            "Leave clan",
            idle,
            false,
        ) {
            s.act(self, Some("You left the clan".into()), |c| c.leave_clan());
        }

        // Middle: chat.
        let mid = Rect::new(440.0, 66.0, 400.0, H - 100.0);
        panel(mid);
        let cx = mid.x + 20.0;
        text("Clan chat", cx, mid.y + 34.0, 22.0, TEXT);
        let max_w = mid.w - 40.0;
        // Wrap the newest messages into lines, bottom up.
        let mut lines: Vec<(String, Color)> = Vec::new();
        for m in &s.chat {
            let (head, col) = match &m.user_id {
                None if m.from.is_empty() => (String::new(), SKY),
                _ => (format!("{}: ", m.from), if m.user_id == Some(me.id) { ACCENT } else { TEXT }),
            };
            let mut line = head;
            for w in m.body.split(' ') {
                let try_ = if line.is_empty() || line.ends_with(": ") {
                    format!("{line}{w}")
                } else {
                    format!("{line} {w}")
                };
                if text_width(&try_, 16.0) > max_w && !line.is_empty() {
                    lines.push((line, col));
                    line = format!("  {w}");
                } else {
                    line = try_;
                }
            }
            lines.push((line, col));
        }
        let fit = ((mid.h - 120.0) / 22.0) as usize;
        let start = lines.len().saturating_sub(fit);
        for (k, (l, c)) in lines[start..].iter().enumerate() {
            text(l, cx, mid.y + 64.0 + k as f32 * 22.0, 16.0, *c);
        }
        let sr = Rect::new(cx, mid.y + mid.h - 56.0, mid.w - 130.0, 40.0);
        if field(&self.ui, sr, &s.say, s.focus == Field::Say, "Say something") {
            s.focus = Field::Say;
        }
        let ok = !s.say.trim().is_empty();
        let send = self.ui.button_ex(Rect::new(mid.x + mid.w - 100.0, sr.y, 80.0, 40.0), "Send", ok && idle, false);
        if ok && idle && (send || (enter && s.focus == Field::Say)) {
            let body = std::mem::take(&mut s.say);
            s.focus = Field::Say;
            s.act(self, None, move |c| c.say(&body).map(|_| ()));
        }

        // Right, top: lending.
        let rx0 = 860.0;
        let rw = W - rx0 - 20.0;
        let lend = Rect::new(rx0, 66.0, rw, 206.0);
        panel(lend);
        let rx = lend.x + 20.0;
        text("Lent troops", rx, lend.y + 32.0, 22.0, TEXT);
        let mine: Vec<&str> = s.mine.iter().map(|l| l.troop.name.trim()).collect();
        let lent = if mine.is_empty() {
            "You lend nothing yet.".to_string()
        } else {
            format!("You lend: {}", mine.join(", "))
        };
        text(&lent, rx, lend.y + 60.0, 16.0, MUTED);
        let paid = me.upgrades.clone();
        let best: Vec<_> = raid_army(self, &paid).into_iter().take(MAX_LOANS).map(|(t, _)| t).collect();
        let label = format!("Lend my best {}", best.len().max(1));
        if self.ui.button_ex(Rect::new(rx, lend.y + 74.0, rw - 40.0, 40.0), &label, idle && !best.is_empty(), false) {
            let n = best.len();
            s.act(self, Some(format!("Lent {n} troop(s) to the clan")), move |c| c.lend(best).map(|_| ()));
        }
        if s.loans.is_empty() {
            text("Nobody lends you troops yet.", rx, lend.y + 140.0, 15.0, MUTED);
        } else {
            text(
                &format!("Lent to you ({}), up to {MAX_BORROWED} fill raid slots:", s.loans.len()),
                rx,
                lend.y + 136.0,
                15.0,
                MUTED,
            );
            for (k, l) in s.loans.iter().take(2).enumerate() {
                let line = format!(
                    "{} the {}, from {}",
                    l.troop.name.trim(),
                    l.troop.spec.name,
                    l.owner.display_name
                );
                let line: String = line.chars().take(44).collect();
                text(&line, rx, lend.y + 158.0 + k as f32 * 20.0, 15.0, TEXT);
            }
        }

        // Right, below: war.
        let war = Rect::new(rx0, 292.0, rw, H - 326.0);
        panel(war);
        self.war_panel(s, v, war, my_role, &me, idle);
    }

    fn war_panel(&mut self, s: &mut ClanState, v: &ClanView, r: Rect, role: ClanRole, me: &User, idle: bool) {
        let x = r.x + 20.0;
        text("Clan war", x, r.y + 32.0, 22.0, TEXT);
        let active = s.war.as_ref().filter(|w| w.state != WarState::Done).cloned();
        let Some(w) = active else {
            let mut y = r.y + 64.0;
            if let Some(last) = s.war.as_ref().and_then(|w| w.result.map(|res| (w, res))) {
                let (w, res) = last;
                let (head, col) = match res {
                    WarResult::Won => ("Last war: won", GOOD),
                    WarResult::Lost => ("Last war: lost", BAD),
                    WarResult::Draw => ("Last war: draw", MUTED),
                };
                text(head, x, y, 18.0, col);
                text(
                    &format!("{} {} - {} {}", w.us.name, w.us.stars, w.them.stars, w.them.name),
                    x,
                    y + 22.0,
                    15.0,
                    MUTED,
                );
                y += 54.0;
            }
            if v.war_searching {
                text("Looking for an opponent...", x, y, 18.0, SKY);
                if role.officer()
                    && self.ui.button_ex(Rect::new(x, y + 16.0, r.w - 40.0, 40.0), "Stop looking", idle, false)
                {
                    s.act(self, None, |c| c.cancel_war_search().map(|_| ()));
                }
            } else if role.officer() {
                if self.ui.button_ex(Rect::new(x, y, r.w - 40.0, 46.0), "Start a war", idle, false) {
                    s.act(self, Some("War search started".into()), |c| c.search_war().map(|_| ()));
                }
                text("Members with a published base fight.", x, y + 70.0, 15.0, MUTED);
            } else {
                text("Your leader or an elder starts wars.", x, y + 10.0, 16.0, MUTED);
            }
            return;
        };

        // Score.
        let y = r.y + 64.0;
        let us = format!("{} {}", w.us.name.chars().take(14).collect::<String>(), w.us.stars);
        let them = format!("{} {}", w.them.stars, w.them.name.chars().take(14).collect::<String>());
        text(&us, x, y, 20.0, ACCENT);
        text("vs", r.x + r.w / 2.0 - 8.0, y, 16.0, MUTED);
        text(&them, r.x + r.w - 20.0 - text_width(&them, 20.0), y, 20.0, BAD);
        let when = match w.state {
            WarState::Preparation => format!("Attacks open in {}", time_left(&w.battle_at)),
            _ => format!("Ends in {}", time_left(&w.ends_at)),
        };
        text(&when, x, y + 24.0, 15.0, SKY);
        let left = if w.in_war {
            format!("Your attacks left: {}", w.my_attacks_left)
        } else {
            "You joined after this war began.".into()
        };
        text(&left, x + 170.0, y + 24.0, 15.0, TEXT);

        // Enemy bases, then ours.
        let mut by = y + 40.0;
        let can_attack = w.state == WarState::Battle && w.my_attacks_left > 0 && idle;
        let rows = ((r.y + r.h - by - 10.0) / 34.0) as usize;
        let ours = w.us.bases.len().min(rows.saturating_sub(w.them.bases.len() + 1));
        for b in w.them.bases.iter().take(rows) {
            let name: String = b.player.display_name.chars().take(12).collect();
            text(&format!("{}. {name}", b.position), x, by + 22.0, 16.0, TEXT);
            text(&format!("TH{}", b.town_hall), x + 130.0, by + 22.0, 14.0, MUTED);
            text(&stars_line(b.best_stars), x + 170.0, by + 22.0, 16.0, ACCENT);
            let br = Rect::new(r.x + r.w - 104.0, by + 2.0, 86.0, 30.0);
            if self.ui.button_ex(br, "Attack", can_attack, false) {
                s.busy = Some(Busy::War(self.war_attack(b.player.id, me)));
            }
            by += 34.0;
        }
        if ours > 0 {
            text("Our bases", x, by + 22.0, 15.0, MUTED);
            by += 30.0;
            for b in w.us.bases.iter().take(ours) {
                let name: String = b.player.display_name.chars().take(12).collect();
                let col = if b.player.id == me.id { ACCENT } else { TEXT };
                text(&format!("{}. {name}", b.position), x, by + 18.0, 15.0, col);
                text(&stars_line(b.best_stars), x + 170.0, by + 18.0, 15.0, BAD);
                let used = format!("{}/{WAR_ATTACKS} attacks", b.attacks_used);
                text(&used, r.x + r.w - 20.0 - text_width(&used, 14.0), by + 18.0, 14.0, MUTED);
                by += 26.0;
            }
        }
    }

    /// Start a war attack on `target`, fight it locally and send it; the
    /// replay is built from what was sent, so it's the battle the server ran.
    fn war_attack(&self, target: Uuid, me: &User) -> Task<(Replay, u32)> {
        let c = self.account.client();
        let paid = me.upgrades.clone();
        let mut own = raid_army(self, &paid);
        let me_pub = bag_client::bag_protocol::PublicPlayer {
            id: me.id,
            display_name: me.display_name.clone(),
            trophies: me.trophies,
            town_hall: me.town_hall,
        };
        Task::spawn(move || {
            let loans = c.loans().unwrap_or_default();
            let free = crate::battle_screen::MAX_SQUAD.saturating_sub(own.len()).min(MAX_BORROWED);
            let spots = army_spots(own.len() + free.min(loans.len()));
            for (k, (_, spot)) in own.iter_mut().enumerate() {
                *spot = spots[k];
            }
            let borrowed: Vec<(Loan, (f32, f32))> = loans
                .into_iter()
                .take(free)
                .zip(spots[own.len()..].iter().copied())
                .collect();
            let m: Match = c.war_target(target).map_err(err)?;
            let army = army_from(&own);
            let (_, out) = c.war_attack(&m, army.clone(), &borrowed).map_err(err)?;
            let own = canonical_army(&army, paid.town_hall).map_err(|e| e.0)?;
            let b: Vec<Borrowed> = borrowed
                .iter()
                .map(|(l, (x, y))| Borrowed { loan_id: l.id, x: *x, y: *y })
                .collect();
            let full = with_borrowed(own, &b, |id| {
                borrowed.iter().find(|(l, _)| l.id == id).map(|(l, _)| l.troop.clone())
            })
            .map_err(|e| e.0)?;
            let replay = Replay {
                log: AttackLog {
                    attack_id: out.attack_id,
                    attacker: me_pub,
                    defender: m.defender.clone(),
                    result: out.result.clone(),
                    trophies: 0,
                    gold_looted: 0,
                    client_matched: out.client_matched,
                    finished_at: String::new(),
                },
                sim_version: SIM_VERSION,
                snapshot: m.snapshot,
                army: full,
            };
            Ok((replay, out.new_stars))
        })
    }
}
