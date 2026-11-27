import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface ClaudeCodeSnapshot {
  block_tokens: number;
  block_message_count: number;
  block_start: string | null;
  block_end: string | null;
  seconds_remaining: number | null;
}

interface UserSettings {
  display_mode: "bar" | "tray" | "both";
  cap_tokens: number;
  bar_layout: "compact" | "stacked";
  show_flame: boolean;
  bar_source?: "claude" | "chatgpt";
}

interface CodexWindow {
  used_percent: number;
  window_minutes: number | null;
  resets_at: string | null;
}

interface CodexSnapshot {
  plan_type: string | null;
  five_hour: CodexWindow | null;
  weekly: CodexWindow | null;
  source: "live" | "codex_logs" | null;
  observed_at: string | null;
  not_signed_in: boolean;
  error: string | null;
}

interface QuotaSnapshot {
  five_hour_utilization: number | null;
  five_hour_resets_at: string | null;
  seven_day_utilization: number | null;
  seven_day_resets_at: string | null;
  fetched_at: string | null;
  error: string | null;
}

let cap = 5_000_000;
let layout: "compact" | "stacked" = "compact";
let showFlame = true;
let lastCC: ClaudeCodeSnapshot | null = null;
let lastQuota: QuotaSnapshot | null = null;
let lastCodex: CodexSnapshot | null = null;
let barSource: "claude" | "chatgpt" = "claude";

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1_000)}k`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

function fmtRemainingFromIso(iso: string | null): string {
  if (!iso) return "—";
  const ms = new Date(iso).getTime() - Date.now();
  if (ms <= 0) return "0m";
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  if (h === 0) return `${m}m`;
  return `${h}h${m.toString().padStart(2, "0")}`;
}

function fmtResetClock(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return d.toLocaleString([], {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  });
}

function fmtRemainingFromSec(seconds: number | null): string {
  if (seconds == null || seconds <= 0) return "—";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h === 0) return `${m}m`;
  return `${h}h${m.toString().padStart(2, "0")}`;
}

function applyLayoutClass() {
  const bar = document.getElementById("bar")!;
  bar.classList.toggle("compact", layout === "compact");
  bar.classList.toggle("stacked", layout === "stacked");
}

function setBarColors(pct: number, stale: boolean) {
  const bar = document.getElementById("bar")!;
  bar.style.setProperty("--pct", `${pct}`);
  bar.classList.toggle("warn", pct >= 50 && pct < 80);
  bar.classList.toggle("high", pct >= 80 && pct < 95);
  bar.classList.toggle("crit", pct >= 95);
  bar.classList.toggle("stale", stale);
}

/**
 * Map a 0..1 fraction across a list of [stop, value] pairs.
 * Values can be 3-tuples (RGB) or single numbers.
 */
function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}
function lerpStops(t: number, stops: Array<[number, [number, number, number]]>): [number, number, number] {
  if (t <= stops[0][0]) return stops[0][1];
  if (t >= stops[stops.length - 1][0]) return stops[stops.length - 1][1];
  for (let i = 1; i < stops.length; i++) {
    const [s1, v1] = stops[i - 1];
    const [s2, v2] = stops[i];
    if (t <= s2) {
      const u = (t - s1) / (s2 - s1);
      return [lerp(v1[0], v2[0], u), lerp(v1[1], v2[1], u), lerp(v1[2], v2[2], u)];
    }
  }
  return stops[stops.length - 1][1];
}
function lerpNumStops(t: number, stops: Array<[number, number]>): number {
  if (t <= stops[0][0]) return stops[0][1];
  if (t >= stops[stops.length - 1][0]) return stops[stops.length - 1][1];
  for (let i = 1; i < stops.length; i++) {
    const [s1, v1] = stops[i - 1];
    const [s2, v2] = stops[i];
    if (t <= s2) {
      const u = (t - s1) / (s2 - s1);
      return lerp(v1, v2, u);
    }
  }
  return stops[stops.length - 1][1];
}

function rgb(c: [number, number, number]): string {
  return `rgb(${Math.round(c[0])}, ${Math.round(c[1])}, ${Math.round(c[2])})`;
}

function applyFlame(pct: number) {
  const bar = document.getElementById("bar")!;
  if (!showFlame || pct < 50) {
    bar.classList.remove("flame-on");
    return;
  }
  bar.classList.add("flame-on");

  // Normalize 50..100 -> 0..1
  const t = Math.min(1, (pct - 50) / 50);

  // Color stops (RGB):
  // 0.0 → deep blue, 0.2 → cyan, 0.45 → yellow, 0.7 → orange, 0.9 → red-orange, 1.0 → red
  const outer = rgb(
    lerpStops(t, [
      [0.0, [59, 130, 246]],    // blue
      [0.2, [56, 189, 248]],    // cyan
      [0.45, [250, 204, 21]],   // yellow
      [0.7, [251, 146, 60]],    // orange
      [0.9, [239, 68, 68]],     // red-orange
      [1.0, [220, 38, 38]],     // red
    ]),
  );
  const inner = rgb(
    lerpStops(t, [
      [0.0, [191, 219, 254]],   // pale blue
      [0.2, [165, 243, 252]],   // pale cyan
      [0.45, [254, 240, 138]],  // pale yellow
      [0.7, [254, 215, 170]],   // pale orange
      [0.9, [254, 226, 226]],   // pale pink
      [1.0, [254, 202, 202]],   // light red
    ]),
  );

  // Size stops — both compact (smaller) and stacked (a touch bigger) get the same multiplier
  const baseW = layout === "stacked" ? 16 : 14;
  const baseH = layout === "stacked" ? 22 : 18;
  const scale = lerpNumStops(t, [
    [0.0, 0.55],
    [0.3, 0.85],
    [0.6, 1.1],
    [0.85, 1.4],
    [1.0, 1.7],
  ]);
  const speed = lerpNumStops(t, [
    [0.0, 1200],   // slow flicker for small ember
    [0.5, 720],
    [0.9, 380],
    [1.0, 260],    // frantic
  ]);
  const glow = lerpNumStops(t, [
    [0.0, 0],
    [0.5, 1],
    [0.85, 3],
    [1.0, 6],
  ]);

  bar.style.setProperty("--flame-color", outer);
  bar.style.setProperty("--flame-inner", inner);
  bar.style.setProperty("--flame-w", `${baseW * scale}px`);
  bar.style.setProperty("--flame-h", `${baseH * scale}px`);
  bar.style.setProperty("--flame-speed", `${Math.round(speed)}ms`);
  bar.style.setProperty("--flame-glow", `${glow}px`);
}

function paintChatGPT() {
  const bar = document.getElementById("bar")!;
  const src5 = document.getElementById("src-5h")!;
  const pct5 = document.getElementById("pct-5h")!;
  const time5 = document.getElementById("time-5h")!;
  const src7 = document.getElementById("src-7d")!;
  const pct7 = document.getElementById("pct-7d")!;
  const time7 = document.getElementById("time-7d")!;
  const row7 = document.querySelector<HTMLElement>(".row-7d")!;
  const stale = document.getElementById("stale-mark")!;

  const c = lastCodex;
  const fromLogs = c?.source === "codex_logs";
  const isStale = !!c && (fromLogs || (!c.five_hour && !c.weekly));
  // The first window shown is the 5-hour one if the plan has it, else weekly.
  const first = c?.five_hour ?? c?.weekly ?? null;
  const second = c?.five_hour ? c.weekly : null;
  const firstLabel = c?.five_hour ? "5H" : "7D";

  bar.title = fromLogs
    ? `ChatGPT limits from your last Codex session${c?.error ? ` (${c.error})` : ""}.`
    : "Live ChatGPT subscription limits (Codex).";

  let displayPct = 0;
  if (first) {
    displayPct = first.used_percent;
    src5.textContent = `GPT ${firstLabel}`;
    pct5.textContent = `${Math.round(first.used_percent)}%`;
    time5.textContent =
      layout === "stacked"
        ? `resets ${fmtResetClock(first.resets_at)}`
        : fmtRemainingFromIso(first.resets_at);
    if (second) {
      row7.style.display = "";
      src7.textContent = "7D";
      pct7.textContent = `${Math.round(second.used_percent)}%`;
      time7.textContent = layout === "stacked" ? `resets ${fmtResetClock(second.resets_at)}` : "";
    } else {
      row7.style.display = "none";
    }
  } else {
    src5.textContent = "GPT";
    pct5.textContent = "—";
    time5.textContent = c?.not_signed_in ? "sign in to Codex" : "—";
    row7.style.display = "none";
  }
  stale.title = isStale ? (c?.error ?? "ChatGPT limits unavailable.") : "";
  setBarColors(displayPct, isStale);
  applyFlame(displayPct);
}

function paint() {
  if (barSource === "chatgpt") {
    paintChatGPT();
    return;
  }
  document.getElementById("bar")!.title = "Live Claude consumer quota.";
  document.getElementById("src-7d")!.textContent = "7D";
  const src5 = document.getElementById("src-5h")!;
  const pct5 = document.getElementById("pct-5h")!;
  const time5 = document.getElementById("time-5h")!;
  const pct7 = document.getElementById("pct-7d")!;
  const time7 = document.getElementById("time-7d")!;
  const row7 = document.querySelector<HTMLElement>(".row-7d")!;
  const stale = document.getElementById("stale-mark")!;

  const haveQuota =
    lastQuota && !lastQuota.error && lastQuota.five_hour_utilization != null;
  const isStale = !!(lastQuota && lastQuota.error);

  let displayPct = 0;

  if (haveQuota) {
    const pct = lastQuota!.five_hour_utilization!;
    displayPct = pct;
    src5.textContent = "5H";
    pct5.textContent = `${Math.round(pct)}%`;
    time5.textContent =
      layout === "stacked"
        ? `resets ${fmtResetClock(lastQuota!.five_hour_resets_at)}`
        : fmtRemainingFromIso(lastQuota!.five_hour_resets_at);

    if (lastQuota!.seven_day_utilization != null) {
      row7.style.display = "";
      pct7.textContent = `${Math.round(lastQuota!.seven_day_utilization)}%`;
      time7.textContent =
        layout === "stacked"
          ? `resets ${fmtResetClock(lastQuota!.seven_day_resets_at)}`
          : "";
    } else {
      row7.style.display = "none";
    }
    setBarColors(pct, isStale);
    stale.title = "";
  } else if (lastCC) {
    const pct = cap > 0 ? Math.min(100, (lastCC.block_tokens / cap) * 100) : 0;
    displayPct = pct;
    src5.textContent = "CC";
    pct5.textContent = fmtTokens(lastCC.block_tokens);
    time5.textContent = fmtRemainingFromSec(lastCC.seconds_remaining);
    row7.style.display = "none";
    setBarColors(pct, isStale);
    if (isStale) {
      stale.title = `Claude.ai session expired or unavailable. ${lastQuota?.error ?? ""}`.trim();
    }
  } else {
    src5.textContent = "—";
    pct5.textContent = "—";
    time5.textContent = "—";
    row7.style.display = "none";
    setBarColors(0, isStale);
  }

  applyFlame(displayPct);
}

async function refreshSettings() {
  try {
    const s = await invoke<UserSettings>("get_settings");
    cap = s.cap_tokens || 5_000_000;
    layout = s.bar_layout || "compact";
    showFlame = s.show_flame !== false;
    barSource = s.bar_source === "chatgpt" ? "chatgpt" : "claude";
    applyLayoutClass();
  } catch {}
}

async function refreshAll() {
  try {
    lastCC = await invoke<ClaudeCodeSnapshot>("get_claude_code_usage");
  } catch {}
  try {
    lastQuota = await invoke<QuotaSnapshot>("get_claude_quota");
  } catch {}
  try {
    lastCodex = await invoke<CodexSnapshot>("get_codex_usage");
  } catch {}
  paint();
}

window.addEventListener("DOMContentLoaded", async () => {
  await refreshSettings();
  await refreshAll();
  setInterval(paint, 30_000);
  setInterval(refreshAll, 60_000);

  const win = getCurrentWindow();
  document.body.addEventListener("mousedown", async (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    try {
      await win.startDragging();
    } catch {}
  });
});

listen<ClaudeCodeSnapshot>("usage:update", (e) => {
  lastCC = e.payload;
  paint();
});
listen<QuotaSnapshot>("quota:update", (e) => {
  lastQuota = e.payload;
  paint();
});
listen<CodexSnapshot>("codex:update", (e) => {
  lastCodex = e.payload;
  paint();
});
listen<UserSettings>("settings:update", async (e) => {
  barSource = e.payload.bar_source === "chatgpt" ? "chatgpt" : "claude";
  cap = e.payload.cap_tokens || 5_000_000;
  layout = e.payload.bar_layout || "compact";
  showFlame = e.payload.show_flame !== false;
  applyLayoutClass();
  paint();
});
