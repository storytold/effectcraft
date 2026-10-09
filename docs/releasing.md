# Releasing EffectCraft

Every push to the `release` branch runs [`.github/workflows/release.yml`](../.github/workflows/release.yml).
It builds signed installers for macOS and Windows, packages for Linux and FreeBSD, and the web
build, then creates or updates a
**draft** GitHub Release named `EffectCraft v<version>`. Nobody sees a draft until a maintainer
publishes it.

User-facing names say **EffectCraft**. Files, binaries and ids stay lowercase
(`effectcraft-<version>-<platform>-<arch>.<ext>`).

## Cutting a release

1. **Bump the version** on `main`. It lives in one place, `[workspace.package] version` in the
   root `Cargo.toml`:

   ```sh
   cargo xtask version                 # prints the current version, e.g. 0.3.1
   cargo xtask version set 0.4.0       # or 0.4.0-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Open a PR titled `Release: EffectCraft v0.4.0` with that change (`Cargo.toml` and our crates
   in `Cargo.lock` only) and merge it.
2. **Check the gate.** Run `cargo xtask ci` on `main` (it runs clippy and the tests in release
   mode, as CI does), and check that `main`'s CI is green on every platform.
3. **Push `main` to `release`** (a fast-forward): `git push origin main:release`. The `release`
   branch is protected; only maintainers can push to it. The workflow starts by itself.
4. **Wait for the draft.** When every job is green (macOS notarization is the slow part), the
   Releases page has a draft `EffectCraft v0.4.0`, tagged `v0.4.0` on the pushed commit, with
   every artifact and `SHA256SUMS.txt`. Its notes are generated from the merged PRs.
5. **Check it.** Download an installer or two, check them against `SHA256SUMS.txt`, and run
   `effectcraft --version` / `effectcraft-cli --version`. Read the job summaries: a `::warning::`
   means a signing secret was missing and that artifact is unsigned.
6. **Publish** the draft in the GitHub UI (or `gh release edit v0.4.0 --draft=false --latest`),
   with a short *Highlights* section above the generated notes. Publishing creates the `v0.4.0`
   tag. Versions with a pre-release suffix (`-rc.1`) are marked as pre-releases.

Pushing to `release` again before you publish rebuilds the same draft and replaces its assets.
Once the draft is published, the workflow refuses to touch that version again
(`Release v<version> is already published. Bump the version (cargo xtask version set) before
pushing to release.`), so bump it first.

**Test runs:** *Actions › Release › Run workflow* runs the whole pipeline by hand. The optional
`version` input (such as `0.4.0-rc.1`) overrides `Cargo.toml` for that run only: the jobs apply it
with `cargo xtask version set` before building, so the binaries report it too. Signing and the
draft release need the `release` environment, which only the `release` branch can use. Run on
any other branch (`gh workflow run release.yml --ref <branch>`), it is a dry run: the jobs that
sign nothing (Linux, Flatpak, FreeBSD, web) build and check everything, the macOS and Windows
jobs are refused by the environment's branch rule, and no draft release is made.

## What gets built

| Platform | Artifacts | Built on |
|---|---|---|
| macOS 11+ (universal: Apple silicon and Intel) | `effectcraft-<v>-macos-universal.dmg`, `effectcraft-cli-<v>-macos-universal.zip` | `macos-15` |
| Windows 10+ x64 | `effectcraft-<v>-windows-x64.msi`, `effectcraft-<v>-windows-x64-portable.zip` | `windows-latest` |
| Windows 10+ x86 (32-bit) | `effectcraft-<v>-windows-x86.msi`, `effectcraft-<v>-windows-x86-portable.zip` | `windows-latest` |
| Windows 11 ARM64 | `effectcraft-<v>-windows-arm64.msi`, `effectcraft-<v>-windows-arm64-portable.zip` | cross-compiled on `windows-latest` |
| Linux x86_64 | `effectcraft-<v>-linux-x86_64.{AppImage,deb,rpm,tar.gz}` | `ubuntu-22.04` |
| Linux aarch64 | `effectcraft-<v>-linux-aarch64.{AppImage,deb,rpm,tar.gz}` | `ubuntu-22.04-arm` |
| Linux AppImage updates | `effectcraft-<v>-linux-{x86_64,aarch64}.AppImage.zsync` | with the AppImages |
| Flatpak x86_64, aarch64 | `effectcraft-<v>-linux-{x86_64,aarch64}.flatpak` (the Linux tarball, repackaged) | `ubuntu-24.04`, `ubuntu-24.04-arm` |
| FreeBSD 14+ x86_64 | `effectcraft-<v>-freebsd-x86_64.tar.gz` | a FreeBSD 14.3 VM on `ubuntu-latest` |
| Web | `effectcraft-web-<v>.zip` (a static site; see [web.md](web.md)) | `ubuntu-latest` |

The ARM64 MSI is installed and run on ARM64 hardware by
[`windows-arm64.yml`](../.github/workflows/windows-arm64.yml).

The MSI's setup wizard ([`installer-ui.wxs`](../packaging/windows/installer-ui.wxs)) lets people
choose the install folder; upgrades and repairs reuse it, and `INSTALLFOLDER="..."` picks it for a
silent (`/qn`) install. [`packaging-lint.yml`](../.github/workflows/packaging-lint.yml) install-tests
that with stub binaries ([`test-msi.ps1`](../packaging/windows/test-msi.ps1)): a custom folder,
an upgrade that keeps it, a repair, uninstall, and the Program Files default. The wizard pages
themselves are checked by hand.

Every binary reports its version: `effectcraft --version`, `effectcraft-cli --version` and
*Help › About EffectCraft*.

### Linux: the glibc baseline

Linux binaries link against the glibc of the machine that builds them and need at least that
version wherever they run. The release builds on the oldest GitHub-hosted image,
**Ubuntu 22.04 (glibc 2.35)**, so the packages run on Ubuntu 22.04+, Debian 12+, Fedora 36+ and
RHEL 10. Building on a newer image would silently raise that floor. `packaging/linux/package.sh`
builds the AppImage, `.deb`, `.rpm` and tarball; the workflow then runs the AppImage's
`--version` as a smoke test.

### Linux: AppImage updates and Flatpak

Each AppImage embeds update information
(`gh-releases-zsync|storytold|effectcraft|latest|effectcraft-*-linux-<arch>.AppImage.zsync`), and
the `.zsync` file published beside it lets [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate)
and AppImageLauncher fetch only the changed blocks from the newest published (non-pre-release)
version. `package.sh` writes the `.zsync` when `zsyncmake` (the `zsync` package) is installed; the
workflow checks both.

The `.flatpak` bundles repackage the Linux job's tarball with
`packaging/linux/flatpak-bundle.sh` and `packaging/linux/flatpak/ai.storyteller.effectcraft.bundle.yml`
(no Rust build inside flatpak-builder), then install the bundle and run `effectcraft-cli --version`
in the sandbox. `ai.storyteller.effectcraft.yml` is the from-source manifest for Flathub; packaging
lint keeps the runtime and sandbox permissions of the two identical.

### FreeBSD

GitHub has no FreeBSD runners, so the job builds in a FreeBSD 14.3 VM
(`vmactions/freebsd-vm`, pinned by commit) with the packages
[`freebsd.yml`](../.github/workflows/freebsd.yml) uses, and `packaging/freebsd/package.sh` makes a
`/usr/local`-style tarball: `tar -xzf effectcraft-<v>-freebsd-x86_64.tar.gz --strip-components 1 -C /usr/local`.

## Signing

The macOS and Windows jobs and the draft-release job run in the `release` environment, which only
the `release` branch can use and which holds the signing secrets. The jobs that sign nothing run
without it and get no secrets. Every secret is optional: a missing one produces an unsigned artifact
and a warning in the job summary, never a failed build.

| Platform | Secrets |
|---|---|
| macOS (Developer ID signing and notarization) | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` |
| Windows (a certificate, or Azure Trusted Signing) | `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD`, or `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` |
