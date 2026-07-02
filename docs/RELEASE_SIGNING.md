# macOS release signing & notarization

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
- Windows `.msi` / `.exe` are still unsigned (SmartScreen). That's a separate
  cert (`WINDOWS_CERTIFICATE`) not covered here.

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
