---
name: release-notes
description: Write a GitHub release note for a new Tokn version in the project's house style. Use when drafting release notes, changelog, or "What's new" copy for a vX.Y.Z tag.
---

# Tokn release notes

Draft the GitHub release note for a Tokn version in the house style below. Tokn
is a free macOS menu-bar app that shows Claude Code usage; the audience is end
users, not developers.

## Steps

1. **Get the version** from the user or from `src-tauri/tauri.conf.json`
   (`version`). Call it `vX.Y.Z`.
2. **Find the previous release tag**: `git tag --list 'v*' --sort=-v:refname | head`.
   The most recent one below the new version is `PREV`.
3. **Gather what changed** since `PREV`:
   `git log PREV..HEAD --oneline` (and the merged PRs, if any). Translate the
   commits into **user-facing** benefits — never list raw commit subjects or
   internal refactors. Group them; usually 1–3 headline items is right.
4. **Draft** using the structure and tone below, then hand it to the user to
   paste into the GitHub release. Do not publish it yourself.

## Structure (in order)

1. **One-line hook** — a single plain sentence stating what this release does
   for the user. No heading above it.
2. `## What's new in vX.Y.Z` — one bullet per headline change:
   - Start with an emoji + **bold feature name**, then a plain-language
     description of what the user gets.
   - Where it helps, add a short **"Previously … now …"** contrast sentence so
     the improvement is obvious.
3. `## Install` — keep the standard block below verbatim, only swapping the
   version in the DMG filename.
4. `## What's Changed` — the auto-generated PR list (leave GitHub's default, or
   paste it in).
5. `**Full Changelog**: https://github.com/sippakorn-prem/tokn/compare/PREV...vX.Y.Z`

## Tone

- Speak to users, in benefits, not implementation. "Opens over full-screen
  apps" — not "reclass the window to a non-activating NSPanel".
- Warm and concrete. Emoji lead on feature bullets only.
- Only claim what shipped. If unsure a change is user-visible, leave it out.

## Standard Install block (swap the version in the filename)

```markdown
## Install

1. Download **`Tokn_X.Y.Z_universal.dmg`** below and open it.
2. Drag **Tokn** into your **Applications** folder.
3. Launch it — Tokn lives in the **menu bar** (left-click for usage, right-click / ⌘Q to quit).

Signed with an Apple Developer ID and notarized by Apple. Universal build (Apple Silicon + Intel), macOS 10.15+.

> Requires being signed in to the Claude Code CLI — Tokn reads that login from the macOS Keychain.

Already running an earlier version? This one installs automatically on next launch.
```

If the previous release (v0.3.0+) already shipped the in-app updater, you may
instead note that existing users get a **"Restart to update"** bar in the
popover and can update with one click.

## Worked example (v0.3.0 — matches the desired style)

```markdown
Tokn now keeps itself up to date and tells you when a new version is ready.

## What's new in v0.3.0

- 🔄 **Smarter auto-updates** — Tokn checks for new versions on launch **and every few hours while it's running**, downloads them quietly in the background, and shows a **"Restart to update"** bar in the popover. One click relaunches straight into the new version — no re-downloading, no reinstalling, no Gatekeeper prompts.

Previously updates only applied when you happened to quit and reopen Tokn; now you'll actually know one's ready and can apply it with a single click.

## Install

1. Download **`Tokn_0.3.0_universal.dmg`** below and open it.
2. Drag **Tokn** into your **Applications** folder.
3. Launch it — Tokn lives in the **menu bar** (left-click for usage, right-click / ⌘Q to quit).

Signed with an Apple Developer ID and notarized by Apple. Universal build (Apple Silicon + Intel), macOS 10.15+.

> Requires being signed in to the Claude Code CLI — Tokn reads that login from the macOS Keychain.

Already running an earlier version? This one installs automatically on next launch.

## What's Changed
* Develop by @sippakorn-prem in https://github.com/sippakorn-prem/tokn/pull/3

**Full Changelog**: https://github.com/sippakorn-prem/tokn/compare/v0.2.0...v0.3.0
```
