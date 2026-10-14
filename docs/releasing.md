# Releasing TextureCraft

The recipe is craftrules' [release playbook](release-playbook.md); this page covers what is
specific to TextureCraft. The pipeline is [`.github/workflows/release.yml`](../.github/workflows/release.yml).

## Cutting a release

1. On `main`, set the version: `cargo xtask version set 1.2.0` (rewrites `[workspace.package]
   version` in the root `Cargo.toml`, the single source of truth, and refreshes `Cargo.lock`).
   `cargo xtask version` prints it. A version with a hyphen (`1.2.0-rc.1`) becomes a pre-release.
2. Merge `main` into `release` (fast-forward) and push. Only org admins and the `release-managers`
   team can push `release`.
3. The run builds every platform and creates or updates the **draft** GitHub Release
   `TextureCraft v<version>`, tag `v<version>`, with `SHA256SUMS.txt`. Review it, then publish.
   A draft is updated in place (assets replaced); a published release is never overwritten, so
   bump the version before building again.

Test builds: dispatch the workflow on `release` with a `version` such as `1.2.0-rc.1`; the input
overrides `Cargo.toml` for that run (`cargo xtask version set` runs in every build job). A
dispatch on any other branch is a dry run: Linux, Flatpak, FreeBSD and web build for real, the
signing jobs are refused by the `release` environment (0 steps), and no draft is created.

Before the first release, an org admin runs craftrules `release/signing-setup.md` steps 1-2 for
`storytold/texturecraft` (the `release` environment, its deployment-branch rule, the ruleset and
the twelve secrets) and adds the repo to the org's Default runner group (`release/cut-a-release.md`
§3), or the macOS job waits forever.

## What gets built

All file names are `texturecraft-<version>-<platform>-<arch>.<ext>`, written to `dist/release/`.

| Platform | Artifacts | Script |
|---|---|---|
| macOS (`macos-15-xlarge`) | `-macos-universal.dmg` (`TextureCraft.app`), `texturecraft-cli-<version>-macos-universal.zip` | `packaging/macos/package.sh`, `verify.sh` |
| Windows x64, x86, arm64 | `-windows-<arch>.msi`, `-windows-<arch>-portable.zip` | `packaging/windows/package.ps1` |
| Linux x86_64, aarch64 (Ubuntu 22.04, glibc 2.35) | `.AppImage` + `.AppImage.zsync`, `.deb`, `.rpm`, `.tar.gz` | `packaging/linux/package.sh` |
| Flatpak x86_64, aarch64 | `-linux-<arch>.flatpak`, repackaged from the Linux tarball | `packaging/linux/flatpak-bundle.sh` |
| FreeBSD x86_64 (14.3 VM) | `-freebsd-x86_64.tar.gz` | `packaging/freebsd/package.sh` |
| Web | `texturecraft-web-<version>.zip` (trunk, `wasm-release` profile; hosting notes in `packaging/web/README.md`) | `packaging/web/package.sh` |

Every desktop package ships both binaries, `texturecraft` and `texturecraft-cli`. The packaging
scripts smoke-test `texturecraft-cli --version`, and the Linux job also runs the AppImage with
`--version`, so both binaries must print their version and exit without opening a window.

## TextureCraft-specific values

| What | Value |
|---|---|
| Bundle / app id | `ai.storyteller.texturecraft` |
| Project format | `.tcraft`, MIME `application/x-texturecraft`, UTI `ai.storyteller.texturecraft.project` (owned) |
| Also opens | `.glb .gltf .obj .ply .stl` ("Open with" only, never the default app) |
| WiX UpgradeCode | `7E670E60-BB22-4E6B-8749-DD3FD4CA8979`, never change it |
| Flathub manifest | `packaging/linux/flatpak/ai.storyteller.TextureCraft.yml` (release bundle: `…TextureCraft.bundle.yml`) |
| macOS minimum | 11.0 (`LSMinimumSystemVersion`, `MACOSX_DEPLOYMENT_TARGET`) |

## Version and build info in the binaries

`apps/texturecraft/build.rs` (also `include!`d by `apps/texturecraft-cli/build.rs`) sets, for
`env!`:

- `TEXTURECRAFT_BUILD_SHA`: the full commit. CI sets it to `github.sha`; `packaging/env.sh` sets
  it from git; otherwise build.rs asks git itself; empty when unknown.
- `TEXTURECRAFT_BUILD_DATE`: `YYYY-MM-DD` (UTC). CI sets it once per run; otherwise today's date,
  or `SOURCE_DATE_EPOCH`'s for reproducible builds.

On Windows targets it also embeds `assets/app-icon/texturecraft.ico` and VERSIONINFO
(ProductName TextureCraft); `package.ps1` sets `TEXTURECRAFT_REQUIRE_WINRES=1` so a release can't
ship an exe without them. The version itself is `CARGO_PKG_VERSION`.

Other environment variables the scripts read: `TEXTURECRAFT_VERSION` (override the version),
`DIST`, `CARGO_TARGET_DIR`, `TEXTURECRAFT_MAINTAINER` (deb/rpm maintainer),
`TEXTURECRAFT_WASM_MAX_BYTES` (web size gate, default 24 MiB), and at run time
`TEXTURECRAFT_NO_DESKTOP_INTEGRATION=1` (the AppImage's AppRun skips installing its `.desktop`
entry and icons).

## Signing

Secrets live only in the `release` environment and are all optional: a missing one gives an
unsigned artifact and a `::warning::`, never a failed build.

- macOS: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD` (import into a
  temporary keychain, `packaging/macos/import-cert.sh`), then `APPLE_ID`, `APPLE_PASSWORD`,
  `APPLE_TEAM_ID` for `notarytool` and stapling. Without them the app is ad-hoc signed.
- Windows: `packaging/windows/sign.ps1` uses a PFX (`WINDOWS_CERTIFICATE`,
  `WINDOWS_CERTIFICATE_PASSWORD`) if set, otherwise Azure Trusted Signing (all six `AZURE_*`).
  It signs both exes before WiX and the MSI after.

## Icons

`assets/app-icon/icon.py` draws the octopus (original artwork, three colours, craftrules
`standards/icon-design.md`) into `icon.svg`. `packaging/icons.sh` renders everything else with
resvg and `cargo xtask ico|icns`: the hicolor PNGs (16-512 px) and scalable SVG, `icon-1024.png`,
`texturecraft.ico` and `texturecraft.icns`. The outputs are committed; rerun both after changing
the drawing and keep `ATTRIBUTION.md` in step.

## Local checks

- `cargo xtask ci`: fmt, clippy, tests, `cargo xtask layers`, `cargo xtask wasm`.
- `bash packaging/linux/apprun-test.sh` and `bash packaging/freebsd/package.sh --dry-run` run
  anywhere with bash.
- `packaging/macos/package.sh --arch aarch64` on a Mac builds an ad-hoc signed DMG.
- `packaging/windows/package.ps1 -Arch x64` on Windows (needs WiX 5.0.2) builds an unsigned MSI.
- `.github/workflows/packaging-lint.yml` runs actionlint, shellcheck, the PowerShell parser and
  the desktop/AppStream/Flatpak validators on every change to `packaging/` or the workflows.
