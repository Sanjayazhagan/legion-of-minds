//! Request limits, so a few scripted clients can't flood the server.
//! Raids and publishing re-run battles on the CPU, so they get a tighter
//! limit than everything else.
//!
//! Signed-in requests are counted per account (by session token), so many
//! players behind one shared address (a mobile carrier, a cafe) don't share
//! one allowance. Each address still has a much larger backstop, so minting
//! tokens can't lift the limit. Requests without a token are counted per
//! address.

use crate::error::ApiError;
use crate::AppState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Instant;

/// A token bucket: `per_min` requests a minute, bursts up to `per_min`.
#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<(Who, Class), Bucket>>,
}

/// Whose allowance a request uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Who {
    Ip(IpAddr),
    /// The first 16 bytes of the session token's hash.
    Token([u8; 16]),
}

/// Which allowance: everyday calls, battle-running calls, or sign-ins.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Class {
    Light,
    Heavy,
    SignIn,
}

/// How many accounts' worth of calls one address may make in total.
const IP_BACKSTOP: u32 = 25;
/// Sign-ins allowed per address, as a multiple of the heavy limit: a whole
/// cafe may sign in at once, but not a script.
const SIGN_IN_FACTOR: u32 = 5;

impl Limiter {
    /// Take one token; on refusal, the seconds until one is free.
    fn take(&self, who: Who, class: Class, per_min: u32) -> Result<(), u64> {
        let now = Instant::now();
        let cap = per_min.max(1) as f64;
        let rate = cap / 60.0;
        let mut map = self.buckets.lock().unwrap();
        if map.len() > 50_000 {
            // Forget clients whose bucket has refilled anyway.
            map.retain(|_, b| b.tokens + now.duration_since(b.at).as_secs_f64() * rate < cap);
        }
        let b = map.entry((who, class)).or_insert(Bucket {
            tokens: cap,
            at: now,
        });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * rate).min(cap);
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - b.tokens) / rate).ceil() as u64)
        }
    }
}

fn class_of(method: &Method, path: &str) -> Class {
    if method == Method::POST && path.starts_with("/v1/auth/") && !path.ends_with("/poll") {
        Class::SignIn
    } else if (method == Method::POST
        && (path == "/v1/matchmaking"
            || path.starts_with("/v1/attacks/")
            || path.starts_with("/v1/war-attacks/")))
        || (method == Method::PUT && path == "/v1/me/base")
    {
        Class::Heavy
    } else {
        Class::Light
    }
}

/// The session token's hash, if the request carries one.
fn token_of(req: &Request) -> Option<[u8; 16]> {
    use sha2::{Digest, Sha256};
    let t = req
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?
        .trim();
    if t.is_empty() {
        return None;
    }
    let h = Sha256::digest(t.as_bytes());
    let mut k = [0u8; 16];
    k.copy_from_slice(&h[..16]);
    Some(k)
}

/// The client's address. Behind our own HTTPS proxy (`TRUST_PROXY`), that is
/// the last address the proxy added to X-Forwarded-For; anything earlier in
/// the header was written by the client and can't be trusted.
fn client_ip(st: &AppState, req: &Request) -> IpAddr {
    if st.cfg.trust_proxy {
        let fwd = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .last()
            .and_then(|ip| ip.trim().parse().ok());
        if let Some(ip) = fwd {
            return ip;
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::from([0, 0, 0, 0]), |c| c.0.ip())
}

impl Limiter {
    /// Charge a request to its account (if any) and its address.
    fn charge(
        &self,
        ip: IpAddr,
        token: Option<[u8; 16]>,
        class: Class,
        light: u32,
        heavy: u32,
    ) -> Result<(), u64> {
        let per_min = match class {
            Class::Light => light,
            Class::Heavy => heavy,
            Class::SignIn => heavy.saturating_mul(SIGN_IN_FACTOR),
        };
        match token {
            Some(t) if class != Class::SignIn => {
                self.take(Who::Token(t), class, per_min)?;
                self.take(Who::Ip(ip), class, per_min.saturating_mul(IP_BACKSTOP))
            }
            _ => self.take(Who::Ip(ip), class, per_min),
        }
    }
}

pub async fn limit(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let ip = client_ip(&st, &req);
    let class = class_of(req.method(), req.uri().path());
    let token = token_of(&req);
    match st.limiter.charge(
        ip,
        token,
        class,
        st.cfg.requests_per_min,
        st.cfg.heavy_per_min,
    ) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            // Read the upload before refusing: answering while a big body
            // (a base, an army) is still arriving drops the connection, and
            // the game would say it can't reach the server instead of
            // "slow down". The body size limit layer bounds this read.
            let _ = axum::body::to_bytes(req.into_body(), usize::MAX).await;
            let mut r = ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "slow_down",
                format!("too many requests, try again in {wait} s"),
            )
            .into_response();
            if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
                r.headers_mut().insert(header::RETRY_AFTER, v);
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_refuse_then_refill() {
        let l = Limiter::default();
        let ip = Who::Ip("10.0.0.1".parse().unwrap());
        for _ in 0..3 {
            assert!(l.take(ip, Class::Heavy, 3).is_ok());
        }
        let wait = l.take(ip, Class::Heavy, 3).unwrap_err();
        assert!((1..=20).contains(&wait), "{wait}");
        // Another client, or the light bucket, is unaffected.
        assert!(l
            .take(Who::Ip("10.0.0.2".parse().unwrap()), Class::Heavy, 3)
            .is_ok());
        assert!(l.take(ip, Class::Light, 3).is_ok());
    }

    #[test]
    fn players_sharing_an_address_each_get_their_own_limit() {
        let l = Limiter::default();
        let ip: IpAddr = "10.0.0.9".parse().unwrap();
        // 20 signed-in players behind one address, each with a limit of 2.
        for p in 0..20u8 {
            let tok = Some([p; 16]);
            for _ in 0..2 {
                assert!(
                    l.charge(ip, tok, Class::Heavy, 240, 2).is_ok(),
                    "player {p}"
                );
            }
            assert!(
                l.charge(ip, tok, Class::Heavy, 240, 2).is_err(),
                "player {p} over their own limit"
            );
        }
        // The address backstop still holds: 25 accounts' worth in total.
        let l = Limiter::default();
        let ok = (0..200u8)
            .filter(|&p| l.charge(ip, Some([p; 16]), Class::Heavy, 240, 2).is_ok())
            .count();
        assert_eq!(ok, 50);
        // Sign-ins are per address, but five times the raid limit.
        let l = Limiter::default();
        let ok = (0..200)
            .filter(|_| l.charge(ip, None, Class::SignIn, 240, 20).is_ok())
            .count();
        assert_eq!(ok, 100);
    }

    #[test]
    fn route_classes() {
        assert_eq!(class_of(&Method::POST, "/v1/matchmaking"), Class::Heavy);
        assert_eq!(class_of(&Method::POST, "/v1/attacks/123"), Class::Heavy);
        assert_eq!(class_of(&Method::PUT, "/v1/me/base"), Class::Heavy);
        assert_eq!(class_of(&Method::POST, "/v1/war-attacks/7"), Class::Heavy);
        assert_eq!(class_of(&Method::POST, "/v1/auth/login"), Class::SignIn);
        assert_eq!(
            class_of(&Method::POST, "/v1/auth/logins/1/poll"),
            Class::Light
        );
        assert_eq!(class_of(&Method::GET, "/v1/me/base"), Class::Light);
        assert_eq!(class_of(&Method::GET, "/v1/attacks/1/replay"), Class::Light);
    }
}
