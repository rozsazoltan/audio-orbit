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

On Windows, test the release executable because the app uses Windows-specific behavior such as executable resources, manifest metadata, self-update replacement, and global media keys.

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
