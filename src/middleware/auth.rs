//! Authentication middleware — DB-backed session validation.
//!
//! Session tokens are opaque CSPRNG hex strings; only their
//! `SHA256(SESSION_SECRET:token)` hash is stored in the SQLite `sessions`
//! table. [`require_auth_state`] rejects missing, malformed, unknown, or
//! expired sessions — expiry is the row's own `expires_at` plus a hard
//! [`SESSION_TTL_DAYS`] ceiling. Cookies carry HttpOnly, SameSite=Strict
//! flags, plus `Secure` when the request arrived over HTTPS (see
//! [`headers_indicate_https`], [`make_session_cookie_for_transport`]).
//!
//! Legacy transition rows (pre-Mission-11 raw UUID ids) are still accepted
//! via raw-value fallback lookup. New logins always mint hashed rows.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Uri},
    middleware::Next,
};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::models::Session;
use crate::AppState;

/// Session cookie name
pub const SESSION_COOKIE: &str = "session_id";

/// Maximum session age (24 hours) — cookie `Max-Age` **and** the `expires_at`
/// written at login.
///
/// This is the *sliding* lifetime: every login mints a fresh row whose
/// `expires_at` is `now + SESSION_MAX_AGE`. It is deliberately kept at 24 h
/// (the cookie `Max-Age` is part of the public contract of the login
/// response and is asserted byte-for-byte by `tests/security.rs`), so it is
/// NOT the same knob as [`SESSION_TTL_DAYS`].
pub const SESSION_MAX_AGE: u64 = 24 * 60 * 60; // seconds

/// Hard ceiling for the lifetime of a session, in days (30).
///
/// A row in `sessions` is accepted only while **both** conditions hold:
///
/// 1. `expires_at` is still in the future (written at login, see
///    [`SESSION_MAX_AGE`]), and
/// 2. `created_at` is not older than this ceiling.
///
/// The second condition is defence in depth: `expires_at` is a plain text
/// column that any future migration, restore from backup or manual `UPDATE`
/// could push arbitrarily far into the future (the pre-Mission-11 rows in
/// production still carry `expires_at = 2999-01-01`). With this ceiling a
/// row cannot be honoured for more than 30 days after it was created, no
/// matter what `expires_at` says.
///
/// The `sessions` table already has the `expires_at` column (see
/// `migrations/20240101000000_init.sql`), so no migration is required.
pub const SESSION_TTL_DAYS: u64 = 30;

/// [`SESSION_TTL_DAYS`] expressed in seconds.
pub const SESSION_TTL_SECS: u64 = SESSION_TTL_DAYS * 24 * 60 * 60;

/// Create a session cookie value (plain-HTTP variant: localhost / Tailscale).
///
/// Delegates to [`make_session_cookie_with_flags`] with `secure = false`.
///
/// The `Secure` attribute must only be added when the request really came
/// over HTTPS (see [`headers_indicate_https`]): a `Secure` cookie is not
/// sent back by the browser over plain HTTP, so adding it unconditionally
/// would break every login on the HTTP deployment.
pub fn make_session_cookie(session_id: &str) -> String {
    make_session_cookie_with_flags(session_id, false)
}

/// Create a cookie deletion string (plain-HTTP variant).
///
/// Delegates to [`clear_session_cookie_with_flags`] with `secure = false`.
pub fn clear_session_cookie() -> String {
    clear_session_cookie_with_flags(false)
}

/// Create a session cookie value, optionally adding the `Secure` attribute.
///
/// Pass `secure = true` when serving over HTTPS; the current deployment is
/// plain HTTP over Tailscale, where a `Secure` cookie would never be sent
/// back by the browser and would silently break auth — hence opt-in rather
/// than default. (Mission 10 fix: `Secure` was previously inexpressible;
/// `docs/SECURITY.md` claims it "when HTTPS available".)
pub fn make_session_cookie_with_flags(session_id: &str, secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        SESSION_COOKIE, session_id, SESSION_MAX_AGE, secure_attr
    )
}

/// Create a cookie deletion string, optionally adding the `Secure` attribute.
///
/// The flags must mirror the `Set-Cookie` that created the session cookie,
/// otherwise the browser may keep the old cookie. (Mission 10 fix: the
/// previous value was missing the `=` after the cookie name, producing
/// `session_id; Path=...`, which browsers do not reliably treat as a
/// deletion for the `session_id` cookie.)
pub fn clear_session_cookie_with_flags(secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}",
        SESSION_COOKIE, secure_attr
    )
}

/// Does the request headers say the client-facing connection was HTTPS?
///
/// `X-Forwarded-Proto` is the header a TLS-terminating reverse proxy
/// (Caddy, nginx, Tailscale Serve…) sets, and it is the only way to know
/// the *client-facing* scheme once TLS is terminated in front of the app.
///
/// The header is client-controllable, so it must not be trusted for anything
/// else: here it only decides whether the cookie we hand out carries
/// `Secure`, and a client that spoofs it can at worst break its own
/// session (the browser refuses to send a `Secure` cookie back over plain
/// HTTP) — never weaken the server side.
pub fn headers_indicate_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        // Left-most entry is the client-facing protocol (RFC 7239 §1).
        .map(|v| {
            v.split(',')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("https")
        })
        .unwrap_or(false)
}

/// Was this request carried over HTTPS?
///
/// Two signals are combined:
/// 1. the `X-Forwarded-Proto` header (see [`headers_indicate_https`]), and
/// 2. the scheme of the request URI, present when the request target is
///    absolute-form (a proxy forwarding the full URL).
///
/// Both the cookie that is *set* on login and the one that *clears* it must
/// be built with this same function, so the deletion cookie always mirrors
/// the creation cookie (otherwise the browser may keep the old one).
pub fn transport_is_https(headers: &HeaderMap, uri: &Uri) -> bool {
    headers_indicate_https(headers)
        || uri
            .scheme_str()
            .map(|s| s.eq_ignore_ascii_case("https"))
            .unwrap_or(false)
}

/// Did the request reach the server over HTTPS?
///
/// Convenience wrapper over [`transport_is_https`] for handlers that hold the
/// whole request.
pub fn request_is_https<B>(req: &Request<B>) -> bool {
    transport_is_https(req.headers(), req.uri())
}

/// Create a session cookie value whose `Secure` attribute follows the
/// transport of the request.
///
/// Use this on the login path: the `Secure` flag is then correct whether the
/// user reaches the app over plain HTTP (localhost, Tailscale) or over HTTPS
/// (reverse proxy terminating TLS).
pub fn make_session_cookie_for_request<B>(session_id: &str, req: &Request<B>) -> String {
    make_session_cookie_with_flags(session_id, request_is_https(req))
}

/// Create a session cookie value whose `Secure` attribute follows the
/// transport of the request.
///
/// Same as [`make_session_cookie_for_request`] but for handlers that hold
/// the headers and the URI separately (the login handler consumes the body
/// as a form, so it cannot also take the whole [`Request`]).
pub fn make_session_cookie_for_transport(
    session_id: &str,
    headers: &HeaderMap,
    uri: &Uri,
) -> String {
    make_session_cookie_with_flags(session_id, transport_is_https(headers, uri))
}

/// Create a cookie deletion string whose `Secure` attribute follows the
/// transport of the request.
///
/// The flags must mirror the ones used when the cookie was created,
/// otherwise the browser may keep the old cookie.
pub fn clear_session_cookie_for_request<B>(req: &Request<B>) -> String {
    clear_session_cookie_with_flags(request_is_https(req))
}

/// Create a cookie deletion string whose `Secure` attribute follows the
/// transport of the request.
pub fn clear_session_cookie_for_transport(headers: &HeaderMap, uri: &Uri) -> String {
    clear_session_cookie_with_flags(transport_is_https(headers, uri))
}

/// Constant-time string comparison for secrets (API keys, tokens).
///
/// Compares every byte so the runtime does not depend on the position of the
/// first difference, unlike `==` which short-circuits. Intended for key
/// comparison such as `mcp_server::server::check_api_key`, which currently
/// uses `==` (see Mission 10 report) — wire this in when that call site can
/// be touched.
///
/// Note: like all byte-wise comparisons, the *length* of the inputs is not
/// hidden (length mismatch returns `false` immediately). Stored API keys are
/// fixed-length hex (`openssl rand -hex 32`), so this leaks nothing useful.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Mission 11: DB-backed sessions with hashed opaque tokens.
// ---------------------------------------------------------------------------
//
// Design notes (see Mission 11 report):
// - Tokens are 32 CSPRNG bytes, hex-encoded (64 chars). They are opaque:
//   no UUID parsing is required for new tokens.
// - Only `SHA256(SESSION_SECRET || ":" || raw_token)` (hex) is stored in
//   `sessions.id` (existing TEXT PK — no schema change needed). A DB leak
//   alone therefore does not allow impersonation: the raw token never leaves
//   the server except inside the HttpOnly session cookie. This is the
//   technical justification for using SESSION_SECRET (keyed hash, HMAC-ish
//   via domain-separated SHA-256; not a full HMAC construction).
// - Consequence: rotating SESSION_SECRET invalidates ALL sessions (stored
//   ids can no longer be recomputed). This is acceptable and documented.
// - Transition: rows created before Mission 11 store the raw UUID. The
//   validator accepts both: it looks up the session by the hashed id first,
//   then falls back to the raw cookie value (legacy rows). New logins always
//   store the hash. Logout deletes both candidates.
// - CSRF: SameSite=Strict on the session cookie is the primary defense
//   (modern browsers withhold the cookie on cross-site requests, including
//   top-level GET navigation). Additionally, POST /login validates the
//   Origin/Referer header when present (see handlers::auth). No CSRF token
//   system is built: for a single-user app over plain HTTP on Tailscale,
//   Strict + Origin check is proportionate.

/// Generate a new opaque session token: 32 CSPRNG bytes, hex-encoded.
pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(64);
    for b in bytes {
        // Manual hex: no `hex` crate in dependencies, keep it dependency-free.
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Compute the session row id for a raw token: `SHA256(secret || ":" || raw)`.
///
/// The `":"` separator is domain separation so `secret || raw` cannot be
/// ambiguous across different secret/token splits.
pub fn hash_session_token(raw_token: &str, secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b":");
    h.update(raw_token.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Plausibility check for a presented session token.
///
/// Accepts new opaque hex tokens (64 chars) and legacy UUID rows (36 chars
/// with dashes) during the transition. Anything else (empty, overlong, or
/// containing cookie-special characters) is rejected before touching the DB.
/// The DB lookup itself is authoritative; this only filters garbage.
pub fn token_format_ok(token: &str) -> bool {
    (32..=128).contains(&token.len())
        && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Extract the raw `session_id` cookie value from a request, if present.
pub fn extract_session_cookie(req: &Request<Body>) -> Option<String> {
    req.headers().get("cookie")?.to_str().ok().and_then(|c| {
        c.split(';').find_map(|cookie| {
            let cookie = cookie.trim();
            if cookie.starts_with("session_id=") {
                Some(cookie["session_id=".len()..].trim().to_string())
            } else {
                None
            }
        })
    })
}

/// Build the rejection response: 401 on `/api/*`, else 302 to `/login`.
fn auth_rejection(path: &str, body: &'static str) -> axum::response::Response {
    if path.starts_with("/api/") {
        axum::response::Response::builder()
            .status(401)
            .body(Body::from(body))
            .unwrap()
    } else {
        axum::response::Response::builder()
            .status(302)
            .header("Location", "/login")
            .body(Body::empty())
            .unwrap()
    }
}

/// Parse a timestamp as stored in the `sessions` table.
///
/// Two formats are in use:
///
/// - RFC 3339 with an explicit offset (`2026-10-13T02:02:14.916856151+00:00`),
///   written by the login handler for `expires_at`;
/// - SQLite's `datetime('now')` (`2026-10-12 02:02:14`, **UTC**), the column
///   default for `created_at`.
///
/// `None` when the value matches none of them.
fn parse_session_timestamp(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
            // SQLite's `datetime('now')` is UTC.
            return Some(naive.and_utc());
        }
    }
    None
}

/// Is this session row past its expiry?
///
/// Two independent limits apply, and the row must satisfy **both** to be
/// usable:
///
/// 1. `expires_at` — the per-row deadline written at login (see
///    [`SESSION_MAX_AGE`]) — must still be in the future;
/// 2. the row must not be older than [`SESSION_TTL_DAYS`] (a hard ceiling
///    measured from `created_at`), so no `expires_at` value can push a
///    session's lifetime beyond the TTL.
///
/// An `expires_at` that cannot be parsed counts as expired (fail closed).
/// A `created_at` that cannot be parsed only disables limit 2: limit 1 still
/// applies, so an unreadable timestamp never grants access.
pub fn session_is_expired(session: &Session, now: chrono::DateTime<chrono::Utc>) -> bool {
    // Limit 1 — per-row expiry.
    match parse_session_timestamp(&session.expires_at) {
        Some(expires_at) => {
            if expires_at <= now {
                return true;
            }
        }
        // Unreadable expiry: fail closed.
        None => return true,
    }

    // Limit 2 — hard TTL ceiling, counted from the row's creation.
    if let Some(created_at) = parse_session_timestamp(&session.created_at) {
        let ttl = chrono::Duration::seconds(SESSION_TTL_SECS as i64);
        if let Some(deadline) = created_at.checked_add_signed(ttl) {
            if deadline <= now {
                return true;
            }
        }
    }

    false
}

/// Middleware: reject requests whose session cookie is missing, malformed,
/// unknown, or expired (DB-backed).
///
/// Lookup order for transition compatibility: `SHA256(secret:token)` first,
/// then the raw cookie value (legacy pre-Mission-11 rows store the raw
/// UUID). Expiry is enforced by [`session_is_expired`] — the row's own
/// `expires_at` plus the absolute [`SESSION_TTL_DAYS`] ceiling; expired rows
/// are deleted best-effort on encounter. Never logs tokens or cookie values.
///
/// Call sites use `from_fn_with_state(state, require_auth_state)` so the
/// middleware receives `State<AppState>`.
pub async fn require_auth_state(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> axum::response::Response {
    let path = req.uri().path().to_string();

    // Bypass de autenticacion cuando AUTH_DISABLED=true (modelo "sin login").
    // No elimina las rutas de login/logout; siguen disponibles.
    if state.config.auth_disabled {
        return next.run(req).await;
    }

    let raw = match extract_session_cookie(&req) {
        Some(v) => v,
        None => return auth_rejection(&path, "Unauthorized"),
    };

    if !token_format_ok(&raw) {
        return auth_rejection(&path, "Invalid session");
    }

    let hashed = hash_session_token(&raw, &state.config.session_secret);

    // Hashed id first (new rows), raw value fallback (legacy UUID rows).
    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ? OR id = ?")
        .bind(&hashed)
        .bind(&raw)
        .fetch_optional(&state.db)
        .await;

    let session = match session {
        Ok(Some(s)) => s,
        // Missing row OR transient DB error: fail closed either way.
        // (A DB outage locks users out rather than letting them in.)
        _ => return auth_rejection(&path, "Invalid session"),
    };

    if session_is_expired(&session, chrono::Utc::now()) {
        // Best-effort cleanup of the expired row; ignore the result.
        let _ = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&session.id)
            .execute(&state.db)
            .await;
        return auth_rejection(&path, "Invalid session");
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(created_at: &str, expires_at: &str) -> crate::models::Session {
        crate::models::Session {
            id: "fictitious-session-id".to_string(),
            user_id: 1,
            created_at: created_at.to_string(),
            expires_at: expires_at.to_string(),
            user_agent: None,
        }
    }

    /// RFC 3339 with an explicit offset — what the login handler writes.
    const RFC3339_FUTURE: &str = "2999-01-01T00:00:00+00:00";
    const RFC3339_PAST: &str = "2000-01-01T00:00:00+00:00";
    /// SQLite's `datetime('now')` — the `created_at` column default (UTC).
    const SQLITE_NOW: &str = "2026-10-19 19:20:16";

    #[test]
    fn parses_both_timestamp_formats() {
        // RFC 3339 with an explicit offset — what the login handler writes.
        assert_eq!(
            parse_session_timestamp(RFC3339_FUTURE),
            Some(
                chrono::DateTime::parse_from_rfc3339(RFC3339_FUTURE)
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            )
        );
        // SQLite format is read as UTC.
        let sqlite = parse_session_timestamp(SQLITE_NOW).expect("sqlite format must parse");
        assert_eq!(sqlite.to_rfc3339(), "2026-10-19T19:20:16+00:00");
        assert!(parse_session_timestamp("").is_none());
        assert!(parse_session_timestamp("not a timestamp").is_none());
    }

    #[test]
    fn fresh_session_is_not_expired() {
        let now = chrono::Utc::now();
        let expires = (now + chrono::Duration::hours(1)).to_rfc3339();
        let created = now.to_rfc3339();
        assert!(!session_is_expired(&session(&created, &expires), now));
    }

    #[test]
    fn session_past_its_expiry_is_expired() {
        let now = chrono::Utc::now();
        let created = (now - chrono::Duration::hours(2)).to_rfc3339();
        assert!(session_is_expired(&session(&created, RFC3339_PAST), now));
    }

    #[test]
    fn unreadable_expiry_fails_closed() {
        let now = chrono::Utc::now();
        assert!(session_is_expired(&session(SQLITE_NOW, "corrupted"), now));
    }

    #[test]
    fn ttl_ceiling_expires_old_sessions_even_with_far_future_expiry() {
        let now = chrono::Utc::now();
        // Created 40 days ago, but `expires_at` was pushed to the year 2999.
        let created = (now - chrono::Duration::days(40)).to_rfc3339();
        assert!(session_is_expired(&session(&created, RFC3339_FUTURE), now));
        // Within the TTL the very same row is still valid.
        let created = (now - chrono::Duration::days(10)).to_rfc3339();
        assert!(!session_is_expired(&session(&created, RFC3339_FUTURE), now));
    }

    #[test]
    fn sqlite_created_at_counts_towards_the_ttl() {
        let now = chrono::Utc::now();
        // `created_at` in SQLite format, 40 days old → past the ceiling.
        let old = (now - chrono::Duration::days(40))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert!(session_is_expired(&session(&old, RFC3339_FUTURE), now));
    }

    #[test]
    fn x_forwarded_proto_selects_the_secure_flag() {
        let mut headers = axum::http::HeaderMap::new();
        assert!(!headers_indicate_https(&headers));
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(headers_indicate_https(&headers));
        // Left-most entry is the client-facing protocol.
        headers.insert("x-forwarded-proto", "https, http".parse().unwrap());
        assert!(headers_indicate_https(&headers));
        headers.insert("x-forwarded-proto", "http".parse().unwrap());
        assert!(!headers_indicate_https(&headers));
    }

    #[test]
    fn request_scheme_is_https_too() {
        let req = axum::http::Request::builder()
            .uri("https://app.example/login")
            .body(())
            .unwrap();
        assert!(request_is_https(&req));
        let req = axum::http::Request::builder()
            .uri("http://app.example/login")
            .body(())
            .unwrap();
        assert!(!request_is_https(&req));
    }

    #[test]
    fn secure_flag_follows_the_transport() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        let https_uri = axum::http::Uri::from_static("https://app.example/login");
        let secure = make_session_cookie_for_transport("fictitious-test-id", &headers, &https_uri);
        assert!(secure.contains("; Secure"), "{secure}");
        assert!(secure.contains("HttpOnly"), "{secure}");
        assert!(secure.contains("SameSite=Strict"), "{secure}");
        assert!(secure.contains("Path=/"), "{secure}");

        // No HTTPS signal at all -> plain cookie (the HTTP deployment keeps working).
        let http_uri = axum::http::Uri::from_static("http://app.example/login");
        let plain = make_session_cookie_for_transport(
            "fictitious-test-id",
            &axum::http::HeaderMap::new(),
            &http_uri,
        );
        assert!(!plain.contains("Secure"), "{plain}");
    }
}
