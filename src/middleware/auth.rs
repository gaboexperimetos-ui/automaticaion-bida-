//! Authentication middleware — DB-backed session validation.
//!
//! Session tokens are opaque CSPRNG hex strings; only their
//! `SHA256(SESSION_SECRET:token)` hash is stored in the SQLite `sessions`
//! table. [`require_auth_state`] rejects missing, malformed, unknown, or
//! expired sessions. Cookies carry HttpOnly, SameSite=Strict flags, plus
//! `Secure` when served over HTTPS (see [`make_session_cookie_with_flags`]).
//!
//! Legacy transition rows (pre-Mission-11 raw UUID ids) are still accepted
//! via raw-value fallback lookup. New logins always mint hashed rows.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::models::Session;
use crate::AppState;

/// Session cookie name
pub const SESSION_COOKIE: &str = "session_id";

/// Maximum session age (24 hours)
pub const SESSION_MAX_AGE: u64 = 24 * 60 * 60; // seconds

/// Create a session cookie value (plain-HTTP variant: localhost / Tailscale).
///
/// Delegates to [`make_session_cookie_with_flags`] with `secure = false`.
pub fn make_session_cookie(session_id: &str) -> String {
    make_session_cookie_with_flags(session_id, false)
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

/// Create a cookie deletion string (plain-HTTP variant).
///
/// Delegates to [`clear_session_cookie_with_flags`] with `secure = false`.
pub fn clear_session_cookie() -> String {
    clear_session_cookie_with_flags(false)
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

/// Middleware: reject requests whose session cookie is missing, malformed,
/// unknown, or expired (DB-backed).
///
/// Lookup order for transition compatibility: `SHA256(secret:token)` first,
/// then the raw cookie value (legacy pre-Mission-11 rows store the raw
/// UUID). Expiry is enforced via [`Session::is_valid`]; expired rows are
/// deleted best-effort on encounter. Never logs tokens or cookie values.
///
/// Call sites use `from_fn_with_state(state, require_auth_state)` so the
/// middleware receives `State<AppState>`.
pub async fn require_auth_state(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> axum::response::Response {
    let path = req.uri().path().to_string();

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

    if !session.is_valid() {
        // Best-effort cleanup of the expired row; ignore the result.
        let _ = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&session.id)
            .execute(&state.db)
            .await;
        return auth_rejection(&path, "Invalid session");
    }

    next.run(req).await
}
