//! Fetching Claude usage limits using the Claude CLI's OAuth login.

use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const SESSION_KEY: &str = "five_hour";

/// One rate-limit window from the usage response (`five_hour`, `seven_day`, ...).
#[derive(Debug, Clone)]
pub struct Limit {
    /// Raw key from the response, e.g. `seven_day_opus`.
    pub key: String,
    /// Percentage (0-100) of the limit used.
    pub utilization: f64,
    /// When the window resets; `None` if not active / not applicable.
    pub resets_at: Option<DateTime<Local>>,
}

impl Limit {
    /// Short human-readable name: `5h`, `7d`, `7d opus`, `extra`, ...
    pub fn label(&self) -> String {
        match self.key.as_str() {
            SESSION_KEY => "5h".into(),
            "seven_day" => "7d".into(),
            k => match k.strip_prefix("seven_day_") {
                Some(rest) => format!("7d {}", rest.replace('_', " ")),
                None => k.trim_end_matches("_usage").replace('_', " "),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Usage {
    /// The 5-hour session window.
    pub session: Limit,
    /// All other limits present in the response (weekly, per-model, extra usage).
    pub others: Vec<Limit>,
    pub fetched_at: DateTime<Local>,
}

#[derive(Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuth>,
}

#[derive(Deserialize)]
struct OAuth {
    #[serde(rename = "accessToken")]
    access_token: String,
    /// Unix epoch milliseconds.
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

fn credentials_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CLAUDE_CREDENTIALS") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
        .unwrap_or_default();
    base.join(".credentials.json")
}

/// Reads the access token the Claude CLI stores. The file is re-read on
/// every call so tokens refreshed by the CLI are picked up. We never refresh
/// the token ourselves, as rotating the refresh token could log the CLI out.
fn read_token() -> Result<String, String> {
    let path = credentials_path();
    let data = std::fs::read_to_string(&path)
        .map_err(|_| "Not logged in - run claude".to_string())?;
    let creds: Credentials =
        serde_json::from_str(&data).map_err(|_| "Bad credentials file".to_string())?;
    let oauth = creds.oauth.ok_or("Not logged in - run claude")?;
    if let Some(exp) = oauth.expires_at {
        if exp <= Utc::now().timestamp_millis() {
            return Err("Token expired - run claude".into());
        }
    }
    Ok(oauth.access_token)
}

/// Collects every top-level object that has a numeric `utilization`, so new
/// or renamed limits show up without code changes.
fn parse(body: &serde_json::Value) -> Result<Usage, String> {
    let obj = body.as_object().ok_or("Unexpected response")?;
    let mut limits: Vec<Limit> = obj
        .iter()
        // Skip turned-off limits (e.g. `extra_usage` with `is_enabled: false`).
        .filter(|(_, v)| v.get("is_enabled").and_then(|e| e.as_bool()) != Some(false))
        .filter_map(|(k, v)| {
            Some(Limit {
                key: k.clone(),
                utilization: v.get("utilization")?.as_f64()?,
                resets_at: v
                    .get("resets_at")
                    .and_then(|s| s.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.with_timezone(&Local)),
            })
        })
        .collect();
    limits.sort_by(|a, b| (a.key.starts_with("extra"), &a.key).cmp(&(b.key.starts_with("extra"), &b.key)));
    let i = limits.iter().position(|l| l.key == SESSION_KEY).ok_or("No 5h usage data")?;
    let session = limits.remove(i);
    Ok(Usage { session, others: limits, fetched_at: Local::now() })
}

pub fn fetch() -> Result<Usage, String> {
    let token = read_token()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent("claude-session-limit/0.1")
        .build();
    let resp = agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call();
    let body: serde_json::Value = match resp {
        Ok(r) => r.into_json().map_err(|_| "Unexpected response".to_string())?,
        Err(ureq::Error::Status(401, _)) => return Err("Token expired - run claude".into()),
        Err(ureq::Error::Status(429, _)) => return Err("Rate limited".into()),
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
        Err(_) => return Err("Network error".into()),
    };
    if std::env::var_os("CLAUDE_USAGE_DEBUG").is_some() {
        eprintln!("{body:#}");
    }
    parse(&body)
}
