//! Sign-in. Every method ends in the same place: an opaque bearer token for
//! a user row. Only the token's SHA-256 is stored.
//!
//! The desktop game signs in through the browser without running a local web
//! server: it asks for a login, opens the returned URL, and polls with a
//! secret until the provider's callback lands here.

use crate::error::{ApiError, ApiResult};
use crate::players::load_user;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::de::DeserializeOwned;
use bag_protocol::{
    AuthResponse, DevLoginRequest, LoginPoll, LoginPollResponse, LoginStart, LoginStartRequest,
    Providers, MAX_NAME, SIM_VERSION, SIM_VERSION_HEADER,
};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub fn random_token(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn later(d: std::time::Duration) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::from_std(d).expect("duration fits")
}

pub fn clean_name(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let s: String = s.chars().take(MAX_NAME).collect();
    if s.is_empty() {
        "Player".into()
    } else {
        s
    }
}

/// The signed-in user, from `Authorization: Bearer <token>`.
pub struct AuthUser {
    pub id: Uuid,
    pub token_hash: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;
        let hash = sha256_hex(token.trim());
        let id: Option<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()",
        )
        .bind(&hash)
        .fetch_optional(&st.db)
        .await?;
        match id {
            Some(id) => Ok(AuthUser {
                id,
                token_hash: hash,
            }),
            None => Err(ApiError::unauthorized()),
        }
    }
}

/// Rejects requests from a game whose battle rules differ from the server's
/// (`x-sim-version` header). Raids are checked by re-running them, which
/// only works when both sides run the same rules.
pub struct SameRules;

fn same_rules(headers: &HeaderMap) -> Result<(), ApiError> {
    let theirs = headers
        .get(SIM_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u32>().ok());
    if theirs == Some(SIM_VERSION) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "update_required",
            format!("this server runs battle rules v{SIM_VERSION}; update the game to play online"),
        ))
    }
}

impl FromRequestParts<AppState> for SameRules {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, ApiError> {
        same_rules(&parts.headers).map(|()| SameRules)
    }
}

/// A JSON body from a game on the same battle rules. Unlike `SameRules`, it
/// reads the whole body before answering `update_required`: answering first
/// drops the connection while a big upload (a base, an army) is still being
/// sent, and the old game sees a broken pipe instead of the 409.
pub struct SameRulesJson<T>(pub T);

impl<T: DeserializeOwned> FromRequest<AppState> for SameRulesJson<T> {
    type Rejection = Response;

    async fn from_request(req: Request, st: &AppState) -> Result<Self, Response> {
        let rules = same_rules(req.headers());
        let body = Bytes::from_request(req, st)
            .await
            .map_err(IntoResponse::into_response)?;
        rules.map_err(IntoResponse::into_response)?;
        let Json(v) = Json::<T>::from_bytes(&body).map_err(IntoResponse::into_response)?;
        Ok(SameRulesJson(v))
    }
}

async fn new_session(st: &AppState, user: Uuid) -> ApiResult<AuthResponse> {
    let token = format!("bag_{}", random_token(32));
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(sha256_hex(&token))
        .bind(user)
        .bind(later(st.cfg.session_ttl))
        .execute(&st.db)
        .await?;
    Ok(AuthResponse {
        token,
        user: load_user(&st.db, user).await?,
    })
}

/// The user behind (provider, subject), created on first sign-in.
pub async fn find_or_create_user(
    st: &AppState,
    provider: &str,
    subject: &str,
    name: &str,
) -> ApiResult<Uuid> {
    let find = || {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM identities WHERE provider = $1 AND subject = $2",
        )
        .bind(provider)
        .bind(subject)
    };
    if let Some(id) = find().fetch_optional(&st.db).await? {
        return Ok(id);
    }
    let mut tx = st.db.begin().await?;
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, display_name, gold) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(clean_name(name))
        .bind(st.cfg.start_gold)
        .execute(&mut *tx)
        .await?;
    let linked = sqlx::query(
        "INSERT INTO identities (provider, subject, user_id) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(provider)
    .bind(subject)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if linked == 1 {
        tx.commit().await?;
        Ok(id)
    } else {
        // Someone signed in with the same account at the same moment.
        tx.rollback().await?;
        find()
            .fetch_one(&st.db)
            .await
            .map_err(ApiError::from)
    }
}

pub async fn providers(State(st): State<AppState>) -> Json<Providers> {
    Json(Providers {
        dev: st.cfg.dev_login,
        oauth: st.cfg.providers.iter().map(|p| p.name.to_string()).collect(),
    })
}

/// Test login by name only. The same name always gives the same account.
pub async fn dev_login(
    State(st): State<AppState>,
    Json(req): Json<DevLoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    if !st.cfg.dev_login {
        return Err(ApiError::not_found("dev login"));
    }
    let name = clean_name(&req.name);
    let id = find_or_create_user(&st, "dev", &name.to_lowercase(), &name).await?;
    Ok(Json(new_session(&st, id).await?))
}

pub async fn logout(State(st): State<AppState>, user: AuthUser) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(&user.token_hash)
        .execute(&st.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn login_start(
    State(st): State<AppState>,
    Json(req): Json<LoginStartRequest>,
) -> ApiResult<Json<LoginStart>> {
    let p = st
        .cfg
        .provider(&req.provider)
        .ok_or_else(|| ApiError::not_found("login provider"))?;
    let id = Uuid::new_v4();
    let state = random_token(24);
    let verifier = random_token(48);
    let poll_secret = random_token(24);
    sqlx::query(
        "INSERT INTO oauth_logins (id, provider, state, pkce_verifier, poll_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(p.name)
    .bind(&state)
    .bind(&verifier)
    .bind(sha256_hex(&poll_secret))
    .bind(later(st.cfg.login_ttl))
    .execute(&st.db)
    .await?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse(&p.auth_url).map_err(ApiError::internal)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &p.client_id)
        .append_pair("redirect_uri", &st.cfg.redirect_uri(p))
        .append_pair("scope", p.scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "consent");
    Ok(Json(LoginStart {
        login_id: id,
        poll_secret,
        url: url.into(),
        expires_in_secs: st.cfg.login_ttl.as_secs(),
    }))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
         <title>Legion of Minds</title>\
         <body style='font-family:system-ui;background:#14161c;color:#eee;display:grid;\
         place-items:center;height:90vh;text-align:center'>\
         <div><h1>{title}</h1><p>{body}</p></div>"
    );
    (status, Html(html)).into_response()
}

pub async fn callback(
    State(st): State<AppState>,
    Path(provider): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    match finish_login(&st, &provider, q).await {
        Ok(()) => page(
            StatusCode::OK,
            "You're signed in",
            "You can close this tab and go back to the game.",
        ),
        Err(e) => {
            tracing::warn!("oauth callback failed: {}", e.message);
            page(
                e.status,
                "Sign-in didn't work",
                "Close this tab and try again from the game.",
            )
        }
    }
}

async fn finish_login(st: &AppState, provider: &str, q: CallbackQuery) -> ApiResult<()> {
    if let Some(e) = q.error {
        return Err(ApiError::bad_request(format!("provider said: {e}")));
    }
    let p = st
        .cfg
        .provider(provider)
        .ok_or_else(|| ApiError::not_found("login provider"))?;
    let (code, state) = q
        .code
        .zip(q.state)
        .ok_or_else(|| ApiError::bad_request("missing code or state"))?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, pkce_verifier FROM oauth_logins
         WHERE state = $1 AND provider = $2 AND user_id IS NULL AND expires_at > now()",
    )
    .bind(&state)
    .bind(p.name)
    .fetch_optional(&st.db)
    .await?;
    let (login_id, verifier) = row.ok_or_else(|| ApiError::not_found("sign-in"))?;

    let token: serde_json::Value = st
        .http
        .post(&p.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", st.cfg.redirect_uri(p).as_str()),
            ("client_id", p.client_id.as_str()),
            ("client_secret", p.client_secret.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| ApiError::bad_request(format!("token exchange failed: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::bad_request(format!("token reply: {e}")))?;
    let access = token["access_token"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("no access_token in reply"))?;
    let info: serde_json::Value = st
        .http
        .get(&p.userinfo_url)
        .bearer_auth(access)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| ApiError::bad_request(format!("userinfo failed: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::bad_request(format!("userinfo reply: {e}")))?;
    let subject = match &info[p.id_field] {
        serde_json::Value::String(s) if !s.is_empty() => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return Err(ApiError::bad_request("userinfo has no account id")),
    };
    let name = p
        .name_fields
        .iter()
        .find_map(|f| info[*f].as_str().filter(|s| !s.trim().is_empty()))
        .unwrap_or("Player");
    let user = find_or_create_user(st, p.name, &subject, name).await?;
    sqlx::query("UPDATE oauth_logins SET user_id = $1 WHERE id = $2")
        .bind(user)
        .bind(login_id)
        .execute(&st.db)
        .await?;
    Ok(())
}

pub async fn login_poll(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<LoginPoll>,
) -> ApiResult<Json<LoginPollResponse>> {
    let row: Option<(String, Option<Uuid>, DateTime<Utc>)> =
        sqlx::query_as("SELECT poll_hash, user_id, expires_at FROM oauth_logins WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
    let Some((hash, user, expires)) = row else {
        return Err(ApiError::not_found("sign-in"));
    };
    if hash != sha256_hex(&req.poll_secret) {
        return Err(ApiError::not_found("sign-in"));
    }
    if expires <= Utc::now() {
        return Err(ApiError::new(
            StatusCode::GONE,
            "expired",
            "sign-in timed out, start again",
        ));
    }
    let Some(user) = user else {
        return Ok(Json(LoginPollResponse::Pending));
    };
    // One token per completed sign-in: whoever deletes the row gets it.
    let gone = sqlx::query("DELETE FROM oauth_logins WHERE id = $1")
        .bind(id)
        .execute(&st.db)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(ApiError::not_found("sign-in"));
    }
    Ok(Json(LoginPollResponse::Done(new_session(&st, user).await?)))
}
