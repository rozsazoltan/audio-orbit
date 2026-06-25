# Audio Orbit

Audio Orbit is a lightweight desktop audio player that creates an orbit-style effect by processing the selected music file directly and moving the decoded signal across the stereo field.

This version does **not** change Windows system volume or endpoint channel levels. It loads an audio file, decodes it, renders a processed stereo signal, and plays that processed audio through your default output device.

## Features

- Open local audio files such as MP3, WAV, FLAC, and OGG.
- Process the music signal itself instead of changing system volume.
- Smooth left/right sweep mode for strong headphone panning.
- 8-step orbit cue mode for a more directional, stepped movement effect.
- Adjustable output level, stereo width, and orbit speed.
- Native desktop GUI built with Rust and egui.
- Windows `.exe` release builds through GitHub Actions.

## Important limitation

Audio Orbit can move the audio it plays itself. It cannot directly move audio from Spotify, YouTube, games, browsers, or other applications.

To process all system audio from other apps, the project would need a virtual audio device, audio driver, or system-wide audio plugin/APO. That is a different architecture from a normal desktop player.

The 8-step orbit mode is a stereo headphone cue. It is not true HRTF surround and cannot perfectly place sound in front, behind, above, or below you. Real 8-direction spatial audio requires HRTF/DSP processing or a dedicated spatial audio engine.

## Recommended test settings

For the strongest effect:

- Use headphones.
- Open a normal stereo music file.
- Set Output Level to 95-100%.
- Set Stereo Width to 100%.
- Start with Smooth left/right sweep.
- Try Orbit Speed between 80% and 150%.

## Requirements

- Windows 10 or newer for release executables.
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
0.2.0
```

The workflow validates the version, converts it to the Git tag `v0.2.0`, updates the release metadata in `Cargo.toml`, `Cargo.lock`, and the Windows executable manifest before building, then uploads the executable to a new GitHub Release as:

```text
audio-orbit-v0.2.0-windows-x64.exe
```
