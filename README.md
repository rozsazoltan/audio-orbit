# Audio Orbit

Audio Orbit is a lightweight desktop audio player that applies smooth orbit-style DSP to local music files. It decodes the selected audio, renders a processed stereo signal, and plays that processed output instead of changing Windows volume or endpoint channel levels.

## What it does

- Plays local audio files such as MP3, WAV, FLAC, and OGG.
- Supports multiple playlists with multiple tracks.
- Supports multiple sound profiles with separate DSP settings.
- Applies smooth stereo orbit panning to the decoded audio signal.
- Includes an experimental virtual 8-direction orbit mode for headphone cues.
- Shows a warning when DSP settings change during playback, because the current track must be restarted to hear the new render.
- Lets you refresh the output device if the Windows speaker/headphone output changes while the app is open.
- Uses the project icon for both the embedded Windows executable resource and the native egui window icon.
- Builds Windows `.exe` releases through GitHub Actions.

## Important audio limitation

Audio Orbit currently processes audio files that it plays itself. It cannot directly modify Spotify, YouTube, browser, game, or other app audio.

Processing the current Windows system sound is possible as a different architecture: capture the default render endpoint with WASAPI loopback, process the captured samples, then route the processed output to another device or to a virtual audio device. Without a virtual audio device/APO style setup, a normal desktop player cannot cleanly replace the system mix in-place.

## About virtual 8-direction mode

The virtual 8-direction mode is not the old “8 step” mode. It is intended as a continuous headphone cue path through these perceived zones:

```text
front-center -> right-front -> right-mid -> right-back -> rear-center -> left-back -> left-mid -> left-front
```

This is still stereo DSP, not true Dolby Atmos, HRTF, or real surround. It uses left/right panning, interaural delay, front/back tone cues, and smoother transition filtering to create a stronger illusion through headphones.

## Recommended settings

For a strong but smoother effect:

- Use headphones.
- Start with `Smooth stereo orbit`.
- Set `Stereo Width` to 100%.
- Set `Orbit Speed` between 60% and 100%.
- Set `Transition Smoothness` between 80% and 100%.
- Try `Virtual 8-direction orbit` with `Front/Back Cue Strength` around 60-80%.

## Icon note on Windows

If the executable icon looks old in File Explorer or the taskbar, Windows may be showing a cached icon. The app now sets both:

- the embedded `.exe` icon through `build.rs` and `winresource`, and
- the native window/taskbar icon through `egui::ViewportBuilder::with_icon`.

If Windows still shows the previous icon, rebuild the release executable and refresh the Windows icon cache or rename the exe/release asset so Explorer treats it as a new file.

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
0.3.0
```

The workflow validates the version, converts it to the Git tag `v0.3.0`, updates the release metadata in `Cargo.toml`, `Cargo.lock`, and the Windows executable manifest before building, then uploads the executable to a new GitHub Release as:

```text
audio-orbit-v0.3.0-windows-x64.exe
```
