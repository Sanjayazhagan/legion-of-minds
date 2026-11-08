//! Per-IP request limits, so a few scripted clients can't flood the server.
//! Raids and publishing re-run battles on the CPU, so they get a tighter
//! limit than everything else.

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
    buckets: Mutex<HashMap<(IpAddr, bool), Bucket>>,
}

impl Limiter {
    /// Take one token; on refusal, the seconds until one is free.
    fn take(&self, ip: IpAddr, heavy: bool, per_min: u32) -> Result<(), u64> {
        let now = Instant::now();
        let cap = per_min.max(1) as f64;
        let rate = cap / 60.0;
        let mut map = self.buckets.lock().unwrap();
        if map.len() > 50_000 {
            // Forget clients whose bucket has refilled anyway.
            map.retain(|_, b| b.tokens + now.duration_since(b.at).as_secs_f64() * rate < cap);
        }
        let b = map.entry((ip, heavy)).or_insert(Bucket { tokens: cap, at: now });
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

/// Requests that make the server run a battle or start a sign-in.
fn is_heavy(method: &Method, path: &str) -> bool {
    (method == Method::POST && (path == "/v1/matchmaking" || path.starts_with("/v1/attacks/")))
        || (method == Method::PUT && path == "/v1/me/base")
        || (method == Method::POST && path.starts_with("/v1/auth/") && !path.ends_with("/poll"))
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

pub async fn limit(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let ip = client_ip(&st, &req);
    let heavy = is_heavy(req.method(), req.uri().path());
    let per_min = if heavy { st.cfg.heavy_per_min } else { st.cfg.requests_per_min };
    match st.limiter.take(ip, heavy, per_min) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
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
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..3 {
            assert!(l.take(ip, true, 3).is_ok());
        }
        let wait = l.take(ip, true, 3).unwrap_err();
        assert!((1..=20).contains(&wait), "{wait}");
        // Another client, or the light bucket, is unaffected.
        assert!(l.take("10.0.0.2".parse().unwrap(), true, 3).is_ok());
        assert!(l.take(ip, false, 3).is_ok());
    }

    #[test]
    fn heavy_routes() {
        assert!(is_heavy(&Method::POST, "/v1/matchmaking"));
        assert!(is_heavy(&Method::POST, "/v1/attacks/123"));
        assert!(is_heavy(&Method::PUT, "/v1/me/base"));
        assert!(is_heavy(&Method::POST, "/v1/auth/login"));
        assert!(!is_heavy(&Method::POST, "/v1/auth/logins/1/poll"));
        assert!(!is_heavy(&Method::GET, "/v1/me/base"));
        assert!(!is_heavy(&Method::GET, "/v1/attacks/1/replay"));
    }
}
