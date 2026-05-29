# BurnBar

**Claude & ChatGPT usage at a glance.** A tiny floating bar / menu-bar app for macOS, Windows, and Linux that watches your AI quota in real time so you don't get surprised by hitting a limit mid-task.

## What it tracks

- **Claude consumer 5h + 7d quota** — pulled directly from your Claude.app session (macOS only, read-only). Same numbers the Claude desktop app shows internally.
- **Claude Code session usage** — parses `~/.claude/projects/**/*.jsonl` for token totals across active 5-hour blocks. Works on all platforms.
- **Anthropic API usage today** — per-model cost & tokens via the org Admin API (requires `sk-ant-admin01-…` key).
- **OpenAI API usage today** — same via OpenAI's `/v1/organization/usage/completions` endpoint (requires `sk-admin-…` key).

## Surfaces

- **Floating bar** — frameless, always-on-top, draggable, color-coded (green → yellow → red as you fill the 5h window). Compact one-liner or stacked two-row layout with reset day/time.
- **Tray / menu-bar icon** — left-click for a live menu showing all four metrics + quick actions.
- **Notifications** at 90% and 95% thresholds for both 5h and 7d windows (once per window, OS-native).
- **Settings window** — display mode, cap calibration (pin to Claude.app's % + reset minutes), API keys (stored in OS keychain).

## Install

### macOS

Download the `.dmg` from the [latest release](https://github.com/gingaboxcs/BurnBar/releases/latest), open it, drag `BurnBar.app` to `/Applications`. Signed + notarized; first launch should "just work."

### Windows

Download the `.msi` installer from the [latest release](https://github.com/gingaboxcs/BurnBar/releases/latest) and run it.

> **Note:** the live Claude consumer quota readout is currently macOS-only (it reads cookies from the local Claude.app install). On Windows you'll see Claude Code session usage + API usage; consumer quota path falls back to the manual calibrate-from-Claude-app flow in settings.

### Linux

Download the `.deb` (Debian/Ubuntu) or `.AppImage` (any distro) from the [latest release](https://github.com/gingaboxcs/BurnBar/releases/latest).

```bash
# .deb
sudo dpkg -i burnbar_0.1.0_amd64.deb

# .AppImage
chmod +x BurnBar_0.1.0_amd64.AppImage
./BurnBar_0.1.0_amd64.AppImage
```

> **Note:** same caveat as Windows — consumer quota readout is macOS-only for now.

## Setup tips

- **Claude consumer quota:** install the official Claude desktop app and sign in once. BurnBar reads the session cookie from its local store; no login flow needed inside BurnBar.
- **Anthropic API:** create an Admin API key at [console.anthropic.com/settings/admin-keys](https://console.anthropic.com/settings/admin-keys) (note: requires admin scope on an organization, not a personal key). Paste in Settings.
- **OpenAI API:** same, create an Admin key at [platform.openai.com/settings/organization/admin-keys](https://platform.openai.com/settings/organization/admin-keys).
- **Auto-start at login:** System Settings → General → Login Items & Extensions → add `BurnBar.app`.
- **Hide from Dock (macOS):** Settings → Display → "Hide from Dock". Tray icon stays visible; no Dock entry.

## Build from source

```bash
git clone https://github.com/gingaboxcs/BurnBar.git
cd BurnBar
pnpm install
pnpm tauri dev    # development with hot reload
pnpm tauri build  # production .app / .msi / .deb
```

Requires:
- **Node 20+** and **pnpm 9+**
- **Rust** stable toolchain (install via [rustup.rs](https://rustup.rs))
- macOS: Xcode Command Line Tools
- Linux: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`
- Windows: Microsoft C++ Build Tools + WebView2

## Privacy

- API keys are stored in your **OS keychain** (macOS Keychain, Windows Credential Manager, Linux Secret Service), never on disk in plaintext.
- Claude.app cookie reading is **read-only** and stays on your machine — nothing is sent anywhere except to `api.anthropic.com`, `api.openai.com`, and `claude.ai` directly using your own credentials.
- No telemetry, no analytics, no remote config.

## License

MIT — see [LICENSE](./LICENSE).