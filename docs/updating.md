# Update EffectCraft

Desktop release builds check GitHub for a newer stable version after startup and every six hours.
Prereleases and downgrades are excluded. Open **Help > Check for Updates** to check manually.
You can turn automatic checks off in that window. Agent launches with `--control` check only on request.

1. Choose **Download update**. You can keep editing while the package downloads.
2. Choose **Install and restart** when you are ready. Downloading alone never installs or restarts.
3. Answer the save prompt in each EffectCraft instance running from this installation.
   **Save** waits for the save to succeed. Canceling Save As also cancels the upgrade.
   **Don't Save** discards that window's changes. **Cancel** in any window stops the upgrade.
4. Wait for installation. EffectCraft reopens the saved projects in separate instances.

Finish active renders and close other dialogs before installing. Approved windows wait without
allowing edits until all windows approve. New instances cannot open during that transaction.
No instance is forcibly terminated. Projects, preferences, plug-ins, portable data, and recovery
files are not reset. An update does not need a settings reset.

## Supported installations

- **Windows MSI** uses Windows Installer with an administrator prompt. It keeps the install folder
  and shortcuts. Declining elevation leaves the existing installation in place.
- **Windows portable** updates the executables and shipped documentation in the same directory.
  `EffectCraftData`, projects, and other user files stay in place. It accepts either portable marker.
  Files from the package are checked against an allowlist before copying. Files are staged on the same volume and swapped with rename operations. A failure restores
  the previous files from a temporary backup beside the executable. A failed restoration reports
  the backup location and preserves it for recovery.
- **macOS app bundle** mounts the DMG, checks the bundle identifier, version, and code signature,
  copies the new app beside the installed app, and replaces the bundle. The folder must be writable.
  A failed final rename restores the previous bundle.
- **Linux AppImage** stages the complete replacement in the same folder and renames it into place.
  It retains the AppImage's file name and makes the replacement executable. Detection checks the
  [runtime environment](https://docs.appimage.org/packaging-guide/environment-variables.html) and
  AppImage header before choosing a replacement target.
- **Linux Debian and RPM packages** use the system package manager through a polkit administrator
  prompt. Debian systems need `apt-get`; RPM systems need `dnf` or `zypper`.
- **Flatpak, Snap, FreeBSD, source builds, and other externally managed copies** show a link to
  releases and ask you to use the package manager or ArtCraft Launcher. They do not overwrite
  files owned by another installation method.

## Failure and retry

Only assets from `storytold/effectcraft` releases are accepted. A download must match both its
published size and SHA-256 digest before installation. Downloads go to a temporary file; failed
or canceled transfers never become installable. Completed downloads are cached separately from
portable settings. The helper verifies the package again immediately before installation. A cached download from a
previous session can be reused by choosing Download again; it is reverified in the background.

The restart helper waits up to two minutes for every participating process to release the
installation lock. A canceled preparation or a window that cannot close prevents replacement. An OS process check
also refuses replacement while an older or unregistered EffectCraft copy is running.
If an installer fails, the helper reports the error and tries to reopen the existing app and saved
projects. Package managers own rollback for their formats. Successful replacement followed by a
runtime failure in the new version is not automatically rolled back.

Unavailable updater storage disables updating and leaves the application usable. Network failures
retry after 15 minutes. Downloads use bounded byte ranges so canceling a stalled request waits
at most 45 seconds for that request to end.

Coordination covers instances of this updater running as the same OS user from the same installation,
including instances using different settings directories. Close older EffectCraft versions and
instances running as another OS user before updating. This version cannot make those processes
participate in its save protocol.

## Maintainer notes

The updater is native-only, in `apps/effectcraft/src/update`. It uses the launcher's pattern of
verified GitHub assets, native installation methods, and a detached helper. It does not depend on
the launcher or add networking to the engine or web app. UI commands remain in the command registry.
The save prompt supplies a small prepare/commit restart interface. The helper passes saved paths
with `--reopen-project` so custom extensions are opened as projects rather than imported as media.

Per-installation state lives under the OS user's ordinary EffectCraft settings directory in
`Updates/<hash of installation path>`, independently of `EFFECTCRAFT_CONFIG_DIR` and portable mode.
A shared file lock tracks running instances. A single transaction owner requests approval and
writes a commit marker only after every live instance approves. The helper needs the exclusive
installation lock before replacing files. OS locks disappear after crashes. An abandoned commit marker does not block the next launch. Failed preparations
can be retried without deleting project or settings files.

Publish the existing platform artifacts with a valid semantic version tag such as `v0.11.0`.
GitHub's `sha256:` asset digest or the release's `SHA256SUMS.txt` supplies integrity metadata.
No extra update feed, service, release job, or signing key is required.
