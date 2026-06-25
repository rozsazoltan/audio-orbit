# Audio Orbit

Audio Orbit is a Windows desktop music player with playlist management, local folder scanning, and headphone-focused orbit-style DSP effects.

It does not modify the Windows system volume. Audio Orbit loads local audio files, decodes them, processes the audio samples, and plays the processed stereo output.

## Features

- Local music playback from files and folder-based libraries.
- Folder playlists with configurable grouping depth, for example `D:\mp3\Artist\Album\song.mp3` grouped as `Artist / Album` when depth is `2`.
- Manual playlists and a built-in non-deletable Favorites playlist.
- Track double-click playback.
- Previous, play/pause, stop, next, auto-next, optional crossfade mixing, and clickable seek waveform.
- Track metadata display: duration, sample rate, bitrate, channel count, and file size when available.
- Waveform-style progress display.
- Track context menu with:
  - play now,
  - show file in File Explorer,
  - add to playlist,
  - remove from playlist,
  - delete from disk.
- Sound profiles with live re-rendering from the current playback position.
- Smooth stereo orbit and experimental virtual 8-direction headphone orbit.
- Optional long-silence skipping, such as skipping silence after 3 seconds.
- Optional AIMP-style crossfade: mix the end of one track into the start of the next for a configurable number of seconds.
- Output device refresh and change detection.
- Full app state backup/export and import using a compressed ZIP file.
- Fixed app data location for upgrades.
- GitHub release update check and Windows self-update support.
- Prerelease update checking is available but disabled by default.
- UI icons are rendered from Lucide icons through the bundled Rust `lucide-icons` font.

## Playlist types

Audio Orbit has three playlist types:

- Favorites: built-in, cannot be deleted, tracks can be added with the Lucide heart button.
- Manual playlist: editable playlist, tracks can be added manually.
- Folder playlist: scanner-owned playlist generated from a folder. Tracks are updated by rescanning the folder, not by manual add.

## Playback settings

Crossfade can be enabled from the top player controls. When enabled, Audio Orbit starts the next track before the current track ends and fades the two tracks into each other for the configured number of seconds. It applies to automatic next-track playback and manual Next when a track is currently playing.

## Sound profiles

Sound profile changes are applied without restarting the track. Internally, Audio Orbit re-renders the current file from the current playback position with the new DSP settings.

The virtual 8-direction mode is a headphone illusion using stereo panning, delay, crossfeed, and front/back tone cues. It is not true Dolby Atmos, real HRTF surround, or native multichannel output.

## Backups

Backups are ZIP files containing Audio Orbit state, including playlists, profiles, Favorites, selected settings, and update preferences.

Backups store references to music file paths. They do not copy your actual music files into the backup.

## App data location

Audio Orbit stores its app state under the platform app data directory resolved by `directories::ProjectDirs`.

On Windows this is usually under the user's local app data folder, so app updates can replace the executable without losing library state.

## Updates

The app checks GitHub Releases from:

```text
https://github.com/rozsazoltan/audio-orbit
```

Stable releases are checked by default. Prereleases are ignored unless the prerelease option is enabled in the app.

## Build

```powershell
cargo build --release
```

## Release

The GitHub Actions release workflow accepts a plain version input such as:

```text
0.5.0
```

The workflow turns it into:

```text
v0.5.0
```

Before building, the workflow verifies that `Cargo.toml` already has the same version. It does not rewrite version files; update the project version manually before starting a release.

Then it uploads a Windows x64 executable release asset.


## Keyboard media keys

On Windows, Audio Orbit registers global keyboard media keys for Play/Pause, Stop, Previous Track, and Next Track, so supported keyboards can control playback while the app is in the background. If another media app already owns one of these keys, Audio Orbit reports partial availability in the status bar.


## Portable data and updates

Audio Orbit stores its application state next to the executable under `audio-orbit-data/state.json`.
When an older AppData-based state file is found, it is migrated into the portable data folder on first launch.

The release watcher checks GitHub releases from inside the app. Stable release checks use the latest-release endpoint; prerelease-aware checks scan releases. To avoid GitHub API rate limits, the app allows at most two release checks per app session.

When installing an update on Windows, Audio Orbit downloads the new executable to `audio-orbit-data/update`, closes itself, replaces the currently running executable, and starts the updated executable again.
