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
    pub fetched_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaResp {
    // Ultra plans (formerly Max 20x) have no 5-hour limit once the account is
    // verified, so either window can be missing or null. Treat both as optional
    // instead of failing the whole fetch.
    #[serde(default)]
    five_hour: Option<QuotaWindow>,
    #[serde(default)]
    seven_day: Option<QuotaWindow>,
}
#[derive(Debug, Deserialize)]
struct QuotaWindow {
    utilization: f64,
    resets_at: DateTime<Utc>,
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
    let q: QuotaResp = client
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

    Ok(QuotaSnapshot {
        five_hour_utilization: q.five_hour.as_ref().map(|w| w.utilization),
        five_hour_resets_at: q.five_hour.as_ref().map(|w| w.resets_at),
        seven_day_utilization: q.seven_day.as_ref().map(|w| w.utilization),
        seven_day_resets_at: q.seven_day.as_ref().map(|w| w.resets_at),
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
