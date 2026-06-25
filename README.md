# Audio Orbit

Audio Orbit is a lightweight Windows desktop app that creates an orbit-style stereo effect by smoothly shifting system volume between the left and right channels.

## Features

- Manual left/right channel volume sliders.
- Toggleable orbit panning using a sine-wave pan cycle.
- Stronger balance-style panning: the dominant side stays loud while the opposite side fades.
- Adjustable orbit volume, strength, and speed sliders.
- Native Windows Core Audio integration through Rust.
- Windows `.exe` release builds through GitHub Actions.

## What to expect

Audio Orbit changes the Windows output endpoint channel balance. It does not process individual app audio streams and it does not create true HRTF/3D surround positioning. For the strongest effect, use headphones, a stereo output device, and set Orbit Strength to 100%.

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
