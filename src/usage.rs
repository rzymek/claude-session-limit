//! Fetching the 5-hour session usage using the Claude CLI's OAuth login.

use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Debug, Clone)]
pub struct Usage {
    /// Percentage (0-100) of the 5h session limit used.
    pub utilization: f64,
    /// When the current 5h window resets; `None` if no session is active.
    pub resets_at: Option<DateTime<Local>>,
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

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    utilization: Option<f64>,
    resets_at: Option<String>,
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
    let body: UsageResponse = match resp {
        Ok(r) => r.into_json().map_err(|_| "Unexpected response".to_string())?,
        Err(ureq::Error::Status(401, _)) => return Err("Token expired - run claude".into()),
        Err(ureq::Error::Status(429, _)) => return Err("Rate limited".into()),
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
        Err(_) => return Err("Network error".into()),
    };
    let w = body.five_hour.ok_or("No 5h usage data")?;
    Ok(Usage {
        utilization: w.utilization.unwrap_or(0.0),
        resets_at: w
            .resets_at
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| t.with_timezone(&Local)),
    })
}
