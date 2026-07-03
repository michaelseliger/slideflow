# Release signing (macOS notarization + updater packages)

Slideflow's release DMGs must be **signed with a Developer ID Application
certificate and notarized by Apple**, or macOS Gatekeeper shows the
"Slideflow is damaged and can't be opened" error on download.

The CI (`.github/workflows/release.yml`) is already wired to sign + notarize —
it just needs six repository secrets. This is the one-time setup to produce them.

## Prerequisites

- **Apple Developer Program** membership ($99/yr): <https://developer.apple.com/programs/>.
  A free Apple ID is **not** enough — Developer ID certs and notarization require
  a paid membership.

## 1. Create a Developer ID Application certificate

> Must be **Developer ID Application** — *not* "Apple Development" or
> "Apple Distribution". Only Developer ID can be notarized for distribution
> outside the App Store.

Easiest path (Xcode): **Settings → Accounts → select your team → Manage
Certificates → + → "Developer ID Application"**. It installs into your login
keychain.

Portal alternative: **Certificates, IDs & Profiles → Certificates → + →
Developer ID Application** and follow the CSR upload steps.

Confirm it's installed and read its identity string:

```bash
security find-identity -v -p codesigning
# → "Developer ID Application: Your Name (ABCDE12345)"
```

That full string is your **APPLE_SIGNING_IDENTITY**. The 10-char code in
parentheses is your **APPLE_TEAM_ID**.

## 2. Export the certificate as a .p12

In **Keychain Access → login → Certificates**, right-click the *Developer ID
Application* entry → **Export** → save as `certificate.p12` and set an export
password (this becomes **APPLE_CERTIFICATE_PASSWORD**).

Base64-encode it for the GitHub secret:

```bash
base64 -i certificate.p12 | pbcopy   # clipboard now holds APPLE_CERTIFICATE
```

Delete `certificate.p12` afterwards — the secret is all CI needs.

## 3. App-specific password for notarization

1. <https://account.apple.com> → **Sign-In and Security → App-Specific
   Passwords → +** → label it e.g. `slideflow-notarization`.
   The generated password is **APPLE_PASSWORD**.
2. **APPLE_ID** is the Apple ID email of that account.

## 4. Add the six repository secrets

**GitHub repo → Settings → Secrets and variables → Actions → New repository
secret**, add all six:

| Secret | Value |
|--------|-------|
| `APPLE_CERTIFICATE` | base64 of the `.p12` (step 2) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password (step 2) |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` (step 1) |
| `APPLE_ID` | your Apple ID email (step 3) |
| `APPLE_PASSWORD` | the app-specific password (step 3) |
| `APPLE_TEAM_ID` | your 10-char Team ID (step 1) |

Add **all six before the next `v*` tag push** — a partial set makes the macOS
build fall back to unsigned.

## 5. Verify a release

Push a `v*` tag, download the resulting DMG on a Mac, drag the app to
`/Applications`, then:

```bash
# Should say: accepted  source=Notarized Developer ID
spctl -a -vvv -t exec /Applications/Slideflow.app

# Authority chain should include "Developer ID Application: ..." and a Team ID
codesign -dvvv /Applications/Slideflow.app 2>&1 | grep -E 'Authority|TeamIdentifier'

# The notarization ticket should be stapled to the DMG
xcrun stapler validate ~/Downloads/Slideflow_*.dmg
```

If all three pass, users can double-click with no warning.

## Notes

- The `x86_64` (Intel) DMG is signed + notarized by the same secrets.
- Developer ID Application certs expire (~5 years) — re-export and update
  `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD` when that happens.
- Windows `.msi` / `.exe` signing is covered separately in
  [`WINDOWS_SIGNING.md`](./WINDOWS_SIGNING.md) (Azure Artifact Signing). There is
  no Windows equivalent of notarization.

## Un-breaking an unsigned build by hand (stopgap)

Until the secrets are in place, an already-downloaded unsigned DMG can be made
to run locally:

```bash
# after copying Slideflow.app to /Applications:
codesign --force --deep --sign - /Applications/Slideflow.app
xattr -dr com.apple.quarantine /Applications/Slideflow.app
open /Applications/Slideflow.app
```

This is a local workaround only — you can't ship a re-signed-by-hand build to
other users. Notarization (above) is the real fix.

---

# Updater signing & the release flow

The in-app auto-updater (`src-tauri/src/updates.rs`) polls
`https://github.com/michaelseliger/slideflow/releases/latest/download/latest.json`
and only installs packages whose **minisign** signature matches the `pubkey`
in `tauri.conf.json`. This is completely separate from Apple code signing.

## One-time setup

1. Generate the keypair (pick a password):

   ```bash
   cd apps/desktop
   pnpm tauri signer generate -w ~/.tauri/slideflow.key
   ```

   **Back up `~/.tauri/slideflow.key` + its password in a password manager.**
   Losing the key permanently strands every installed client — they would
   never accept another update and need a manual reinstall.

2. Paste the content of `~/.tauri/slideflow.key.pub` into
   `plugins.updater.pubkey` in `apps/desktop/src-tauri/tauri.conf.json`.

3. Add the two repository secrets:

   | Secret | Value |
   |--------|-------|
   | `TAURI_SIGNING_PRIVATE_KEY` | content of `~/.tauri/slideflow.key` |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the key password |

## Local builds now need the key

Because `tauri.conf.json` sets `createUpdaterArtifacts: true`, **every**
`pnpm tauri build` — local or CI — fails with *"A public key has been found,
but no private key"* unless these env vars are set:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/slideflow.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="…"
```

## Shipping an update, end to end

1. Bump + tag: `cargo release patch` in `apps/desktop/src-tauri/` (keeps
   `Cargo.toml`, `tauri.conf.json` and `package.json` in lockstep), then push
   the `v*` tag.
2. CI populates a **draft** release: per-platform installers, updater
   packages (`.app.tar.gz` / `.exe` / `.msi` / `.AppImage`) each with a
   `.sig`, and a merged `latest.json`.
3. **Before publishing, verify the draft**: `latest.json` must contain all
   four platform keys (`darwin-aarch64`, `darwin-x86_64`, `linux-x86_64`,
   `windows-x86_64`) and every updater artifact needs its `.sig` sibling.
   The manifest merge across the four CI jobs is read-modify-write without
   locking — if a rare race dropped a platform, re-run that platform's job.
4. Write the release notes in the draft body (the in-app "What's new" links
   here).
5. **Publish the release — that's the go-live switch.** Running apps pick the
   update up on their next daily check (or a manual "Check for Updates…" in
   the About dialog), download it in the background, and install it on
   restart or quit.

Notes:

- macOS jobs must keep `--bundles app,dmg` — the `.app.tar.gz` updater
  artifact is only produced when `app` is an explicitly requested target.
- Linux: only AppImage installs self-update; deb/rpm users update via their
  package manager (the app hides update UI there).
- Draft releases are invisible to the endpoint, so a tag can sit unpublished
  as long as needed.
