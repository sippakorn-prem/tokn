# Tokn

[![Latest release](https://img.shields.io/github/v/release/sippakorn-prem/tokn?sort=semver)](https://github.com/sippakorn-prem/tokn/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/sippakorn-prem/tokn/total)](https://github.com/sippakorn-prem/tokn/releases)
![Platform: macOS](https://img.shields.io/badge/platform-macOS%2010.15%2B-lightgrey)
[![License: MIT](https://img.shields.io/github/license/sippakorn-prem/tokn)](LICENSE)

Tokn is a free, open-source macOS menu bar app for watching Claude Code usage.

It shows your current Claude usage at a glance, including:

- current session usage
- weekly limit usage
- reset countdowns
- recent burn-rate trend
- a tray icon that reflects the highest active usage window

Tokn is built with Tauri, Rust, React, and TypeScript.

## Install

**[⬇︎ Download the latest release](https://github.com/sippakorn-prem/tokn/releases/latest)**, then:

1. Open the downloaded `Tokn_<version>_universal.dmg`.
2. Drag **Tokn** into your **Applications** folder.
3. Launch Tokn. It runs in the **menu bar** — there is no Dock icon or app-switcher entry.
   - **Left-click** the tray icon → the usage popover
   - **Right-click** (or Control-click) the tray icon → **Quit Tokn** (or press `⌘Q` while the popover is open)

Tokn is signed with an Apple Developer ID and notarized by Apple, so it opens without Gatekeeper warnings. It's a universal build (Apple Silicon + Intel) and requires **macOS 10.15 or later**.

You'll need to be signed in to the **Claude Code CLI** first — Tokn reads that existing login from the macOS Keychain (see [How It Works](#how-it-works)).

### Updates

Tokn checks for new releases on launch and installs them in the background — you don't need to re-download to update.

### Zoom

Press `⌘+` / `⌘−` to resize the popover, `⌘0` to reset, or use the zoom control in the footer.

## Important

Tokn is an unofficial app. It is not made by Anthropic and is not affiliated with Anthropic.

Tokn currently works with the Claude Code CLI login on macOS. It does not log you in to Claude, and it does not read Claude Desktop login state directly.

That means:

- logging out of Claude Desktop may not log out Claude Code
- Tokn can still work if Claude Code is still logged in
- Tokn does not currently support `ANTHROPIC_API_KEY` usage tracking

## How It Works

Claude Code stores its login credential in macOS Keychain. Tokn reads that local Keychain item and uses it to request usage data from Anthropic.

This behavior is documented by Anthropic in [Claude Code docs: Authentication — Credential management](https://code.claude.com/docs/en/authentication#credential-management):

> On macOS, credentials are stored in the encrypted macOS Keychain.

The flow is:

1. You sign in to Claude Code in your terminal.
2. Claude Code saves a credential in macOS Keychain.
3. Tokn asks macOS Keychain for:

   ```sh
   Claude Code-credentials
   ```

4. Tokn reads the Claude Code OAuth access token from that Keychain value.
5. Tokn calls Anthropic's usage endpoint with that token.
6. Tokn renders the usage meters locally in the menu bar popover.

Tokn does not run a server, proxy your token, or send the token to a third-party backend.

## Safety Model

Tokn is designed to be local-first:

- The Claude Code credential is read from macOS Keychain on your Mac.
- macOS controls whether Tokn can access the Keychain item.
- Tokn does not store the Claude Code token in its own database or config file.
- Tokn does not include analytics or telemetry.
- Tokn sends usage requests directly to Anthropic from your Mac.

What still leaves your Mac:

- The Claude Code access token is sent to Anthropic as a bearer token so Anthropic can return usage data.
- The usage API response comes back from Anthropic.

Risks and limitations:

- Tokn depends on Claude Code's current Keychain credential format.
- Tokn depends on an Anthropic usage endpoint that may change.
- Any app that can read a credential can misuse it, so only run builds you trust.
- Public or commercial distribution should be reviewed carefully against Anthropic's current terms and policies.

## Auth States

If Claude Code is logged in and the token is valid, Tokn shows the live meters.

If Claude Code is not logged in, Tokn shows:

```text
Log in to Claude Code first
Open Claude Code, sign in, then come back to Tokn and retry.
```

If the token is expired, Tokn asks you to sign in to Claude Code again.

If macOS blocks Keychain access, Tokn asks you to allow Keychain access and retry.

## Local Development

Requirements:

- macOS
- Node.js and pnpm
- Rust
- Tauri system dependencies
- Claude Code CLI, if you want to test live usage

Install dependencies:

```sh
pnpm install
```

Run the Tauri app:

```sh
pnpm tauri dev
```

Build the frontend:

```sh
pnpm build
```

Check the Rust backend:

```sh
cd src-tauri
cargo check
```

## Testing The Login Flow

Happy path:

1. Sign in to Claude Code in your terminal.
2. Run Tokn.
3. Allow Keychain access if macOS asks.
4. Tokn should show usage.

Unauthenticated path:

```sh
claude logout
pnpm tauri dev
```

If Tokn still shows usage, Claude Code may still have a valid Keychain credential. To force a clean unauthenticated test, remove the saved credential:

```sh
security delete-generic-password -s "Claude Code-credentials"
```

Only run that command if you are okay signing in to Claude Code again.

## Tech Stack

- Tauri 2
- Rust
- React 19
- TypeScript
- Vite

## License

Tokn is free and open source under the MIT License. See [LICENSE](LICENSE).
