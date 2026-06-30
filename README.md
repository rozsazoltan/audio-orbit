# Audio Orbit

Audio Orbit is a Windows-first desktop music player for local libraries and internet radio. It focuses on a clean AIMP-like player workflow: folder playlists, favorites, repeat modes, crossfade, silence skipping, radio streams, backups, updates, media keys, and a compact waveform seek bar.

Audio Orbit can be used as a normal stereo player. Orbit processing is optional and belongs to sound profiles.

## Contents

- [Features](#features)
- [Local music](#local-music)
- [Internet radio](#internet-radio)
- [Waveform seek bar](#waveform-seek-bar)
- [Recording](#recording)
- [Backups](#backups)
- [Updates](#updates)
- [Windows behavior](#windows-behavior)
- [Data location](#data-location)
- [Limitations](#limitations)
- [License & Acknowledgments](#license--acknowledgments)

## Features

- folder-based music playlists with configurable grouping depth
- manual playlists and a built-in non-deletable Favorites playlist
- double-click playback from the track list
- repeat off, repeat track, and repeat selected tracks
- optional shuffle and auto-play next
- configurable crossfade between tracks
- AIMP-style long silence skipping
- AIMP-style single-lane waveform seek bar
- internet radio playback
- internet radio StreamTitle display and copy support
- original internet radio stream recording
- sound profiles with optional orbit-style stereo movement
- global media keys on Windows
- ZIP backup export/import for the complete app state
- GitHub release update checks and Windows self-update
- portable app data next to the executable
- single-instance Windows behavior

## Local music

Create a folder playlist from a music root directory. Audio Orbit scans supported audio files and groups them by folder depth.

Example:

```text
D:\Music\Artist A\Album A\song.mp3
D:\Music\Artist A\Album B\song.mp3
D:\Music\Artist B\Album A\song.mp3
```

With grouping depth `2`, the playlist groups are:

```text
Artist A / Album A
Artist A / Album B
Artist B / Album A
```

Folder playlists are scanner-owned. Manual tracks cannot be added to them. Use a manual playlist when you want to curate tracks manually.

## Internet radio

The Internet radio tab plays direct stream URLs. Radio playback ignores local-only features such as shuffle, repeat, auto-play next, crossfade, and silence skipping, but the active sound profile can still apply orbit processing to the live stream.

Radio rows support favorites, search, details, and now-playing navigation. When stream metadata is available, Audio Orbit can show the current station/title metadata.

## Waveform seek bar

Audio Orbit uses an AIMP-style amplitude waveform seek bar, not a frequency spectrum analyzer.

Local music:

- gray = not yet played
- blue = played section
- yellow = long silence region that can be skipped
- white vertical marker = current playhead

Internet radio:

- uses a subdued live amplitude waveform
- shows recent decoded stream level movement
- avoids saturated full-height visualizer bars
- keeps the display close to a seek-bar style waveform rather than a colorful spectrum display

## Recording

Audio Orbit can record internet radio streams from the original incoming stream bytes. Recording happens before volume, orbit processing, silence skipping, or any other playback processing.

The record button is available in the radio player controls. Right-click the record button to open the current recordings folder.

Default recording folder:

```text
.audio-orbit-records/
```

next to the executable. The folder can be changed in Settings.

Recording file names include a timestamp, station name, and current stream title when available.

## Backups

Backup export creates a compressed ZIP file with timestamped default file names, for example:

```text
audio-orbit-backup-2026-06-30-12-30-00.zip
```

Backups include the complete app state:

- folder playlist definitions
- manual playlists
- Favorites
- selected playlist and profile
- playback settings
- repeat/crossfade/silence skip settings
- radio station list
- recording folder settings
- update settings
- UI layout settings

Audio files and radio recordings themselves are not embedded in the backup.

## Updates

Audio Orbit can check GitHub releases for Windows executable updates. Stable releases are checked by default. Prerelease watching can be enabled from the update panel.

The self-update flow is Windows-first: Audio Orbit downloads a new executable, replaces the current executable, and restarts the app.

## Windows behavior

Audio Orbit is Windows-first.

The app includes:

- Windows executable manifest
- Windows taskbar/window icon
- Windows global media keys
- Windows file explorer integration
- single-instance startup guard
- release workflow for Windows executable builds

A second launch exits immediately when an Audio Orbit instance is already running.

## Data location

Audio Orbit stores app data next to the executable:

```text
.audio-orbit-data/state.json
```

This keeps settings portable when the executable is replaced during an update.

Radio recordings are stored by default next to the executable:

```text
.audio-orbit-records/
```

## Limitations

Audio Orbit is a music/radio player, not a system-wide Windows audio processor. It plays audio inside the app and applies its own optional DSP to app playback only.

The orbit effect is headphone-friendly stereo processing, not real HRTF, Dolby Atmos, or multichannel surround virtualization.

Format support depends on the bundled Rust audio stack. Common MP3, WAV, FLAC, OGG, OPUS, M4A, MP4, and AAC files are intended to work, but every codec/container combination cannot be guaranteed without an FFmpeg or GStreamer backend.

## Contributing

Development notes live in [CONTRIBUTING.md](CONTRIBUTING.md).

## License & Acknowledgments

Audio Orbit is released under the [GNU Affero General Public License v3.0](https://www.gnu.org/licenses/agpl-3.0.html).

Created by Zoltán Rózsa.

Audio Orbit uses Rust ecosystem libraries including egui/eframe for the desktop UI, rodio/cpal/Symphonia-backed decoding for playback, lofty for metadata, reqwest for HTTP requests, zip for backups, and Lucide icons for the interface.
