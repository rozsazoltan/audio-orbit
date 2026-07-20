# audio-orbit

`audio-orbit` is a lightweight Windows music player for local audio libraries, internet radio streams, folder-based playlists, smooth crossfade playback, silence skipping, and headphone-friendly orbit-style stereo movement.


- [What it does](#what-it-does)
  - [Local music libraries](#local-music-libraries)
  - [Internet radio](#internet-radio)
  - [Playback](#playback)
  - [Sound profiles](#sound-profiles)
  - [Backups](#backups)
  - [Radio recordings](#radio-recordings)
  - [Updates](#updates)
- [Get started](#get-started)
- [Usage](#usage)
  - [Create a folder playlist](#create-a-folder-playlist)
  - [Play music](#play-music)
  - [Use keyboard shortcuts](#use-keyboard-shortcuts)
  - [Play internet radio](#play-internet-radio)
  - [Use repeat modes](#use-repeat-modes)
  - [Search tracks](#search-tracks)
  - [Manage Favorites](#manage-favorites)
  - [Manage playlist files and multi-select tracks](#manage-playlist-files-and-multi-select-tracks)
  - [Build a DJ mix](#build-a-dj-mix)
  - [Open audio files and configure file associations](#open-audio-files-and-configure-file-associations)
  - [Export and import backups](#export-and-import-backups)
- [Window behavior](#window-behavior)
- [Data location](#data-location)
- [Known limitations](#known-limitations)
- [Contributing](#contributing)

## What it does

Audio Orbit plays local music files from manual playlists or scanner-owned folder playlists. It can group a folder library by subfolder depth, so a structure such as `D:\Music\Artist\Album\song.mp3` can be browsed as `Artist / Album`.

### Local music libraries

Folder playlists are created from a selected directory. You choose how many folder levels should be used for grouping, and Audio Orbit scans supported audio files under that folder.

Folder playlists are scanner-owned. You do not manually add individual tracks to them; instead, add files to the folder and sync the playlist. Missing files remain visible as dimmed entries, while files that return are restored automatically on the next sync. Missing entries can be removed from any playlist through the track context menu without touching the disk. Manual playlists and Favorites receive the same missing-file status without losing their saved ordering.

Any playlist can be exported by copying its available audio files into a chosen folder. Original files stay in place. Filename collisions are resolved with numeric suffixes instead of overwriting existing files.

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
- optionally switch playback automatically when the system default output device changes
- retain missing playlist entries as dimmed rows and skip them during playback
- optionally watch selected folder playlist through native Windows change notifications without periodic polling
- remember the last played local track between app launches
- play saved internet radio streams from the Radio tab
- favorite radio stations and filter the Radio list to favorites
- select multiple local tracks with Ctrl-click or Shift-click, then copy, add, delete, or mix them together
- export deterministic DJ-style MP3 mixes from full tracks or selected highlights with BPM/downbeat analysis, pitch-preserving tempo sync, phrase-aligned bass swaps, EBU R128 loudness measurement, and transition diagnostics
- show a live radio visualizer with elapsed listening time
- record the original internet radio stream bytes to timestamped files
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
- library synchronization settings
- UI layout settings

Audio files themselves are not embedded in the backup. The backup stores library and playlist state, not your music collection.


### Radio recordings

Internet radio recordings are captured from the original stream bytes before volume, orbit, or any playback processing is applied. The toolbar microphone button starts recording the active radio station; while recording, the Lucide microphone icon turns red and blinks. Press it again to stop and save the file.

Saved files use the stop-time based format `audio-orbit-records-yyyy-mm-dd-hh-mm-ss.mp3`. By default, recordings are saved next to the executable in `.audio-orbit-records/`. Right-click the microphone button to open the current recordings folder, or open/change it from **Settings > Recording**.

### Updates

Audio Orbit can check GitHub releases for newer Windows executable builds. Stable releases are checked by default. Prerelease watching can be enabled from **Settings > Updates**, and returning from prerelease watching to stable releases is handled through the **Switch back to stable** action.

On release builds, Audio Orbit performs a background update check at most once per hour. Manual checks are available from the Updates panel, which can also open the GitHub Releases page, download and install an available executable update, or reinstall the latest stable build when switching back from prerelease watching.


## Get started


Recommended layout:

```text
audio-orbit/
├─ audio-orbit.exe
└─ .audio-orbit-data/
   ├─ state.json
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

Use **Sync playlist** to update only the selected playlist. Folder playlists scan their source folder for added, restored, or missing files; manual playlists and Favorites check only their saved track paths. Deleted or unavailable files stay in place as dimmed rows, preserving Favorites order, manual playlist order, and folder context. Use **Remove missing entry** from a missing track's context menu to remove only that saved item without attempting another disk deletion.

Enable **Settings > Playlist sync > Automatically watch selected folder playlist** for background synchronization. Audio Orbit uses `ReadDirectoryChangesW` for selected folder playlist root and waits in kernel state while folder is idle. No periodic folder polling, timestamp walk, or full-library scan runs in background. Windows reports changed relative paths; Audio Orbit debounces bursts, checks only those paths, and scans only a newly added or renamed subtree when required. Startup checks only saved track paths, avoiding a recursive folder walk. A full selected-playlist scan is reserved for manual sync or rare notification-buffer overflow. Files added while Audio Orbit was closed appear after manual sync; changes made while it is running arrive through Windows notifications. Manual playlists and Favorites remain available through **Sync selected playlist now**.

### Play music

Use the center track list to browse tracks. Double-click a track to start it immediately.

The top player bar keeps the current title left-aligned and truncates long titles so track names never overlap the technical details or controls. Local track metadata stays separated from the title, and player-only mode shows a reduced metadata set with just duration.

### Use keyboard shortcuts

Press **F1** to open About, **F2** to open Updates, and **F3** to open Settings. These panel shortcuts work in both full-layout and player-only mode. Pressing another panel shortcut replaces the currently open panel instead of adding it to panel navigation history.

### Play internet radio

Open the Internet radio tab, paste a stream URL, optionally enter a readable station name, then double-click or use the station three-dot menu to play it. If the name is empty, Audio Orbit tries to read it from the stream. Radio rows use the same left-aligned list layout, search behavior, scrollbar gutter, favorite marking, and Details modal style as local tracks. The Radio tab hides local-only playback options such as shuffle, repeat, auto-play next, crossfade, playback transitions, and silence skipping, but the active sound profile's orbit processing can still be applied to the live stream.

### Use repeat modes

Audio Orbit supports three repeat modes:

| Mode | Behavior |
|---|---|
| Repeat off | Normal playback order. |
| Repeat track | Repeats the current track. |
| Repeat selection | Repeats only the checked tracks; Shuffle can randomize this selected set. |

When repeat selection is active, checkboxes appear in the track list so you can choose the tracks that should loop. Enable Shuffle to pick randomly from that selected set.

### Search tracks

Use the search button in the track list header to reveal search. Search filters by track title, folder group, and file path. Use **Next result** to jump between matches. Folder playlist context remains visible above the list, the folder dropdown grows with available entries, and **Now playing** scrolls the list back to the active track. A track's right-click menu also offers **Search in playlist** when the same exact path or file name exists in another playlist; selecting a result opens that playlist and centers the matching track. Matching is evaluated only while the context menu is open and never scans the file system.

### Waveform and silence skip

Local track waveforms mark long quiet sections that silence skipping will bypass. Audio Orbit uses a RustFFT-backed analyzer for both local music and internet radio, but renders the result as an AIMP-style amplitude bar instead of a colored spectral stack: unplayed waveform bars are gray, played sections are blue, and skipped quiet sections are yellow. Internet radio uses the same smoothed analyzer in a 15-second live visualizer window with softer adaptive normalization so live streams do not collapse into constant full-height bars.

### Manage Favorites

Use the heart button next to a track to add or remove it from Favorites. Newly favorited tracks appear at the top. Favorites remembers when each track was added, so the **Added** sort restores newest-first favorite order after A-Z or Z-A sorting. Manual drag-and-drop ordering is saved with the rest of the app state. In folder playlists, tracks can only be reordered inside their existing folder group; moving a track across folder boundaries is blocked. Favorites is a built-in playlist and cannot be deleted.

### Manage playlist files and multi-select tracks

Use **Export playlist...** in the Library panel to copy every available file from the current playlist into a chosen folder. Export never moves source files and never overwrites an existing destination file; duplicate names receive a numeric suffix. In full-layout or player-only mode, right-click an empty area of the Music view to access playlist export, whole-playlist file deletion, and DJ mix creation.

Use Ctrl-click to add or remove individual tracks from the current selection. Use Shift-click to select a visible range. Press Escape to clear the track selection when no menu, search, or modal needs to close first. Right-click any selected row to copy the selected files into a folder, add all selected tracks to an existing manual playlist, or permanently delete the selected files.

The final **Add to playlist** submenu item is **New**. It asks for a playlist name, creates a manual playlist, and adds the selected track or tracks.

Use **Delete all files...** to permanently delete every file referenced by the current playlist. Multi-track deletion and whole-playlist deletion require typing `DELETE` in a confirmation modal. Successfully deleted files are removed from every playlist that references them. These actions delete files from disk and cannot be undone.

### Build a DJ mix

Use **DJ mix...** in Library panel to mix current playlist, or select at least two tracks and choose **Create DJ mix...** from track context menu. Same command appears in full-layout and player-only Music view empty-area context menus.

DJ Mix Builder offers two engines:

- **Crossfade** keeps original playback speed and joins selected sections with a simple equal-power overlap.
- **Smart DJ** plans phrase-level handoffs instead of fading two complete songs through each other. It analyzes rhythm, downbeats, energy, loudness, and harmonic compatibility; chooses a drum swap, harmonic bridge, echo drop, stem mashup, or custom bridge; keeps one bass owner; and prevents simultaneous lead-vocal handoffs.

Smart DJ can use an optional professional toolchain already installed on the machine:

- **Essentia Music Extractor** for BPM, beat positions, confidence, and musical key.
- **Rubber Band R3** for high-quality pitch-preserving tempo matching.
- **Demucs** for drums, bass, accompaniment, and vocal stems.

No external binary or model is bundled. Audio Orbit detects tools on `PATH` and supports explicit paths through:

```text
AUDIO_ORBIT_ESSENTIA_PATH
AUDIO_ORBIT_RUBBERBAND_PATH
AUDIO_ORBIT_DEMUCS_PATH
AUDIO_ORBIT_DEMUCS_PYTHON
```

Use **Refresh tools** after changing `PATH` or these variables. Missing or failed tools fall back independently to built-in rhythm analysis, deterministic WSOLA tempo matching, full-mix vocal guarding, and deck-derived loops. Export therefore remains functional without the optional toolchain.

Automatic bridge recipes:

- **Drum swap** introduces the next groove while the outgoing musical phrase finishes.
- **Harmonic bridge** is selected only for compatible detected keys and uses accompaniment stems or filtered deck material.
- **Echo drop** removes the outgoing phrase with an echo throw and performs a short controlled drop reveal.
- **Stem mashup** combines selected drums/accompaniment while vocals and bass remain mutually exclusive.
- **Custom audio** repeats a chosen MP3/WAV/FLAC/OGG range from a configurable start timestamp and loop length. Loop boundaries use a short overlap crossfade instead of periodic fade-to-zero modulation.

The complete incoming track does not sit underneath the outgoing track for a long fade. With stems, drums, bass, accompaniment, and vocals receive independent phrase gates. Without stems, a short filtered handoff and center-vocal guard are used. Every Smart DJ transition keeps a low instrumental continuity bed, including echo-drop transitions, so the handoff never reaches a complete mute.

Requested transition length is a target. When selected sections cannot fit it, Audio Orbit shortens the transition automatically and reserves middle-track audio for both neighboring transitions. The bridge follows the complete available overlap phrase instead of repeatedly cycling a one-bar fragment. Favorite ranges as short as 0.25 seconds remain valid for imported/saved plans; the interactive editor snaps range endpoints to whole seconds. Only unavailable or effectively empty decoded audio fails.

Set target mix length from 1 to 180 minutes. Each track supports:

- **Auto highlight**: select a deterministic energetic section and align its boundaries to 8/16/32-bar phrases.
- **Full track**: keep complete playable track after detected leading/trailing silence.
- **Favorite range**: set the usable section on a compact waveform while the track plays. Left-click plays or seeks, right-click moves the range start, and both blue edges are draggable. Moving the start immediately seeks preview playback to the new whole-second position. Range preview stops at the selected end and never advances to the next track. Leaving Favorite range or closing the DJ builder stops the preview and removes the main playback bar. Numeric start/end fields use whole seconds; export still phrase-aligns safe boundaries.

Smart order keeps the first track and scores following tracks by BPM distance, analysis confidence, harmonic compatibility, and energy continuity. Tempo change is capped at ±6%. Loudness leveling targets approximately -14 LUFS while peak-aware gain and output limiting prevent clipping. MP3 output uses 192/256/320 kbps.

Export runs in a background worker with stage text, progress, elapsed time, and cancellation. External processes are also cancellable. Analysis, separated stems, and professional tempo-stretch outputs are cached under the application data directory. Cache keys include source path, file size, modification time, selected section, analyzer version, and tempo ratio.

Every completed export creates:

```text
mix-name.mp3
mix-name.dj-plan.json
```

Diagnostics record selected sections, analysis backend, BPM/downbeat estimates, key confidence, tempo ratios, gain decisions, actual transition recipes, detected professional tools, fallback warnings, measured output loudness/peaks, and timing data. Use **Show diagnostics** to reveal the report. Use **Play** to add the generated MP3 to **Temporary playback** and start it immediately.

### Open audio files and configure file associations

Audio files passed to `audio-orbit.exe` open in the built-in **Temporary playback** playlist and start playing. This also works when Audio Orbit is already running: the new process forwards the requested files to the existing player instead of opening a second window.

Temporary playback is read-only. Files cannot be added through normal playlist actions, reordered, removed, exported, deleted, or included in DJ mix operations. Its contents and playback resume state are never saved, so the playlist starts empty after every app restart.

On Windows, open **Settings > File associations** and choose **Associate audio files...**. Audio Orbit registers itself for MP3, WAV, FLAC, OGG, OPUS, M4A, MP4, AAC, AIFF, AIF, APE, and WV files, then opens Windows Default Apps so the final default-app choices can be confirmed. Registration uses the current executable path and does not require administrator rights. Use **Remove registration** to remove Audio Orbit from registered audio-file applications.

### Export and import backups

Open **Settings**, then use **Backup and data**.

Export creates a compressed ZIP backup of the full app state and suggests a filename that includes the app version and UTC timestamp. Import restores the state from a ZIP backup.

### Identify the current song






## Window behavior

Audio Orbit remembers the window size and position when the app closes and restores the same layout on the next launch. Player-only and full-layout sizes are kept separately, and switching modes restores that mode's own saved width and height.


Only one Audio Orbit instance can run at a time. If the app is already open and another invocation contains supported audio files, those files are forwarded to the existing window and played through Temporary playback. Invocations without audio files exit without opening another player window.

## Data location

Release builds store portable app data next to the executable:

```text
.audio-orbit-data/state.json
```

Development runs use a stable project-local path so changing Cargo build directories does not hide settings, playlists, cached metadata, or waveforms:

```text
.cache/app-data/state.json
```

On first development start after this change, Audio Orbit migrates state and existing DJ analysis cache from previous `target/debug/.audio-orbit-data` and `.cache/cargo-target/debug/.audio-orbit-data` locations.


## Known limitations

Audio Orbit is a local music player, not a system-wide Windows audio processor. It plays files through the app and applies its own playback DSP to those files.

The orbit effect is headphone-friendly stereo processing, not true HRTF-based 3D surround virtualization.

Support for audio formats depends on the bundled Rust audio decoding stack. Common formats such as MP3, WAV, FLAC, OGG, OPUS, M4A, MP4, and AAC are intended to work, but every possible codec/container combination cannot be guaranteed without an FFmpeg backend.

Automatic synchronization is scoped to selected folder playlist. Windows wakes Audio Orbit only after matching file or directory changes and supplies changed paths. Short debounce coalesces bursty add, remove, and rename events before incremental reconciliation. Existing trees are not walked for ordinary file changes; only newly added or renamed directories are scanned. Rare notification-buffer overflow falls back to one full selected-playlist scan. Startup checks saved track paths without walking the folder tree. Idle folders are never polled. Recursive scans do not follow symbolic links or Windows reparse points.

## Contributing


## License & Acknowledgments



Copyright (C) 2020–present [Zoltán Rózsa](https://github.com/rozsazoltan)


DJ Mix Builder uses built-in pure-Rust rhythm analysis and WSOLA fallback, `ebur128` under MIT for EBU R128 measurement, and `shine-rs` under LGPL-2.0 for MP3 encoding. Optional Essentia, Rubber Band, and Demucs executables are invoked only when separately installed. Redistribution notices are tracked in `THIRD_PARTY_NOTICES.md`. Audio Orbit remains licensed under AGPL-3.0-or-later.

Audio Orbit renders local and live radio waveform bars through a RustFFT-backed amplitude analysis path. The visual design intentionally follows AIMP-like progress bars: neutral gray for the upcoming waveform, blue for the played region, and yellow markers for silence-skip sections. The analyzer still uses spectral information internally to shape a stable loudness envelope, but the UI does not draw colored bass/mid/treble stacks.



## Development runner

Use `cargo dev` from the repository root. Development state stays in `.cache/app-data`, independent from executable build location. Mutagen workflows exclude root `/.cache/` and `/target/`, so Windows build artifacts and development state remain local to Windows mirror. The repository contains `.cargo/config.toml`, so Cargo uses the built-in polling dev runner instead of requiring the external `cargo-watch` subcommand. On Windows, `scripts/dev.ps1` runs the same project-local runner directly with `cargo run --bin audio-orbit-dev --`.

### DJ mix export responsiveness

DJ mix rendering runs on a named background worker thread. The export dialog shows an animated activity indicator, current processing stage, elapsed time, percentage, and cancellation control while the main player and library UI remain responsive.


### DJ transition behavior

Smart DJ chooses phrase-aware drum swaps, harmonic bridges, echo drops, stem mashups, or custom looped bridge audio. When Demucs stems are available, it gives drums, bass, accompaniment, and vocals separate handoff curves. Without stems, the full-mix fallback uses short filtered deck handoffs and vocal guarding rather than a long two-song fade.
