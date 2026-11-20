# Hippo.app as a download

Status: accepted, to build in 0.7.1.

## The problem

Hippo.app is built from source today (`app/scripts/bundle.sh`), which needs Rust, the pinned toolchain and a GPUI build of several minutes. Most people who would use the app will not do that. The maintainer has no Apple Developer ID, so the app cannot be signed with one or notarised, and a downloaded app that is neither is stopped by Gatekeeper when the browser marked it as downloaded.

## What changes for a user

```sh
hippo app install          # or: curl -fsSL …/install.sh | sh -s -- --app
```

downloads `Hippo-<version>-macos.tar.gz` and its `.sha256` from the GitHub release of the same version as the CLI, checks the checksum, unpacks `Hippo.app` into `~/Applications` (no `sudo`), and records it in `~/.hippo/install.json`. It opens from Spotlight, Launchpad or `open ~/Applications/Hippo.app` like any app. `hippo app status` prints the installed version and whether the bundle still has the recorded checksum; `hippo app uninstall` removes the bundle it installed and its record. `hippo doctor` gains an `app` row when an app is recorded. The Claude Code plugin's `/hippo:setup` and the Codex plugin's `$hippo` offer `hippo app install` after the CLI, on macOS, only on a yes.

For a browser download from the releases page, the docs say plainly that the app is not notarised, and how to open it the first time: try to open it once, then System Settings › Privacy & Security › **Open Anyway** next to "Hippo was blocked", and confirm.

## Why this works without a Developer ID

- **Ad-hoc signed.** The release bundle is signed with `codesign --sign -` (an ad-hoc signature: no identity, no certificate). Apple silicon runs only signed code, and an ad-hoc signature is enough for code the system has not been told came from the internet. No other account's certificate is ever used.
- **No quarantine from curl.** Gatekeeper's check runs on files with the `com.apple.quarantine` extended attribute, which browsers and other apps that adopt the attribute set on what they download. `curl` and `tar` do not set it, so a bundle that `hippo app install` fetched with `curl` and unpacked with `tar` opens without the "unidentified developer" stop.
- **The checksum is the trust.** The bundle is the release's: `hippo app install` refuses it unless its SHA-256 matches the `.sha256` published beside it, as `install.sh` does for the CLI. The record in `install.json` lets `doctor` notice a bundle changed after install.
- **hippo never strips quarantine.** It never runs `xattr` on anything, and the docs never suggest `xattr -d`. A bundle the person downloaded in a browser keeps its quarantine, and Gatekeeper's prompt stays the person's decision.

## The release asset

The release workflow gains a macOS job: Hippo.app built for `aarch64-apple-darwin` and `x86_64-apple-darwin`, the two binaries joined with `lipo` into one universal binary, bundled by `app/scripts/bundle.sh` (Info.plist with the version, the icon), signed ad hoc, then `Hippo-<version>-macos.tar.gz` (one top-level `Hippo.app/`) and `Hippo-<version>-macos.tar.gz.sha256` (`<hex>  <name>`, as the CLI's), attached to the release beside the CLI's assets. `app/scripts/release-app.sh <version>` does the same locally.

## install.json

`install.sh` writes the CLI's record (`version`, `target`, `binary`, `sha256`, `asset`, `asset_sha256`). `hippo app install` adds one key and keeps the rest:

```json
"app": {
  "version": "0.7.1",
  "path": "/Users/you/Applications/Hippo.app",
  "asset": "Hippo-0.7.1-macos.tar.gz",
  "asset_sha256": "…",
  "binary_sha256": "…"
}
```

`binary_sha256` is the checksum of `Contents/MacOS/hippo-app`, which `doctor` and `hippo app status` compare. When there is no `install.json` (hippo built with Cargo), the file is created with the `app` key only. The file stays `0600`.

## The command

`hippo app install [--version <v>] [--dir <folder>]`, `hippo app uninstall`, `hippo app status`. macOS only; elsewhere they say so and exit 1.

- The version is the CLI's own unless `--version` names one. The source is `https://github.com/VanitasCaesar1/hippo/releases/download/v<version>/`, or `HIPPO_BASE_URL` (as for `install.sh`: a folder of release files, `file://` or `http://127.0.0.1:…` for tests).
- Download: `curl -fL` into a new temporary folder, with a progress bar on a terminal (a line with the size and time otherwise), a 15-second connect timeout and a 30-second stall limit (under 1 byte a second). A failed download says why (timed out, refused, no such release) and that running `hippo app install` again retries; nothing is installed. Unpack: `tar -xzf` there; the archive must hold exactly `Hippo.app/` with `Contents/MacOS/hippo-app` and an `Info.plist` whose `CFBundleShortVersionString` is the version asked for.
- Install: the old bundle, if any, is moved aside, the new one moved into `--dir` (default `~/Applications`, created if missing), the old one deleted. Then the record. If Hippo is running, it says to quit and reopen it.
- Uninstall removes only the path recorded, and only if its name is `Hippo.app`.

## Tests

- `crates/hippo/tests/app.rs` (macOS): a synthetic bundle packed as a release in a temporary folder and served as `file://`; install into a scratch `--dir` with a scratch `HOME`; the record, the checksum, a refused tampered archive, a refused wrong version, `status`, `doctor`'s row before and after the binary is changed, and `uninstall`. No real `~/Applications` is touched.
- `scripts/setup-e2e/run.sh … app`, by hand before a release: the real release-built bundle served on 127.0.0.1, `install.sh --app` in a scratch `HOME`, the bundle checked to carry no quarantine attribute, and launched in the background from the scratch folder (`open -g -n`), captured by window id and closed.

## Alternatives

- **A `.dmg` or `.zip` for the browser.** Same Gatekeeper prompt, more packaging; the tarball serves both paths.
- **Homebrew cask.** Casks are expected to be signed and notarised; without a Developer ID the cask would need `--no-quarantine`, which strips the check this design keeps. Not now.
- **Removing quarantine in the installer.** Rejected: it would teach people to turn off a check for software they did not verify.
