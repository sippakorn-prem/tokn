# Releasing Tokn

Tokn ships as a signed + notarized universal macOS app, published to
[GitHub Releases](https://github.com/sippakorn-prem/tokn/releases) by CI, with
in-app auto-updates via the Tauri updater.

Two pipelines exist:

- **CI (primary)** — push a `v*` tag, GitHub Actions builds/signs/notarizes and
  publishes the release. See [`.github/workflows/release.yml`](.github/workflows/release.yml).
- **Local (fallback)** — `make release` builds the same artifacts on your Mac
  from `.env`. Needs the signing cert installed locally.

---

## One-time setup

### 1. Developer ID Application certificate

You need a **Developer ID Application** cert in your login keychain (an Apple
Developer Program membership alone is not enough).

Xcode → Settings → Accounts → (your Apple ID) → **Manage Certificates** → **+**
→ **Developer ID Application**. Confirm it installed:

```sh
security find-identity -v -p codesigning
# -> "Developer ID Application: Your Name (TEAMID1234)"
```

### 2. Export the cert for CI (base64 .p12)

CI can't read your keychain, so export the cert + private key as a `.p12` and
base64-encode it:

1. Keychain Access → **login** keychain → **My Certificates** → right-click the
   "Developer ID Application" cert → **Export…** → save as `cert.p12`, set an
   export password.
2. Encode it:

```sh
base64 -i cert.p12 | pbcopy   # now on your clipboard for APPLE_CERTIFICATE
```

### 3. Set GitHub Actions secrets

Settings → Secrets and variables → Actions, or via `gh`:

```sh
gh secret set APPLE_CERTIFICATE               # paste the base64 blob
gh secret set APPLE_CERTIFICATE_PASSWORD      # the .p12 export password
gh secret set APPLE_SIGNING_IDENTITY          # Developer ID Application: Name (TEAMID1234)
gh secret set APPLE_ID                        # your Apple ID email
gh secret set APPLE_PASSWORD                  # app-specific password (account.apple.com)
gh secret set APPLE_TEAM_ID                   # 10-char Team ID

# Updater signing key (generated once, kept outside the repo):
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/tokn-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # the key password ("" if none)
```

> The updater **public** key lives in `src-tauri/tauri.conf.json`
> (`plugins.updater.pubkey`) and is safe to commit. The **private** key
> (`~/.tauri/tokn-updater.key`) must never be committed — losing it means you
> can no longer ship updates existing installs will accept.

---

## Cutting a release

1. Bump `version` in `src-tauri/tauri.conf.json` (must be higher than the
   installed version for the updater to pick it up).
2. Commit, then tag and push:

```sh
git commit -am "chore: release v0.2.0"
git tag v0.2.0
git push && git push origin v0.2.0
```

CI builds the universal app, signs + notarizes it, and publishes a release with:

- `Tokn_x.y.z_universal.dmg` — the download for new users
- `Tokn.app.tar.gz` + `.sig` — the updater payload
- `latest.json` — the updater manifest (served at
  `releases/latest/download/latest.json`, which the app polls on launch)

## How updates reach users

On launch the app checks `latest.json`; if a newer signed build exists it
downloads and installs it in the background. The update takes effect the next
time the app is launched — no prompt, no interruption.
