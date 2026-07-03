# Windows release signing

Slideflow's Windows installers (`.msi` / `.exe`) are **Authenticode-signed via
Azure Artifact Signing** (Microsoft's cloud signing service, formerly called
"Trusted Signing" / "Azure Code Signing").

You can do **all of this from a Mac** — you never need a Windows machine or a
USB hardware token. The signing happens on the Windows GitHub Actions runner
using credentials stored as repo secrets, exactly like the macOS notarization
setup. The CI (`.github/workflows/release.yml`) is already wired; it just needs
six repository secrets.

> **Not ready to pay? That's fine.** Windows builds ship **unsigned by default** —
> if none of the `AZURE_*` secrets are set, the signing step is skipped and the
> build still succeeds (users just get the SmartScreen "unknown publisher"
> notice). There is **nothing to do** to stay unsigned.
>
> **Free option for open source:** Because Slideflow is MIT-licensed,
> [**SignPath Foundation**](https://signpath.org/) issues a **free** code-signing
> certificate to qualifying OSS projects, on their cloud HSM, usable from CI on a
> Mac — no monthly fee and no hardware token. It's an application/review process
> and they favour projects with some track record, so it's most realistic once
> Slideflow has a bit of adoption. Same OV semantics as Azure (publisher name
> shows; SmartScreen reputation still builds over time). This is the recommended
> path if cost is the blocker. The paid Azure route below is the fallback for
> when the project is commercial / doesn't qualify for the Foundation.

## First, the honest part: there is no Windows "notarization"

macOS notarization gives you a clean install with **zero warnings** the moment
Apple approves the build. Windows has no equivalent. What a signature buys you:

- **Unsigned today:** SmartScreen says *"Windows protected your PC — unknown
  publisher"* with only a hidden "Run anyway". Scary.
- **Signed with Azure Artifact Signing (this doc):** the publisher name shows as
  **you/your org** and the UAC prompt is blue instead of yellow. But SmartScreen
  can still warn *"not commonly downloaded"* until the file's hash accumulates
  enough download reputation. That reputation builds automatically over time and
  downloads; a standard (OV) certificate — which is what Azure issues — does
  **not** grant it instantly.
- **Only an EV certificate** grants *instant* SmartScreen trust. EV certs
  require a physical FIPS hardware token plugged into the signing machine, which
  can't run in headless CI and can't be driven from a Mac. Not recommended here.

So: Azure Artifact Signing is the right, Mac-friendly, CI-friendly choice. Just
expect the SmartScreen "not commonly downloaded" notice to fade with adoption
rather than vanishing on day one.

> **Why not the old `.pfx`-in-a-secret trick?** Since June 2023 the CA/Browser
> Forum requires every new OV code-signing key to live on certified hardware or
> a cloud HSM — you can no longer download a `.pfx` and base64 it into a secret
> the way the macOS `.p12` works. Azure Artifact Signing *is* that cloud HSM,
> which is why it's the modern path.

## Prerequisites

- An **Azure account** (the free tier is enough to create the resources; signing
  itself is cheap — roughly **$10/month** for the Artifact Signing account plus
  negligible per-signature cost).
- **Eligibility for a Public Trust certificate.** As of April 2026 this covers
  **individual/self-employed developers in the USA and Canada** (no more 3-year
  history requirement) and **organizations in the USA, Canada, the EU, and the
  UK**. Microsoft verifies your identity as part of setup.
  - If you're an individual **outside** the US/Canada, you can't use Azure
    Artifact Signing yet — see [Alternative](#alternative-if-youre-not-eligible-for-azure)
    at the bottom.

## 1. Create the Azure signing resources

In the [Azure Portal](https://portal.azure.com):

1. **Create a Trusted/Artifact Signing account.** Search the marketplace for
   **"Trusted Signing"** (a.k.a. Artifact Signing) → Create. Pick a resource
   group and a region — the region determines your **endpoint URL**, e.g.
   `https://eus.codesigning.azure.net` (East US), `https://weu.codesigning.azure.net`
   (West Europe). Give the account a **name** (this is your
   `AZURE_CODE_SIGNING_ACCOUNT_NAME`).
2. **Verify your identity.** Under the account → **Identity validations**, create
   one for yourself (individual) or your organization and complete Microsoft's
   verification. This can take from minutes to a few days.
3. **Create a Certificate Profile.** Once the identity is *Completed*, go to
   **Certificate profiles → Create** and choose profile type **Public Trust**
   (this is what browsers/Windows trust for downloaded apps). Its **name** is
   your `AZURE_CERT_PROFILE_NAME`.

## 2. Create an App Registration (the CI's login)

CI authenticates to Azure as a service principal:

1. **Microsoft Entra ID → App registrations → New registration.** Name it e.g.
   `slideflow-signing`. After creating it, copy:
   - **Application (client) ID** → `AZURE_CLIENT_ID`
   - **Directory (tenant) ID** → `AZURE_TENANT_ID`
2. **Certificates & secrets → New client secret.** Copy the secret **Value**
   immediately (it's shown once) → `AZURE_CLIENT_SECRET`.

## 3. Grant the App Registration permission to sign

On the **Trusted/Artifact Signing account → Access control (IAM) → Add role
assignment**, assign the role **`Trusted Signing Certificate Profile Signer`**
to the `slideflow-signing` app registration. Without this the build fails with a
403 at signing time.

## 4. Add the six repository secrets

**GitHub repo → Settings → Secrets and variables → Actions → New repository
secret**, add all six:

| Secret | Value | From |
|--------|-------|------|
| `AZURE_CLIENT_ID` | App registration client ID | step 2 |
| `AZURE_CLIENT_SECRET` | App registration client secret **value** | step 2 |
| `AZURE_TENANT_ID` | Directory (tenant) ID | step 2 |
| `AZURE_ENDPOINT` | Region endpoint, e.g. `https://eus.codesigning.azure.net` | step 1 |
| `AZURE_CODE_SIGNING_ACCOUNT_NAME` | Signing account name | step 1 |
| `AZURE_CERT_PROFILE_NAME` | Certificate profile name | step 1 |

Add **all six before the next `v*` tag push**. With any of them missing, the
`Set up Windows signing` step is skipped and the Windows build falls back to
**unsigned** (it still succeeds — just no signature).

## 5. Verify a release

Push a `v*` tag. On the resulting draft release, download the `.msi` (or the
NSIS `.exe`). On a Windows machine (or ask anyone with one):

- Right-click the installer → **Properties → Digital Signatures** tab. You
  should see a signature with your verified name and a valid timestamp.
- Or in PowerShell: `Get-AuthenticodeSignature .\Slideflow_*.msi` should report
  `Status : Valid`.

If you have no Windows machine at all, the CI log of the `Build the app` step is
your confirmation — a successful `trusted-signing-cli` invocation over each
artifact means it signed.

## How the CI wiring works (for maintainers)

- The six `AZURE_*` secrets are exposed as **job-level env** in
  `release.yml` so the `if:` guard can detect their presence.
- The **`Set up Windows signing`** step runs only on `windows-latest` and only
  when `AZURE_CLIENT_ID` is non-empty. It `cargo install`s
  [`trusted-signing-cli`](https://crates.io/crates/trusted-signing-cli) and
  writes `apps/desktop/sign.windows.conf.json` containing a single
  `bundle.windows.signCommand`.
- That overlay is merged into the build via `--config` appended to the
  tauri-action `args`. Keeping `signCommand` out of the committed
  `tauri.conf.json` means local `pnpm tauri build` and unsigned CI both keep
  working — the overlay only exists on a secrets-present Windows run.
- `trusted-signing-cli` authenticates using `AZURE_CLIENT_ID` /
  `AZURE_CLIENT_SECRET` / `AZURE_TENANT_ID` from the environment.

> **Tool naming note:** the crate is mid-rename to `artifact-signing-cli` to
> match Azure's rebrand. If a future Tauri release expects that binary, change
> both the `cargo install` name in the workflow and the first token of
> `signCommand` accordingly.

## Notes

- Client secrets expire (default ~6–24 months). When CI starts failing auth,
  create a new secret value in the App Registration and update
  `AZURE_CLIENT_SECRET`.
- The macOS side is documented separately in
  [`RELEASE_SIGNING.md`](./RELEASE_SIGNING.md).

## Alternative if you're not eligible for Azure

If you can't use Azure Artifact Signing (e.g. an individual outside the
US/Canada), a **cloud code-signing service** gives the same "sign in CI, no
hardware token, works from a Mac" experience with a traditional OV certificate:

- **SSL.com eSigner** or **DigiCert KeyLocker** — both issue an OV cert kept in
  their cloud HSM and provide a CLI/credentials you drop into the same
  `signCommand` slot (different command, same wiring). Costs more than Azure
  (typically ~$100–250/yr) and identity verification is broader by country.

The signature semantics are identical to Azure's: publisher name shown,
SmartScreen reputation still builds over time.
