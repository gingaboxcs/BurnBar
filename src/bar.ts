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

// Flame stages: 1 = small blue (50%+), 2 = blue + orange (70%+),
// 3 = large orange + yellow (90%+). Nothing below 50%.
function flameStage(pct: number): number {
  if (pct >= 90) return 3;
  if (pct >= 70) return 2;
  if (pct >= 50) return 1;
  return 0;
}

function applyFlame(pct: number) {
  const bar = document.getElementById("bar")!;
  const stage = showFlame ? flameStage(pct) : 0;
  bar.classList.toggle("flame-on", stage > 0);
  document.querySelectorAll<HTMLImageElement>(".flame-stage").forEach((img) => {
    img.classList.toggle("on", Number(img.dataset.stage) === stage);
  });
}

function paint() {
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
listen<UserSettings>("settings:update", async (e) => {
  cap = e.payload.cap_tokens || 5_000_000;
  layout = e.payload.bar_layout || "compact";
  showFlame = e.payload.show_flame !== false;
  applyLayoutClass();
  paint();
});
