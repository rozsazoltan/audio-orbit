# Contributing to Audio Orbit


## Development goals

Audio Orbit should stay lightweight, portable, and predictable. Prefer incremental changes over rewrites.

Important priorities:

- keep playback responsive while UI state changes
- avoid blocking the UI thread with expensive audio rendering work where possible

## Local development

Install a recent Rust toolchain and build the project with Cargo.

```sh
cargo check
cargo build
```

For WSL → Windows sync workflows, edit files in WSL, let Mutagen sync them into the Windows checkout, then run the dev watcher from the Windows checkout, for example `D:\github\rozsazoltan\audio-orbit`:

```powershell
.\scripts\dev.ps1
```

You can also run the same watcher directly:

```sh
cargo dev
```

`cargo dev` is a project-local Cargo alias that runs the built-in `audio-orbit-dev` helper. It does not require `cargo-watch`. The helper uses polling-friendly file watching for Mutagen/WSL sync workflows, watches `src`, `Cargo.toml`, `Cargo.lock`, `assets`, and `build.rs`, ignores `target` and portable app data folders, rebuilds `audio-orbit`, and restarts the desktop app after synced file changes.




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
├─ bin/audio-orbit-dev.rs # local polling dev runner used by `cargo dev`
├─ audio_player.rs        # playback, seek, crossfade, output device handling
├─ config.rs              # saved state, playlists, metadata, backups, app data path
├─ dsp.rs                 # stereo/orbit rendering and silence handling
├─ icon.rs                # app icon loading
├─ main.rs                # egui app state and UI composition
├─ media_keys.rs          # Windows global media key listener
├─ ui_icons.rs            # Lucide icon font setup
```

## Commit messages

Use Conventional Commits.

Examples:

```text
feat(player): add repeat selection playback
fix(ui): prevent playlist editor overlap
refactor(ui): separate settings sections
```



```text
```

The version metadata should be committed as:

```text
```



## Pull request checklist

Before merging, check:

- `cargo check` passes
- no unexpected version number changes were committed
- no unused Rust warnings were introduced
- playback still works after profile changes, seeking, crossfade, and media key commands

## License

By contributing, you agree that your contribution is licensed under the GNU Affero General Public License v3.0 or later.


## Development runner

Use `cargo dev` from the repository root. The repository contains both `.cargo/config.toml` and `.cargo/config` so Cargo uses the built-in polling dev runner instead of requiring the external `cargo-watch` subcommand. On Windows, `scripts/dev.ps1` runs the same project-local runner directly with `cargo run --bin audio-orbit-dev --`.
