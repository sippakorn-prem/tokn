# Tokn

[![Latest release](https://img.shields.io/github/v/release/sippakorn-prem/tokn?sort=semver)](https://github.com/sippakorn-prem/tokn/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/sippakorn-prem/tokn/total)](https://github.com/sippakorn-prem/tokn/releases)
![Platform: macOS](https://img.shields.io/badge/platform-macOS%2010.15%2B-lightgrey)
[![License: MIT](https://img.shields.io/github/license/sippakorn-prem/tokn)](LICENSE)

Tokn is a free, open-source macOS menu bar app that shows your coding-agent usage at a glance. It tracks both **Claude Code** and the **OpenAI Codex CLI** — switch between them with the selector in the popover, and the tray icon follows your choice.

For each provider it shows current-session and weekly usage, reset countdowns, and a recent burn-rate trend.

## Install

**[⬇︎ Download the latest release](https://github.com/sippakorn-prem/tokn/releases/latest)**, then:

1. Open `Tokn_<version>_universal.dmg`.
2. Drag **Tokn** into **Applications**.
3. Launch it — Tokn lives in the **menu bar** (no Dock icon).
   - **Left-click** the tray icon for the usage popover.
   - **Right-click** it to **Quit** (or press `⌘Q` while the popover is open).

Signed and notarized by Apple, universal (Apple Silicon + Intel), requires **macOS 10.15+**. Tokn auto-updates on launch. Resize the popover with `⌘+` / `⌘−`, reset with `⌘0`.

To see usage you need:

- **Claude** — the **Claude Code CLI** signed in.
- **Codex** — the **Codex CLI** to have run at least once.

## How it works

Everything runs locally — no server, no telemetry.

- **Claude:** Tokn reads Claude Code's login from the macOS Keychain and calls Anthropic's usage endpoint with it. The token goes only to Anthropic; Tokn never stores or proxies it.
- **Codex:** the Codex CLI records its own per-turn usage under `~/.codex/sessions/`. Tokn just reads the newest window from disk — no network, no token, no Keychain, and it never touches Codex's credentials.

Because Codex logs are written per turn, Codex numbers refresh as you use Codex; when it's idle, Tokn shows the last recorded state. Codex limits are account-wide.

## Notes

Tokn is unofficial — not made by, or affiliated with, Anthropic or OpenAI. It depends on Claude Code's Keychain format, Anthropic's usage endpoint, and Codex's local log format, any of which may change. Only run builds you trust.

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) to build, run, and test locally.

## License

MIT — see [LICENSE](LICENSE). Built with Tauri, Rust, React, and TypeScript.
