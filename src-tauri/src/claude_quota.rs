// Reads the Claude desktop app's session cookie store, decrypts it using the
// Chromium-on-macOS scheme (Keychain "Claude Safe Storage" + PBKDF2-SHA1 +
// AES-128-CBC), then calls https://claude.ai/api/organizations/{uuid}/usage
// to obtain the authoritative consumer quota numbers.
//
// macOS-only. Non-macOS builds get a stubbed implementation that always
// returns "not available".

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize)]
pub struct QuotaSnapshot {
    pub five_hour_utilization: Option<f64>,
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    pub seven_day_utilization: Option<f64>,
    pub seven_day_resets_at: Option<DateTime<Utc>>,
    /// Separate weekly limit for Fable (shown on Max and Ultra plans).
    pub fable_utilization: Option<f64>,
    pub fable_resets_at: Option<DateTime<Utc>>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Default, PartialEq)]
pub struct UsageWindows {
    pub five_hour: Option<(f64, Option<DateTime<Utc>>)>,
    pub seven_day: Option<(f64, Option<DateTime<Utc>>)>,
    pub fable: Option<(f64, Option<DateTime<Utc>>)>,
}

/// Parses the claude.ai usage response leniently, so a missing or renamed
/// window never fails the whole fetch.
///
/// Known shapes (not officially documented):
/// - Top-level windows `{ "utilization": 0-100, "resets_at": "..." }` under
///   `five_hour` and `seven_day`. Verified Ultra accounts send
///   `"five_hour": null` because they have no 5-hour limit.
/// - A `limits` array. Unscoped entries use `kind: "session"` (5-hour) and
///   `kind: "weekly_all"` (weekly). Per-model weekly limits use
///   `kind: "weekly_scoped"` with `scope.model.display_name` (e.g. "Fable")
///   and report `percent` instead of `utilization`.
/// - Older responses carried Fable as a top-level `seven_day_fable`
///   (also seen as `fable_seven_day` / `fable_weekly`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_usage(v: &serde_json::Value) -> UsageWindows {
    fn ts(v: &serde_json::Value) -> Option<DateTime<Utc>> {
        v.as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
    }
    fn window(v: Option<&serde_json::Value>) -> Option<(f64, Option<DateTime<Utc>>)> {
        let w = v?;
        let pct = w
            .get("utilization")
            .and_then(|x| x.as_f64())
            .or_else(|| w.get("percent").and_then(|x| x.as_f64()))?;
        Some((pct, w.get("resets_at").and_then(ts)))
    }

    let limits: Vec<&serde_json::Value> = v
        .get("limits")
        .and_then(|l| l.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let limit_of_kind = |kind: &str| {
        limits
            .iter()
            .find(|l| l.get("kind").and_then(|k| k.as_str()) == Some(kind))
            .copied()
    };
    let fable_limit = limits
        .iter()
        .find(|l| {
            l.get("kind").and_then(|k| k.as_str()) == Some("weekly_scoped")
                && l.pointer("/scope/model/display_name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.to_lowercase().contains("fable"))
                    .unwrap_or(false)
        })
        .copied();

    UsageWindows {
        five_hour: window(v.get("five_hour")).or_else(|| window(limit_of_kind("session"))),
        seven_day: window(v.get("seven_day")).or_else(|| window(limit_of_kind("weekly_all"))),
        fable: window(fable_limit)
            .or_else(|| window(v.get("seven_day_fable")))
            .or_else(|| window(v.get("fable_seven_day")))
            .or_else(|| window(v.get("fable_weekly"))),
    }
}

#[derive(Debug, Deserialize)]
struct OrgRow {
    uuid: String,
}

#[cfg(target_os = "macos")]
pub async fn fetch() -> QuotaSnapshot {
    match fetch_inner().await {
        Ok(s) => s,
        Err(e) => QuotaSnapshot {
            error: Some(e),
            fetched_at: Some(Utc::now()),
            ..Default::default()
        },
    }
}

#[cfg(not(target_os = "macos"))]
pub async fn fetch() -> QuotaSnapshot {
    QuotaSnapshot {
        error: Some("Claude.app cookie reader is macOS-only.".into()),
        fetched_at: Some(Utc::now()),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
async fn fetch_inner() -> Result<QuotaSnapshot, String> {
    let cookies = read_cookies()?;
    if !cookies.iter().any(|(n, _)| n == "sessionKey") {
        return Err(
            "No claude.ai session in Claude.app. Open Claude.app and sign in first."
                .to_string(),
        );
    }
    let cookie_header = cookies
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(n, v)| format!("{}={}", n, v))
        .collect::<Vec<_>>()
        .join("; ");

    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(ua)
        .build()
        .map_err(|e| e.to_string())?;

    let orgs: Vec<OrgRow> = client
        .get("https://claude.ai/api/organizations")
        .header("Cookie", &cookie_header)
        .header("anthropic-client-platform", "web_claude_ai")
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("orgs: {}", e))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let org = orgs
        .into_iter()
        .next()
        .ok_or_else(|| "No organizations returned".to_string())?;

    let url = format!("https://claude.ai/api/organizations/{}/usage", org.uuid);
    let body: serde_json::Value = client
        .get(&url)
        .header("Cookie", &cookie_header)
        .header("anthropic-client-platform", "web_claude_ai")
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("usage: {}", e))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let q = parse_usage(&body);

    Ok(QuotaSnapshot {
        five_hour_utilization: q.five_hour.map(|w| w.0),
        five_hour_resets_at: q.five_hour.and_then(|w| w.1),
        seven_day_utilization: q.seven_day.map(|w| w.0),
        seven_day_resets_at: q.seven_day.and_then(|w| w.1),
        fable_utilization: q.fable.map(|w| w.0),
        fable_resets_at: q.fable.and_then(|w| w.1),
        fetched_at: Some(Utc::now()),
        error: None,
    })
}

#[cfg(target_os = "macos")]
fn read_cookies() -> Result<Vec<(String, String)>, String> {
    use std::path::PathBuf;

    let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
    let db: PathBuf = home
        .join("Library")
        .join("Application Support")
        .join("Claude")
        .join("Cookies");
    if !db.exists() {
        return Err(format!(
            "Claude desktop app cookies not found at {}. Install / sign in to Claude.app first.",
            db.display()
        ));
    }

    let key = derive_key()?;
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("open cookies db: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT name, encrypted_value FROM cookies WHERE host_key LIKE '%claude.ai%'",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let enc: Vec<u8> = row.get(1)?;
            Ok((name, enc))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        let (name, enc) = r.map_err(|e| e.to_string())?;
        let value = decrypt_cookie(&key, &enc).unwrap_or_default();
        out.push((name, value));
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
fn derive_key() -> Result<[u8; 16], String> {
    use std::process::Command;

    let out = Command::new("security")
        .args(["find-generic-password", "-s", "Claude Safe Storage", "-w"])
        .output()
        .map_err(|e| format!("invoke security: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "Could not read 'Claude Safe Storage' from Keychain. \
             Make sure Claude.app is installed and you've signed in at least once."
        ));
    }
    let pw = String::from_utf8(out.stdout)
        .map_err(|_| "non-utf8 keychain password".to_string())?
        .trim()
        .as_bytes()
        .to_vec();

    let mut key = [0u8; 16];
    pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(&pw, b"saltysalt", 1003, &mut key)
        .map_err(|e| format!("pbkdf2: {}", e))?;
    Ok(key)
}

#[cfg(target_os = "macos")]
fn decrypt_cookie(key: &[u8; 16], blob: &[u8]) -> Option<String> {
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

    if blob.is_empty() {
        return None;
    }
    let ct = if blob.starts_with(b"v10") || blob.starts_with(b"v11") {
        &blob[3..]
    } else {
        blob
    };
    if ct.len() < 16 || ct.len() % 16 != 0 {
        return None;
    }
    let iv = [0x20u8; 16];
    let mut buf = ct.to_vec();
    let pt = Aes128CbcDec::new(key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?;
    // Newer Chromium prepends a 32-byte SHA-256 of the cookie metadata.
    let stripped: &[u8] = if pt.len() > 32 { &pt[32..] } else { pt };
    Some(String::from_utf8_lossy(stripped).to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_usage;
    use serde_json::json;

    #[test]
    fn legacy_windows() {
        let v = json!({
            "five_hour": {"utilization": 25.0, "resets_at": "2026-11-27T10:00:00.000000+00:00"},
            "seven_day": {"utilization": 45.0, "resets_at": "2026-11-30T10:00:00.000000+00:00"},
            "seven_day_opus": null
        });
        let u = parse_usage(&v);
        assert_eq!(u.five_hour.map(|w| w.0), Some(25.0));
        assert!(u.five_hour.unwrap().1.is_some());
        assert_eq!(u.seven_day.map(|w| w.0), Some(45.0));
        assert_eq!(u.fable, None);
    }

    #[test]
    fn ultra_without_five_hour_with_fable_in_limits() {
        let v = json!({
            "five_hour": null,
            "seven_day": {"utilization": 71.0, "resets_at": "2026-11-30T10:00:00+00:00"},
            "limits": [
                {"kind": "weekly_all", "group": "weekly", "percent": 71, "resets_at": "2026-11-30T10:00:00+00:00", "scope": null},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 12.5, "resets_at": "2026-11-30T10:00:00+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 80, "resets_at": null,
                 "scope": {"model": {"id": null, "display_name": "Opus"}, "surface": null}}
            ]
        });
        let u = parse_usage(&v);
        assert_eq!(u.five_hour, None);
        assert_eq!(u.seven_day.map(|w| w.0), Some(71.0));
        assert_eq!(u.fable.map(|w| w.0), Some(12.5));
    }

    #[test]
    fn legacy_fable_key_and_limits_fallback() {
        let v = json!({
            "seven_day_fable": {"utilization": 33.0, "resets_at": null},
            "limits": [
                {"kind": "session", "percent": 10, "resets_at": "2026-11-27T10:00:00Z"},
                {"kind": "weekly_all", "percent": 20, "resets_at": "2026-11-30T10:00:00Z"}
            ]
        });
        let u = parse_usage(&v);
        assert_eq!(u.five_hour.map(|w| w.0), Some(10.0));
        assert_eq!(u.seven_day.map(|w| w.0), Some(20.0));
        assert_eq!(u.fable, Some((33.0, None)));
    }

    #[test]
    fn garbage_does_not_panic() {
        let u = parse_usage(&json!({"five_hour": "oops", "limits": 3}));
        assert_eq!(u, Default::default());
    }
}
