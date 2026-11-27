mod api_usage;
mod claude_ai;
mod claude_code;
mod claude_quota;
mod secrets;
mod settings;

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, LogicalPosition, Manager, PhysicalPosition, State, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;

use api_usage::ApiSnapshot;
use claude_code::ClaudeCodeSnapshot;
use claude_quota::QuotaSnapshot;
use secrets::KeyKind;
use settings::{BarLayout, DisplayMode, Settings};

struct AppState {
    settings: Mutex<Settings>,
    api_cache: Mutex<ApiSnapshot>,
    quota_cache: Mutex<QuotaSnapshot>,
    threshold_state: Mutex<ThresholdState>,
}

#[derive(Default)]
struct ThresholdState {
    five_hour_resets_at: Option<DateTime<Utc>>,
    five_hour_90_fired: bool,
    five_hour_95_fired: bool,
    seven_day_resets_at: Option<DateTime<Utc>>,
    seven_day_90_fired: bool,
    seven_day_95_fired: bool,
}

fn check_thresholds_and_notify(app: &tauri::AppHandle, snap: &QuotaSnapshot) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let enabled = state.settings.lock().unwrap().notify_thresholds;
    if !enabled {
        return;
    }
    let mut t = state.threshold_state.lock().unwrap();

    // 5-hour window
    if let (Some(util), Some(resets)) = (snap.five_hour_utilization, snap.five_hour_resets_at) {
        if t.five_hour_resets_at != Some(resets) {
            t.five_hour_resets_at = Some(resets);
            t.five_hour_90_fired = false;
            t.five_hour_95_fired = false;
        }
        if util >= 95.0 && !t.five_hour_95_fired {
            t.five_hour_95_fired = true;
            t.five_hour_90_fired = true;
            send_notification(
                app,
                "Claude 5h quota at 95%",
                &format!("{:.0}% used. Resets {}.", util, fmt_reset(resets)),
            );
        } else if util >= 90.0 && !t.five_hour_90_fired {
            t.five_hour_90_fired = true;
            send_notification(
                app,
                "Claude 5h quota at 90%",
                &format!("{:.0}% used. Resets {}.", util, fmt_reset(resets)),
            );
        }
    }

    // 7-day window
    if let (Some(util), Some(resets)) = (snap.seven_day_utilization, snap.seven_day_resets_at) {
        if t.seven_day_resets_at != Some(resets) {
            t.seven_day_resets_at = Some(resets);
            t.seven_day_90_fired = false;
            t.seven_day_95_fired = false;
        }
        if util >= 95.0 && !t.seven_day_95_fired {
            t.seven_day_95_fired = true;
            t.seven_day_90_fired = true;
            send_notification(
                app,
                "Claude weekly quota at 95%",
                &format!("{:.0}% used. Resets {}.", util, fmt_reset(resets)),
            );
        } else if util >= 90.0 && !t.seven_day_90_fired {
            t.seven_day_90_fired = true;
            send_notification(
                app,
                "Claude weekly quota at 90%",
                &format!("{:.0}% used. Resets {}.", util, fmt_reset(resets)),
            );
        }
    }
}

fn fmt_reset(ts: DateTime<Utc>) -> String {
    use chrono::Local;
    ts.with_timezone(&Local).format("%a %-I:%M%P").to_string()
}

fn send_notification(app: &tauri::AppHandle, title: &str, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

fn fmt_pct(v: Option<f64>) -> String {
    v.map(|x| format!("{:.0}%", x)).unwrap_or_else(|| "—".into())
}

fn fmt_cost(v: f64) -> String {
    if v >= 100.0 {
        format!("${:.0}", v)
    } else if v >= 1.0 {
        format!("${:.2}", v)
    } else if v > 0.0 {
        format!("${:.3}", v)
    } else {
        "$0".into()
    }
}

fn rebuild_tray(app: &tauri::AppHandle) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    let quota = state.quota_cache.lock().unwrap().clone();
    let api = state.api_cache.lock().unwrap().clone();

    // ---- Build menu ----
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

    let label_5h = format!(
        "5H quota: {}{}",
        fmt_pct(quota.five_hour_utilization),
        match (quota.five_hour_utilization, quota.five_hour_resets_at) {
            (Some(_), Some(t)) => format!("  ·  resets {}", fmt_reset(t)),
            _ =>
                if quota.error.is_some() {
                    "  (Claude.app session unavailable)".to_string()
                } else {
                    String::new()
                },
        }
    );
    let label_7d = format!(
        "7D quota: {}{}",
        fmt_pct(quota.seven_day_utilization),
        match (quota.seven_day_utilization, quota.seven_day_resets_at) {
            (Some(_), Some(t)) => format!("  ·  resets {}", fmt_reset(t)),
            _ => String::new(),
        }
    );
    let label_anth = match (&api.anthropic, &api.anthropic_error) {
        (Some(u), _) => format!(
            "Anthropic API today: {}  ·  {} tok",
            fmt_cost(u.estimated_cost_usd),
            human_tokens(u.input_tokens + u.output_tokens + u.cache_creation)
        ),
        (None, Some(_)) => "Anthropic API: error".to_string(),
        (None, None) => "Anthropic API: not configured".to_string(),
    };
    let label_oai = match (&api.openai, &api.openai_error) {
        (Some(u), _) => format!(
            "OpenAI API today: {}  ·  {} tok",
            fmt_cost(u.estimated_cost_usd),
            human_tokens(u.input_tokens + u.output_tokens)
        ),
        (None, Some(_)) => "OpenAI API: error".to_string(),
        (None, None) => "OpenAI API: not configured".to_string(),
    };

    // Disabled info rows + active actions.
    let mk_info = |s: &str| MenuItem::with_id(app, format!("info-{}", s), s, false, None::<&str>);
    let res = (|| -> tauri::Result<()> {
        let i_5h = mk_info(&label_5h)?;
        let i_7d = mk_info(&label_7d)?;
        let sep1 = PredefinedMenuItem::separator(app)?;
        let i_an = mk_info(&label_anth)?;
        let i_oa = mk_info(&label_oai)?;
        let sep2 = PredefinedMenuItem::separator(app)?;
        let m_settings =
            MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
        let m_toggle =
            MenuItem::with_id(app, "toggle_bar", "Toggle floating bar", true, None::<&str>)?;
        let m_reset =
            MenuItem::with_id(app, "reset_pos", "Reset bar position", true, None::<&str>)?;
        let sep3 = PredefinedMenuItem::separator(app)?;
        let m_quit =
            MenuItem::with_id(app, "quit", "Quit BurnBar", true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &i_5h, &i_7d, &sep1, &i_an, &i_oa, &sep2, &m_settings, &m_toggle, &m_reset,
                &sep3, &m_quit,
            ],
        )?;
        tray.set_menu(Some(menu))?;
        Ok(())
    })();
    let _ = res;

    // Tooltip — short, glanceable.
    let tip = format!(
        "BurnBar — 5H {} · 7D {}",
        fmt_pct(quota.five_hour_utilization),
        fmt_pct(quota.seven_day_utilization),
    );
    let _ = tray.set_tooltip(Some(tip));
}

fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", (n as f64) / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", (n as f64) / 1_000.0)
    } else {
        format!("{}", n)
    }
}

fn apply_mode(app: &tauri::AppHandle, mode: DisplayMode) {
    if let Some(bar) = app.get_webview_window("bar") {
        let want_bar = matches!(mode, DisplayMode::Bar | DisplayMode::Both);
        if want_bar {
            let _ = bar.show();
        } else {
            let _ = bar.hide();
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_dock_visibility(app: &tauri::AppHandle, hide: bool) {
    use tauri::ActivationPolicy;
    let policy = if hide {
        ActivationPolicy::Accessory
    } else {
        ActivationPolicy::Regular
    };
    let _ = app.set_activation_policy(policy);
}

#[cfg(not(target_os = "macos"))]
fn apply_dock_visibility(_app: &tauri::AppHandle, _hide: bool) {
    // No-op on Windows/Linux: the bar already has skipTaskbar, and the
    // settings window naturally shows/hides as a normal app window.
}

fn position_bar(app: &tauri::AppHandle, saved: Option<(i32, i32)>) {
    let Some(bar) = app.get_webview_window("bar") else {
        return;
    };
    if let Some((x, y)) = saved {
        let _ = bar.set_position(PhysicalPosition::new(x, y));
        return;
    }
    // Default: top-right of the primary monitor, with a small inset.
    if let Ok(Some(monitor)) = bar.primary_monitor() {
        let scale = monitor.scale_factor();
        let m_size = monitor.size();
        let m_pos = monitor.position();
        let bar_logical_w = 220.0;
        let inset = 16.0;
        let x = m_pos.x as f64 + (m_size.width as f64 / scale) - bar_logical_w - inset;
        let y = m_pos.y as f64 / scale + 36.0;
        let _ = bar.set_position(LogicalPosition::new(x, y));
    }
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_display_mode(app: tauri::AppHandle, state: State<AppState>, mode: DisplayMode) {
    {
        let mut s = state.settings.lock().unwrap();
        s.display_mode = mode;
        settings::save(&app, &s);
    }
    apply_mode(&app, mode);
}

fn current_anchor(state: &State<AppState>) -> Option<chrono::DateTime<Utc>> {
    let s = state.settings.lock().unwrap();
    let start = s.calibrated_block_start?;
    if Utc::now() < start + ChronoDuration::hours(5) {
        Some(start)
    } else {
        None
    }
}

fn emit_settings_update(app: &tauri::AppHandle, state: &State<AppState>) {
    let s = state.settings.lock().unwrap().clone();
    let _ = app.emit("settings:update", &s);
}

#[tauri::command]
fn set_cap(app: tauri::AppHandle, state: State<AppState>, cap_tokens: u64) {
    {
        let mut s = state.settings.lock().unwrap();
        s.cap_tokens = cap_tokens;
        settings::save(&app, &s);
    }
    emit_settings_update(&app, &state);
    let anchor = current_anchor(&state);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let snap = claude_code::collect_snapshot(anchor).await;
        let _ = handle.emit("usage:update", &snap);
    });
}

#[tauri::command]
async fn calibrate(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    pct_used: f64,
    reset_minutes: u64,
) -> Result<(), String> {
    if pct_used <= 0.0 || pct_used > 100.0 {
        return Err("pct_used must be between 0 and 100".into());
    }
    if reset_minutes > 300 {
        return Err("reset_minutes must be <= 300".into());
    }

    // First, derive the anchor from the user's resetMinutes input.
    let elapsed = ChronoDuration::minutes((300 - reset_minutes as i64).max(0));
    let block_start = Utc::now() - elapsed;

    // Compute tokens using the SAME anchored window we'll display from, so the
    // cap and the displayed % share a denominator.
    let snap_now = claude_code::collect_snapshot(Some(block_start)).await;
    let tokens = snap_now.block_tokens.max(1);
    let cap = ((tokens as f64) / (pct_used / 100.0)).round() as u64;

    {
        let mut s = state.settings.lock().unwrap();
        s.cap_tokens = cap.max(10_000);
        s.calibrated_block_start = Some(block_start);
        settings::save(&app, &s);
    }

    emit_settings_update(&app, &state);
    let _ = app.emit("usage:update", &snap_now);
    Ok(())
}

#[tauri::command]
fn clear_calibration(app: tauri::AppHandle, state: State<AppState>) {
    {
        let mut s = state.settings.lock().unwrap();
        s.calibrated_block_start = None;
        settings::save(&app, &s);
    }
    emit_settings_update(&app, &state);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let snap = claude_code::collect_snapshot(None).await;
        let _ = handle.emit("usage:update", &snap);
    });
}

#[tauri::command]
fn set_hide_dock_icon(app: tauri::AppHandle, state: State<AppState>, hide: bool) {
    {
        let mut s = state.settings.lock().unwrap();
        s.hide_dock_icon = hide;
        settings::save(&app, &s);
    }
    apply_dock_visibility(&app, hide);
}

#[tauri::command]
fn set_bar_layout(app: tauri::AppHandle, state: State<AppState>, layout: BarLayout) {
    {
        let mut s = state.settings.lock().unwrap();
        s.bar_layout = layout;
        settings::save(&app, &s);
    }
    if let Some(bar) = app.get_webview_window("bar") {
        let height = match layout {
            BarLayout::Compact => 36u32,
            BarLayout::Stacked => 64u32,
        };
        let _ = bar.set_size(tauri::LogicalSize::new(220u32, height));
    }
    emit_settings_update(&app, &state);
}

#[tauri::command]
fn set_notify_thresholds(app: tauri::AppHandle, state: State<AppState>, enabled: bool) {
    {
        let mut s = state.settings.lock().unwrap();
        s.notify_thresholds = enabled;
        settings::save(&app, &s);
    }
    emit_settings_update(&app, &state);
}

#[tauri::command]
fn set_show_flame(app: tauri::AppHandle, state: State<AppState>, enabled: bool) {
    {
        let mut s = state.settings.lock().unwrap();
        s.show_flame = enabled;
        settings::save(&app, &s);
    }
    emit_settings_update(&app, &state);
}

#[tauri::command]
fn reset_bar_position(app: tauri::AppHandle, state: State<AppState>) {
    {
        let mut s = state.settings.lock().unwrap();
        s.bar_position = None;
        settings::save(&app, &s);
    }
    position_bar(&app, None);
}

#[tauri::command]
async fn get_claude_code_usage(state: State<'_, AppState>) -> Result<ClaudeCodeSnapshot, ()> {
    let anchor = current_anchor(&state);
    Ok(claude_code::collect_snapshot(anchor).await)
}

#[derive(Serialize)]
struct KeyStatus {
    anthropic_set: bool,
    openai_set: bool,
}

#[tauri::command]
fn get_key_status() -> KeyStatus {
    KeyStatus {
        anthropic_set: secrets::load(KeyKind::AnthropicAdmin).is_some(),
        openai_set: secrets::load(KeyKind::OpenAiAdmin).is_some(),
    }
}

#[tauri::command]
async fn set_anthropic_key(app: tauri::AppHandle, key: String) -> Result<(), String> {
    secrets::save(KeyKind::AnthropicAdmin, &key)?;
    refresh_api_usage(app).await;
    Ok(())
}

#[tauri::command]
async fn set_openai_key(app: tauri::AppHandle, key: String) -> Result<(), String> {
    secrets::save(KeyKind::OpenAiAdmin, &key)?;
    refresh_api_usage(app).await;
    Ok(())
}

#[tauri::command]
fn clear_anthropic_key(state: State<AppState>) -> Result<(), String> {
    secrets::clear(KeyKind::AnthropicAdmin)?;
    let mut cache = state.api_cache.lock().unwrap();
    cache.anthropic = None;
    cache.anthropic_error = None;
    Ok(())
}

#[tauri::command]
fn clear_openai_key(state: State<AppState>) -> Result<(), String> {
    secrets::clear(KeyKind::OpenAiAdmin)?;
    let mut cache = state.api_cache.lock().unwrap();
    cache.openai = None;
    cache.openai_error = None;
    Ok(())
}

#[tauri::command]
fn get_api_usage(state: State<AppState>) -> ApiSnapshot {
    state.api_cache.lock().unwrap().clone()
}

#[tauri::command]
fn claude_ai_connect(app: tauri::AppHandle) -> Result<(), String> {
    claude_ai::ensure_window(&app, true)
}

#[tauri::command]
async fn claude_ai_probe(app: tauri::AppHandle, url: String) -> claude_ai::ProbeResult {
    claude_ai::probe(&app, &url).await
}

#[tauri::command]
fn claude_ai_close(app: tauri::AppHandle) -> Result<(), String> {
    claude_ai::close(&app);
    Ok(())
}

#[tauri::command]
fn get_claude_quota(state: State<AppState>) -> QuotaSnapshot {
    state.quota_cache.lock().unwrap().clone()
}

#[tauri::command]
async fn refresh_claude_quota(app: tauri::AppHandle) -> QuotaSnapshot {
    let snap = claude_quota::fetch().await;
    if let Some(state) = app.try_state::<AppState>() {
        *state.quota_cache.lock().unwrap() = snap.clone();
    }
    let _ = app.emit("quota:update", &snap);
    snap
}

async fn refresh_api_usage(app: tauri::AppHandle) {
    let mut snap = ApiSnapshot::default();

    if let Some(key) = secrets::load(KeyKind::AnthropicAdmin) {
        match api_usage::fetch_anthropic(&key).await {
            Ok(u) => snap.anthropic = Some(u),
            Err(e) => snap.anthropic_error = Some(e),
        }
    }
    if let Some(key) = secrets::load(KeyKind::OpenAiAdmin) {
        match api_usage::fetch_openai(&key).await {
            Ok(u) => snap.openai = Some(u),
            Err(e) => snap.openai_error = Some(e),
        }
    }

    if let Some(state) = app.try_state::<AppState>() {
        *state.api_cache.lock().unwrap() = snap.clone();
    }
    let _ = app.emit("api:update", &snap);
    rebuild_tray(&app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_display_mode,
            set_cap,
            calibrate,
            clear_calibration,
            set_hide_dock_icon,
            set_bar_layout,
            set_notify_thresholds,
            set_show_flame,
            reset_bar_position,
            get_claude_code_usage,
            get_key_status,
            set_anthropic_key,
            set_openai_key,
            clear_anthropic_key,
            clear_openai_key,
            get_api_usage,
            claude_ai_connect,
            claude_ai_probe,
            claude_ai_close,
            get_claude_quota,
            refresh_claude_quota,
        ])
        .on_window_event(|window, event| {
            if window.label() == "bar" {
                if let WindowEvent::Moved(pos) = event {
                    let app = window.app_handle();
                    if let Some(state) = app.try_state::<AppState>() {
                        let mut s = state.settings.lock().unwrap();
                        s.bar_position = Some((pos.x, pos.y));
                        settings::save(app, &s);
                    }
                }
            }
        })
        .setup(|app| {
            let loaded = settings::load(app.handle());
            app.manage(AppState {
                settings: Mutex::new(loaded.clone()),
                api_cache: Mutex::new(ApiSnapshot::default()),
                quota_cache: Mutex::new(QuotaSnapshot::default()),
                threshold_state: Mutex::new(ThresholdState::default()),
            });

            apply_dock_visibility(app.handle(), loaded.hide_dock_icon);

            let show_settings =
                MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let toggle_bar = MenuItem::with_id(
                app,
                "toggle_bar",
                "Toggle floating bar",
                true,
                None::<&str>,
            )?;
            let reset_pos = MenuItem::with_id(
                app,
                "reset_pos",
                "Reset bar position",
                true,
                None::<&str>,
            )?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit BurnBar", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_settings, &toggle_bar, &reset_pos, &sep, &quit])?;

            let _tray = TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .icon_as_template(true)
                .tooltip("BurnBar")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(w) = app.get_webview_window("settings") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "toggle_bar" => {
                        if let Some(w) = app.get_webview_window("bar") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                            }
                        }
                    }
                    "reset_pos" => {
                        let app2 = app.clone();
                        if let Some(state) = app.try_state::<AppState>() {
                            let mut s = state.settings.lock().unwrap();
                            s.bar_position = None;
                            settings::save(&app2, &s);
                        }
                        position_bar(app, None);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|_tray, _event| { /* menu handles left-click now */ })
                .build(app)?;

            apply_mode(app.handle(), loaded.display_mode);
            position_bar(app.handle(), loaded.bar_position);
            if let Some(bar) = app.get_webview_window("bar") {
                let height = match loaded.bar_layout {
                    BarLayout::Compact => 36u32,
                    BarLayout::Stacked => 64u32,
                };
                let _ = bar.set_size(tauri::LogicalSize::new(220u32, height));
            }

            if let Some(settings) = app.get_webview_window("settings") {
                let _ = settings.show();
                let _ = settings.set_focus();
            }
            if let Some(bar) = app.get_webview_window("bar") {
                let _ = bar.show();
            }

            let h_cc = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let anchor = h_cc.try_state::<AppState>().and_then(|s| {
                        let g = s.settings.lock().unwrap();
                        g.calibrated_block_start
                            .filter(|t| Utc::now() < *t + ChronoDuration::hours(5))
                    });
                    let snap = claude_code::collect_snapshot(anchor).await;
                    let _ = h_cc.emit("usage:update", &snap);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            });

            let h_api = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    refresh_api_usage(h_api.clone()).await;
                    tokio::time::sleep(Duration::from_secs(300)).await;
                }
            });

            // Claude.ai consumer quota poller (60s — endpoint is cheap and
            // gives us authoritative numbers).
            let h_q = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let snap = claude_quota::fetch().await;
                    if let Some(state) = h_q.try_state::<AppState>() {
                        *state.quota_cache.lock().unwrap() = snap.clone();
                    }
                    check_thresholds_and_notify(&h_q, &snap);
                    let _ = h_q.emit("quota:update", &snap);
                    rebuild_tray(&h_q);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
