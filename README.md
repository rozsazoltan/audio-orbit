# Audio Orbit

Audio Orbit is a lightweight desktop music player that applies smooth orbit-style DSP to local music files. It decodes the selected audio, renders a processed stereo signal, and plays that processed output instead of changing Windows volume or endpoint channel levels.

## What it does

- Plays local audio files such as MP3, WAV, FLAC, and OGG.
- Supports regular playlists and folder-based playlists.
- Can import a whole music folder and group tracks by folder depth.
- Lets you play all tracks or only one imported folder group.
- Supports multiple sound profiles with separate DSP settings.
- Shows the currently playing track, playback progress, elapsed time, and duration.
- Applies smooth stereo orbit panning to the decoded audio signal.
- Includes an experimental headphone surround orbit mode for an 8-zone stereo illusion.
- Warns when DSP settings change during playback, because the current track must be restarted to hear the new rendered audio.
- Detects when the default output device name changes and offers a refresh action.
- Supports library backup export/import as JSON.
- Uses the project icon for both the embedded Windows executable resource and the native egui window icon.
- Builds Windows `.exe` releases through GitHub Actions.

## Folder playlists

You can import a folder as a playlist and choose how many subfolder levels should be used for grouping.

Example:

```text
D:\mp3\Artist A\Album A\song.mp3
D:\mp3\Artist A\Album B\song.mp3
D:\mp3\Artist B\Album A\song.mp3
```

With folder depth `2`, Audio Orbit creates these groups:

```text
Artist A / Album A
Artist A / Album B
Artist B / Album A
```

You can then switch the playlist view to `All folders` or to a single folder group and play only that group.

## Library backup

Use `Export backup` to save playlists, imported folder metadata, selected groups, and sound profiles into a JSON file.

Use `Import backup` to restore that library later or move it to another machine. The backup stores file paths, so the music files must still exist at those paths for playback to work.

## Important audio limitation

Audio Orbit currently processes audio files that it plays itself. It cannot directly modify Spotify, YouTube, browser, game, or other app audio.

Processing current Windows system sound requires a different architecture: capture the default render endpoint with WASAPI loopback, process the captured samples, then route the processed output to another device or to a virtual audio device. Without a virtual audio device/APO style setup, a normal desktop music player cannot cleanly replace the system mix in-place.

## About headphone surround orbit

The surround mode is not an 8-step effect and it is not real Dolby Atmos/HRTF surround. It is a continuous stereo headphone illusion through these perceived zones:

```text
front-center -> right-front -> right-center -> right-back -> rear-center -> left-back -> left-center -> left-front
```

It uses left/right panning, interaural delay, crossfeed, front presence cues, rear low-pass cues, and smoother transition filtering. Headphones are strongly recommended.

## Recommended settings

For a strong but smoother effect:

- Use headphones.
- Start with `Smooth stereo sweep`.
- Set `Stereo Width` to 100%.
- Set `Orbit Speed` between 60% and 100%.
- Set `Motion Smoothness` between 85% and 100%.
- Try `Headphone surround orbit` with `Surround Cue Strength` around 70-90%.

## Icon note on Windows

If the executable icon looks old in File Explorer or the taskbar, Windows may be showing a cached icon. The app sets both:

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
0.4.0
```

The workflow validates the version, converts it to the Git tag `v0.4.0`, updates the release metadata in `Cargo.toml`, `Cargo.lock`, and the Windows executable manifest before building, then uploads the executable to a new GitHub Release as:

```text
audio-orbit-v0.4.0-windows-x64.exe
```
