//! Application modules split by responsibility.
//!
//! `main.rs` owns startup and top-level shared types. The feature modules below
//! provide the `AudioOrbitApp` implementation in focused units so playback,
//! playlist, radio, updater, modal, and UI behavior can evolve independently.

#[cfg(debug_assertions)]
pub(crate) mod dev_metrics;
mod core;
mod dj_mix;
mod input;
mod library_backup;
mod lifecycle;
mod ordering;
mod playback;
mod playlist_files;
mod playlist_state;
mod radio;
mod ui_folder_import;
mod ui_library;
mod ui_modals;
mod ui_player;
mod ui_profiles;
mod ui_radio;
mod ui_status;
mod ui_tracks;
mod updates;
