use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputDeviceChangeAction {
    None,
    ClearPending,
    Notify,
    Refresh,
}

fn output_device_change_action(
    last_known_output: &str,
    detected_output: Option<&str>,
    current_output: &str,
    auto_switch_output_device: bool,
) -> OutputDeviceChangeAction {
    if current_output == last_known_output {
        return if detected_output.is_some() {
            OutputDeviceChangeAction::ClearPending
        } else {
            OutputDeviceChangeAction::None
        };
    }

    if detected_output == Some(current_output) {
        return OutputDeviceChangeAction::None;
    }

    if auto_switch_output_device {
        OutputDeviceChangeAction::Refresh
    } else {
        OutputDeviceChangeAction::Notify
    }
}

fn output_device_change_message(output_name: &str) -> String {
    format!(
        "Output device changed to {output_name}. Refresh output to continue on the new device."
    )
}

impl AudioOrbitApp {
    pub(crate) fn add_profile(&mut self) {
        let settings = self.current_settings();
        let number = self.state.profiles.len() + 1;
        self.state
            .profiles
            .push(config::DspProfile::new(format!("Profile {number}"), settings));
        self.state.selected_profile_index = self.state.profiles.len() - 1;
        self.status_message = "Created a new sound profile from the current settings.".to_owned();
        self.save_state_silently();
    }
    pub(crate) fn remove_current_profile(&mut self) {
        if self.state.profiles.len() <= 1 {
            self.error_message = Some("At least one sound profile is required.".to_owned());
            return;
        }

        self.state.profiles.remove(self.state.selected_profile_index);
        self.state.selected_profile_index = self.state.selected_profile_index.saturating_sub(1);
        self.status_message = "Removed sound profile.".to_owned();
        self.save_state_silently();
        self.schedule_current_profile_apply();
    }
    pub(crate) fn play_selected_or_first_track(&mut self) {
        if !self.selected_track_is_visible() {
            self.selected_track_index = self.eligible_track_indexes().first().copied();
        }

        let Some(path) = self.selected_track_path() else {
            self.error_message = Some("Select a track first.".to_owned());
            return;
        };

        let start_seconds = self.saved_paused_resume_position_for_track(&path).unwrap_or(0.0);
        self.play_path(path, self.selected_track_index, start_seconds);
    }
    pub(crate) fn saved_paused_resume_position_for_track(&self, path: &Path) -> Option<f32> {
        let session = &self.state.playback_session;
        if !session.was_paused || session.source != "track" {
            return None;
        }
        let session_path = session.track_path.as_ref()?;
        if same_path(session_path, path) {
            Some(session.position_seconds.max(0.0))
        } else {
            None
        }
    }
    pub(crate) fn play_path(&mut self, path: PathBuf, index: Option<usize>, start_seconds: f32) {
        let crossfade_seconds = if start_seconds <= 0.05 {
            self.configured_manual_crossfade_seconds()
        } else {
            0.0
        };
        self.play_path_with_crossfade(path, index, start_seconds, crossfade_seconds);
    }
    pub(crate) fn play_path_with_crossfade(
        &mut self,
        path: PathBuf,
        index: Option<usize>,
        start_seconds: f32,
        crossfade_seconds: f32,
    ) {
        self.prepare_track_playback(path, index, start_seconds, crossfade_seconds, false);
    }

    fn request_active_track_scroll_if_changed(
        &mut self,
        previous_index: Option<usize>,
        previous_path: Option<PathBuf>,
        new_index: Option<usize>,
        new_path: &Path,
    ) {
        let path_changed = previous_path
            .as_ref()
            .map(|path| !same_path(path, new_path))
            .unwrap_or(true);
        if path_changed || previous_index != new_index {
            self.scroll_to_active_track_requested = true;
        }
    }

    pub(crate) fn cached_track_for_path(&self, index: Option<usize>, path: &Path) -> Option<&Track> {
        let playlist = self.current_playlist()?;
        index
            .and_then(|index| playlist.tracks.get(index))
            .filter(|track| same_path(&track.path, path))
            .or_else(|| playlist.tracks.iter().find(|track| same_path(&track.path, path)))
    }
    pub(crate) fn cached_waveform_for_track(&self, index: Option<usize>, path: &Path) -> Option<(Vec<f32>, Vec<f32>)> {
        let track = self.cached_track_for_path(index, path)?;

        if track.waveform.is_empty() || track.waveform_brightness.is_empty() {
            None
        } else {
            Some((track.waveform.clone(), track.waveform_brightness.clone()))
        }
    }
    pub(crate) fn known_duration_for_track(&self, index: Option<usize>, path: &Path) -> Option<f32> {
        self.cached_track_for_path(index, path)
            .and_then(|track| track.metadata.duration_seconds)
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
    }
    fn silence_settings_fingerprint(settings: DspSettings) -> SilenceSettingsFingerprint {
        let trigger_millis = if settings.silence_trigger_millis == 0 {
            settings.silence_threshold_seconds.max(1) as u16 * 1000
        } else {
            settings.silence_trigger_millis
        };

        SilenceSettingsFingerprint {
            skip_silence_enabled: settings.skip_silence_enabled,
            trigger_millis,
            threshold_db: settings.silence_threshold_db,
            trim_end_regardless_of_duration: settings.silence_trim_end_regardless_of_duration,
        }
    }
    fn audio_file_cache_identity(path: &Path) -> (Option<u64>, Option<u128>) {
        let Ok(metadata) = fs::metadata(path) else {
            return (None, None);
        };

        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());

        (Some(metadata.len()), modified_nanos)
    }
    pub(crate) fn cached_silence_ranges_for_track(
        &self,
        path: &Path,
        settings: DspSettings,
    ) -> Option<Vec<(f32, f32)>> {
        if !settings.skip_silence_enabled {
            return None;
        }

        let entry = self.silence_analysis_cache.get(path)?;
        let (file_len, modified_nanos) = Self::audio_file_cache_identity(path);
        let settings = Self::silence_settings_fingerprint(settings);
        if entry.file_len == file_len && entry.modified_nanos == modified_nanos && entry.settings == settings {
            Some(entry.ranges.clone())
        } else {
            None
        }
    }
    fn silence_adjusted_seek_position(seconds: f32, silence_ranges: Option<&[(f32, f32)]>) -> f32 {
        let mut position = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        let Some(ranges) = silence_ranges else {
            return position;
        };

        for _ in 0..8 {
            let previous = position;
            for (start, end) in ranges {
                if *end > *start && position >= *start && position < *end {
                    position = *end;
                }
            }
            if (position - previous).abs() < 0.001 {
                break;
            }
        }

        position
    }

    pub(crate) fn remember_silence_ranges_for_track(
        &mut self,
        path: &Path,
        settings: DspSettings,
        ranges: Vec<(f32, f32)>,
    ) {
        if !settings.skip_silence_enabled {
            return;
        }

        let (file_len, modified_nanos) = Self::audio_file_cache_identity(path);
        self.silence_analysis_cache.insert(
            path.to_path_buf(),
            SilenceAnalysisCacheEntry {
                file_len,
                modified_nanos,
                settings: Self::silence_settings_fingerprint(settings),
                ranges,
            },
        );

        const MAX_SILENCE_ANALYSIS_CACHE_ENTRIES: usize = 256;
        while self.silence_analysis_cache.len() > MAX_SILENCE_ANALYSIS_CACHE_ENTRIES {
            let Some(oldest_key) = self.silence_analysis_cache.keys().next().cloned() else {
                break;
            };
            self.silence_analysis_cache.remove(&oldest_key);
        }
    }
    fn ensure_track_available_for_playback(&mut self, path: &Path) -> bool {
        let is_present = fs::metadata(path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);
        let mut changed = false;

        for playlist in &mut self.state.playlists {
            for track in &mut playlist.tracks {
                if !same_path(&track.path, path) {
                    continue;
                }
                let missing = !is_present;
                if track.missing != missing {
                    track.missing = missing;
                    changed = true;
                }
            }
        }

        if changed {
            self.save_state_silently();
        }
        if !is_present {
            self.error_message = Some(format!("File not found: {}", path.display()));
        }
        is_present
    }

    pub(crate) fn prepare_track_playback(
        &mut self,
        path: PathBuf,
        index: Option<usize>,
        start_seconds: f32,
        crossfade_seconds: f32,
        live_position_compensation: bool,
    ) {
        if self.player.is_none() {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        }
        if !self.ensure_track_available_for_playback(&path) {
            return;
        }

        let settings = self.current_settings();
        let playlist_index = self.state.selected_playlist_index;
        let cached_waveform = self.cached_waveform_for_track(index, &path);
        let cached_silence_ranges = self.cached_silence_ranges_for_track(&path, settings);
        let start_seconds = Self::silence_adjusted_seek_position(start_seconds, cached_silence_ranges.as_deref());
        let known_duration_seconds = self.known_duration_for_track(index, &path);

        if !settings.skip_silence_enabled {
            self.pending_prepared_track_receiver = None;
            let result = self
                .player
                .as_mut()
                .expect("audio player was checked above")
                .play_file_streaming_with_cached_waveform_and_crossfade(
                    &path,
                    settings,
                    start_seconds,
                    cached_waveform,
                    None,
                    known_duration_seconds,
                    crossfade_seconds,
                );

            match result {
                Ok(mut info) => {
                    if let Some(ranges) = cached_silence_ranges.clone() {
                        info.silence_ranges = ranges;
                    }
                    let mode_label = if settings.orbit_enabled {
                        settings.mode.label()
                    } else {
                        "normal stereo playback"
                    };
                    self.active_tab = MainContentTab::Music;
                    self.active_radio_index = None;
                    self.active_radio_station_name = None;
                    self.active_radio_title = None;
                    self.radio_started_at = None;
                    self.last_radio_title_lookup_at = None;
                    self.radio_title_receiver = None;
                    let previous_track_index = self.active_track_index;
                    let previous_track_path = self.active_track_path.clone();
                    self.active_playlist_index = Some(playlist_index);
                    self.selected_track_index = index;
                    self.active_track_index = index;
                    self.active_track_path = Some(info.path.clone());
                    self.request_active_track_scroll_if_changed(previous_track_index, previous_track_path, index, &info.path);
                    self.pending_track_switch = None;
                    self.crossfade_started_for_path = None;
                    self.store_playback_metadata(&info);
                    self.remember_last_played_track(index, &info.path);
                    self.last_playback = Some(info.clone());
                    self.status_message = if live_position_compensation {
                        format!("Applied sound profile and continued {} through {}.", display_file_name(&info.path), mode_label)
                    } else if crossfade_seconds > 0.05 {
                        format!(
                            "Crossfading to {} through {}; previous source is fading out.",
                            display_file_name(&info.path),
                            mode_label
                        )
                    } else {
                        format!("Playing {} through {}.", display_file_name(&info.path), mode_label)
                    };
                    self.persist_playback_session();
                    self.save_state_silently();
                    self.error_message = None;
                }
                Err(error) => {
                    self.error_message = Some(error.to_string());
                    self.status_message = "Playback failed.".to_owned();
                }
            }
            return;
        }

        let fast_result = self
            .player
            .as_mut()
            .expect("audio player was checked above")
            .play_file_streaming_with_cached_waveform_and_crossfade(
                &path,
                settings,
                start_seconds,
                cached_waveform.clone(),
                cached_silence_ranges.clone(),
                known_duration_seconds,
                crossfade_seconds,
            );

        let mut quick_started = false;
        let mut requested_at = Instant::now();
        match fast_result {
            Ok(mut info) => {
                if let Some(ranges) = cached_silence_ranges.clone() {
                    info.silence_ranges = ranges;
                }
                quick_started = true;
                requested_at = Instant::now();
                let mode_label = if settings.orbit_enabled {
                    settings.mode.label()
                } else {
                    "normal stereo playback"
                };
                self.active_tab = MainContentTab::Music;
                self.active_radio_index = None;
                self.active_radio_station_name = None;
                self.active_radio_title = None;
                self.radio_started_at = None;
                self.last_radio_title_lookup_at = None;
                self.radio_title_receiver = None;
                let previous_track_index = self.active_track_index;
                let previous_track_path = self.active_track_path.clone();
                self.active_playlist_index = Some(playlist_index);
                self.selected_track_index = index;
                self.active_track_index = index;
                self.active_track_path = Some(info.path.clone());
                self.request_active_track_scroll_if_changed(previous_track_index, previous_track_path, index, &info.path);
                self.pending_track_switch = None;
                self.crossfade_started_for_path = None;
                self.store_playback_metadata(&info);
                self.remember_last_played_track(index, &info.path);
                self.last_playback = Some(info.clone());
                self.status_message = if crossfade_seconds > 0.05 {
                    format!(
                        "Crossfading to {} immediately through {}; preparing silence skip in the background.",
                        display_file_name(&info.path),
                        mode_label
                    )
                } else {
                    format!(
                        "Playing {} immediately through {}; preparing silence skip in the background.",
                        display_file_name(&info.path),
                        mode_label
                    )
                };
                self.persist_playback_session();
                self.save_state_silently();
                self.error_message = None;
            }
            Err(error) => {
                self.status_message = if live_position_compensation {
                    "Fast playback failed; preparing updated playback without stopping the current audio...".to_owned()
                } else if crossfade_seconds > 0.05 {
                    format!(
                        "Fast playback failed; preparing crossfade for {:.1} second(s)...",
                        crossfade_seconds
                    )
                } else {
                    format!("Fast playback failed; preparing {}...", display_file_name(&path))
                };
                self.error_message = Some(error.to_string());
            }
        }

        let background_crossfade_seconds = if quick_started { 0.0 } else { crossfade_seconds };
        let background_live_position_compensation = quick_started || live_position_compensation;
        let background_upgrade = quick_started;
        let (sender, receiver) = mpsc::channel();
        let path_for_thread = path.clone();

        self.pending_prepared_track_receiver = Some(receiver);
        self.error_message = if quick_started { None } else { self.error_message.take() };

        thread::spawn(move || {
            let result = AudioPlayer::prepare_file_with_cached_analysis(
                path_for_thread,
                settings,
                start_seconds,
                cached_waveform,
                cached_silence_ranges,
            )
                .map(|prepared| PreparedTrackPlayback {
                    playlist_index,
                    index,
                    crossfade_seconds: background_crossfade_seconds,
                    live_position_compensation: background_live_position_compensation,
                    background_upgrade,
                    prepared,
                    requested_at,
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }
    pub(crate) fn process_prepared_track_playback(&mut self) {
        let Some(receiver) = &self.pending_prepared_track_receiver else {
            return;
        };

        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_prepared_track_receiver = None;
                return;
            }
        };

        self.pending_prepared_track_receiver = None;
        let prepared = match message {
            Ok(prepared) => prepared,
            Err(error) => {
                self.error_message = Some(error);
                self.status_message = "Playback preparation failed.".to_owned();
                return;
            }
        };

        let PreparedTrackPlayback {
            playlist_index,
            index,
            crossfade_seconds,
            live_position_compensation,
            background_upgrade,
            prepared: prepared_audio,
            requested_at,
        } = prepared;

        let (prepared_path, prepared_settings, prepared_silence_ranges) =
            prepared_audio.silence_analysis_cache_data();
        self.remember_silence_ranges_for_track(
            prepared_path,
            prepared_settings,
            prepared_silence_ranges,
        );

        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        let render_elapsed_seconds = requested_at.elapsed().as_secs_f32();
        let was_playing = player.is_playing();
        let result = if crossfade_seconds > 0.05 {
            player.crossfade_to_prepared(prepared_audio, crossfade_seconds)
        } else if live_position_compensation && was_playing {
            player.play_prepared_from_live_position(prepared_audio, render_elapsed_seconds)
        } else {
            player.play_prepared(prepared_audio)
        };

        match result {
            Ok(info) => {
                let settings = self.current_settings();
                let mode_label = if settings.orbit_enabled {
                    settings.mode.label()
                } else {
                    "normal stereo playback"
                };
                self.active_tab = MainContentTab::Music;
                self.active_radio_index = None;
                self.active_radio_station_name = None;
                self.active_radio_title = None;
                self.radio_started_at = None;
                self.last_radio_title_lookup_at = None;
                self.radio_title_receiver = None;
                let previous_track_index = self.active_track_index;
                let previous_track_path = self.active_track_path.clone();
                self.active_playlist_index = Some(playlist_index);
                self.selected_track_index = index;

                self.active_track_index = index;
                self.active_track_path = Some(info.path.clone());
                self.request_active_track_scroll_if_changed(previous_track_index, previous_track_path, index, &info.path);
                self.pending_track_switch = None;
                self.crossfade_started_for_path = None;
                self.store_playback_metadata(&info);
                self.remember_last_played_track(index, &info.path);
                self.last_playback = Some(info.clone());
                self.status_message = if background_upgrade {
                    format!("Silence-skip preparation finished for {}.", display_file_name(&info.path))
                } else if live_position_compensation {
                    format!("Applied sound profile and continued {} through {}.", display_file_name(&info.path), mode_label)
                } else if crossfade_seconds > 0.05 {
                    format!(
                        "Crossfading to {} through {}; previous source is fading out.",
                        display_file_name(&info.path),
                        mode_label
                    )
                } else {
                    format!("Playing {} through {}.", display_file_name(&info.path), mode_label)
                };
                self.persist_playback_session();
                self.save_state_silently();
                self.error_message = None;
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Playback failed.".to_owned();
            }
        }
    }
    pub(crate) fn play_next_track(&mut self) {
        let crossfade_seconds = self.configured_manual_crossfade_seconds();
        self.play_next_track_with_crossfade(crossfade_seconds);
    }
    pub(crate) fn play_next_track_with_crossfade(&mut self, crossfade_seconds: f32) {
        let Some((next_index, path)) = self.next_track_candidate() else {
            return;
        };

        self.selected_track_index = Some(next_index);
        self.play_path_with_crossfade(path, Some(next_index), 0.0, crossfade_seconds);
    }
    pub(crate) fn next_track_candidate(&self) -> Option<(usize, PathBuf)> {
        let indexes = self.playback_sequence_indexes();
        if indexes.is_empty() {
            return None;
        }

        let current_index = self.active_track_index.or(self.selected_track_index);
        let next_index = if self.state.playback.repeat_mode == RepeatMode::Track {
            current_index
                .filter(|index| indexes.contains(index))
                .unwrap_or(indexes[0])
        } else if self.state.playback.shuffle_enabled {
            self.random_sequence_index(&indexes, current_index)?
        } else {
            let current_position = current_index.and_then(|index| indexes.iter().position(|candidate| *candidate == index));
            let next_position = current_position.map(|position| (position + 1) % indexes.len()).unwrap_or(0);
            indexes[next_position]
        };

        let path = self
            .current_playlist()?
            .tracks
            .get(next_index)?
            .path
            .clone();

        Some((next_index, path))
    }
    pub(crate) fn configured_manual_crossfade_seconds(&self) -> f32 {
        let is_currently_playing = self
            .player
            .as_ref()
            .map(AudioPlayer::is_playing)
            .unwrap_or(false);

        if self.state.playback.crossfade_enabled && is_currently_playing {
            self.state.playback.crossfade_seconds.max(1) as f32
        } else {
            0.0
        }
    }
    pub(crate) fn play_previous_track(&mut self) {
        let indexes = self.playback_sequence_indexes();
        if indexes.is_empty() {
            return;
        }

        let current_index = self.active_track_index.or(self.selected_track_index);
        let previous_index = if self.state.playback.repeat_mode == RepeatMode::Track {
            current_index
                .filter(|index| indexes.contains(index))
                .unwrap_or(indexes[0])
        } else {
            let current_position = current_index.and_then(|index| indexes.iter().position(|candidate| *candidate == index));
            let previous_position = current_position
                .map(|position| if position == 0 { indexes.len() - 1 } else { position - 1 })
                .unwrap_or(0);
            indexes[previous_position]
        };

        let Some(path) = self
            .current_playlist()
            .and_then(|playlist| playlist.tracks.get(previous_index))
            .map(|track| track.path.clone())
        else {
            return;
        };

        self.selected_track_index = Some(previous_index);
        let crossfade_seconds = self.configured_manual_crossfade_seconds();
        self.play_path_with_crossfade(path, Some(previous_index), 0.0, crossfade_seconds);
    }
    pub(crate) fn current_waveform_for_seek(&self) -> Option<(Vec<f32>, Vec<f32>)> {
        if let Some(playback) = &self.last_playback {
            if !playback.waveform.is_empty() && !playback.waveform_brightness.is_empty() {
                return Some((playback.waveform.clone(), playback.waveform_brightness.clone()));
            }
        }

        let path = self.active_track_path.as_ref()?;
        self.cached_waveform_for_track(self.active_track_index, path)
    }
    pub(crate) fn seek_current(&mut self, seconds: f32) {
        let settings = self
            .player
            .as_ref()
            .and_then(AudioPlayer::current_settings)
            .unwrap_or_else(|| self.current_settings());
        let known_duration_seconds = self
            .last_playback
            .as_ref()
            .map(|playback| playback.original_duration_seconds)
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .or_else(|| {
                self.active_track_path
                    .as_ref()
                    .and_then(|path| self.known_duration_for_track(self.active_track_index, path))
            });

        self.schedule_or_run_fast_seek(
            seconds,
            settings,
            known_duration_seconds,
            settings.skip_silence_enabled,
        );
    }

    fn schedule_or_run_fast_seek(
        &mut self,
        seconds: f32,
        settings: DspSettings,
        known_duration_seconds: Option<f32>,
        prepare_after_streaming: bool,
    ) {
        let Some(path) = self.active_track_path.clone() else {
            return;
        };
        let Some(player) = self.player.as_ref() else {
            return;
        };
        if !(player.is_playing() || player.is_paused()) {
            return;
        }

        let duration = known_duration_seconds.unwrap_or_else(|| self.displayed_playback_duration_seconds());
        let requested_position_seconds = if duration.is_finite() && duration > 0.0 {
            seconds.clamp(0.0, duration)
        } else {
            seconds.max(0.0)
        };
        let cached_silence_ranges = self.cached_silence_ranges_for_track(&path, settings);
        let position_seconds = Self::silence_adjusted_seek_position(
            requested_position_seconds,
            cached_silence_ranges.as_deref(),
        );
        let now = Instant::now();
        let playlist_index = self.active_playlist_index.unwrap_or(self.state.selected_playlist_index);
        let index = self.active_track_index;

        // Any new seek makes older prepared results stale. The user-visible path is the
        // fast streaming seek; heavier processed playback is only allowed to catch up
        // after the final settled seek target.
        self.pending_seek_prepare = None;
        self.pending_prepared_track_receiver = None;

        let can_restart_now = self.pending_fast_seek.is_none()
            && self
                .last_fast_seek_started_at
                .map(|started| now.saturating_duration_since(started) >= FAST_SEEK_COALESCE_INTERVAL)
                .unwrap_or(true);

        if can_restart_now {
            self.execute_fast_streaming_seek(
                path,
                playlist_index,
                index,
                position_seconds,
                settings,
                known_duration_seconds,
                prepare_after_streaming,
            );
            return;
        }

        self.pending_fast_seek = Some(PendingFastSeek {
            run_after: now + FAST_SEEK_COALESCE_INTERVAL,
            playlist_index,
            index,
            path,
            position_seconds,
            settings,
            known_duration_seconds,
            prepare_after_streaming,
        });
        self.status_message = format!("Seek to {}...", format_duration(position_seconds));
        self.status_updated_at = now;
        self.waveform_drag_position_seconds = None;
        self.persist_playback_session();
    }

    fn execute_fast_streaming_seek(
        &mut self,
        path: PathBuf,
        playlist_index: usize,
        index: Option<usize>,
        seconds: f32,
        settings: DspSettings,
        known_duration_seconds: Option<f32>,
        prepare_after_streaming: bool,
    ) {
        let cached_waveform = self.current_waveform_for_seek();
        let cached_silence_ranges = self.cached_silence_ranges_for_track(&path, settings);
        let Some(player) = &mut self.player else {
            return;
        };
        self.last_fast_seek_started_at = Some(Instant::now());

        match player.play_file_streaming_with_cached_waveform_and_crossfade(
            &path,
            settings,
            seconds,
            cached_waveform.clone(),
            cached_silence_ranges.clone(),
            known_duration_seconds,
            0.0,
        ) {
            Ok(mut info) => {
                if let Some(ranges) = cached_silence_ranges.clone() {
                    info.silence_ranges = ranges;
                }
                self.status_message = format!("Seeked to {}.", format_duration(seconds));
                self.status_updated_at = Instant::now();
                self.active_tab = MainContentTab::Music;
                self.active_radio_index = None;
                self.active_radio_station_name = None;
                self.active_radio_title = None;
                self.radio_started_at = None;
                self.last_radio_title_lookup_at = None;
                self.radio_title_receiver = None;
                self.active_playlist_index = Some(playlist_index);
                self.selected_track_index = index;
                self.active_track_index = index;
                self.active_track_path = Some(info.path.clone());
                self.store_playback_metadata(&info);
                self.remember_last_played_track(index, &info.path);
                self.last_playback = Some(info);
                self.waveform_drag_position_seconds = None;
                self.pending_fast_seek = None;
                self.error_message = None;

                if prepare_after_streaming {
                    self.pending_seek_prepare = Some(PendingSeekPrepare {
                        run_after: Instant::now() + SEEK_PREPARE_DEBOUNCE,
                        requested_at: Instant::now(),
                        playlist_index,
                        index,
                        path,
                        start_seconds: seconds,
                        settings,
                        cached_waveform,
                        cached_silence_ranges,
                    });
                } else {
                    self.pending_seek_prepare = None;
                }

                self.persist_playback_session();
                if !prepare_after_streaming {
                    self.save_state_silently();
                }
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.waveform_drag_position_seconds = None;
            }
        }
    }

    pub(crate) fn process_pending_fast_seek(&mut self) {
        let Some(pending) = self.pending_fast_seek.as_ref() else {
            return;
        };
        if Instant::now() < pending.run_after {
            return;
        }

        let pending = self.pending_fast_seek.take().expect("pending fast seek was checked above");
        if self
            .active_track_path
            .as_ref()
            .map(|path| !same_path(path, &pending.path))
            .unwrap_or(true)
        {
            return;
        }

        self.execute_fast_streaming_seek(
            pending.path,
            pending.playlist_index,
            pending.index,
            pending.position_seconds,
            pending.settings,
            pending.known_duration_seconds,
            pending.prepare_after_streaming,
        );
    }

    pub(crate) fn process_pending_seek_prepare(&mut self) {
        let Some(pending) = self.pending_seek_prepare.as_ref() else {
            return;
        };
        if Instant::now() < pending.run_after {
            return;
        }

        let pending = self.pending_seek_prepare.take().expect("pending seek prepare was checked above");
        if self
            .active_track_path
            .as_ref()
            .map(|path| !same_path(path, &pending.path))
            .unwrap_or(true)
        {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        self.pending_prepared_track_receiver = Some(receiver);
        thread::spawn(move || {
            let result = AudioPlayer::prepare_file_with_cached_analysis(
                pending.path,
                pending.settings,
                pending.start_seconds,
                pending.cached_waveform,
                pending.cached_silence_ranges,
            )
            .map(|prepared| PreparedTrackPlayback {
                playlist_index: pending.playlist_index,
                index: pending.index,
                crossfade_seconds: 0.0,
                live_position_compensation: true,
                background_upgrade: true,
                prepared,
                requested_at: pending.requested_at,
            })
            .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }
    pub(crate) fn schedule_current_profile_apply(&mut self) {
        self.pending_profile_apply_at = Some(Instant::now() + Duration::from_secs(3));
        self.profile_apply_applied_until = None;
    }
    pub(crate) fn process_pending_profile_apply(&mut self) {
        if let Some(until) = self.profile_apply_applied_until {
            if Instant::now() >= until {
                self.profile_apply_applied_until = None;
            }
        }

        let Some(apply_at) = self.pending_profile_apply_at else {
            return;
        };

        if Instant::now() < apply_at {
            return;
        }

        self.pending_profile_apply_at = None;
        self.profile_apply_applied_until = Some(Instant::now() + Duration::from_secs(2));
        self.save_state_silently();
        self.apply_current_profile_live();
    }
    pub(crate) fn profile_apply_status_text(&self) -> Option<String> {
        let now = Instant::now();
        if let Some(apply_at) = self.pending_profile_apply_at {
            let seconds = apply_at.saturating_duration_since(now).as_secs_f32().ceil().max(1.0) as u64;
            return Some(format!("Apply in {seconds}s..."));
        }
        if self
            .profile_apply_applied_until
            .map(|until| now < until)
            .unwrap_or(false)
        {
            return Some("Sound profile applied.".to_owned());
        }
        None
    }
    pub(crate) fn apply_current_profile_live(&mut self) {
        let settings = self.current_settings();

        if let Some(radio_index) = self.active_radio_index {
            let Some(station) = self.state.radio_stations.get(radio_index).cloned() else {
                return;
            };
            let crossfade_seconds = self.configured_manual_crossfade_seconds();
            let play_result = {
                let Some(player) = &mut self.player else {
                    return;
                };

                if !(player.is_playing() || player.is_paused()) {
                    return;
                }

                player.play_radio_stream_with_crossfade(&station.url, settings, crossfade_seconds)
            };

            match play_result {
                Ok(()) => {
                    self.active_tab = MainContentTab::Radio;
                    self.active_radio_station_name = station.last_station_name.clone();
                    self.active_radio_title = station.last_stream_title.clone();
                    self.radio_started_at = Some(Instant::now());
                    self.last_radio_title_lookup_at = Some(Instant::now());
                    self.error_message = None;
                    self.persist_playback_session();
                    self.save_state_silently();
                    self.start_radio_title_lookup(radio_index, station.url);
                }
                Err(error) => {
                    self.error_message = Some(error.to_string());
                }
            }
            return;
        }

        let position = self.displayed_playback_position_seconds();
        let Some(path) = self.active_track_path.clone() else {
            return;
        };
        let (is_active, live_position_compensation) = {
            let Some(player) = &self.player else {
                return;
            };
            (player.is_playing() || player.is_paused(), player.is_playing())
        };

        if !is_active {
            return;
        }

        self.prepare_track_playback(path, self.active_track_index, position, 0.0, live_position_compensation);
    }
    pub(crate) fn process_pending_track_switch(&mut self) {
        let Some(pending) = self.pending_track_switch.clone() else {
            return;
        };

        if Instant::now() < pending.switch_at {
            return;
        }

        self.pending_track_switch = None;
        let previous_track_index = self.active_track_index;
        let previous_track_path = self.active_track_path.clone();
        self.active_track_index = pending.index;
        self.active_playlist_index = Some(pending.playlist_index);
        self.active_track_path = Some(pending.info.path.clone());
        self.selected_track_index = pending.index;
        self.request_active_track_scroll_if_changed(previous_track_index, previous_track_path, pending.index, &pending.info.path);
        self.crossfade_started_for_path = None;
        self.remember_last_played_track(pending.index, &pending.info.path);
        self.store_playback_metadata(&pending.info);
        self.last_playback = Some(pending.info);
        self.persist_playback_session();
        self.save_state_silently();
    }
    pub(crate) fn displayed_playback_position_seconds(&self) -> f32 {
        if let Some(pending) = &self.pending_track_switch {
            let elapsed = pending.started_at.elapsed().as_secs_f32();
            return (pending.previous_position + elapsed).min(pending.previous_duration);
        }

        if let Some(pending) = &self.pending_fast_seek {
            return pending.position_seconds;
        }

        let Some(player) = self.player.as_ref() else {
            return 0.0;
        };

        let rendered_position = player.playback_position_seconds();
        let render_start = player.current_start_offset_seconds();
        if let Some(playback) = &self.last_playback {
            rendered_to_original_position(rendered_position, render_start, playback)
        } else {
            rendered_position
        }
    }
    pub(crate) fn displayed_playback_duration_seconds(&self) -> f32 {
        if let Some(pending) = &self.pending_track_switch {
            return pending.previous_duration;
        }

        self.last_playback
            .as_ref()
            .map(|playback| playback.original_duration_seconds)
            .or_else(|| self.player.as_ref().and_then(AudioPlayer::playback_duration_seconds))
            .unwrap_or(0.0)
    }
    pub(crate) fn stop(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }

        self.active_track_index = None;
        self.active_playlist_index = None;
        self.active_track_path = None;
        self.active_radio_index = None;
        self.active_radio_station_name = None;
        self.active_radio_title = None;
        self.radio_started_at = None;
        self.last_radio_title_lookup_at = None;
        self.radio_title_receiver = None;
        self.pending_track_switch = None;
        self.crossfade_started_for_path = None;
        self.last_playback = None;
        self.status_message = "Playback stopped.".to_owned();
        self.persist_playback_session();
        self.save_state_silently();
    }
    pub(crate) fn pause_or_resume(&mut self) {
        if let Some(player) = &mut self.player {
            player.pause_or_resume();

            if player.is_paused() {
                self.status_message = "Playback paused.".to_owned();
            } else if player.is_playing() {
                self.status_message = "Playback resumed.".to_owned();
            }
        }
        self.persist_playback_session();
        self.save_state_silently();
    }
    pub(crate) fn refresh_output_device(&mut self) {
        let resume_path = self.active_track_path.clone();
        let resume_position = self.displayed_playback_position_seconds();
        let resume_index = self.active_track_index;
        let resume_radio_index = self.active_radio_index;
        let was_active = self
            .player
            .as_ref()
            .map(|player| player.is_playing() || player.is_paused())
            .unwrap_or(false);
        let was_paused = self
            .player
            .as_ref()
            .map(AudioPlayer::is_paused)
            .unwrap_or(false);
        let was_radio_recording = self
            .player
            .as_ref()
            .map(AudioPlayer::is_radio_recording)
            .unwrap_or(false);

        match AudioPlayer::new() {
            Ok(mut player) => {
                player.set_volume_percent(self.effective_volume_percent());
                let output_name = player.output_device_name().to_owned();
                if let Some(previous_player) = &mut self.player {
                    previous_player.stop();
                }
                self.player = Some(player);
                self.detected_output_change = None;
                self.last_known_output_name = output_name.clone();
                self.status_message = format!("Output refreshed: {output_name}.");
                self.error_message = None;

                if was_active {
                    if let Some(radio_index) = resume_radio_index {
                        self.play_radio_station(radio_index);
                        if was_radio_recording
                            && self
                                .player
                                .as_ref()
                                .map(AudioPlayer::is_playing)
                                .unwrap_or(false)
                        {
                            self.toggle_radio_recording();
                        }
                    } else if let Some(path) = resume_path {
                        self.play_path(path, resume_index, resume_position);
                    }

                    if was_paused
                        && self
                            .player
                            .as_ref()
                            .map(AudioPlayer::is_playing)
                            .unwrap_or(false)
                    {
                        self.pause_or_resume();
                    }
                }
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Could not refresh output device.".to_owned();
            }
        }
    }
    pub(crate) fn effective_volume_percent(&self) -> u8 {
        if self.state.playback.muted {
            0
        } else {
            self.state.playback.volume_percent
        }
    }
    pub(crate) fn apply_effective_volume_to_player(&mut self) {
        let effective_volume = self.effective_volume_percent();
        if let Some(player) = &mut self.player {
            player.set_volume_percent(effective_volume);
        }
    }
    pub(crate) fn set_volume_percent(&mut self, volume_percent: u8) {
        let next_volume = volume_percent.clamp(0, 100);
        let next_muted = next_volume == 0;
        if self.state.playback.volume_percent == next_volume && self.state.playback.muted == next_muted {
            return;
        }

        self.state.playback.volume_percent = next_volume;
        self.state.playback.muted = next_muted;
        self.apply_effective_volume_to_player();
        self.status_message = if self.state.playback.muted {
            "Volume muted.".to_owned()
        } else {
            format!("Volume: {next_volume}%.")
        };
        self.save_state_silently();
    }
    pub(crate) fn toggle_mute(&mut self) {
        if self.state.playback.muted || self.state.playback.volume_percent == 0 {
            if self.state.playback.volume_percent == 0 {
                self.state.playback.volume_percent = 50;
            }
            self.state.playback.muted = false;
            self.status_message = format!("Volume: {}%.", self.state.playback.volume_percent);
        } else {
            self.state.playback.muted = true;
            self.status_message = "Volume muted.".to_owned();
        }
        self.apply_effective_volume_to_player();
        self.save_state_silently();
    }
    pub(crate) fn adjust_volume(&mut self, delta_percent: i16) {
        let current = if self.state.playback.muted {
            0
        } else {
            self.state.playback.volume_percent as i16
        };
        let next = (current + delta_percent).clamp(0, 100) as u8;
        self.set_volume_percent(next);
    }
    pub(crate) fn handle_top_panel_volume_wheel(&mut self, response: &egui::Response, context: &egui::Context) {
        if !response.hovered() {
            return;
        }

        let scroll_y = context.input(|input| input.raw_scroll_delta.y + input.smooth_scroll_delta.y);
        if scroll_y.abs() < 0.5 {
            return;
        }

        let steps = (scroll_y / 80.0).round() as i16;
        let steps = if steps == 0 { scroll_y.signum() as i16 } else { steps };
        self.adjust_volume(steps * 2);
    }
    pub(crate) fn seek_relative(&mut self, delta_seconds: f32) {
        let Some(player) = self.player.as_ref() else {
            return;
        };

        if !(player.is_playing() || player.is_paused()) {
            return;
        }

        let current = self.displayed_playback_position_seconds();
        let duration = self.displayed_playback_duration_seconds().max(current.max(0.0));
        let next = (current + delta_seconds).clamp(0.0, duration.max(0.0));
        self.seek_current(next);
    }
    pub(crate) fn update_playback_status(&mut self) {
        if self.maybe_start_crossfade_to_next_track() {
            return;
        }

        let finished = self
            .player
            .as_ref()
            .map(AudioPlayer::has_finished)
            .unwrap_or(false);

        if finished && self.active_track_index.is_some() {
            if self.state.playback.auto_advance || self.state.playback.repeat_mode != RepeatMode::Off {
                self.play_next_track_with_crossfade(0.0);
            } else {
                self.active_track_index = None;
                self.active_playlist_index = None;
                self.active_track_path = None;
                self.crossfade_started_for_path = None;
            }
        }
    }
    pub(crate) fn maybe_start_crossfade_to_next_track(&mut self) -> bool {
        if !(self.state.playback.auto_advance || self.state.playback.repeat_mode != RepeatMode::Off)
            || !self.state.playback.crossfade_enabled
        {
            return false;
        }

        let Some(player) = self.player.as_ref() else {
            return false;
        };
        if !player.is_playing() {
            return false;
        }

        let Some(active_path) = self.active_track_path.clone() else {
            return false;
        };
        if self
            .crossfade_started_for_path
            .as_ref()
            .map(|path| same_path(path, &active_path))
            .unwrap_or(false)
        {
            return false;
        }

        let Some(duration) = player.playback_duration_seconds() else {
            return false;
        };
        let position = player.playback_position_seconds();
        if duration <= 0.0 || position <= 0.25 {
            return false;
        }

        let requested_fade = self.state.playback.crossfade_seconds.max(1) as f32;
        let effective_fade = requested_fade.min((duration * 0.45).max(0.25));
        let remaining = duration - position;

        if remaining <= effective_fade {
            self.crossfade_started_for_path = Some(active_path);
            self.play_next_track_with_crossfade(effective_fade);
            return true;
        }

        false
    }
    pub(crate) fn poll_output_device_change(&mut self) {
        if self.last_output_check.elapsed() < Duration::from_secs(2) {
            return;
        }

        self.last_output_check = Instant::now();
        let current_output = current_default_output_device_name();

        match output_device_change_action(
            &self.last_known_output_name,
            self.detected_output_change.as_deref(),
            &current_output,
            self.state.playback.auto_switch_output_device,
        ) {
            OutputDeviceChangeAction::None => {}
            OutputDeviceChangeAction::ClearPending => {
                if let Some(detected_output) = self.detected_output_change.take() {
                    let pending_message = output_device_change_message(&detected_output);
                    if self.status_message == pending_message {
                        self.status_message.clear();
                    }
                }
            }
            OutputDeviceChangeAction::Notify => {
                self.detected_output_change = Some(current_output.clone());
                self.status_message = output_device_change_message(&current_output);
            }
            OutputDeviceChangeAction::Refresh => {
                self.detected_output_change = Some(current_output);
                self.refresh_output_device();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_unchanged_output_without_pending_change() {
        assert_eq!(
            output_device_change_action("Speakers", None, "Speakers", false),
            OutputDeviceChangeAction::None
        );
    }

    #[test]
    fn clears_pending_change_when_output_returns_to_active_device() {
        assert_eq!(
            output_device_change_action(
                "Speakers",
                Some("Headphones"),
                "Speakers",
                false,
            ),
            OutputDeviceChangeAction::ClearPending
        );
    }

    #[test]
    fn does_not_repeat_action_for_same_detected_output() {
        assert_eq!(
            output_device_change_action(
                "Speakers",
                Some("Headphones"),
                "Headphones",
                true,
            ),
            OutputDeviceChangeAction::None
        );
    }

    #[test]
    fn notifies_when_auto_switch_is_disabled() {
        assert_eq!(
            output_device_change_action("Speakers", None, "Headphones", false),
            OutputDeviceChangeAction::Notify
        );
    }

    #[test]
    fn refreshes_when_auto_switch_is_enabled() {
        assert_eq!(
            output_device_change_action("Speakers", None, "Headphones", true),
            OutputDeviceChangeAction::Refresh
        );
    }
}
