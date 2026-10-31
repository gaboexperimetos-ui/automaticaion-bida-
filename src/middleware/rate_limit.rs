//! Rate limiting middleware — simple in-memory rate limiter per client IP.
//!
//! Limits: 60 requests per minute per IP (general), plus a stricter
//! 10 requests per minute per IP on `POST /login` as brute-force protection
//! (implements the "10 req/min" claim in `docs/SECURITY.md`).
//! Returns 429 on excess.
//! Implementation: thread-safe HashMap with sliding window.
//!
//! EXEMPTION — `POST /api/ai/chat/stream` (SSE): a per-minute request
//! budget is meaningless for a long-lived stream (it would abort a stream
//! mid-flight on the 61st request of the minute), so that route is exempt
//! from the sliding window and is bounded instead by a process-wide cap of
//! [`MAX_CONCURRENT_STREAMS`] simultaneous streams. Over the cap the request
//! is rejected with `429` + `{"error":"Demasiados streams activos"}`.
//! The slot is released when the response body is dropped — i.e. when the
//! stream ends or the client disconnects.
//!
//! KNOWN LIMIT: the client IP prefers the `ConnectInfo` socket address when
//! the router provides it, falling back to `X-Forwarded-For` and then
//! `"unknown"`. `X-Forwarded-For` is client-controlled and therefore
//! spoofable; it is only a fallback for deployments without a trusted proxy.
//! `main.rs` now serves behind `into_make_service_with_connect_info`, so in
//! production the rate limiter always keys on the real socket address; the
//! spoofable headers are only reachable from in-memory test routers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::{
    body::{Body, Bytes, HttpBody},
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use http_body::{Frame, SizeHint};
use serde_json::json;

/// Rate limit: 60 requests per minute (general traffic)
const RATE_LIMIT: usize = 60;
/// Rate limit: 10 requests per minute on POST /login (brute-force protection)
pub const LOGIN_RATE_LIMIT: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// SSE chat endpoint: long-lived, exempt from the per-minute window.
const AI_CHAT_STREAM_PATH: &str = "/api/ai/chat/stream";
/// Maximum number of SSE streams running at the same time (process-wide).
const MAX_CONCURRENT_STREAMS: usize = 4;

/// Number of SSE streams currently in flight (process-wide).
static ACTIVE_STREAMS: AtomicUsize = AtomicUsize::new(0);

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

/// Occupies one slot of the global SSE concurrency cap.
///
/// The slot is released when the permit is dropped, i.e. when the response
/// body is dropped — either because the stream ended or because the client
/// disconnected.
struct StreamPermit {
    counter: &'static AtomicUsize,
}

impl StreamPermit {
    /// Take one slot of the global cap, or `None` when the cap is reached.
    fn acquire() -> Option<Self> {
        let mut current = ACTIVE_STREAMS.load(Ordering::SeqCst);
        loop {
            if current >= MAX_CONCURRENT_STREAMS {
                return None;
            }
            match ACTIVE_STREAMS.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Some(Self {
                        counter: &ACTIVE_STREAMS,
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Response body that keeps a [`StreamPermit`] alive until it is dropped.
struct GuardedBody {
    inner: Body,
    _permit: StreamPermit,
}

impl HttpBody for GuardedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut(); // `Body` and `StreamPermit` are `Unpin`
        HttpBody::poll_frame(std::pin::Pin::new(&mut this.inner), cx)
    }

    fn is_end_stream(&self) -> bool {
        HttpBody::is_end_stream(&self.inner)
    }

    fn size_hint(&self) -> SizeHint {
        HttpBody::size_hint(&self.inner)
    }
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
    // SSE streams are long-lived: the per-minute window would abort them
    // mid-flight, so they are exempt from it and bounded only by a global
    // concurrency cap.
    if req.uri().path() == AI_CHAT_STREAM_PATH {
        return stream_limited(req, next).await;
    }

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

/// Bound the number of simultaneous SSE streams process-wide.
///
/// The per-minute sliding window is skipped for this route (a stream lasts
/// minutes, so a request budget would kill it in flight); the only limit is
/// the global concurrency cap. The slot is held by the response body, so it
/// is released when the stream ends or the client goes away.
async fn stream_limited(req: Request<Body>, next: Next) -> Response {
    let permit = match StreamPermit::acquire() {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "Demasiados streams activos" })),
            )
                .into_response();
        }
    };

    let response = next.run(req).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(GuardedBody {
            inner: body,
            _permit: permit,
        }),
    )
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

    // --- SSE exemption: concurrency cap instead of the per-minute window ---

    /// Serializes the tests that share the process-global SSE counter
    /// (Rust runs tests in parallel threads).
    static STREAM_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Router with only the rate-limit middleware, so the tests exercise the
    /// middleware in isolation (no auth, no DB).
    fn test_app() -> axum::Router {
        axum::Router::new()
            .route(AI_CHAT_STREAM_PATH, axum::routing::post(|| async { "ok" }))
            .route("/health", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(rate_limit_middleware))
    }

    fn stream_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(AI_CHAT_STREAM_PATH)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_path_is_exempt_from_the_per_minute_window() {
        use tower::ServiceExt;

        let _guard = STREAM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let app = test_app();
        // More requests than the 60/min general window allows: the stream
        // route must not be rate limited by it.
        for _ in 0..(RATE_LIMIT + 10) {
            let resp = app.clone().oneshot(stream_request()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            drop(resp); // releases the concurrency slot
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stream_path_rejects_over_the_global_concurrency_cap() {
        use tower::ServiceExt;

        let _guard = STREAM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let app = test_app();
        // Hold the cap: the bodies (and with them the slots) stay alive.
        let mut held = Vec::new();
        for _ in 0..MAX_CONCURRENT_STREAMS {
            let resp = app.clone().oneshot(stream_request()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            held.push(resp);
        }

        // One more stream must be rejected with the documented JSON error.
        let resp = app.clone().oneshot(stream_request()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload["error"].as_str(),
            Some("Demasiados streams activos")
        );

        // Releasing the held bodies frees the slots again.
        drop(held);
        let resp = app.clone().oneshot(stream_request()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_stream_routes_keep_the_per_minute_window() {
        use tower::ServiceExt;

        let app = test_app();
        let mut statuses = Vec::new();
        for _ in 0..(RATE_LIMIT + 1) {
            let req = Request::builder()
                .uri("/health")
                .header("x-forwarded-for", "192.0.2.77")
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            statuses.push(resp.status());
            drop(resp);
        }
        // The first RATE_LIMIT requests pass, the next one is rejected.
        assert_eq!(statuses.len(), RATE_LIMIT + 1);
        assert!(statuses[..RATE_LIMIT].iter().all(|s| *s == StatusCode::OK));
        assert_eq!(statuses[RATE_LIMIT], StatusCode::TOO_MANY_REQUESTS);
    }
}
