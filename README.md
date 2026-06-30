# audio-orbit

`audio-orbit` is a lightweight Windows music player for local audio libraries, internet radio streams, folder-based playlists, smooth crossfade playback, silence skipping, and headphone-friendly orbit-style stereo movement.

It is designed for people who keep music in local folders and want an AIMP-like desktop player with portable app data, playlist backups, global media keys, release updates, and simple spatial stereo controls. The orbit effect can be turned off per sound profile, so the app can also be used as a normal stereo music player.

- [What it does](#what-it-does)
  - [Local music libraries](#local-music-libraries)
  - [Internet radio](#internet-radio)
  - [Playback](#playback)
  - [Sound profiles](#sound-profiles)
  - [Backups](#backups)
  - [Updates](#updates)
  - [Radio recordings](#radio-recordings)
- [Get started](#get-started)
- [Usage](#usage)
  - [Create a folder playlist](#create-a-folder-playlist)
  - [Play music](#play-music)
  - [Play internet radio](#play-internet-radio)
  - [Copy the current radio song title](#copy-the-current-radio-song-title)
  - [Use repeat modes](#use-repeat-modes)
  - [Search tracks](#search-tracks)
  - [Manage Favorites](#manage-favorites)
  - [Export and import backups](#export-and-import-backups)
  - [Check for updates](#check-for-updates)
- [Window behavior](#window-behavior)
- [Data location](#data-location)
- [Known limitations](#known-limitations)
- [Contributing](#contributing)

## What it does

Audio Orbit plays local music files from manual playlists or scanner-owned folder playlists. It can group a folder library by subfolder depth, so a structure such as `D:\Music\Artist\Album\song.mp3` can be browsed as `Artist / Album`.

### Local music libraries

Folder playlists are created from a selected directory. You choose how many folder levels should be used for grouping, and Audio Orbit scans supported audio files under that folder.

Folder playlists are scanner-owned. You do not manually add individual tracks to them; instead, you add files to the folder and rescan. Manual playlists and Favorites can receive individual tracks.

### Internet radio

Audio Orbit includes an Internet radio tab. You can save stream URLs with optional custom names; when the name is empty, Audio Orbit tries to read the station name from stream headers. Radio stations use the same list behavior as local tracks: search, left-aligned names, right-aligned actions, a dedicated scrollbar gutter, and no overflowing text. Radio playback uses a clean direct stream path and does not apply shuffle, repeat, auto-play next, crossfade, playback transitions, or silence skipping.

### Playback

Audio Orbit supports common desktop-player behavior:

- double-click a track to play it
- seek by clicking the waveform progress bar or with Left/Right for 10-second jumps
- use keyboard media keys for play/pause, stop, previous, and next
- automatically continue to the next track
- shuffle playback across the playlist or the active repeat selection
- crossfade tracks with configurable overlap seconds
- skip long silence with configurable threshold seconds
- repeat the current track or a selected set of tracks
- adjust volume from the top player bar, including player-only mode
- adjust volume with the mouse wheel over the top player bar when the pointer is over the title, waveform, or controls
- remember the last played local track between app launches
- play saved internet radio streams from the Radio tab
- favorite radio stations and filter the Radio list to favorites
- show a live radio visualizer with elapsed listening time
- record the original internet radio stream bytes to timestamped files
- copy the current internet radio song title from StreamTitle metadata when the station provides it
- remember the window size and position between app launches
- prevent multiple app instances from running at the same time

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
- internet radio station list
- update settings
- UI layout settings

Audio files themselves are not embedded in the backup. The backup stores library and playlist state, not your music collection.


### Radio recordings

Internet radio recordings are captured from the original stream bytes before volume, orbit, or any playback processing is applied. The toolbar microphone button starts recording the active radio station; while recording, the Lucide microphone icon turns red and blinks. Press it again to stop and save the file.

Saved files use the stop-time based format `audio-orbit-records-yyyy-mm-dd-hh-mm-ss.mp3`. By default, recordings are saved next to the executable in `.audio-orbit-records/`. Right-click the microphone button to open the current recordings folder, or open/change it from **Settings > Recording**.

### Updates

Audio Orbit can check GitHub releases for new Windows executable builds. Stable releases are checked by default. Prerelease watching can be enabled in the release watcher.

On startup, Audio Orbit performs a background update check at most once per hour. If a newer release is available, the release watcher modal opens automatically. To avoid GitHub rate limiting, manual update checks are also limited per app session.

## Get started

Download the Windows executable from the GitHub Releases page and place it in a folder where Audio Orbit can store its portable data next to the executable.

Recommended layout:

```text
audio-orbit/
├─ audio-orbit.exe
└─ .audio-orbit-data/
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

Folder groups can be collapsed or expanded in the track list. When a folder playlist only has one group, Audio Orbit hides the redundant folder group headers.

### Play music

Use the center track list to browse tracks. Double-click a track to start it immediately.

The top player bar keeps the current title left-aligned and truncates long titles so track names never overlap the technical details or controls. Local track metadata stays separated from the title, and player-only mode shows a reduced metadata set with just duration.

### Play internet radio

Open the Internet radio tab, paste a stream URL, optionally enter a readable station name, then double-click or use the station three-dot menu to play it. If the name is empty, Audio Orbit tries to read it from the stream. Radio rows use the same left-aligned list layout, search behavior, scrollbar gutter, favorite marking, and Details modal style as local tracks. The Radio tab hides local-only playback options such as shuffle, repeat, auto-play next, crossfade, playback transitions, and silence skipping, but the active sound profile's orbit processing can still be applied to the live stream.

### Copy the current radio song title

When an internet radio station provides `StreamTitle` metadata, Audio Orbit shows it as the current radio title. Use the copy button in the top player bar while listening to radio to copy the current song title to the clipboard.

### Use repeat modes

Audio Orbit supports three repeat modes:

| Mode | Behavior |
|---|---|
| Repeat off | Normal playback order. |
| Repeat track | Repeats the current track. |
| Repeat selection | Repeats only the checked tracks; Shuffle can randomize this selected set. |

When repeat selection is active, checkboxes appear in the track list so you can choose the tracks that should loop. Enable Shuffle to pick randomly from that selected set.

### Search tracks

Use the search button in the track list header to reveal search. Search filters by track title, folder group, and file path. Use **Next result** to jump between matches. Folder playlist context remains visible above the list, the folder dropdown grows with available entries, and **Now playing** scrolls the list back to the active track.

### Waveform and silence skip

Local track waveforms mark long quiet sections that silence skipping will bypass. Local tracks and internet radio use the same AIMP-style amplitude analyzer: unplayed audio is gray, played audio is blue, and silence-skip ranges are yellow. The analyzer combines peak, RMS, crest, and transient energy so the bars follow the musical rhythm without turning every loud section into a constant full-height wall.

### Manage Favorites

Use the heart button next to a track to add or remove it from Favorites. Favorites is a built-in playlist and cannot be deleted.

### Export and import backups

Open **Settings**, then use **Backup and data**.

Export creates a compressed ZIP backup of the full app state and suggests a timestamped filename such as `audio-orbit-backup-2026-06-30-09-15-42.zip`. Import restores the state from a ZIP backup.

### Check for updates

Open **Settings** or **Release watcher**.

By default, only stable releases are checked. Enable prerelease watching when you want to include prerelease builds.

If an update is available, Audio Orbit can replace its current executable and restart itself.

## Window behavior

Audio Orbit remembers the window size and position when the app closes and restores the same layout on the next launch. Player-only and full-layout sizes are kept separately, and switching modes restores that mode's own saved width and height.

Settings, Updates, Backup, About, folder import, add-radio, and Details dialogs use responsive modal layouts with internal scrolling on small windows and a fixed bottom info area for status/error messages.

Only one Audio Orbit instance can run at a time. If the app is already open, starting the executable again exits immediately instead of opening a second player window.

## Data location

Audio Orbit stores app data next to the executable:

```text
.audio-orbit-data/state.json
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

### Notes on waveform analysis

Audio Orbit intentionally uses a clean one-color waveform lane instead of RGB/spectrum bars. The UI layer is simple and readable: gray for unplayed waveform, blue for played waveform, and yellow for silence ranges that will be skipped. The local-file and live-radio analyzers share the same peak/RMS/transient shaping rules so both views stay visually consistent.

