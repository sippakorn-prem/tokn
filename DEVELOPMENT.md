# Development

How to build, run, and test Tokn locally. For what Tokn is and how to install it, see the [README](README.md).

## Requirements

- macOS
- Node.js and pnpm
- Rust
- Tauri system dependencies
- Claude Code CLI and/or Codex CLI, if you want to test live usage

## Commands

```sh
pnpm install      # install dependencies
pnpm tauri dev    # run the app
pnpm build        # build the frontend
```

Check the Rust backend:

```sh
cd src-tauri
cargo check
cargo test
```

## Testing usage

### Claude

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

### Codex

Happy path:

1. Run the Codex CLI in your terminal so it records a turn.
2. Run Tokn and switch to **Codex** in the popover.
3. Tokn should show usage from the newest `~/.codex/sessions/` log.

No-activity path: if `~/.codex/sessions/` has no usage yet, the Codex view shows the "No Codex CLI activity yet" gate until you run Codex.

## Tech stack

Tauri 2 · Rust · React 19 · TypeScript · Vite
