import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type DisplayMode = "bar" | "tray" | "both";

type BarLayout = "compact" | "stacked";

interface UserSettings {
  display_mode: DisplayMode;
  cap_tokens: number;
  hide_dock_icon: boolean;
  bar_position: [number, number] | null;
  calibrated_block_start: string | null;
  bar_layout: BarLayout;
  notify_thresholds: boolean;
}

interface KeyStatus {
  anthropic_set: boolean;
  openai_set: boolean;
}

interface ModelLine {
  model: string;
  input_tokens: number;
  output_tokens: number;
  estimated_cost_usd: number;
}

interface ProviderUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation: number;
  cache_read: number;
  estimated_cost_usd: number;
  by_model: ModelLine[];
  fetched_at: string | null;
}

interface ApiSnapshot {
  anthropic: ProviderUsage | null;
  openai: ProviderUsage | null;
  anthropic_error: string | null;
  openai_error: string | null;
}

interface ClaudeCodeSnapshot {
  block_tokens: number;
  block_message_count: number;
  seconds_remaining: number | null;
}

const PRESETS: Array<{ label: string; value: number; note: string }> = [
  { label: "Pro", value: 500_000, note: "~500k tokens / 5h" },
  { label: "Max 5×", value: 2_500_000, note: "~2.5M tokens / 5h" },
  { label: "Max 20×", value: 10_000_000, note: "~10M tokens / 5h" },
];

let current: UserSettings = {
  display_mode: "both",
  cap_tokens: 5_000_000,
  hide_dock_icon: false,
  bar_position: null,
  calibrated_block_start: null,
  bar_layout: "compact",
  notify_thresholds: true,
};

type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; version: string; notes: string; update: Update }
  | { kind: "uptodate" }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "installing" }
  | { kind: "error"; message: string };
let updateState: UpdateStatus = { kind: "idle" };

async function checkForUpdates() {
  updateState = { kind: "checking" };
  renderUpdateBanner();
  try {
    const update = await check();
    if (update) {
      updateState = {
        kind: "available",
        version: update.version,
        notes: update.body ?? "",
        update,
      };
    } else {
      updateState = { kind: "uptodate" };
    }
  } catch (e) {
    updateState = { kind: "error", message: String(e) };
  }
  renderUpdateBanner();
}

async function installUpdate() {
  if (updateState.kind !== "available") return;
  const upd = updateState.update;
  let downloaded = 0;
  let total: number | null = null;
  updateState = { kind: "downloading", downloaded, total };
  renderUpdateBanner();
  try {
    await upd.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? null;
        updateState = { kind: "downloading", downloaded: 0, total };
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        updateState = { kind: "downloading", downloaded, total };
      } else if (event.event === "Finished") {
        updateState = { kind: "installing" };
      }
      renderUpdateBanner();
    });
    await relaunch();
  } catch (e) {
    updateState = { kind: "error", message: String(e) };
    renderUpdateBanner();
  }
}

function renderUpdateBanner() {
  const banner = document.getElementById("update-banner");
  if (!banner) return;
  const close = `<button type="button" id="update-dismiss" class="update-dismiss" title="Dismiss">×</button>`;
  switch (updateState.kind) {
    case "idle":
      banner.style.display = "none";
      banner.innerHTML = "";
      return;
    case "checking":
      banner.style.display = "";
      banner.className = "update-banner muted";
      banner.innerHTML = `<span>Checking for updates…</span>${close}`;
      break;
    case "available":
      banner.style.display = "";
      banner.className = "update-banner";
      banner.innerHTML = `
        <div class="update-text">
          <strong>BurnBar ${updateState.version}</strong> available
          ${updateState.notes ? `<em>${updateState.notes.slice(0, 200)}</em>` : ""}
        </div>
        <button type="button" id="update-install" class="btn">Install &amp; relaunch</button>
        ${close}
      `;
      break;
    case "downloading": {
      const pct = updateState.total
        ? Math.round((updateState.downloaded / updateState.total) * 100)
        : null;
      banner.style.display = "";
      banner.className = "update-banner";
      banner.innerHTML = `<span>Downloading ${pct != null ? pct + "%" : ""}…</span>`;
      break;
    }
    case "installing":
      banner.style.display = "";
      banner.className = "update-banner";
      banner.innerHTML = `<span>Installing… app will relaunch.</span>`;
      break;
    case "uptodate":
      banner.style.display = "";
      banner.className = "update-banner muted";
      banner.innerHTML = `<span>You're on the latest version.</span>${close}`;
      break;
    case "error":
      banner.style.display = "";
      banner.className = "update-banner error";
      banner.innerHTML = `<span>Update check failed: ${updateState.message}</span>${close}`;
      break;
  }
  const installBtn = document.getElementById("update-install");
  if (installBtn) installBtn.addEventListener("click", installUpdate);
  const dismiss = document.getElementById("update-dismiss");
  if (dismiss) {
    dismiss.addEventListener("click", () => {
      updateState = { kind: "idle" };
      renderUpdateBanner();
    });
  }
}

const isMac =
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || "");
let keyStatus: KeyStatus = { anthropic_set: false, openai_set: false };
let apiSnap: ApiSnapshot = {
  anthropic: null,
  openai: null,
  anthropic_error: null,
  openai_error: null,
};
let ccSnap: ClaudeCodeSnapshot | null = null;

async function loadAll() {
  try {
    current = await invoke<UserSettings>("get_settings");
  } catch {}
  try {
    keyStatus = await invoke<KeyStatus>("get_key_status");
  } catch {}
  try {
    apiSnap = await invoke<ApiSnapshot>("get_api_usage");
  } catch {}
  try {
    ccSnap = await invoke<ClaudeCodeSnapshot>("get_claude_code_usage");
  } catch {}
}

async function calibrate(pctUsed: number, resetMinutes: number) {
  if (pctUsed <= 0 || pctUsed > 100) return;
  if (resetMinutes < 0 || resetMinutes > 300) return;
  await invoke("calibrate", { pctUsed, resetMinutes });
  await loadAll();
  render();
}

async function clearCalibration() {
  await invoke("clear_calibration");
  current.calibrated_block_start = null;
  await loadAll();
  render();
}

async function setMode(mode: DisplayMode) {
  current.display_mode = mode;
  await invoke("set_display_mode", { mode });
}

async function setCap(cap: number) {
  current.cap_tokens = cap;
  await invoke("set_cap", { capTokens: cap });
}

async function setHideDockIcon(hide: boolean) {
  current.hide_dock_icon = hide;
  await invoke("set_hide_dock_icon", { hide });
}

async function setBarLayout(layout: BarLayout) {
  current.bar_layout = layout;
  await invoke("set_bar_layout", { layout });
}

async function setNotifyThresholds(enabled: boolean) {
  current.notify_thresholds = enabled;
  await invoke("set_notify_thresholds", { enabled });
}

async function resetBarPosition() {
  await invoke("reset_bar_position");
}

async function saveKey(provider: "anthropic" | "openai", key: string) {
  if (!key.trim()) return;
  const cmd = provider === "anthropic" ? "set_anthropic_key" : "set_openai_key";
  await invoke(cmd, { key: key.trim() });
  keyStatus[`${provider}_set` as keyof KeyStatus] = true;
  render();
}

async function clearKey(provider: "anthropic" | "openai") {
  const cmd = provider === "anthropic" ? "clear_anthropic_key" : "clear_openai_key";
  await invoke(cmd);
  keyStatus[`${provider}_set` as keyof KeyStatus] = false;
  if (provider === "anthropic") {
    apiSnap.anthropic = null;
    apiSnap.anthropic_error = null;
  } else {
    apiSnap.openai = null;
    apiSnap.openai_error = null;
  }
  render();
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return `${n}`;
}

function fmtCost(usd: number): string {
  if (usd >= 100) return `$${usd.toFixed(0)}`;
  if (usd >= 1) return `$${usd.toFixed(2)}`;
  if (usd > 0) return `$${usd.toFixed(3)}`;
  return "$0";
}

function providerCard(
  set: boolean,
  usage: ProviderUsage | null,
  error: string | null,
): string {
  if (!set) {
    return `<div class="prov-empty">Not configured</div>`;
  }
  if (error) {
    return `<div class="prov-error">
      <strong>Error fetching usage.</strong>
      <pre class="prov-error-detail">${escape(error)}</pre>
    </div>`;
  }
  if (!usage) {
    return `<div class="prov-loading">Loading…</div>`;
  }
  const total = usage.input_tokens + usage.output_tokens + usage.cache_creation;
  const modelLines = usage.by_model
    .slice(0, 3)
    .map(
      (m) => `<li><span class="m-name">${escape(m.model)}</span> <span class="m-cost">${fmtCost(m.estimated_cost_usd)}</span></li>`,
    )
    .join("");
  return `
    <div class="prov-stats">
      <div class="prov-headline">
        <span class="prov-cost">${fmtCost(usage.estimated_cost_usd)}</span>
        <span class="prov-tokens">${fmtTokens(total)} tok</span>
      </div>
      <ul class="prov-models">${modelLines || "<li class=\"muted\">No usage today</li>"}</ul>
    </div>`;
}

function escape(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function render() {
  const app = document.getElementById("app")!;
  app.innerHTML = `
    <header>
      <h1>BurnBar <span class="subtitle">for Claude/GPT</span></h1>
      <p class="sub">Claude &amp; ChatGPT usage at a glance.</p>
    </header>

    <div id="update-banner" class="update-banner" style="display:none"></div>

    <section>
      <h2>App version</h2>
      <div class="toggle-row">
        <button type="button" id="check-updates" class="btn-secondary">Check for updates</button>
        <span class="muted small">BurnBar checks once on launch automatically.</span>
      </div>
    </section>

    <section>
      <h2>Display</h2>
      <div class="radio-group">
        ${(["bar", "tray", "both"] as DisplayMode[])
          .map(
            (m) => `
          <label class="radio">
            <input type="radio" name="mode" value="${m}" ${m === current.display_mode ? "checked" : ""} />
            <span class="radio-label">
              <strong>${labelFor(m)}</strong>
              <em>${descriptionFor(m)}</em>
            </span>
          </label>`,
          )
          .join("")}
      </div>

      <div class="toggle-row">
        <label class="toggle">
          <input type="checkbox" id="hide-dock" ${current.hide_dock_icon ? "checked" : ""} ${isMac ? "" : "disabled"} />
          <span>
            <strong>Hide from Dock</strong>
            <em>${
              isMac
                ? "Run as a menu-bar accessory (no Dock icon). Tray icon stays visible."
                : "macOS only — bar already skips the taskbar on Windows/Linux."
            }</em>
          </span>
        </label>
      </div>

      <div class="toggle-row">
        <span class="cal-label">Bar layout:</span>
        <label class="radio inline">
          <input type="radio" name="layout" value="compact" ${current.bar_layout === "compact" ? "checked" : ""} />
          <span>Compact (one line)</span>
        </label>
        <label class="radio inline">
          <input type="radio" name="layout" value="stacked" ${current.bar_layout === "stacked" ? "checked" : ""} />
          <span>Stacked (5H + 7D rows with reset day)</span>
        </label>
      </div>

      <div class="toggle-row">
        <label class="toggle">
          <input type="checkbox" id="notify-thresholds" ${current.notify_thresholds ? "checked" : ""} />
          <span>
            <strong>Notify at 90% and 95%</strong>
            <em>Desktop notification when 5H or 7D quota crosses 90% or 95% (once per window).</em>
          </span>
        </label>
      </div>

      <div class="toggle-row">
        <button type="button" id="reset-pos" class="btn-secondary">Reset bar position</button>
        <span class="muted small">Drag the bar to move it; position is remembered automatically.</span>
      </div>
    </section>

    <section>
      <h2>Fallback display (Claude Code only)</h2>
      <p class="muted">
        When Claude.app is signed in, the bar pulls your real 5h / 7d quota directly. These
        settings only kick in as a <strong>fallback</strong> — for example if Claude.app's session
        expires or you're on Windows/Linux. They control the % you see in fallback mode.
      </p>
      <div class="cap-input">
        <input id="cap" type="number" min="1000" step="100000" value="${current.cap_tokens}" />
        <span class="cap-formatted" id="cap-formatted">${fmtTokens(current.cap_tokens)} tok</span>
      </div>
      <div class="presets">
        ${PRESETS.map(
          (p) => `
          <button type="button" class="preset" data-cap="${p.value}">
            <strong>${p.label}</strong>
            <em>${p.note}</em>
          </button>`,
        ).join("")}
      </div>
      ${calibrateBlock()}
    </section>

    <section>
      <h2>API usage (today, UTC)</h2>
      <p class="muted">
        Requires <strong>Admin API keys</strong>, not regular API keys. Anthropic admin keys start with <code>sk-ant-admin01-</code> and are created in the
        <a href="#" data-href="https://console.anthropic.com/settings/admin-keys">Console → Admin Keys</a>.
        OpenAI admin keys start with <code>sk-admin-</code> and live under
        <a href="#" data-href="https://platform.openai.com/settings/organization/admin-keys">Settings → Admin Keys</a>.
        Keys are stored in your OS keychain.
      </p>

      <div class="api-grid">
        <div class="api-card">
          <header class="api-card-header">
            <h3>Anthropic API</h3>
            <span class="dot ${keyStatus.anthropic_set ? "ok" : "pending"}"></span>
          </header>
          ${providerCard(keyStatus.anthropic_set, apiSnap.anthropic, apiSnap.anthropic_error)}
          <div class="api-card-actions">
            ${
              keyStatus.anthropic_set
                ? `<button type="button" class="btn-secondary" data-clear="anthropic">Clear key</button>`
                : `<input type="password" placeholder="sk-ant-admin01-..." class="key-input" data-set="anthropic" />
                   <button type="button" class="btn" data-set-btn="anthropic">Save</button>`
            }
          </div>
        </div>

        <div class="api-card">
          <header class="api-card-header">
            <h3>OpenAI API</h3>
            <span class="dot ${keyStatus.openai_set ? "ok" : "pending"}"></span>
          </header>
          ${providerCard(keyStatus.openai_set, apiSnap.openai, apiSnap.openai_error)}
          <div class="api-card-actions">
            ${
              keyStatus.openai_set
                ? `<button type="button" class="btn-secondary" data-clear="openai">Clear key</button>`
                : `<input type="password" placeholder="sk-admin-..." class="key-input" data-set="openai" />
                   <button type="button" class="btn" data-set-btn="openai">Save</button>`
            }
          </div>
        </div>
      </div>
    </section>

    <section>
      <h2>Claude.ai consumer quota <span class="experimental">EXPERIMENTAL</span></h2>
      <p class="muted">
        Open the embedded claude.ai window, sign in, then try probing endpoints to find your real
        quota. Endpoints aren't documented and will probably break — that's why this is opt-in.
      </p>
      <div class="probe-actions">
        <button type="button" id="claude-ai-connect" class="btn">Open claude.ai window</button>
        <button type="button" id="claude-ai-close" class="btn-secondary">Close window</button>
      </div>
      <div class="probe-input">
        <input id="probe-url" type="text"
               placeholder="https://claude.ai/api/account_status (or any path you want to test)"
               value="https://claude.ai/api/account_status" />
        <button type="button" id="probe-btn" class="btn-secondary">Probe</button>
      </div>
      <pre id="probe-result" class="probe-result">No probe yet.</pre>
      <p class="muted small">
        <strong>Tip:</strong> in the claude.ai window, open the page's developer tools (right-click → Inspect),
        go to the Network tab, then in the Claude UI navigate to your usage / settings page. Look for
        any XHR call with <code>quota</code>, <code>limit</code>, or <code>usage</code> in the path —
        paste that URL above and probe it.
      </p>
    </section>

    <section>
      <h2>Sources</h2>
      <ul class="status">
        <li><span class="dot ok"></span>Claude Code (local logs)</li>
        <li><span class="dot ${keyStatus.anthropic_set ? "ok" : "pending"}"></span>Anthropic API</li>
        <li><span class="dot ${keyStatus.openai_set ? "ok" : "pending"}"></span>OpenAI API</li>
        <li><span class="dot pending"></span>Claude.ai / ChatGPT subscription — <em>Phase 4 (experimental)</em></li>
      </ul>
    </section>
  `;

  attachHandlers();
}

function attachHandlers() {
  const app = document.getElementById("app")!;

  app.querySelectorAll<HTMLInputElement>('input[name="mode"]').forEach((el) => {
    el.addEventListener("change", async () => {
      if (el.checked) await setMode(el.value as DisplayMode);
    });
  });

  const hideDock = app.querySelector<HTMLInputElement>("#hide-dock");
  if (hideDock) {
    hideDock.addEventListener("change", async () => {
      await setHideDockIcon(hideDock.checked);
    });
  }

  app.querySelectorAll<HTMLInputElement>('input[name="layout"]').forEach((el) => {
    el.addEventListener("change", async () => {
      if (el.checked) await setBarLayout(el.value as BarLayout);
    });
  });

  const notifyToggle = app.querySelector<HTMLInputElement>("#notify-thresholds");
  if (notifyToggle) {
    notifyToggle.addEventListener("change", async () => {
      await setNotifyThresholds(notifyToggle.checked);
    });
  }

  const checkBtn = app.querySelector<HTMLButtonElement>("#check-updates");
  if (checkBtn) {
    checkBtn.addEventListener("click", () => {
      checkForUpdates();
    });
  }

  const resetPos = app.querySelector<HTMLButtonElement>("#reset-pos");
  if (resetPos) {
    resetPos.addEventListener("click", async () => {
      await resetBarPosition();
    });
  }

  const capEl = app.querySelector<HTMLInputElement>("#cap")!;
  const capFmt = app.querySelector<HTMLElement>("#cap-formatted")!;
  capEl.addEventListener("input", () => {
    const v = Number(capEl.value);
    capFmt.textContent = isFinite(v) && v > 0 ? `${fmtTokens(v)} tok` : "—";
  });
  capEl.addEventListener("change", async () => {
    const v = Number(capEl.value);
    if (isFinite(v) && v > 0) await setCap(Math.round(v));
  });

  app.querySelectorAll<HTMLButtonElement>(".preset").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const v = Number(btn.dataset.cap);
      capEl.value = String(v);
      capFmt.textContent = `${fmtTokens(v)} tok`;
      await setCap(v);
    });
  });

  const calPct = app.querySelector<HTMLInputElement>("#cal-pct");
  const calMin = app.querySelector<HTMLInputElement>("#cal-min");
  const calBtn = app.querySelector<HTMLButtonElement>("#cal-btn");
  const calCapPreview = app.querySelector<HTMLElement>("#cal-cap-preview");
  const calTimePreview = app.querySelector<HTMLElement>("#cal-time-preview");
  if (calPct && calMin && calBtn && calCapPreview && calTimePreview && ccSnap) {
    const updatePreview = () => {
      const pct = Number(calPct.value);
      const min = Number(calMin.value);
      if (
        isFinite(pct) &&
        pct > 0 &&
        pct <= 100 &&
        ccSnap!.block_tokens > 0
      ) {
        const c = Math.round(ccSnap!.block_tokens / (pct / 100));
        calCapPreview.textContent = `${fmtTokens(c)} tok`;
      } else {
        calCapPreview.textContent = "—";
      }
      if (isFinite(min) && min >= 0 && min <= 300) {
        const end = new Date(Date.now() + min * 60_000);
        calTimePreview.textContent = `to expire ${end.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
      } else {
        calTimePreview.textContent = "—";
      }
    };
    calPct.addEventListener("input", updatePreview);
    calMin.addEventListener("input", updatePreview);
    const fire = async () => {
      const pct = Number(calPct.value);
      const min = Number(calMin.value);
      if (
        isFinite(pct) &&
        pct > 0 &&
        pct <= 100 &&
        isFinite(min) &&
        min >= 0 &&
        min <= 300
      ) {
        await calibrate(pct, min);
      }
    };
    calBtn.addEventListener("click", fire);
    calPct.addEventListener("keydown", (e) => {
      if (e.key === "Enter") fire();
    });
    calMin.addEventListener("keydown", (e) => {
      if (e.key === "Enter") fire();
    });
  }
  const calClear = app.querySelector<HTMLButtonElement>("#cal-clear");
  if (calClear) {
    calClear.addEventListener("click", clearCalibration);
  }

  app.querySelectorAll<HTMLButtonElement>("[data-set-btn]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const provider = btn.dataset.setBtn as "anthropic" | "openai";
      const input = app.querySelector<HTMLInputElement>(
        `input[data-set="${provider}"]`,
      );
      if (input) await saveKey(provider, input.value);
    });
  });

  app.querySelectorAll<HTMLInputElement>('input[data-set]').forEach((input) => {
    input.addEventListener("keydown", async (e) => {
      if (e.key === "Enter") {
        const provider = input.dataset.set as "anthropic" | "openai";
        await saveKey(provider, input.value);
      }
    });
  });

  app.querySelectorAll<HTMLButtonElement>("[data-clear]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const provider = btn.dataset.clear as "anthropic" | "openai";
      await clearKey(provider);
    });
  });

  const connectBtn = app.querySelector<HTMLButtonElement>("#claude-ai-connect");
  const closeBtn = app.querySelector<HTMLButtonElement>("#claude-ai-close");
  const probeBtn = app.querySelector<HTMLButtonElement>("#probe-btn");
  const probeUrl = app.querySelector<HTMLInputElement>("#probe-url");
  const probeOut = app.querySelector<HTMLElement>("#probe-result");
  if (connectBtn) {
    connectBtn.addEventListener("click", async () => {
      try {
        await invoke("claude_ai_connect");
      } catch (e) {
        if (probeOut) probeOut.textContent = `Error opening webview: ${e}`;
      }
    });
  }
  if (closeBtn) {
    closeBtn.addEventListener("click", async () => {
      try {
        await invoke("claude_ai_close");
      } catch {}
    });
  }
  if (probeBtn && probeUrl) {
    probeBtn.addEventListener("click", async () => {
      const url = probeUrl.value.trim();
      if (!url) return;
      if (probeOut) probeOut.textContent = `Probing ${url}…`;
      try {
        await invoke("claude_ai_probe", { url });
      } catch (e) {
        if (probeOut) probeOut.textContent = `Probe failed (no webview open?): ${e}`;
      }
    });
  }

  app.querySelectorAll<HTMLAnchorElement>("a[data-href]").forEach((a) => {
    a.addEventListener("click", async (e) => {
      e.preventDefault();
      const url = a.dataset.href!;
      const { open } = await import("@tauri-apps/plugin-shell");
      await open(url);
    });
  });
}

function calibrateBlock(): string {
  const calibrated = current.calibrated_block_start;
  if (calibrated) {
    const start = new Date(calibrated);
    const end = new Date(start.getTime() + 5 * 60 * 60 * 1000);
    const now = Date.now();
    const stillValid = now < end.getTime();
    const remainingMin = Math.max(0, Math.floor((end.getTime() - now) / 60000));
    return `
      <div class="calibrate calibrate-active">
        <div class="cal-row">
          <strong>${stillValid ? "Pinned to Claude app" : "Calibration expired"}</strong>
          <button type="button" class="link-btn" id="cal-clear">Clear</button>
        </div>
        <div class="cal-row muted small">
          <span>Window started ${start.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}.</span>
          <span>${stillValid ? `Resets in ~${remainingMin}m.` : "Recalibrate to re-pin."}</span>
        </div>
        ${stillValid ? "" : calibrateInputs()}
      </div>
    `;
  }
  if (!ccSnap || ccSnap.block_tokens <= 0) return "";
  return `<div class="calibrate">${calibrateInputs()}</div>`;
}

function calibrateInputs(): string {
  return `
    <div class="cal-row">
      <span class="cal-label">Calibrate from the Claude app:</span>
    </div>
    <div class="cal-row cal-fields">
      <span class="cal-prefix">%&nbsp;used</span>
      <input id="cal-pct" type="number" min="0.1" max="100" step="0.1" placeholder="70" />
      <span class="cal-prefix">resets in</span>
      <input id="cal-min" type="number" min="0" max="300" step="1" placeholder="2" />
      <span class="cal-prefix">min</span>
      <button type="button" id="cal-btn" class="btn-secondary">Pin</button>
    </div>
    <div class="cal-row muted small">
      <span>Implies cap of <strong id="cal-cap-preview">—</strong>; window pinned <strong id="cal-time-preview">—</strong>.</span>
    </div>
  `;
}

function labelFor(m: DisplayMode) {
  return m === "bar" ? "Floating bar" : m === "tray" ? "Menu-bar / tray" : "Both";
}
function descriptionFor(m: DisplayMode) {
  return m === "bar"
    ? "Always-visible draggable window."
    : m === "tray"
      ? "Lives in the OS status area."
      : "Show both at once.";
}

window.addEventListener("DOMContentLoaded", async () => {
  await loadAll();
  render();
  await getCurrentWindow().show();

  // Auto-check on launch. Errors are swallowed — no banner if endpoint down etc.
  checkForUpdates().catch(() => {});
});

listen<ApiSnapshot>("api:update", (e) => {
  apiSnap = e.payload;
  render();
});

interface ProbeResult {
  url: string;
  status: number;
  body: unknown;
  error: string | null;
}

listen<ProbeResult>("claude-ai:result", (e) => {
  const out = document.getElementById("probe-result");
  if (!out) return;
  const r = e.payload;
  const lines = [
    `URL:    ${r.url}`,
    `Status: ${r.status}`,
    r.error ? `Error:  ${r.error}` : "",
    "",
    "Body:",
    typeof r.body === "string"
      ? r.body
      : JSON.stringify(r.body, null, 2),
  ].filter(Boolean);
  out.textContent = lines.join("\n");
});
