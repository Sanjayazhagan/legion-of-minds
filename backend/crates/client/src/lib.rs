//! Client for the Become a God server, for the desktop game.
//!
//! Blocking calls, so run them off the render thread. A typical session:
//!
//! ```no_run
//! # use bag_client::*;
//! # let (home, defenders, army) = (Default::default(), vec![], vec![]);
//! let mut c = Client::new("http://localhost:8080");
//! c.dev_login("Sanjay")?;                       // or start_login("google") + wait_for_login
//! c.publish_base(&snapshot_from(&home, &defenders))?;
//! let m = c.find_match()?;                      // someone else's base
//! let (local, outcome) = c.attack(&m, army_from(&army))?;
//! assert_eq!(outcome.client_matched, Some(true));
//! # Ok::<(), Error>(())
//! ```

use bag_protocol::*;
use godcore::battle::Battle;
use godcore::scenario::{home_spots, BuildKind, HomeBase, Placed, RAID_ENTRY};
use godcore::troop::Troop;
use reqwest::blocking::{Client as Http, RequestBuilder};
use serde::de::DeserializeOwned;
use std::time::{Duration, Instant};

pub use bag_protocol;
pub use uuid::Uuid;

#[derive(Debug)]
pub enum Error {
    /// The server answered with an error, e.g. code "invalid" or "no_opponent".
    Api {
        status: u16,
        code: String,
        message: String,
    },
    Network(String),
    /// Caught locally before sending.
    Invalid(String),
    LoginTimedOut,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Api { message, .. } => f.write_str(message),
            Error::Network(e) => write!(f, "can't reach the server: {e}"),
            Error::Invalid(e) => f.write_str(e),
            Error::LoginTimedOut => f.write_str("sign-in timed out"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Invalid> for Error {
    fn from(e: Invalid) -> Self {
        Error::Invalid(e.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Client {
    base: String,
    http: Http,
    token: Option<String>,
}

impl Client {
    pub fn new(server: &str) -> Client {
        Client {
            base: server.trim_end_matches('/').to_string(),
            http: Http::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("http client"),
            token: None,
        }
    }

    /// Reuse a saved token from an earlier sign-in.
    pub fn with_token(mut self, token: impl Into<String>) -> Client {
        self.token = Some(token.into());
        self
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn req(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        let r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header(SIM_VERSION_HEADER, SIM_VERSION.to_string());
        match &self.token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    }

    fn send<T: DeserializeOwned>(&self, r: RequestBuilder) -> Result<T> {
        let resp = r.send().map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            return resp.json().map_err(|e| Error::Network(e.to_string()));
        }
        let body: Option<ApiErrorBody> = resp.json().ok();
        Err(Error::Api {
            status: status.as_u16(),
            code: body.as_ref().map_or("http".into(), |b| b.error.clone()),
            message: body.map_or(format!("server returned {status}"), |b| b.message),
        })
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.req(reqwest::Method::GET, path))
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &impl serde::Serialize) -> Result<T> {
        self.send(self.req(reqwest::Method::POST, path).json(body))
    }

    // ------------------------------------------------------------ sign-in

    pub fn providers(&self) -> Result<Providers> {
        self.get("/v1/auth/providers")
    }

    /// Test login by name (only when the server has DEV_LOGIN on).
    pub fn dev_login(&mut self, name: &str) -> Result<User> {
        let a: AuthResponse = self.post(
            "/v1/auth/dev",
            &DevLoginRequest {
                name: name.into(),
            },
        )?;
        self.token = Some(a.token);
        Ok(a.user)
    }

    /// Start a browser sign-in. Open `url` in the player's browser, then call
    /// `poll_login` every second or two, or `wait_for_login`.
    pub fn start_login(&self, provider: &str) -> Result<LoginStart> {
        self.post(
            "/v1/auth/login",
            &LoginStartRequest {
                provider: provider.into(),
            },
        )
    }

    /// `Some(user)` once the player finished signing in.
    pub fn poll_login(&mut self, l: &LoginStart) -> Result<Option<User>> {
        let r: LoginPollResponse = self.post(
            &format!("/v1/auth/logins/{}/poll", l.login_id),
            &LoginPoll {
                poll_secret: l.poll_secret.clone(),
            },
        )?;
        Ok(match r {
            LoginPollResponse::Pending => None,
            LoginPollResponse::Done(a) => {
                self.token = Some(a.token);
                Some(a.user)
            }
        })
    }

    pub fn wait_for_login(&mut self, l: &LoginStart) -> Result<User> {
        let end = Instant::now() + Duration::from_secs(l.expires_in_secs);
        while Instant::now() < end {
            if let Some(u) = self.poll_login(l)? {
                return Ok(u);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        Err(Error::LoginTimedOut)
    }

    pub fn logout(&mut self) -> Result<()> {
        let r = self.req(reqwest::Method::POST, "/v1/auth/logout");
        let resp = r.send().map_err(|e| Error::Network(e.to_string()))?;
        self.token = None;
        match resp.status().is_success() {
            true => Ok(()),
            false => Err(Error::Network(format!("logout: {}", resp.status()))),
        }
    }

    // ------------------------------------------------------------ profile

    pub fn me(&self) -> Result<User> {
        self.get("/v1/me")
    }

    pub fn rename(&self, name: &str) -> Result<User> {
        self.send(
            self.req(reqwest::Method::PATCH, "/v1/me").json(&RenameRequest {
                display_name: name.into(),
            }),
        )
    }

    pub fn leaderboard(&self, limit: u32) -> Result<Vec<PublicPlayer>> {
        self.get(&format!("/v1/leaderboard?limit={limit}"))
    }

    pub fn player(&self, id: Uuid) -> Result<PublicPlayer> {
        self.get(&format!("/v1/players/{id}"))
    }

    // ------------------------------------------------------------ base

    /// Publish the base others will raid. Checked locally first, and only
    /// the defenders' actors are sent (about a third of a troop file).
    pub fn publish_base(&self, s: &BaseSnapshot) -> Result<Published> {
        let s = canonical_snapshot(s)?;
        self.send(self.req(reqwest::Method::PUT, "/v1/me/base").json(&s))
    }

    // ------------------------------------------------------------ online upgrades

    /// Raise the online Town Hall one level, paid with online gold.
    pub fn upgrade_town_hall(&self) -> Result<User> {
        self.post("/v1/me/town-hall", &serde_json::json!({}))
    }

    /// Unlock a weapon online for every troop.
    pub fn buy_weapon(&self, weapon: godcore::weapons::Weapon) -> Result<User> {
        self.post("/v1/me/weapons", &BuyWeapon { weapon })
    }

    /// Level troop `troop_id` up online from `from` (its online level now).
    pub fn level_up(&self, troop_id: u64, from: u32) -> Result<User> {
        self.post(&format!("/v1/me/troops/{troop_id}/level"), &LevelUp { from })
    }

    pub fn my_base(&self) -> Result<BaseSnapshot> {
        self.get("/v1/me/base")
    }

    // ------------------------------------------------------------ raids

    pub fn find_match(&self) -> Result<Match> {
        self.post("/v1/matchmaking", &serde_json::json!({}))
    }

    /// Fight `m` locally, then send the army so the server can check it.
    /// Returns the local result (for the battle screen) and the server's
    /// verdict, which is the one that counts.
    pub fn attack(&self, m: &Match, army: Vec<Placement>) -> Result<(RaidResult, AttackOutcome)> {
        let paid = self.me()?.upgrades;
        let army = canonical_army(&army, paid.town_hall)?;
        paid.check(&army)?;
        let local = run_raid(&m.snapshot, &army);
        let out = self.submit(m.attack_id, army, Some(local.clone()))?;
        Ok((local, out))
    }

    pub fn submit(
        &self,
        attack_id: Uuid,
        army: Vec<Placement>,
        client_result: Option<RaidResult>,
    ) -> Result<AttackOutcome> {
        self.post(
            &format!("/v1/attacks/{attack_id}"),
            &AttackSubmit {
                army,
                client_result,
            },
        )
    }

    /// Battle log. `role` is "defense", "attack" or "all".
    pub fn log(&self, role: &str) -> Result<Vec<AttackLog>> {
        self.get(&format!("/v1/me/attacks?role={role}"))
    }

    pub fn replay(&self, attack_id: Uuid) -> Result<Replay> {
        self.get(&format!("/v1/attacks/{attack_id}/replay"))
    }
}

// ---------------------------------------------------------------- helpers

/// A snapshot from the game's home base and placed defenders.
pub fn snapshot_from(base: &HomeBase, defenders: &[(Troop, (f32, f32))]) -> BaseSnapshot {
    BaseSnapshot {
        base: base.clone(),
        defenders: placements(defenders),
    }
}

pub fn army_from(army: &[(Troop, (f32, f32))]) -> Vec<Placement> {
    placements(army)
}

fn placements(v: &[(Troop, (f32, f32))]) -> Vec<Placement> {
    v.iter()
        .map(|(t, (x, y))| Placement {
            troop: t.clone(),
            x: *x,
            y: *y,
        })
        .collect()
}

/// A valid layout with every building a Town Hall level allows: towers on
/// an outer ring, cannons inside. For tests and bagctl; players build their
/// own in the game.
pub fn starter_layout(town_hall: u32) -> HomeBase {
    let mut base = HomeBase {
        town_hall,
        buildings: Vec::new(),
    };
    for (kind, r, phase) in [(BuildKind::Tower, 5.4f32, 0.0f32), (BuildKind::Cannon, 3.2, 0.5)] {
        for k in 0..24 {
            let a = (k as f32 + phase) / 24.0 * std::f32::consts::TAU;
            let (x, y) = (r * a.cos(), r * a.sin());
            if base.can_add(kind) && base.fits(x, y) {
                base.buildings.push(Placed { kind, x, y });
            }
        }
    }
    base
}

/// Default defender spots (the game's auto-place).
pub fn defender_spots(n: usize) -> Vec<(f32, f32)> {
    home_spots(n)
}

/// Default army spots at the raid entry, as on the game's Defend tab.
pub fn army_spots(n: usize) -> Vec<(f32, f32)> {
    (0..n)
        .map(|k| {
            (
                RAID_ENTRY.0 + 1.4 * k as f32 - 1.0,
                RAID_ENTRY.1 - 1.0 + 0.6 * k as f32,
            )
        })
        .collect()
}

/// A battle to step through on the battle screen, from a replay.
pub fn replay_battle(r: &Replay) -> Battle {
    raid_battle(&r.snapshot, &r.army)
}

/// The raiders from a replay, to train defenders against (the game's
/// self-play pool, `--vs` in godctl).
pub fn raiders(r: &Replay) -> Vec<Troop> {
    r.army.iter().map(|p| p.troop.clone()).collect()
}
