//! OpenRouter API client with mock fallback and a persistent budget gate.
//!
//! Budget design (Mission 11):
//! - Every OpenRouter call (live AND mock) goes through
//!   [`check_budget_and_reserve`], which runs `BEGIN IMMEDIATE` (SQLite write
//!   lock), computes the current month's spend from the `usage_metrics`
//!   table, and rejects with [`AiError::BudgetExceeded`] (HTTP 429) WITHOUT
//!   calling the provider when `spend >= config.openrouter_max_cost_usd`.
//! - On success the caller holds a [`BudgetGuard`]; after the provider
//!   responds it MUST call [`BudgetGuard::record`] (INSERT actual usage +
//!   COMMIT) or [`BudgetGuard::abort`] (ROLLBACK). Holding the write lock
//!   across the HTTP call serializes AI requests — a deliberate tradeoff so
//!   parallel requests can never overshoot the cap on this low-traffic app.
//! - Mock mode (no API key) uses the same gate but records 0.0-cost rows
//!   (`estimated = 1`), so history is complete and mock traffic can never
//!   trip the gate by itself. If the month is already exhausted, even mock
//!   calls are rejected — fail-closed.
//! - Monthly spend = `SUM(cost_usd)` over the rows of the current calendar
//!   month (UTC), selected by [`MONTH_FILTER`]: a half-open range on
//!   `created_at`, whose default is `datetime('now')`.
//! - The cost summary exposes `alert = true` once spend reaches 90% of the
//!   budget ([`ALERT_RATIO`]; $1.80 on the default $2.00 budget).
//!
//! Cost honesty (never invent a number):
//! - The cost written to `usage_metrics` is the one the provider reported.
//!   When the provider reports none, the row is stored with `cost_usd = 0.0`
//!   and `estimated = 1`. We do NOT guess a $/token rate: an invented rate
//!   would silently distort the monthly budget.
//! - The REAL token counts are stored in every row, so the true cost of an
//!   `estimated = 1` call can be computed later from the model's published
//!   price without running the call again.
//! - Consequently `total_cost` is a LOWER BOUND of the real spend: calls
//!   with an unknown cost contribute 0.0, never a made-up number.

use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::SqlitePool;
use std::time::Duration;

use crate::config::Config;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "cohere/north-mini-code:free";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Spend ratio that triggers the `alert` flag in [`CostSummary`].
pub const ALERT_RATIO: f64 = 0.9;

/// Returns true when `spend` has reached the alert threshold for `budget`.
pub fn is_alert(spend: f64, budget: f64) -> bool {
    budget > 0.0 && spend >= budget * ALERT_RATIO
}

/// Error type for AI operations.
///
/// `Display` strings are safe to log server-side. They must NOT be sent to
/// clients verbatim for internal variants — use [`AiError::client_message`]
/// for responses and [`AiError::log_detail`] for logs (never user content,
/// only provider statuses and byte/char lengths, which callers attach).
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("OpenRouter API error: {0}")]
    Api(String),
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("Budget exceeded: spent ${spent} of ${budget}")]
    BudgetExceeded { spent: f64, budget: f64 },
    #[error("No API key configured")]
    NoApiKey,
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("AI service temporarily unavailable")]
    Db(String),
}

impl From<sqlx::Error> for AiError {
    fn from(e: sqlx::Error) -> Self {
        AiError::Db(e.to_string())
    }
}

impl AiError {
    /// HTTP status for this error. Budget exhaustion maps to 429 so clients
    /// (and tests) can distinguish "over budget" from upstream failures.
    pub fn status_code(&self) -> axum::http::StatusCode {
        match self {
            AiError::BudgetExceeded { .. } => axum::http::StatusCode::TOO_MANY_REQUESTS,
            AiError::Db(_) | AiError::NoApiKey => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            AiError::Api(_) | AiError::Http(_) | AiError::Parse(_) => {
                axum::http::StatusCode::BAD_GATEWAY
            }
        }
    }

    /// Safe client-facing message. Contains no internals, no user content.
    pub fn client_message(&self) -> String {
        match self {
            AiError::BudgetExceeded { spent, budget } => format!(
                "Monthly AI budget exhausted: spent ${:.2} of ${:.2}. Try again next month.",
                spent, budget
            ),
            AiError::Api(_) => "AI provider error. Please try again later.".to_string(),
            AiError::Http(_) => "AI request failed. Please try again later.".to_string(),
            AiError::NoApiKey => "AI assistant is not configured.".to_string(),
            AiError::Parse(_) => {
                "AI response could not be processed. Please try again later.".to_string()
            }
            AiError::Db(_) => {
                "AI service temporarily unavailable. Please try again later.".to_string()
            }
        }
    }

    /// Server-side log detail. Never includes user message/content — callers
    /// log input lengths separately. May include provider status text.
    pub fn log_detail(&self) -> String {
        match self {
            AiError::Db(_) => "database error".to_string(),
            other => other.to_string(),
        }
    }
}

/// Cost tracker for OpenRouter usage (per-`AiAssistant`-instance, in-memory).
///
/// Kept for backward compatibility and fast local accounting. The
/// authoritative spend is the `usage_metrics` table (see [`month_spend`]) —
/// the in-memory tracker resets every request because handlers build a new
/// `AiAssistant` per call, so it must never be used as a budget gate.
#[derive(Debug, Clone, Default)]
pub struct CostTracker {
    /// Total cost in USD
    pub total_cost: f64,
    /// Budget limit in USD
    pub budget_limit: f64,
    /// Number of API requests made
    pub requests_count: u64,
}

impl CostTracker {
    pub fn new(budget_limit: f64) -> Self {
        Self {
            total_cost: 0.0,
            budget_limit,
            requests_count: 0,
        }
    }

    /// Check if we're within budget. Returns true if can proceed.
    pub fn can_proceed(&self) -> bool {
        self.total_cost < self.budget_limit
    }

    /// Record a request and check budget. Returns Err if exceeded.
    ///
    /// A budget of `0.0` means "no AI spend allowed": the request is rejected
    /// even when it costs nothing, which keeps this tracker consistent with
    /// [`CostTracker::can_proceed`] (also `false` when the budget is 0.0).
    pub fn record_request(&mut self, cost: f64) -> Result<(), AiError> {
        self.requests_count += 1;
        self.total_cost += cost;
        if self.budget_limit <= 0.0 || self.total_cost > self.budget_limit {
            Err(AiError::BudgetExceeded {
                spent: self.total_cost,
                budget: self.budget_limit,
            })
        } else {
            Ok(())
        }
    }

    pub fn summary(&self) -> CostSummary {
        CostSummary {
            total_cost: self.total_cost,
            budget_limit: self.budget_limit,
            requests_count: self.requests_count,
            alert: is_alert(self.total_cost, self.budget_limit),
        }
    }
}

/// Cost tracking summary for API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub total_cost: f64,
    pub budget_limit: f64,
    pub requests_count: u64,
    /// True once `total_cost` reaches 90% of `budget_limit`.
    #[serde(default)]
    pub alert: bool,
}

/// Chat message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessage {
    pub role: String, // "user" or "assistant"
    pub content: String,
}

/// OpenRouter request
#[derive(Debug, Serialize)]
struct OrRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
}

/// OpenRouter response
#[derive(Debug, Deserialize)]
struct OrResponse {
    choices: Vec<OrChoice>,
    usage: Option<OrUsage>,
}

#[derive(Debug, Deserialize)]
struct OrChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct OrUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    #[serde(default)]
    cost: Option<f64>,
}

/// Cost stored for a call whose cost the provider did not report.
///
/// We deliberately do NOT keep a $/token price table: an invented (or stale)
/// rate would silently distort the monthly budget. The row is stored with
/// `estimated = 1` and the REAL token counts, so the true cost can be
/// computed later from the model's published price without repeating the
/// call.
const UNKNOWN_COST_USD: f64 = 0.0;

/// Map the provider's `usage` object to the values stored in `usage_metrics`.
///
/// - cost reported by the provider → stored as-is, `estimated = false`;
/// - no cost reported (or no `usage` object at all) → `cost_usd = 0.0` and
///   `estimated = true`; the token counts are still stored whenever the
///   provider sent them.
///
/// Returns `(input_tokens, output_tokens, cost_usd, estimated)`.
fn usage_to_row(usage: Option<&OrUsage>) -> (i64, i64, f64, bool) {
    match usage {
        Some(u) => (
            u.prompt_tokens as i64,
            u.completion_tokens as i64,
            u.cost.unwrap_or(UNKNOWN_COST_USD),
            u.cost.is_none(),
        ),
        None => (0, 0, UNKNOWN_COST_USD, true),
    }
}

// ---------------------------------------------------------------------------
// Persistent usage accounting (`usage_metrics` table)
// ---------------------------------------------------------------------------

/// DDL of the `usage_metrics` table, embedded from the migration that owns
/// it (`migrations/20240101000005_usage_metrics.sql`).
///
/// Single source of truth: the runtime guard and the migration can never
/// drift apart. Both statements are idempotent (`IF NOT EXISTS`).
const USAGE_METRICS_DDL: &str = include_str!("../../migrations/20240101000005_usage_metrics.sql");

/// SQL fragment selecting the rows of the current calendar month (UTC).
///
/// Half-open range on `created_at`: `[start of month, start of next month)`.
/// It is equivalent to `strftime('%Y-%m', created_at) = strftime('%Y-%m',
/// 'now')` but keeps the `created_at` index usable (a function call on the
/// column would defeat it) and it excludes rows dated in the future, which
/// would otherwise be counted as this month's spend.
const MONTH_FILTER: &str = "created_at >= date('now', 'start of month') \
                            AND created_at < date('now', 'start of month', '+1 month')";

/// Ensure the `usage_metrics` table exists.
///
/// The migration creates it in production; this idempotent guard keeps the
/// gate (and tests that build their schema manually) robust when migrations
/// have not run. When the table is already there — the normal case — the
/// migration DDL is NOT executed again, so the schema is never duplicated.
pub async fn ensure_usage_metrics_table(db: &SqlitePool) -> Result<(), AiError> {
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'usage_metrics'",
    )
    .fetch_optional(db)
    .await?;
    if exists.is_some() {
        return Ok(());
    }
    sqlx::query(USAGE_METRICS_DDL).execute(db).await?;
    Ok(())
}

/// Current month's total AI spend in USD (0.0 when no usage recorded).
pub async fn month_spend(db: &SqlitePool) -> Result<f64, AiError> {
    ensure_usage_metrics_table(db).await?;
    let sql = format!(
        "SELECT SUM(cost_usd) FROM usage_metrics WHERE {}",
        MONTH_FILTER
    );
    let sum: Option<f64> = sqlx::query_scalar::<_, Option<f64>>(&sql)
        .fetch_one(db)
        .await?;
    Ok(sum.unwrap_or(0.0))
}

/// Current month's number of recorded AI calls.
pub async fn month_request_count(db: &SqlitePool) -> Result<u64, AiError> {
    ensure_usage_metrics_table(db).await?;
    let sql = format!("SELECT COUNT(*) FROM usage_metrics WHERE {}", MONTH_FILTER);
    let count: i64 = sqlx::query_scalar::<_, i64>(&sql).fetch_one(db).await?;
    Ok(count.max(0) as u64)
}

/// Persistent cost summary: month spend + month request count from the DB.
///
/// This is what `GET /api/ai/cost` serves, so spend survives restarts and
/// `AiAssistant` re-creation (handlers build a new instance per request).
///
/// Honesty note: `total_cost` sums the costs the provider reported. Rows
/// stored with `estimated = 1` (mock calls, or calls whose cost the provider
/// did not report) contribute 0.0 because their real cost is unknown — the
/// figure is a lower bound, never an invented number.
pub async fn persistent_summary(db: &SqlitePool, config: &Config) -> Result<CostSummary, AiError> {
    ensure_usage_metrics_table(db).await?;
    let total_cost = month_spend(db).await?;
    let requests_count = month_request_count(db).await?;
    let budget_limit = config.openrouter_max_cost_usd;
    Ok(CostSummary {
        total_cost,
        budget_limit,
        requests_count,
        alert: is_alert(total_cost, budget_limit),
    })
}

/// One recorded AI call, written by [`BudgetGuard::record`].
#[derive(Debug, Clone)]
pub struct UsageEntry {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    /// True when the cost did not come from the provider (mock mode, or the
    /// provider response carried no `cost` field). Those rows store
    /// `cost_usd = 0.0` plus the real token counts, so the cost can be
    /// estimated later without repeating the call.
    pub estimated: bool,
    /// API endpoint that originated the call (`chat`, `summary`, ...).
    pub endpoint: String,
}

/// Guard for one gated AI call. Holds a dedicated SQLite connection with an
/// open `BEGIN IMMEDIATE` transaction (write lock) from the budget check
/// until [`BudgetGuard::record`] (COMMIT) or [`BudgetGuard::abort`]
/// (ROLLBACK).
///
/// The guard MUST be consumed by `record` or `abort`; dropping it without
/// either returns the connection to the pool with an open transaction.
pub struct BudgetGuard {
    conn: Option<PoolConnection<sqlx::Sqlite>>,
    spend_at_check: f64,
    budget: f64,
}

impl std::fmt::Debug for BudgetGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetGuard")
            .field("spend_at_check", &self.spend_at_check)
            .field("budget", &self.budget)
            .field("open", &self.conn.is_some())
            .finish()
    }
}

impl BudgetGuard {
    /// Month spend observed inside the locked budget check.
    pub fn spend_at_check(&self) -> f64 {
        self.spend_at_check
    }

    /// Budget limit the check enforced.
    pub fn budget(&self) -> f64 {
        self.budget
    }

    /// INSERT the call's actual usage and COMMIT. Consumes the guard.
    pub async fn record(mut self, entry: &UsageEntry) -> Result<(), AiError> {
        let mut conn = self.conn.take().expect("BudgetGuard already consumed");
        let result = async {
            sqlx::query(
                "INSERT INTO usage_metrics
                 (model, input_tokens, output_tokens, cost_usd, estimated, endpoint)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&entry.model)
            .bind(entry.input_tokens)
            .bind(entry.output_tokens)
            .bind(entry.cost_usd)
            .bind(if entry.estimated { 1 } else { 0 })
            .bind(&entry.endpoint)
            .execute(&mut *conn)
            .await?;
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok::<(), AiError>(())
        }
        .await;
        if result.is_err() {
            // Best-effort rollback so the pooled connection is left clean.
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        }
        result
    }

    /// ROLLBACK without recording (e.g. provider call failed). Consumes the guard.
    pub async fn abort(mut self) -> Result<(), AiError> {
        let mut conn = self.conn.take().expect("BudgetGuard already consumed");
        sqlx::query("ROLLBACK").execute(&mut *conn).await?;
        Ok(())
    }
}

/// Persistent budget gate: `BEGIN IMMEDIATE`, then compute the month's spend;
/// reject with [`AiError::BudgetExceeded`] (no provider call) when
/// `spend >= config.openrouter_max_cost_usd`, else return a [`BudgetGuard`]
/// holding the write lock until the usage is recorded.
///
/// Concurrent callers serialize on the write lock, so the check-then-insert
/// sequence is atomic and total spend can never overshoot the cap.
///
/// Edge cases of the comparison `spend >= budget`:
/// - budget 2.0, spend 0.0 → allowed (the normal fresh-month case);
/// - budget 0.0 (no spend allowed), spend 0.0 → BLOCKED, because `0.0 >= 0.0`;
/// - spend exactly at the budget → blocked (the cap is never exceeded).
pub async fn check_budget_and_reserve(
    db: &SqlitePool,
    config: &Config,
) -> Result<BudgetGuard, AiError> {
    ensure_usage_metrics_table(db).await?;
    let mut conn = db.acquire().await?;
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&mut *conn)
        .await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    let sql = format!(
        "SELECT SUM(cost_usd) FROM usage_metrics WHERE {}",
        MONTH_FILTER
    );
    let spend: Option<f64> = sqlx::query_scalar::<_, Option<f64>>(&sql)
        .fetch_one(&mut *conn)
        .await?;
    let spend = spend.unwrap_or(0.0);
    let budget = config.openrouter_max_cost_usd;
    if spend >= budget {
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        return Err(AiError::BudgetExceeded {
            spent: spend,
            budget,
        });
    }
    Ok(BudgetGuard {
        conn: Some(conn),
        spend_at_check: spend,
        budget,
    })
}

/// Send a chat completion request to OpenRouter (or mock mode).
///
/// `endpoint` names the originating API (`chat`, `summary`, `quiz`,
/// `study-sheet`) and is stored on the usage row. When no API key is
/// configured, returns a mock response starting with `[Modo simulado]` and
/// records a 0.0-cost row (`estimated = 1`).
///
/// The persistent budget gate applies to both paths: an exhausted month
/// rejects before any provider call. `tracker` is updated for backward
/// compatibility but is NOT the gate (it resets per request).
pub async fn chat_completion(
    db: &SqlitePool,
    config: &Config,
    tracker: &mut CostTracker,
    endpoint: &str,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
) -> Result<String, AiError> {
    let model = config.openrouter_model.clone();
    let api_key: Option<String> = match &config.openrouter_api_key {
        Some(k) if !k.is_empty() => Some(k.clone()),
        _ => None,
    };

    // Persistent gate first — uniform for mock and live paths.
    let guard = check_budget_and_reserve(db, config).await?;

    // Mock mode — no API key configured. No provider call, 0-cost row.
    let Some(key) = api_key else {
        let user_msg = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("Hola");
        // Char-boundary-safe truncation (byte slicing can panic on UTF-8).
        let snippet: String = user_msg.chars().take(30).collect();

        let response = format!(
            "[Modo simulado] Entendido: '{}'. Esta es una respuesta de prueba del asistente académico. Cuando configures OPENROUTER_API_KEY, recibirás respuestas de {}.",
            snippet, model
        );

        guard
            .record(&UsageEntry {
                model,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
                estimated: true,
                endpoint: endpoint.to_string(),
            })
            .await?;
        tracker.requests_count += 1;
        return Ok(response);
    };

    // Build request
    let req_body = OrRequest {
        model: model.clone(),
        messages,
        max_tokens: Some(max_tokens),
        temperature: Some(0.7),
    };

    // Make HTTP request (with reqwest — requires network). The budget write
    // lock is held across this call so parallel requests cannot overshoot.
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| AiError::Http(e.to_string()))?;

    let resp = client
        .post(OPENROUTER_URL)
        .header("Authorization", format!("Bearer {}", key))
        .header("HTTP-Referer", "https://study-platform.local")
        .header("X-Title", "Study Platform")
        .json(&req_body)
        .send()
        .await
        .map_err(|e| AiError::Http(e.to_string()));

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let _ = guard.abort().await;
            return Err(e);
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let _ = guard.abort().await;
        if status == 429 {
            return Err(AiError::Api("Rate limited".to_string()));
        }
        return Err(AiError::Api(format!("HTTP {}: {}", status, text)));
    }

    let or_resp: OrResponse = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            let _ = guard.abort().await;
            return Err(AiError::Parse(e.to_string()));
        }
    };

    let response = match or_resp.choices.into_iter().next() {
        Some(c) => c.message.content,
        None => {
            let _ = guard.abort().await;
            return Err(AiError::Api("No choices in response".to_string()));
        }
    };

    // Actual usage: tokens from the provider when present else 0s; cost from
    // the provider when present, else 0.0 with `estimated = true` (no $/token
    // rate is ever invented — see `usage_to_row`).
    let (input_tokens, output_tokens, cost_usd, estimated) = usage_to_row(or_resp.usage.as_ref());

    guard
        .record(&UsageEntry {
            model,
            input_tokens,
            output_tokens,
            cost_usd,
            estimated,
            endpoint: endpoint.to_string(),
        })
        .await?;
    tracker.requests_count += 1;
    tracker.total_cost += cost_usd;

    Ok(response)
}

#[allow(dead_code)]
fn default_model_name() -> &'static str {
    DEFAULT_MODEL
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config with a fixed budget and no API key (mock mode, no network).
    fn test_config(budget_usd: f64) -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 1,
            session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
            db_path: std::path::PathBuf::from(":memory:"),
            notes_dir: std::path::PathBuf::from("/tmp"),
            openrouter_api_key: None,
            openrouter_model: "cohere/north-mini-code:free".to_string(),
            openrouter_max_tokens: 100,
            openrouter_max_cost_usd: budget_usd,
            mikareno_mcp_url: String::new(),
            genoffice_mcp_url: String::new(),
            custom_mcp_api_key: None,
            rust_log: "test".to_string(),
            auth_disabled: false,
            registration_token: None,
            db_max_connections: 4,
            mcp_timeout_ms: 5_000,
        }
    }

    /// In-memory pool with a single connection (SQLite in-memory DBs are
    /// per-connection).
    async fn test_pool() -> sqlx::SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[test]
    fn test_cost_tracker_within_budget() {
        let mut tracker = CostTracker::new(2.0);
        tracker.record_request(0.5).unwrap();
        tracker.record_request(0.5).unwrap();
        assert!(tracker.can_proceed());
        assert_eq!(tracker.summary().total_cost, 1.0);
    }

    #[test]
    fn test_cost_tracker_exceeds_budget() {
        let mut tracker = CostTracker::new(0.001);
        tracker.record_request(0.001).unwrap();
        let result = tracker.record_request(0.001);
        assert!(result.is_err());
        match result.unwrap_err() {
            AiError::BudgetExceeded { spent, budget } => {
                assert!((spent - 0.002).abs() < 0.0001);
                assert!((budget - 0.001).abs() < 0.0001);
            }
            _ => panic!("Expected BudgetExceeded"),
        }
    }

    /// A budget of 0.0 means "no AI spend allowed": the tracker must refuse
    /// the request even when it costs nothing.
    #[test]
    fn test_cost_tracker_zero_budget_is_exhausted() {
        let mut tracker = CostTracker::new(0.0);
        assert!(!tracker.can_proceed());
        let err = tracker.record_request(0.0).unwrap_err();
        assert!(matches!(err, AiError::BudgetExceeded { .. }));
    }

    /// The cost of a call is the one the provider reported. When it reports
    /// none, we store 0.0 + `estimated = 1` and keep the REAL token counts —
    /// no $/token rate is invented.
    #[test]
    fn test_unknown_cost_is_not_invented() {
        let usage = OrUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            cost: Some(0.42),
        };
        assert_eq!(usage_to_row(Some(&usage)), (120, 30, 0.42, false));

        let usage = OrUsage {
            prompt_tokens: 120,
            completion_tokens: 30,
            cost: None,
        };
        assert_eq!(usage_to_row(Some(&usage)), (120, 30, 0.0, true));

        assert_eq!(usage_to_row(None), (0, 0, 0.0, true));
    }

    #[test]
    fn test_alert_threshold() {
        assert!(!is_alert(1.79, 2.0));
        assert!(is_alert(1.80, 2.0));
        assert!(is_alert(2.0, 2.0));
        assert!(!is_alert(0.0, 0.0));
    }

    #[tokio::test]
    async fn test_gate_rejects_when_month_exhausted() {
        let pool = test_pool().await;
        ensure_usage_metrics_table(&pool).await.unwrap();
        let config = test_config(1.0);
        sqlx::query(
            "INSERT INTO usage_metrics (model, cost_usd, endpoint) VALUES ('m', 1.0, 'test')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let err = check_budget_and_reserve(&pool, &config).await.unwrap_err();
        assert!(matches!(err, AiError::BudgetExceeded { .. }));
        assert_eq!(err.status_code(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    /// Fresh month + budget 2.0 → the gate must let the call through.
    #[tokio::test]
    async fn test_gate_allows_fresh_month() {
        let pool = test_pool().await;
        let config = test_config(2.0);
        let guard = check_budget_and_reserve(&pool, &config)
            .await
            .expect("fresh month must be allowed");
        assert_eq!(guard.spend_at_check(), 0.0);
        assert_eq!(guard.budget(), 2.0);
        guard.abort().await.unwrap();
    }

    /// Budget 0.0 (no spend allowed) → the gate must block even with a
    /// 0.0 spend, because `0.0 >= 0.0`.
    #[tokio::test]
    async fn test_gate_blocks_with_zero_budget() {
        let pool = test_pool().await;
        let config = test_config(0.0);
        let err = check_budget_and_reserve(&pool, &config).await.unwrap_err();
        match err {
            AiError::BudgetExceeded { spent, budget } => {
                assert_eq!(spent, 0.0);
                assert_eq!(budget, 0.0);
            }
            other => panic!("Expected BudgetExceeded, got {other:?}"),
        }
    }

    /// `usage_metrics` must always exist: the guard creates it (and its
    /// index) from the migration DDL when the migrations did not run.
    #[tokio::test]
    async fn test_ensure_usage_metrics_table_creates_table_and_index() {
        let pool = test_pool().await;
        let table: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'usage_metrics'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(table.is_none(), "table must not exist before the guard");

        ensure_usage_metrics_table(&pool).await.unwrap();

        let table: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'usage_metrics'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(table.as_deref(), Some("usage_metrics"));

        let index: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_usage_metrics_created_at'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(index.as_deref(), Some("idx_usage_metrics_created_at"));

        // Idempotent: calling it again on the existing table is a no-op.
        ensure_usage_metrics_table(&pool).await.unwrap();
    }

    /// Spend and request count cover the CURRENT calendar month only: rows of
    /// the previous month and rows dated in the future must be ignored.
    #[tokio::test]
    async fn test_month_filter_is_the_calendar_month() {
        let pool = test_pool().await;
        ensure_usage_metrics_table(&pool).await.unwrap();

        for (model, cost, created_at) in [
            ("prev", 5.0, "datetime('now', 'start of month', '-1 day')"),
            ("next", 7.0, "datetime('now', 'start of month', '+1 month')"),
        ] {
            sqlx::query(
                "INSERT INTO usage_metrics (model, cost_usd, estimated, endpoint, created_at)
                 VALUES (?, ?, 0, 'test', ?)",
            )
            .bind(model)
            .bind(cost)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        // Current month (default `created_at`): counts.
        sqlx::query(
            "INSERT INTO usage_metrics (model, cost_usd, estimated, endpoint)
             VALUES ('now', 0.25, 0, 'test')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!((month_spend(&pool).await.unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(month_request_count(&pool).await.unwrap(), 1);
    }
}
