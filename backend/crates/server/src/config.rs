//! Settings, all from environment variables. See `.env.example`.

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct OAuthProvider {
    /// URL path name: "google", "discord".
    pub name: &'static str,
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scope: &'static str,
    /// JSON field in the userinfo reply that holds the stable account id.
    pub id_field: &'static str,
    /// Fields to try, in order, for a display name.
    pub name_fields: &'static [&'static str],
}

impl OAuthProvider {
    pub fn google(client_id: String, client_secret: String) -> Self {
        OAuthProvider {
            name: "google",
            client_id,
            client_secret,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            scope: "openid profile",
            id_field: "sub",
            name_fields: &["given_name", "name"],
        }
    }

    pub fn discord(client_id: String, client_secret: String) -> Self {
        OAuthProvider {
            name: "discord",
            client_id,
            client_secret,
            auth_url: "https://discord.com/oauth2/authorize".into(),
            token_url: "https://discord.com/api/oauth2/token".into(),
            userinfo_url: "https://discord.com/api/users/@me".into(),
            scope: "identify",
            id_field: "id",
            name_fields: &["global_name", "username"],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    /// Where players reach this server, used for OAuth redirect URLs.
    pub public_url: String,
    /// Name-only test login. Never turn this on for a public server.
    pub dev_login: bool,
    pub providers: Vec<OAuthProvider>,
    pub session_ttl: Duration,
    pub login_ttl: Duration,
    /// How long an attacker has to submit a raid after matchmaking.
    pub attack_window: Duration,
    /// Protection after being raided for at least one star.
    pub shield: Duration,
    pub start_gold: i64,
    /// Matchmaking looks this many trophies either side first.
    pub match_range: i32,
    /// Largest request body as sent (gzipped bodies count compressed).
    pub max_body_bytes: usize,
    /// How long finished raids (battle log and replays) are kept.
    pub keep_raids: Duration,
    /// Gold the server pays a defender who holds a raid to no stars, and how
    /// many such rewards a player can get in 24 hours.
    pub defence_reward: i64,
    pub defence_rewards_per_day: i64,
    /// Gold for the first raid of the day (UTC) that wins a star.
    pub daily_win_bonus: i64,
    /// Gold per Town Hall level for a practice raid at 100% destruction, and
    /// how many practice raids pay in 24 hours.
    pub practice_gold: i64,
    pub practice_paid_per_day: i64,
    /// Largest JSON body once unzipped. A full army of 256-neuron brains is
    /// about 6 MB.
    pub max_json_bytes: usize,
    /// How long a season lasts.
    pub season: Duration,
    /// Time between a war being matched and attacks opening.
    pub war_prep: Duration,
    /// How long war attacks stay open.
    pub war_battle: Duration,
    /// Members with a published base a clan needs to start a war.
    pub war_min_size: usize,
    /// Requests a minute per client address, and for raids, publishing and
    /// sign-ins (which cost the server more).
    pub requests_per_min: u32,
    pub heavy_per_min: u32,
    /// Behind our own HTTPS proxy: take the client address from the last
    /// X-Forwarded-For entry.
    pub trust_proxy: bool,
}

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn num<T: std::str::FromStr>(k: &str, default: T) -> Result<T, String> {
    match var(k) {
        None => Ok(default),
        Some(v) => v.trim().parse().map_err(|_| format!("{k} must be a number")),
    }
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let database_url = var("DATABASE_URL").ok_or("DATABASE_URL is required")?;
        let bind = var("BIND_ADDR").unwrap_or_else(|| {
            // Most hosts (Fly.io, Railway, Render) hand the port in PORT.
            format!("0.0.0.0:{}", var("PORT").unwrap_or_else(|| "8080".into()))
        });
        let public_url = var("PUBLIC_URL")
            .unwrap_or_else(|| "http://localhost:8080".into())
            .trim_end_matches('/')
            .to_string();
        let dev_login = matches!(
            var("DEV_LOGIN").as_deref(),
            Some("1" | "true" | "yes" | "on")
        );
        let mut providers = Vec::new();
        let pair = |id: &str, secret: &str| -> Result<Option<(String, String)>, String> {
            match (var(id), var(secret)) {
                (Some(i), Some(s)) => Ok(Some((i, s))),
                (None, None) => Ok(None),
                _ => Err(format!("set both {id} and {secret}, or neither")),
            }
        };
        if let Some((i, s)) = pair("GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET")? {
            providers.push(OAuthProvider::google(i, s));
        }
        if let Some((i, s)) = pair("DISCORD_CLIENT_ID", "DISCORD_CLIENT_SECRET")? {
            providers.push(OAuthProvider::discord(i, s));
        }
        let mins = |m: u64| Duration::from_secs(m * 60);
        Ok(Config {
            database_url,
            bind,
            public_url,
            dev_login,
            providers,
            session_ttl: Duration::from_secs(num("SESSION_DAYS", 30u64)? * 86_400),
            login_ttl: mins(10),
            attack_window: Duration::from_secs(num("ATTACK_WINDOW_SECS", 300u64)?),
            shield: mins(num("SHIELD_MINUTES", 60u64)?),
            start_gold: num("START_GOLD", 500i64)?,
            match_range: num("MATCH_TROPHY_RANGE", 200i32)?,
            keep_raids: Duration::from_secs(num("KEEP_RAIDS_DAYS", 14u64)? * 86_400),
            defence_reward: num("DEFENCE_REWARD", 30i64)?.max(0),
            defence_rewards_per_day: num("DEFENCE_REWARDS_PER_DAY", 5i64)?,
            daily_win_bonus: num("DAILY_WIN_BONUS", 100i64)?.max(0),
            practice_gold: num("PRACTICE_GOLD", 20i64)?.max(0),
            practice_paid_per_day: num("PRACTICE_PAID_PER_DAY", 5i64)?,
            max_body_bytes: 8 << 20,
            max_json_bytes: 16 << 20,
            season: Duration::from_secs(num("SEASON_DAYS", 28u64)? * 86_400),
            war_prep: mins(num("WAR_PREP_MINUTES", 360u64)?),
            war_battle: mins(num("WAR_BATTLE_MINUTES", 1440u64)?),
            war_min_size: num("WAR_MIN_SIZE", 3usize)?.max(1),
            requests_per_min: num("REQUESTS_PER_MINUTE", 240u32)?,
            heavy_per_min: num("RAIDS_PER_MINUTE", 20u32)?,
            trust_proxy: matches!(var("TRUST_PROXY").as_deref(), Some("1" | "true" | "yes" | "on")),
        })
    }

    pub fn provider(&self, name: &str) -> Option<&OAuthProvider> {
        self.providers.iter().find(|p| p.name == name)
    }

    pub fn redirect_uri(&self, p: &OAuthProvider) -> String {
        format!("{}/v1/auth/{}/callback", self.public_url, p.name)
    }
}
