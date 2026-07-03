# Contributing to Audio Orbit


## Development goals

Audio Orbit should stay lightweight, portable, and predictable. Prefer incremental changes over rewrites.

Important priorities:

- keep playback responsive while UI state changes
- preserve GitHub release checks and Windows self-update behavior
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



## Project structure

```text
src/
├─ app/                   # AudioOrbitApp implementation split by feature area
│  ├─ core.rs             # startup state, window mode, modal navigation, status lifecycle
│  ├─ input.rs            # keyboard shortcuts and media key command dispatch
│  ├─ library_backup.rs   # file import, folder scans, backup export/import actions
│  ├─ lifecycle.rs        # eframe update loop and shutdown persistence
│  ├─ ordering.rs         # playlist/radio ordering, drag/drop, sorting, undo snapshots
│  ├─ playback.rs         # local playback, seek, crossfade, profiles, output devices
│  ├─ playlist_state.rs   # playlist selection, playback session, favorites, metadata state
│  ├─ radio.rs            # radio stations, metadata lookup, recording actions
│  ├─ updates.rs          # updater UI actions and background updater jobs
│  └─ ui_*.rs             # focused egui rendering modules for player, modals, lists, status
├─ bin/audio-orbit-dev.rs # local polling dev runner used by `cargo dev`
├─ audio_player.rs        # playback engine, seek, crossfade, output device handling
├─ config.rs              # saved state, playlists, metadata, backups, app data path
├─ dsp.rs                 # stereo/orbit rendering and silence handling
├─ icon.rs                # app icon loading
├─ main.rs                # app entrypoint, shared app state/types, startup wiring
├─ media_keys.rs          # Windows global media key listener
├─ ui_icons.rs            # Lucide icon font setup
└─ updater.rs             # GitHub release checks and Windows self-update
```

Keep new feature work close to the feature module that owns it. Prefer adding small helpers to the relevant `src/app/*.rs` file over expanding `main.rs` with unrelated UI, playback, or persistence behavior.

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
