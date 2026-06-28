# Contributing to Audio Orbit

Thank you for improving Audio Orbit. This document is for development workflow, build notes, release preparation, and project maintenance. The README is intentionally focused on app usage.

## Development goals

Audio Orbit should stay lightweight, portable, and predictable. Prefer incremental changes over rewrites.

Important priorities:

- keep playback responsive while UI state changes
- avoid blocking the UI thread with expensive audio rendering work where possible
- keep playlist, playback, update, and backup responsibilities separated
- preserve user data across executable updates
- avoid broad regex replacements in release automation
- keep release metadata committed before building release artifacts

## Local development

Install a recent Rust toolchain and build the project with Cargo.

```sh
cargo check
cargo build
cargo build --release
```

For day-to-day development, use the Cargo alias:

```sh
cargo dev
```

The repository Cargo config writes build artifacts to `.cache/cargo-target` and runs the app with development state in `.cache/app-data`. Debug builds show a development version label in the app, for example `v0.0.0-abc123def456`, derived from the current Git commit. Release builds keep the normal `v<package-version>` label.

If you also want Cargo registry and Git dependency caches inside the repository cache, use the wrapper scripts instead of calling Cargo directly:

```sh
./scripts/dev.sh
```

```powershell
./scripts/dev.ps1
```

Those wrappers set `CARGO_HOME`, `CARGO_TARGET_DIR`, and Audio Orbit development data paths under `.cache` before starting Cargo. This keeps local development artifacts, downloaded Cargo dependencies, target files, and dev app state out of the project root and out of the user-level Cargo cache.

On Windows, test the release executable because the app uses Windows-specific behavior such as executable resources, manifest metadata, self-update replacement, and global media keys.


## Windows development from a WSL-hosted repository

For the best Windows development experience, keep the Git repository on the WSL filesystem and run the app from a native Windows mirror synchronized with Mutagen. Do not run the Windows executable directly from `\\wsl$\...`, because Rust builds and the app runtime perform many small file operations.

Recommended local layout:

```text
WSL Git repository: /path/to/audio-orbit
Windows dev mirror: D:\path\to\audio-orbit
Windows app runtime: D:\path\to\audio-orbit
```

Install `mutagen.exe` on Windows and make sure it is available on `PATH`. For the first setup, run the helper from Windows PowerShell through the WSL UNC path of your repository. The helper treats its own repository root as the source workspace and asks where the Windows mirror should be created.

```powershell
& "\\wsl$\<Distribution>\<path-to-repository>\scripts\setup-mutagen-wsl-dev.ps1"
```

When prompted, enter the Windows mirror directory, for example:

```text
D:\path\to\audio-orbit
```

The script creates or reuses this synchronization session by default:

```text
audio-orbit-win-dev
```

The source is always the workspace root that contains the running `.ps1` file. The target is the Windows mirror path entered during setup. The session uses Mutagen VCS ignores and also ignores `.cache`, `target`, and ZIP artifacts. Keep Git operations on the source workspace side:

```sh
cd /path/to/audio-orbit
git status
git add .
git commit -m "..."
```

Run the Windows app from the Windows mirror:

```powershell
cd D:\path\to\audio-orbit
cargo dev
```

Use these commands when needed:

```powershell
mutagen sync list
mutagen sync monitor audio-orbit-win-dev
mutagen sync flush audio-orbit-win-dev
mutagen sync terminate audio-orbit-win-dev
```

If you want to pass the mirror path without an interactive prompt, use:

```powershell
& "\\wsl$\<Distribution>\<path-to-repository>\scripts\setup-mutagen-wsl-dev.ps1" `
  -WindowsProjectPath "D:\path\to\audio-orbit"
```

## Project structure

```text
src/
├─ audio_player.rs     # playback, seek, crossfade, output device handling
├─ config.rs           # saved state, playlists, metadata, backups, app data path
├─ dsp.rs              # stereo/orbit rendering and silence handling
├─ icon.rs             # app icon loading
├─ main.rs             # egui app state and UI composition
├─ media_keys.rs       # Windows global media key listener
├─ ui_icons.rs         # Lucide icon font setup
└─ updater.rs          # GitHub release checks and self-update
```

## Commit messages

Use Conventional Commits.

Examples:

```text
feat(player): add repeat selection playback
fix(ui): prevent playlist editor overlap
refactor(ui): separate settings sections
ci(release): cache Rust dependencies by dependency metadata
```

## Release workflow notes

The release workflow should prepare the release version on a temporary branch named like:

```text
chore/release-vx.y.z
```

The version metadata should be committed as:

```text
chore: release vx.y.z
```

After squash merge into the default branch, the temporary release branch can be deleted. The release executable should be built from the merged commit.

Do not update unrelated manifest dependency versions. Only the app assembly identity version should be changed.

## Pull request checklist

Before merging, check:

- `cargo check` passes
- release workflow YAML is valid
- no unexpected version number changes were committed
- no unused Rust warnings were introduced
- playback still works after profile changes, seeking, crossfade, and media key commands
- backups still include playlists, folders, Favorites, playback settings, sound profiles, update settings, and UI settings

## License

By contributing, you agree that your contribution is licensed under the GNU Affero General Public License v3.0 or later.


## Non-Windows development builds

Audio Orbit is released as a Windows audio player. On Linux and macOS, `cargo dev` builds a UI-only development preview and uses an audio-player stub. This keeps local development and CI checks independent from native audio backend packages such as ALSA.

Use `scripts/dev.sh` or `scripts/dev.ps1` when you want Cargo registry, git dependencies, build artifacts, and app state to stay under `.cache`.
