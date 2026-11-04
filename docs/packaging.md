# Packaging

How VDA is built into installers, what each one contains, and what is still
to decide before it is handed to people outside the team.

## Building

```sh
cd apps/desktop
npm ci
npm run bundle                                     # this computer's installers
npm run bundle -- --target universal-apple-darwin  # macOS: Intel and Apple silicon in one app
npm run bundle -- --bundles deb                    # only some kinds
```

`npm run bundle` is `tauri build` with `src-tauri/tauri.bundle.conf.json`
added. That file does two things the everyday build does not:

1. Before the app is compiled, `scripts/sidecar.mjs` builds `vda-run` in
   release mode and copies it to `src-tauri/binaries/vda-run-<target>`
   (for a universal Mac build it builds both architectures and joins them
   with `lipo`).
2. It lists `binaries/vda-run` as an `externalBin`, so Tauri puts it in
   every installer, next to the app.

Plain `cargo build` and `npm run tauri dev` stay as they were: tests and
the end-to-end run use `target/debug/vda-run` directly. **Use
`npm run bundle`, not `npm run tauri build`, to make installers**: the
second makes an app without `vda-run`, whose schedules would fail. The
release workflow checks every installer for it.

Installers land in `target/release/bundle/` (or
`target/<target>/release/bundle/` with `--target`).

## What each installer contains

| Platform | Installers | Where the app and `vda-run` go |
|---|---|---|
| Linux | `.deb`, `.rpm`, `.AppImage` | `/usr/bin/vda-desktop` and `/usr/bin/vda-run` (deb, rpm); inside the AppImage, `usr/bin/` |
| macOS | `.app` in a `.dmg` | `VDA.app/Contents/MacOS/` |
| Windows | `.msi`, setup `.exe` (NSIS) | the install folder, both `.exe`s side by side |

Python and R are not bundled: steps use the user's own, chosen in
Settings → Python and R. DuckDB and SQLite are compiled into both
programs.

## Schedules and where `vda-run` lives

A schedule (⋯ → Refresh on a schedule) is a cron, launchd or Task
Scheduler entry that runs `vda-run` by its full path, so that path must
last:

- **Installed** (deb, rpm, `/Applications`, Windows install folder): the
  entry points at `vda-run` beside the app.
- **AppImage**: it is mounted at a new temporary path each time it starts,
  so the app copies `vda-run` to the user's data folder
  (`~/.local/share/vda/bin/vda-run`) and the entry points there. Each
  start of an updated AppImage refreshes that copy (only if one exists).
- **macOS, run from the disk image or before being moved to
  Applications** (macOS runs such apps from a temporary place): the same,
  in `~/Library/Application Support/vda/bin/`.

If the app is moved or uninstalled after a schedule is made, the entry
stops working; making the schedule again fixes it.

Code: `crates/vda-app/src/refresh.rs` (`runner_path`, tested in
`schedules_do_not_point_into_a_temporary_mount` and
`the_kept_copy_follows_the_app`).

## Releases

`.github/workflows/release.yml` builds the installers on Ubuntu 22.04
(so they run on distributions with an older glibc), macOS (universal)
and Windows, checks each carries `vda-run`, and uploads them. Pushing a
tag such as `v0.2.0` also makes a **draft** GitHub release with them
attached; publishing it is a manual step.

## Still to decide

These need an account, money or a product decision, so they're left
open:

1. **Name and identifier.** The app is "VDA", identifier
   `dev.vda.desktop`. The identifier names the settings folder on macOS
   and can't change later without moving users' settings, so settle it
   before the first public release.
2. **macOS signing and notarization.** Unsigned apps are blocked by
   Gatekeeper ("can't be opened"). Needs an Apple Developer account
   ($99/year); then set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
   `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (an
   app-specific password) and `APPLE_TEAM_ID` as repository secrets; the
   workflow already passes them to the build.
3. **Windows signing.** Unsigned installers show SmartScreen's "Windows
   protected your PC". Options: an OV/EV certificate, or Azure Trusted
   Signing; either is set up as `bundle.windows.signCommand`. Windows also
   isn't verified yet (the CI test job allows it to fail).
4. **Updates.** Tauri's updater plugin can check a release feed and
   install updates; it needs a signing key pair for update files and a
   place to host the feed (GitHub releases work).
5. **Linux.** Whether to also publish a Flatpak (Flathub) or only the
   deb, rpm and AppImage.
6. **Icon.** The current icon is a placeholder redrawn at 1024 px
   (`src-tauri/icons/icon.svg`); `npx tauri icon <1024px png>` makes every
   size from a new one.
