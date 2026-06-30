# Contributing to Audio Orbit

This document covers local development, Windows build requirements, WSL/Windows workflows, release notes, and commit conventions. The README is intentionally focused on app usage.

## Development goals

Audio Orbit should stay responsive, portable, and predictable.

Priorities:

- preserve the v8-style UX baseline unless a change is explicitly requested
- keep UI work off blocking network/audio paths where possible
- avoid unbounded memory growth in audio processing
- keep waveform rendering single-lane and AIMP-style, not colorful spectrum-style
- keep Windows behavior first-class
- keep user data compatible across executable updates
- avoid broad release workflow replacements that touch unrelated manifest/dependency versions
- use Lucide icons consistently with similar sizing and spacing

## Local requirements

Install:

- Rust stable
- Visual Studio Build Tools 2022
- Desktop development with C++ workload
- MSVC v143 build tools
- Windows 10/11 SDK

The MSVC linker is required. VS Code alone is not enough.

Verify from PowerShell:

```powershell
cargo --version
where link
```

## Running the app on Windows

From a native Windows filesystem path:

```powershell
cargo dev
```

`cargo dev` is defined as:

```text
cargo run --bin audio-orbit
```

Manual alternatives:

```powershell
cargo check
cargo run
cargo build --release
```

Do not run the Windows app directly from a `\\wsl$` path. Use a native Windows path such as:

```text
D:\github\owner\audio-orbit
```

## WSL-hosted repository with Windows runtime

Recommended structure:

```text
WSL repository:
  /path/to/audio-orbit

Windows mirror:
  D:\path\to\audio-orbit
```

The WSL side is the Git working copy. The Windows side is the build/runtime mirror.

Create a Mutagen sync session:

```powershell
mutagen sync create `
  --name audio-orbit-win-dev `
  --mode two-way-safe `
  --ignore ".git" `
  --ignore ".cache" `
  --ignore "target" `
  --ignore "*.zip" `
  "\\wsl$\<Distribution>\<path-to-audio-orbit>" `
  "D:\path\to\audio-orbit"
```

Useful commands:

```powershell
mutagen sync list
mutagen sync monitor audio-orbit-win-dev
mutagen sync flush audio-orbit-win-dev
mutagen sync pause audio-orbit-win-dev
mutagen sync resume audio-orbit-win-dev
mutagen sync terminate audio-orbit-win-dev
```

Daily flow:

```text
1. Edit in WSL or in the Windows mirror.
2. Flush Mutagen before building when needed.
3. Run cargo dev from the Windows mirror.
4. Commit from the WSL repository.
```

## Project structure

```text
src/
├─ audio_player.rs     # playback, radio stream, recording, seek, crossfade
├─ config.rs           # saved state, playlists, metadata, backups, app data path
├─ dsp.rs              # stereo/orbit rendering, waveform points, silence handling
├─ icon.rs             # app icon loading
├─ main.rs             # egui app state and UI composition
├─ media_keys.rs       # Windows global media key listener
├─ single_instance.rs  # Windows single-instance startup guard
├─ ui_icons.rs         # Lucide icon font setup
└─ updater.rs          # GitHub release checks and self-update
```

## Waveform policy

The waveform is an AIMP-style amplitude/seek waveform.

Do not replace it with:

- colorful spectrum bars
- stacked low/mid/high bands
- full-height normalized visualizer columns
- constantly saturated radio bars

Expected behavior:

- local waveform: gray base, blue played section, yellow silence-skip ranges
- radio waveform: subdued gray live amplitude envelope
- no unbounded sample buffers
- no UI-thread blocking work for large audio files where avoidable

## Recording policy

Radio recording stores original stream bytes, not processed playback output.

Rules:

- write to a `.part` file while recording
- finalize to a timestamped audio file on stop
- keep recording folder configurable
- do not record silently after the player is stopped
- do not embed recordings in app backup ZIPs

## Commit messages

Use Conventional Commits.

Use breaking markers when behavior, state, config, or core architecture changes:

```text
feat!: replace audio engine pipeline
perf!: move waveform analysis off startup playback path
fix!: change persisted state format
```

Use non-breaking types only when appropriate:

```text
fix(player): correct radio recording finalization
feat(radio): add original stream recording
perf(player): reduce waveform drawing allocations
ci(release): build Windows executable from release commit
```

Every breaking commit should include a body:

```text
BREAKING CHANGE: describe the changed behavior or migration impact.
```

## Release workflow notes

The release workflow asks for a version without the `v` prefix, for example:

```text
0.8.20
```

It creates release metadata for:

```text
v0.8.20
```

Release preparation may update:

- `Cargo.toml` package version
- `Cargo.lock`
- Audio Orbit Windows assembly identity version in `src/audio-orbit.exe.manifest`

Do not update unrelated dependency versions or unrelated manifest values during release preparation.

## Pull request checklist

Before merging:

- `cargo check` passes
- app launches from a native Windows path
- radio playback starts and stops cleanly
- radio recording starts, writes bytes, stops, and finalizes a file
- local playback seek still works
- waveform remains AIMP-style and not saturated
- Favorites still works
- backups import/export correctly
- update panel still opens and checks releases
- global media keys still work on Windows
- no unexpected version changes are committed unless it is a release commit

## License

By contributing, you agree that your contribution is licensed under the GNU Affero General Public License v3.0 or later.
