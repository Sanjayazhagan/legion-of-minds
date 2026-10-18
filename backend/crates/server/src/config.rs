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
    pub max_body_bytes: usize,
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
            max_body_bytes: 4 << 20,
        })
    }

    pub fn provider(&self, name: &str) -> Option<&OAuthProvider> {
        self.providers.iter().find(|p| p.name == name)
    }

    pub fn redirect_uri(&self, p: &OAuthProvider) -> String {
        format!("{}/v1/auth/{}/callback", self.public_url, p.name)
    }
}
