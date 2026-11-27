// ChatGPT subscription (Plus / Pro / Business) usage limits.
//
// ChatGPT's consumer plans don't publish a usage meter for chat itself. The
// limits that do exist and are metered are the Codex limits that come with the
// subscription: a 5-hour window and a weekly window, each reported as a
// percentage used. The Codex app, CLI and IDE extension show the same numbers
// under /status.
//
// Source 1 (live): the same endpoint the Codex clients call,
//   GET https://chatgpt.com/backend-api/wham/usage
// authenticated with the ChatGPT sign-in that Codex saved to
// $CODEX_HOME/auth.json (default ~/.codex/auth.json). We only read that file.
// We never refresh the token ourselves: the refresh token rotates on use, so
// refreshing here would sign Codex out. When the token has expired we say so
// and fall back to source 2.
//
// Source 2 (fallback): Codex writes the latest rate-limit snapshot into its
// session logs ($CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl) as
// `token_count` events. That's only as fresh as your last Codex turn, so we
// mark it with `source = "codex_logs"`.
//
// Works on every OS (no Keychain involved).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct CodexWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CodexSnapshot {
    /// "pro", "plus", "business", … as reported by ChatGPT.
    pub plan_type: Option<String>,
    /// The shorter window (5 hours on current plans).
    pub five_hour: Option<CodexWindow>,
    /// The longer window (weekly on current plans).
    pub weekly: Option<CodexWindow>,
    /// "live" (chatgpt.com) or "codex_logs" (last Codex session on disk).
    pub source: Option<String>,
    /// When the numbers were observed (for the log fallback, the log time).
    pub observed_at: Option<DateTime<Utc>>,
    pub fetched_at: Option<DateTime<Utc>>,
    /// True when no Codex sign-in exists at all (feature simply unused).
    pub not_signed_in: bool,
    pub error: Option<String>,
}

pub async fn fetch() -> CodexSnapshot {
    let home = codex_home();
    let now = Utc::now();

    let live_err = match read_auth(&home) {
        Ok(Some(auth)) => match fetch_live(&auth).await {
            Ok(mut s) => {
                s.fetched_at = Some(now);
                return s;
            }
            Err(e) => Some(e),
        },
        Ok(None) => None,
        Err(e) => Some(e),
    };

    let from_logs = latest_from_logs(&home.join("sessions"));
    match (from_logs, live_err) {
        (Some(mut s), err) => {
            s.fetched_at = Some(now);
            // Keep the reason the live call failed so the UI can explain why
            // the numbers may be stale.
            s.error = err;
            s
        }
        (None, Some(e)) => CodexSnapshot {
            fetched_at: Some(now),
            error: Some(e),
            ..Default::default()
        },
        (None, None) => CodexSnapshot {
            fetched_at: Some(now),
            not_signed_in: true,
            error: Some(
                "No ChatGPT sign-in found. Install the Codex app or CLI and sign in with ChatGPT."
                    .into(),
            ),
            ..Default::default()
        },
    }
}

fn codex_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(h);
    }
    dirs::home_dir()
        .map(|h| h.join(".codex"))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

struct Auth {
    access_token: String,
    account_id: Option<String>,
}

/// Ok(None) = no ChatGPT sign-in saved (file missing, or API-key-only login).
fn read_auth(home: &Path) -> Result<Option<Auth>, String> {
    let path = home.join("auth.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {}", path.display(), e)),
    };
    let v: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {}", path.display(), e))?;
    Ok(auth_from_json(&v))
}

fn auth_from_json(v: &Value) -> Option<Auth> {
    let tokens = v.get("tokens")?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(Auth {
        access_token,
        account_id,
    })
}

async fn fetch_live(auth: &Auth) -> Result<CodexSnapshot, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("codex-cli")
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client
        .get(USAGE_URL)
        .bearer_auth(&auth.access_token)
        .header("accept", "application/json")
        .header("OAI-Product-Sku", "codex");
    if let Some(id) = &auth.account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("chatgpt.com: {}", e))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(
            "ChatGPT sign-in has expired. Open Codex (app or CLI) once to refresh it.".into(),
        );
    }
    if !status.is_success() {
        return Err(format!("chatgpt.com usage: HTTP {}", status.as_u16()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("chatgpt.com usage: {}", e))?;
    parse_usage(&body, Utc::now())
        .ok_or_else(|| "chatgpt.com usage: no rate limits in response".into())
}

#[derive(Debug, Deserialize)]
struct WindowPayload {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

/// Parse the /wham/usage response body.
fn parse_usage(body: &Value, now: DateTime<Utc>) -> Option<CodexSnapshot> {
    let rl = body.get("rate_limit")?;
    let mut windows: Vec<CodexWindow> = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|k| rl.get(*k))
        .filter(|w| !w.is_null())
        .filter_map(|w| serde_json::from_value::<WindowPayload>(w.clone()).ok())
        .filter_map(|w| {
            let used = w.used_percent.filter(|p| p.is_finite())?;
            let resets_at = w.reset_at.and_then(epoch_to_utc).or_else(|| {
                w.reset_after_seconds
                    .map(|s| now + ChronoDuration::seconds(s))
            });
            Some(CodexWindow {
                used_percent: used.clamp(0.0, 100.0),
                window_minutes: w.limit_window_seconds.map(|s| (s + 59) / 60),
                resets_at,
            })
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    let (five_hour, weekly) = split_windows(&mut windows);
    Some(CodexSnapshot {
        plan_type: body
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        five_hour,
        weekly,
        source: Some("live".into()),
        observed_at: Some(now),
        ..Default::default()
    })
}

/// Shorter window goes in the 5-hour slot, longer in the weekly slot. If a
/// plan only has one window and it's longer than a day, treat it as weekly.
fn split_windows(windows: &mut Vec<CodexWindow>) -> (Option<CodexWindow>, Option<CodexWindow>) {
    windows.sort_by_key(|w| w.window_minutes.unwrap_or(i64::MAX));
    match windows.len() {
        0 => (None, None),
        1 => {
            let w = windows.remove(0);
            if w.window_minutes.is_some_and(|m| m > 24 * 60) {
                (None, Some(w))
            } else {
                (Some(w), None)
            }
        }
        _ => {
            let a = windows.remove(0);
            let b = windows.remove(0);
            (Some(a), Some(b))
        }
    }
}

fn epoch_to_utc(secs: i64) -> Option<DateTime<Utc>> {
    // Accept milliseconds too, just in case.
    let secs = if secs > 100_000_000_000 {
        secs / 1000
    } else {
        secs
    };
    Utc.timestamp_opt(secs, 0).single()
}

// ---------------------------------------------------------------------------
// Session log fallback
// ---------------------------------------------------------------------------

/// Scan the newest few rollout files for the last `token_count` event that
/// carries `rate_limits`.
fn latest_from_logs(sessions: &Path) -> Option<CodexSnapshot> {
    if !sessions.is_dir() {
        return None;
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = walkdir::WalkDir::new(sessions)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let m = e.metadata().ok()?.modified().ok()?;
            Some((m, e.into_path()))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));

    for (_, path) in files.into_iter().take(5) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(s) = text.lines().rev().find_map(parse_log_line) {
            return Some(s);
        }
    }
    None
}

fn parse_log_line(line: &str) -> Option<CodexSnapshot> {
    if !line.contains("rate_limits") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let payload = v.get("payload").unwrap_or(&v);
    let rl = payload.get("rate_limits").filter(|r| !r.is_null())?;
    let observed = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let parse_win = |w: &Value| -> Option<CodexWindow> {
        let used = w.get("used_percent")?.as_f64().filter(|p| p.is_finite())?;
        let window_minutes = w.get("window_minutes").and_then(Value::as_i64).or_else(|| {
            w.get("limit_window_seconds")
                .and_then(Value::as_i64)
                .map(|s| (s + 59) / 60)
        });
        let resets_at = w
            .get("resets_at")
            .and_then(Value::as_i64)
            .and_then(epoch_to_utc)
            .or_else(|| {
                w.get("resets_in_seconds")
                    .and_then(Value::as_i64)
                    .map(|s| observed + ChronoDuration::seconds(s))
            });
        Some(CodexWindow {
            used_percent: used.clamp(0.0, 100.0),
            window_minutes,
            resets_at,
        })
    };

    let mut windows: Vec<CodexWindow> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| rl.get(*k))
        .filter(|w| !w.is_null())
        .filter_map(parse_win)
        .collect();
    if windows.is_empty() {
        return None;
    }
    let (five_hour, weekly) = split_windows(&mut windows);
    Some(CodexSnapshot {
        plan_type: rl
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        five_hour,
        weekly,
        source: Some("codex_logs".into()),
        observed_at: Some(observed),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_live_response() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let body = json!({
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 23,
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 3600,
                    "reset_at": 1_800_003_600
                },
                "secondary_window": {
                    "used_percent": 41.5,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 86400,
                    "reset_at": 1_800_086_400
                }
            },
            "credits": { "has_credits": false, "unlimited": false, "balance": null }
        });
        let s = parse_usage(&body, now).unwrap();
        assert_eq!(s.plan_type.as_deref(), Some("pro"));
        let f = s.five_hour.unwrap();
        assert_eq!(f.used_percent, 23.0);
        assert_eq!(f.window_minutes, Some(300));
        assert_eq!(f.resets_at, Some(now + ChronoDuration::seconds(3600)));
        let w = s.weekly.unwrap();
        assert_eq!(w.used_percent, 41.5);
        assert_eq!(w.window_minutes, Some(10080));
    }

    #[test]
    fn single_weekly_window_goes_to_weekly() {
        let body = json!({
            "rate_limit": {
                "primary_window": { "used_percent": 10, "limit_window_seconds": 604800, "reset_at": 1_800_000_000 },
                "secondary_window": null
            }
        });
        let s = parse_usage(&body, Utc::now()).unwrap();
        assert!(s.five_hour.is_none());
        assert_eq!(s.weekly.unwrap().used_percent, 10.0);
    }

    #[test]
    fn no_rate_limit_is_none() {
        assert!(parse_usage(&json!({ "plan_type": "free" }), Utc::now()).is_none());
    }

    #[test]
    fn parses_log_line() {
        let line = r#"{"timestamp":"2026-11-27T07:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":12.0,"window_minutes":299,"resets_in_seconds":600},"secondary":{"used_percent":55.0,"window_minutes":10079,"resets_at":1800000000},"plan_type":"pro"}}}"#;
        let s = parse_log_line(line).unwrap();
        assert_eq!(s.source.as_deref(), Some("codex_logs"));
        let f = s.five_hour.unwrap();
        assert_eq!(f.used_percent, 12.0);
        assert_eq!(
            f.resets_at.unwrap().to_rfc3339(),
            "2026-11-27T07:10:00+00:00"
        );
        assert_eq!(s.weekly.unwrap().used_percent, 55.0);
        assert_eq!(s.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn auth_requires_access_token() {
        assert!(auth_from_json(&json!({ "OPENAI_API_KEY": "sk-x", "tokens": null })).is_none());
        let a =
            auth_from_json(&json!({ "tokens": { "access_token": "abc", "account_id": "acct" } }))
                .unwrap();
        assert_eq!(a.access_token, "abc");
        assert_eq!(a.account_id.as_deref(), Some("acct"));
    }
}
