# Contributing to Audio Orbit

## Development goals

Audio Orbit should stay lightweight, portable, and predictable. Prefer incremental changes over rewrites.

Important priorities:

- keep playback responsive while UI state changes
- preserve GitHub release checks and Windows self-update behavior
- avoid blocking the UI thread with expensive audio rendering work where possible

## Local development

Project is currently Windows-only. It uses [mise](https://mise.jdx.dev/) for Rust and developer-tool installation, task execution, and exact local/CI parity on `windows-latest`.

Install configured tools and repository hooks:

```sh
mise run setup
```

This installs all tools pinned in `mise.toml` and installs repository Git hooks through hk.

DJ tempo stretching uses built-in pure-Rust WSOLA implementation, so local builds need no Clang, LLVM, `libclang`, C++ compiler, or `LIBCLANG_PATH` setup.

Available validation commands:

```sh
mise run format          # apply Rust, TOML, and Pkl formatting
mise run format:check    # verify Rust, TOML, and Pkl formatting
mise run tooling:check   # validate mise tasks and hk/Pkl config
mise run check           # cargo check --locked --all-targets
mise run clippy          # cargo clippy --locked --all-targets -- -D warnings
mise run test            # cargo nextest run --locked --all-targets
mise run test:doc        # doctests not run by nextest
mise run ci              # complete suite also used by GitHub Actions
```

Git hooks use `hk`:

- `pre-commit` fixes Rust, TOML, and Pkl formatting; validates TOML, mise, and hk/Pkl config; checks merge markers and private keys
- `pre-push` runs locked Cargo check, Clippy with warnings denied, nextest, and doctests
- `mise run hooks:pre-commit` checks the full pre-commit hook against all tracked files
- `mise run hooks:pre-push` checks the full pre-push hook against all tracked files
- `mise run hooks:check` aliases the full pre-commit check
- `mise run hooks:fix` applies supported pre-commit fixes across all tracked files
- `mise run ci` runs both hooks exactly as Windows GitHub Actions does

`HK_MISE=1` is used when installing hooks, so generated hook commands execute through `mise` even when shell activation is unavailable. Do not install both global and repository-local hk hooks, because Git can execute both.

For WSL → Windows sync workflows, edit files in WSL, let Mutagen sync them into Windows checkout, then run dev watcher from Windows checkout, for example `D:\github\rozsazoltan\audio-orbit`:

```powershell
.\scripts\dev.ps1
```

You can also run same watcher directly:

```sh
cargo dev
```

`cargo dev` is project-local Cargo alias running built-in `audio-orbit-dev` helper. It does not require `cargo-watch`. Helper uses polling-friendly file watching for Mutagen/WSL sync workflows, watches `src`, `Cargo.toml`, `Cargo.lock`, `assets`, and `build.rs`, ignores `target` and portable app data folders, rebuilds `audio-orbit`, and restarts desktop app after synced file changes.

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




## Pull request checklist

Before merging, check:

- `mise run ci` passes
- no unexpected version number changes were committed
- no unused Rust warnings were introduced
- playback still works after profile changes, seeking, crossfade, and media key commands

## License

By contributing, you agree that your contribution is licensed under the GNU Affero General Public License v3.0 or later.


## Build cache

Cargo build artifacts are stored under `.cache/cargo-target` to keep the repository root clean.

