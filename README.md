# Audio Orbit

Audio Orbit is a lightweight Windows desktop app for stereo left/right audio panning.

It can create a simple orbit-style headphone effect by moving the Windows output endpoint balance between the left and right channels. It does **not** create true 8-direction, front/back, up/down, or HRTF surround audio.

## Features

- Manual left/right channel volume sliders.
- Toggleable stereo orbit panning using an equal-power pan curve.
- Adjustable output level, stereo width, and orbit speed.
- Left-only, center, and right-only channel test buttons.
- Native Windows Core Audio integration through Rust.
- Windows `.exe` release builds through GitHub Actions.

## What to expect

Audio Orbit controls the default Windows output endpoint channel levels. This means it can only move sound between the left and right channels when the selected audio device exposes usable per-channel volume control.

It cannot move audio in 8 real directions. It cannot place sound in front, behind, above, or below you. For that, the app would need real audio processing, such as a virtual audio device, DSP pipeline, or HRTF-based engine.

For the strongest effect:

- Use headphones.
- Use a stereo output device.
- Disable mono audio in Windows accessibility settings.
- Set Output Level to 100%.
- Set Stereo Width to 100%.
- Set Orbit Speed between 100% and 200%.

Use the **Left only** and **Right only** test buttons first. If those buttons only change loudness or still sound centered, the selected Windows audio device or driver does not expose usable left/right endpoint control, so Orbit Mode will also sound like volume pumping instead of stereo movement.

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
0.1.1
```

The workflow validates the version, converts it to the Git tag `v0.1.1`, updates the release metadata in `Cargo.toml`, `Cargo.lock`, and the Windows executable manifest before building, then uploads the executable to a new GitHub Release as:

```text
audio-orbit-v0.1.1-windows-x64.exe
```
