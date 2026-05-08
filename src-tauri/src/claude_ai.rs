// Experimental: poll claude.ai for consumer quota by piggybacking on a
// hidden webview that the user has logged in to. We don't try to read
// HttpOnly cookies directly — instead we eval a `fetch()` inside the
// webview, which uses the session cookies automatically, and post the
// response back to Rust via the Tauri event bus.
//
// Endpoints this hits are unofficial and undocumented. They WILL break.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const CLAUDE_AI_URL: &str = "https://claude.ai/";
const WINDOW_LABEL: &str = "claude-ai";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProbeResult {
    pub url: String,
    pub status: u16,
    pub body: Option<Value>,
    pub error: Option<String>,
}

pub fn ensure_window(app: &tauri::AppHandle, visible: bool) -> Result<(), String> {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        if visible {
            if let Some(w) = app.get_webview_window(WINDOW_LABEL) {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        return Ok(());
    }

    // Spoof a current Safari user-agent so Google OAuth doesn't block the
    // embedded webview as "an unsupported browser." Without this, the WKWebView
    // default UA is detected by Google and login fails outright.
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

    WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::External(CLAUDE_AI_URL.parse().unwrap()))
        .title("Claude.ai (background)")
        .inner_size(900.0, 700.0)
        .visible(visible)
        .user_agent(ua)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn show(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(WINDOW_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        ensure_window(app, true)?;
    }
    Ok(())
}

pub fn probe(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    let Some(win) = app.get_webview_window(WINDOW_LABEL) else {
        return Err("Claude.ai webview not initialized. Click 'Connect' first.".into());
    };
    let escaped = serde_json::to_string(url).unwrap();
    let script = format!(
        r#"
        (async () => {{
          const url = {url_lit};
          try {{
            const resp = await fetch(url, {{ credentials: 'include', headers: {{ 'accept': 'application/json' }} }});
            let body = null;
            const text = await resp.text();
            try {{ body = JSON.parse(text); }} catch {{ body = text.slice(0, 4000); }}
            window.__TAURI_INTERNALS__.invoke('claude_ai_probe_result', {{
              result: {{ url: url, status: resp.status, body: body, error: null }}
            }});
          }} catch (e) {{
            window.__TAURI_INTERNALS__.invoke('claude_ai_probe_result', {{
              result: {{ url: url, status: 0, body: null, error: String(e) }}
            }});
          }}
        }})();
        "#,
        url_lit = escaped
    );
    win.eval(script).map_err(|e| e.to_string())
}

pub fn close(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW_LABEL) {
        let _ = w.close();
    }
}

