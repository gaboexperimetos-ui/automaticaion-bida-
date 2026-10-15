//! Rate limiting middleware — simple in-memory rate limiter per client IP.
//!
//! Limits: 60 requests per minute per IP (general), plus a stricter
//! 10 requests per minute per IP on `POST /login` as brute-force protection
//! (implements the "10 req/min" claim in `docs/SECURITY.md`).
//! Returns 429 on excess.
//! Implementation: thread-safe HashMap with sliding window.
//!
//! KNOWN LIMIT: the client IP prefers the `ConnectInfo` socket address when
//! the router provides it, falling back to `X-Forwarded-For` and then
//! `"unknown"`. `X-Forwarded-For` is client-controlled and therefore
//! spoofable; it is only a fallback for deployments without a trusted proxy.
//! Serve behind `into_make_service_with_connect_info` (a `main.rs` change,
//! outside this module's ownership) to make the IP spoof-proof.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::{body::Body, extract::Request, middleware::Next, response::Response};

/// Rate limit: 60 requests per minute (general traffic)
const RATE_LIMIT: usize = 60;
/// Rate limit: 10 requests per minute on POST /login (brute-force protection)
pub const LOGIN_RATE_LIMIT: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Thread-safe request counter per IP
struct RateLimiter {
    inner: std::sync::Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn check(&self, ip: &str) -> bool {
        self.check_with_limit(ip, RATE_LIMIT)
    }

    fn check_with_limit(&self, key: &str, limit: usize) -> bool {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        let window_start = now - RATE_WINDOW;

        let requests = map.entry(key.to_string()).or_default();
        requests.retain(|t| *t > window_start);

        if requests.len() >= limit {
            false
        } else {
            requests.push(now);
            true
        }
    }
}

/// Global rate limiter instance
static RATE_LIMITER: OnceLock<RateLimiter> = OnceLock::new();

fn get_limiter() -> &'static RateLimiter {
    RATE_LIMITER.get_or_init(RateLimiter::new)
}

/// Resolve the client identity for rate limiting.
///
/// Prefers the `ConnectInfo` socket address (set by the server socket, not
/// client-controlled) when the router provides it via
/// `into_make_service_with_connect_info`; falls back to the first
/// `X-Forwarded-For` entry (spoofable — only trustworthy behind a proxy that
/// overwrites it), then to `"unknown"`.
fn client_ip(req: &Request<Body>) -> String {
    if let Some(addr) = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
    {
        return addr.0.ip().to_string();
    }
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next().map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Middleware: rate limit per client IP
pub async fn rate_limit_middleware(req: Request<Body>, next: Next) -> Response {
    let limiter = get_limiter();

    let ip = client_ip(&req);

    // Stricter brute-force bucket for login attempts: separate key space so
    // general traffic neither consumes nor is blocked by the login budget.
    let allowed = if req.method() == axum::http::Method::POST && req.uri().path() == "/login" {
        let key = format!("login:{ip}");
        limiter.check_with_limit(&key, LOGIN_RATE_LIMIT)
    } else {
        limiter.check(&ip)
    };

    if !allowed {
        return axum::response::Response::builder()
            .status(429)
            .header("Retry-After", "60")
            .body(Body::from("Too Many Requests"))
            .unwrap();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_bucket_allows_up_to_limit_then_blocks() {
        let limiter = RateLimiter::new();
        for _ in 0..RATE_LIMIT {
            assert!(limiter.check("10.9.9.1"));
        }
        assert!(!limiter.check("10.9.9.1"));
    }

    #[test]
    fn login_bucket_is_stricter_than_general_bucket() {
        assert!(
            LOGIN_RATE_LIMIT < RATE_LIMIT,
            "login brute-force limit must stay stricter than the general limit"
        );
        let limiter = RateLimiter::new();
        for _ in 0..LOGIN_RATE_LIMIT {
            assert!(limiter.check_with_limit("login:10.9.9.2", LOGIN_RATE_LIMIT));
        }
        assert!(!limiter.check_with_limit("login:10.9.9.2", LOGIN_RATE_LIMIT));
    }

    #[test]
    fn login_and_general_buckets_do_not_share_budget() {
        let limiter = RateLimiter::new();
        // Exhaust the login bucket…
        for _ in 0..LOGIN_RATE_LIMIT {
            assert!(limiter.check_with_limit("login:10.9.9.3", LOGIN_RATE_LIMIT));
        }
        assert!(!limiter.check_with_limit("login:10.9.9.3", LOGIN_RATE_LIMIT));
        // …the general bucket for the same IP must be unaffected.
        assert!(limiter.check("10.9.9.3"));
    }

    #[test]
    fn buckets_are_isolated_per_key() {
        let limiter = RateLimiter::new();
        for _ in 0..RATE_LIMIT {
            assert!(limiter.check("10.9.9.4"));
        }
        assert!(!limiter.check("10.9.9.4"));
        // A different IP gets a fresh budget.
        assert!(limiter.check("10.9.9.5"));
    }

    #[test]
    fn client_ip_prefers_connect_info_over_forwarded_header() {
        use axum::extract::ConnectInfo;

        let body = Body::empty();
        let mut req = Request::new(body);
        req.headers_mut()
            .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 12345))));

        // Spoofed X-Forwarded-For must lose to the socket address.
        assert_eq!(client_ip(&req), "192.0.2.1");
    }

    #[test]
    fn client_ip_falls_back_to_forwarded_header_then_unknown() {
        let mut req = Request::new(Body::empty());
        req.headers_mut().insert(
            "x-forwarded-for",
            "203.0.113.7, 70.41.3.18".parse().unwrap(),
        );
        // First entry wins.
        assert_eq!(client_ip(&req), "203.0.113.7");

        let req = Request::new(Body::empty());
        assert_eq!(client_ip(&req), "unknown");
    }
}
