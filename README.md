# audio-orbit

`audio-orbit` is a lightweight Windows music player for local audio libraries, folder-based playlists, smooth crossfade playback, silence skipping, and headphone-friendly orbit-style stereo movement.

It is designed for people who keep music in local folders and want an AIMP-like desktop player with portable app data, playlist backups, global media keys, release updates, and simple spatial stereo controls. The orbit effect can be turned off per sound profile, so the app can also be used as a normal stereo music player.

- [What it does](#what-it-does)
  - [Local music libraries](#local-music-libraries)
  - [Playback](#playback)
  - [Sound profiles](#sound-profiles)
  - [Backups](#backups)
  - [Updates](#updates)
- [Get started](#get-started)
- [Usage](#usage)
  - [Create a folder playlist](#create-a-folder-playlist)
  - [Play music](#play-music)
  - [Use repeat modes](#use-repeat-modes)
  - [Search tracks](#search-tracks)
  - [Manage Favorites](#manage-favorites)
  - [Export and import backups](#export-and-import-backups)
  - [Check for updates](#check-for-updates)
- [Data location](#data-location)
- [Known limitations](#known-limitations)
- [Contributing](#contributing)

## What it does

Audio Orbit plays local music files from manual playlists or scanner-owned folder playlists. It can group a folder library by subfolder depth, so a structure such as `D:\Music\Artist\Album\song.mp3` can be browsed as `Artist / Album`.

### Local music libraries

Folder playlists are created from a selected directory. You choose how many folder levels should be used for grouping, and Audio Orbit scans supported audio files under that folder.

Folder playlists are scanner-owned. You do not manually add individual tracks to them; instead, you add files to the folder and rescan. Manual playlists and Favorites can receive individual tracks.

### Playback

Audio Orbit supports common desktop-player behavior:

- double-click a track to play it
- seek by clicking the waveform progress bar
- use keyboard media keys for play/pause, stop, previous, and next
- automatically continue to the next track
- crossfade tracks with configurable overlap seconds
- skip long silence with configurable threshold seconds
- repeat the current track or a selected set of tracks

Crossfade is an overlap mix: the current track fades out while the next track fades in. The visible track switch happens halfway through the crossfade, so a long mix does not feel like an abrupt early track change.

### Sound profiles

Sound profiles store DSP settings such as whether orbit processing is enabled, orbit mode, output level, stereo width, orbit speed, motion smoothness, and surround cue strength.

Profile changes are applied after the UI settles, reducing playback lag while sliders are being moved.

### Backups

Backups are ZIP files containing the full app state:

- music folder playlist definitions
- manual playlists
- Favorites
- selected playlist and profile
- playback settings
- repeat settings
- crossfade settings
- silence skip settings
- update settings
- UI layout settings

Audio files themselves are not embedded in the backup. The backup stores library and playlist state, not your music collection.

### Updates

Audio Orbit can check GitHub releases for new Windows executable builds. Stable releases are checked by default. Prerelease watching can be enabled in the release watcher.

To avoid GitHub rate limiting, update checks are limited per app session.

## Get started

Download the Windows executable from the GitHub Releases page and place it in a folder where Audio Orbit can store its portable data next to the executable.

Recommended layout:

```text
audio-orbit/
├─ audio-orbit.exe
└─ audio-orbit-data/
   ├─ state.json
   └─ update/
```

Run `audio-orbit.exe`, add a folder playlist, and start playback from the track list.

## Usage

### Create a folder playlist

Use **Add folder...** from the Library panel.

Choose the music root folder, enter a playlist name, and set the grouping depth.

Example:

```text
D:\Music\Artist A\Album A\song.mp3
D:\Music\Artist A\Album B\song.mp3
D:\Music\Artist B\Album A\song.mp3
```

With grouping depth `2`, Audio Orbit groups this as:

```text
Artist A / Album A
Artist A / Album B
Artist B / Album A
```

Folder groups can be collapsed or expanded in the track list.

### Play music

Use the center track list to browse tracks. Double-click a track to start it immediately.

The top player bar shows the current track, playback metadata, waveform progress, and playback controls.

### Use repeat modes

Audio Orbit supports three repeat modes:

| Mode | Behavior |
|---|---|
| Repeat off | Normal playback order. |
| Repeat track | Repeats the current track. |
| Repeat selection | Repeats only the checked tracks in playlist order. |

When repeat selection is active, checkboxes appear in the track list so you can choose the tracks that should loop.

### Search tracks

Use the search button in the track list header to reveal search. Search filters by track title, folder group, and file path. Use **Next result** to jump between matches.

### Manage Favorites

Use the heart button next to a track to add or remove it from Favorites. Favorites is a built-in playlist and cannot be deleted.

### Export and import backups

Open **Settings**, then use **Backup and data**.

Export creates a compressed ZIP backup of the full app state. Import restores the state from a ZIP backup.

### Check for updates

Open **Settings** or **Release watcher**.

By default, only stable releases are checked. Enable prerelease watching when you want to include prerelease builds.

If an update is available, Audio Orbit can replace its current executable and restart itself.

## Data location

Audio Orbit stores app data next to the executable:

```text
audio-orbit-data/state.json
```

This keeps settings portable across version updates when the new executable replaces the old one in the same folder.

## Known limitations

Audio Orbit is a local music player, not a system-wide Windows audio processor. It plays files through the app and applies its own playback DSP to those files.

The orbit effect is headphone-friendly stereo processing, not true HRTF-based 3D surround virtualization.

Support for audio formats depends on the bundled Rust audio decoding stack. Common formats such as MP3, WAV, FLAC, OGG, OPUS, M4A, MP4, and AAC are intended to work, but every possible codec/container combination cannot be guaranteed without an FFmpeg backend.

## Contributing

Development notes, local build steps, release workflow notes, and contribution guidance live in [CONTRIBUTING.md](CONTRIBUTING.md).

## License & Acknowledgments

Audio Orbit is open source and released under the [GNU Affero General Public License v3.0 (AGPL-3.0)](https://www.gnu.org/licenses/agpl-3.0.html).

The app uses Rust ecosystem libraries for the desktop UI, audio decoding/playback, metadata reading, ZIP backups, HTTP update checks, and Lucide icons.

Copyright (C) 2020–present [Zoltán Rózsa](https://github.com/rozsazoltan)
