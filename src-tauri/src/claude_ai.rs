// Experimental: probe claude.ai's undocumented endpoints with the user's
// session so they can find where their consumer quota lives.
//
// The "Connect" window is a normal webview pointed at claude.ai that the
// user signs in to. Probing does NOT run inside that page: a remote page has
// no access to Tauri IPC, so results posted back from there never arrived and
// the Probe button hung forever. Instead we read the session cookies out of
// the webview's cookie store (HttpOnly included) and make the request from
// Rust with a hard timeout. If the embedded window isn't signed in, we fall
// back to the Claude desktop app's session on macOS.
//
// Endpoints this hits are unofficial and undocumented. They WILL break.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const CLAUDE_AI_URL: &str = "https://claude.ai/";
const WINDOW_LABEL: &str = "claude-ai";
const POPUP_LABEL_PREFIX: &str = "claude-ai-popup-";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

// Present as current Safari so Google sign-in doesn't reject the embedded
// WKWebView as an unsupported browser. Probes made with this window's cookies
// reuse the same UA, because Cloudflare ties its clearance cookie to it.
const WEBVIEW_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";
// Same UA claude_quota.rs uses with the Claude desktop app's cookies.
#[cfg(target_os = "macos")]
const DESKTOP_APP_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

static POPUP_COUNTER: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProbeResult {
    pub url: String,
    pub status: u16,
    pub body: Option<Value>,
    pub error: Option<String>,
    /// Which session the request used: "window" or "desktop-app".
    pub source: Option<String>,
}

pub fn ensure_window(app: &tauri::AppHandle, visible: bool) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(WINDOW_LABEL) {
        if visible {
            let _ = w.show();
            let _ = w.set_focus();
        }
        return Ok(());
    }

    let app_for_popups = app.clone();
    let app_for_nav = app.clone();
    WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::External(CLAUDE_AI_URL.parse().unwrap()))
        .title("Sign in to claude.ai")
        .inner_size(900.0, 760.0)
        .visible(visible)
        .user_agent(WEBVIEW_UA)
        // "Continue with Google" opens Google's sign-in in a popup and waits
        // for it to message back through window.opener. Webviews block
        // window.open unless we create the popup ourselves, which is why
        // claude.ai said "There was an error logging you in" straight away.
        .on_new_window(move |url, features| open_popup(&app_for_popups, url, features))
        // Once the main window finishes loading a claude.ai page again
        // (it reloads after a successful sign-in), any leftover Google popup
        // has done its job; not every webview honours the popup's own
        // window.close(), so tidy it up here.
        .on_page_load(move |_w, payload| {
            if payload.event() == PageLoadEvent::Finished
                && payload.url().host_str() == Some("claude.ai")
            {
                close_popups(&app_for_nav);
            }
        })
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn open_popup(
    app: &tauri::AppHandle,
    url: tauri::Url,
    features: tauri::webview::NewWindowFeatures,
) -> NewWindowResponse<tauri::Wry> {
    let n = POPUP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let label = format!("{POPUP_LABEL_PREFIX}{n}");
    // The popup must start blank and share the opener's webview
    // configuration (window_features does that), or the page loses its
    // window.opener link and the sign-in result can't be handed back.
    let built = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::External("about:blank".parse().unwrap()),
    )
    .window_features(features)
    .user_agent(WEBVIEW_UA)
    .title(url.host_str().unwrap_or("Sign in"))
    .on_document_title_changed(|w, title| {
        let _ = w.set_title(&title);
    })
    .build();
    match built {
        Ok(window) => NewWindowResponse::Create { window },
        Err(e) => {
            eprintln!("[claude_ai] could not open sign-in popup for {url}: {e}");
            NewWindowResponse::Deny
        }
    }
}

fn close_popups(app: &tauri::AppHandle) {
    for (label, w) in app.webview_windows() {
        if label.starts_with(POPUP_LABEL_PREFIX) {
            let _ = w.close();
        }
    }
}

/// Validate that the probe target is claude.ai over https, so session
/// cookies are never sent to another host.
fn parse_probe_url(url: &str) -> Result<tauri::Url, String> {
    let parsed: tauri::Url = url
        .trim()
        .parse()
        .map_err(|_| format!("Not a valid URL: {url}"))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("claude.ai") {
        return Err("Probe only works on https://claude.ai/ URLs.".into());
    }
    Ok(parsed)
}

/// Cookies from the embedded window, if it exists and is signed in.
async fn window_cookies(app: &tauri::AppHandle) -> Option<Vec<(String, String)>> {
    let win = app.get_webview_window(WINDOW_LABEL)?;
    let url: tauri::Url = CLAUDE_AI_URL.parse().ok()?;
    // Reading cookies waits on the webview's main thread; keep it off the
    // async runtime and give up rather than hang if the webview is stuck.
    let task = tauri::async_runtime::spawn_blocking(move || win.cookies_for_url(url));
    let cookies = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .ok()?
        .ok()?
        .ok()?;
    let pairs: Vec<(String, String)> = cookies
        .iter()
        .map(|c| (c.name().to_string(), c.value().to_string()))
        .collect();
    pairs.iter().any(|(n, _)| n == "sessionKey").then_some(pairs)
}

#[cfg(target_os = "macos")]
fn desktop_app_cookies() -> Option<Vec<(String, String)>> {
    let pairs = crate::claude_quota::read_cookies().ok()?;
    pairs.iter().any(|(n, _)| n == "sessionKey").then_some(pairs)
}

#[cfg(not(target_os = "macos"))]
fn desktop_app_cookies() -> Option<Vec<(String, String)>> {
    None
}

pub async fn probe(app: &tauri::AppHandle, url: &str) -> ProbeResult {
    let fail = |error: String, source: Option<&str>| ProbeResult {
        url: url.to_string(),
        status: 0,
        body: None,
        error: Some(error),
        source: source.map(str::to_string),
    };

    let target = match parse_probe_url(url) {
        Ok(u) => u,
        Err(e) => return fail(e, None),
    };

    let (cookies, ua, source) = if let Some(c) = window_cookies(app).await {
        (c, WEBVIEW_UA, "window")
    } else if let Some(c) = desktop_app_cookies() {
        #[cfg(target_os = "macos")]
        let ua = DESKTOP_APP_UA;
        #[cfg(not(target_os = "macos"))]
        let ua = WEBVIEW_UA;
        (c, ua, "desktop-app")
    } else {
        return fail(
            "Not signed in. Click Connect and sign in to claude.ai (if Google sign-in fails, \
             use \"Continue with email\" with the same address), or sign in to the Claude \
             desktop app."
                .into(),
            None,
        );
    };

    let cookie_header = cookies
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ");

    let client = match reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .user_agent(ua)
        .build()
    {
        Ok(c) => c,
        Err(e) => return fail(e.to_string(), Some(source)),
    };

    let resp = client
        .get(target.as_str())
        .header("Cookie", cookie_header)
        .header("anthropic-client-platform", "web_claude_ai")
        .header("accept", "application/json")
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            return fail(
                format!("Timed out after {}s with no response.", PROBE_TIMEOUT.as_secs()),
                Some(source),
            )
        }
        Err(e) => return fail(e.to_string(), Some(source)),
    };

    let status = resp.status().as_u16();
    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => return fail(format!("Reading response failed: {e}"), Some(source)),
    };
    let body = serde_json::from_str::<Value>(&text)
        .unwrap_or_else(|_| Value::String(text.chars().take(4000).collect()));
    let error = match status {
        401 | 403 => Some(format!(
            "HTTP {status}: the {} session was rejected. Sign in again.",
            if source == "window" { "claude.ai window" } else { "Claude desktop app" }
        )),
        _ => None,
    };

    ProbeResult {
        url: url.to_string(),
        status,
        body: Some(body),
        error,
        source: Some(source.to_string()),
    }
}

pub fn close(app: &tauri::AppHandle) {
    close_popups(app);
    if let Some(w) = app.get_webview_window(WINDOW_LABEL) {
        let _ = w.close();
    }
}
