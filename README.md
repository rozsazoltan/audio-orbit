# Audio Orbit

Audio Orbit is a small Windows desktop app for controlling left/right audio balance and enabling smooth spatial auto-panning.

## Features

- Manual left/right channel volume sliders.
- Toggleable spatial auto-panning using a sine-wave pan cycle.
- Maximum panning intensity slider.
- Native Windows Core Audio integration through Rust.
- Windows `.exe` release builds through GitHub Actions.

## Requirements

- Windows 10 or newer.
- Rust stable toolchain for local builds.

## Run locally

```powershell
cargo run --release
```

## Build locally

```powershell
cargo build --release
```

The executable is created at:

```powershell
target\release\audio-orbit.exe
```

## Create a release executable

Open GitHub Actions, choose the `Release` workflow, and run it manually with a version input such as:

```text
0.1.0
```

The workflow validates the version, converts it to the Git tag `v0.1.0`, updates the release metadata in `Cargo.toml`, `Cargo.lock`, and the Windows executable manifest before building, then uploads the executable to a new GitHub Release as:

```text
audio-orbit-v0.1.0-windows-x64.exe
```
