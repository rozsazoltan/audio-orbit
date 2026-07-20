use crate::*;
use std::sync::atomic::Ordering;

#[derive(Default)]
struct DjFavoriteRangeWaveformAction {
    seek_seconds: Option<f32>,
    start_changed: bool,
    end_changed: bool,
}

fn snap_favorite_range_second(seconds: f32, duration_seconds: f32) -> f32 {
    seconds.round().clamp(0.0, duration_seconds)
}

fn draw_dj_favorite_range_waveform(
    ui: &mut egui::Ui,
    id: egui::Id,
    waveform: &[f32],
    duration_seconds: f32,
    range_start_seconds: &mut f32,
    range_end_seconds: &mut f32,
    playhead_seconds: Option<f32>,
    enabled: bool,
) -> DjFavoriteRangeWaveformAction {
    let duration_seconds = duration_seconds.max(0.25).round().max(1.0);
    let minimum_range_seconds = 1.0_f32.min(duration_seconds);
    *range_start_seconds = snap_favorite_range_second(*range_start_seconds, duration_seconds)
        .clamp(0.0, (duration_seconds - minimum_range_seconds).max(0.0));
    *range_end_seconds = snap_favorite_range_second(*range_end_seconds, duration_seconds).clamp(
        (*range_start_seconds + minimum_range_seconds).min(duration_seconds),
        duration_seconds,
    );

    let desired_size = egui::vec2(ui.available_width().max(180.0).floor(), 44.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    let seconds_from_x = |x: f32| {
        snap_favorite_range_second(
            ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) * duration_seconds,
            duration_seconds,
        )
    };
    let x_from_seconds =
        |seconds: f32| rect.left() + rect.width() * (seconds / duration_seconds).clamp(0.0, 1.0);

    let initial_start_x = x_from_seconds(*range_start_seconds);
    let initial_end_x = x_from_seconds(*range_end_seconds);
    let handle_width = 14.0;
    let start_hit_rect = egui::Rect::from_min_max(
        egui::pos2(initial_start_x - handle_width * 0.5, rect.top()),
        egui::pos2(initial_start_x + handle_width * 0.5, rect.bottom()),
    );
    let end_hit_rect = egui::Rect::from_min_max(
        egui::pos2(initial_end_x - handle_width * 0.5, rect.top()),
        egui::pos2(initial_end_x + handle_width * 0.5, rect.bottom()),
    );
    let handle_sense = if enabled {
        egui::Sense::drag()
    } else {
        egui::Sense::hover()
    };
    let start_response = ui.interact(start_hit_rect, id.with("start"), handle_sense);
    let end_response = ui.interact(end_hit_rect, id.with("end"), handle_sense);
    let mut action = DjFavoriteRangeWaveformAction::default();

    if enabled && start_response.dragged() {
        if let Some(pointer) = start_response.interact_pointer_pos() {
            *range_start_seconds = seconds_from_x(pointer.x)
                .clamp(0.0, (*range_end_seconds - minimum_range_seconds).max(0.0));
        }
    }
    if enabled && end_response.dragged() {
        if let Some(pointer) = end_response.interact_pointer_pos() {
            *range_end_seconds = seconds_from_x(pointer.x).clamp(
                (*range_start_seconds + minimum_range_seconds).min(duration_seconds),
                duration_seconds,
            );
        }
    }
    action.start_changed = enabled && start_response.drag_stopped();
    action.end_changed = enabled && end_response.drag_stopped();

    let handles_active = start_response.hovered()
        || end_response.hovered()
        || start_response.dragged()
        || end_response.dragged();
    if enabled && response.secondary_clicked() && !handles_active {
        if let Some(pointer) = response.interact_pointer_pos() {
            let new_start = seconds_from_x(pointer.x)
                .clamp(0.0, (duration_seconds - minimum_range_seconds).max(0.0));
            *range_start_seconds = new_start;
            if *range_end_seconds - *range_start_seconds < minimum_range_seconds {
                *range_end_seconds =
                    (*range_start_seconds + minimum_range_seconds).min(duration_seconds);
                action.end_changed = true;
            }
            action.start_changed = true;
        }
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, egui::Color32::from_black_alpha(220));
    if waveform.is_empty() {
        painter.line_segment(
            [
                egui::pos2(rect.left() + 6.0, rect.center().y),
                egui::pos2(rect.right() - 6.0, rect.center().y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(74, 82, 96)),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Play preview to load duration + waveform",
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(132, 140, 154),
        );
    } else {
        let column_count = (rect.width() / 3.0).floor().max(1.0) as usize + 1;
        let values = (0..column_count)
            .map(|column| sample_waveform_column(waveform, column, column_count))
            .collect::<Vec<_>>();
        let peak = values.iter().copied().fold(0.0_f32, f32::max).max(0.08);
        let center_y = rect.center().y.round();
        for (column, value) in values.iter().enumerate() {
            let x = rect.left() + column as f32 * 3.0;
            if x > rect.right() {
                break;
            }
            let normalized = (*value / peak).clamp(0.025, 1.0).powf(1.05);
            let height = (rect.height() * 0.78 * normalized)
                .max(2.0)
                .min(rect.height() - 6.0);
            painter.line_segment(
                [
                    egui::pos2(x, center_y - height * 0.5),
                    egui::pos2(x, center_y + height * 0.5),
                ],
                egui::Stroke::new(1.5, egui::Color32::from_rgb(94, 103, 118)),
            );
        }
    }

    let start_x = x_from_seconds(*range_start_seconds);
    let end_x = x_from_seconds(*range_end_seconds);
    let selected_rect = egui::Rect::from_min_max(
        egui::pos2(start_x, rect.top()),
        egui::pos2(end_x, rect.bottom()),
    );
    painter.rect_filled(
        selected_rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(64, 126, 236, 48),
    );
    if start_x > rect.left() {
        painter.rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(start_x, rect.bottom())),
            0.0,
            egui::Color32::from_black_alpha(105),
        );
    }
    if end_x < rect.right() {
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(end_x, rect.top()), rect.max),
            0.0,
            egui::Color32::from_black_alpha(105),
        );
    }

    let marker_color = egui::Color32::from_rgb(100, 166, 255);
    for x in [start_x, end_x] {
        painter.line_segment(
            [
                egui::pos2(x, rect.top() + 2.0),
                egui::pos2(x, rect.bottom() - 2.0),
            ],
            egui::Stroke::new(2.0, marker_color),
        );
        painter.circle_filled(egui::pos2(x, rect.top() + 5.0), 4.0, marker_color);
        painter.circle_filled(egui::pos2(x, rect.bottom() - 5.0), 4.0, marker_color);
    }

    if let Some(playhead_seconds) = playhead_seconds {
        let playhead_x = x_from_seconds(playhead_seconds.clamp(0.0, duration_seconds));
        painter.line_segment(
            [
                egui::pos2(playhead_x, rect.top() + 1.0),
                egui::pos2(playhead_x, rect.bottom() - 1.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::WHITE),
        );
    }

    let response = response.on_hover_text(
        "Left click: play/seek · Right click: set range start · Drag blue handles: resize range",
    );
    start_response.on_hover_text("Drag favorite range start");
    end_response.on_hover_text("Drag favorite range end");

    if enabled && response.clicked_by(egui::PointerButton::Primary) && !handles_active {
        action.seek_seconds = response
            .interact_pointer_pos()
            .map(|pointer| seconds_from_x(pointer.x));
    }

    action
}

impl AudioOrbitApp {
    pub(crate) fn dj_mix_is_running(&self) -> bool {
        self.dj_mix_event_receiver.is_some()
    }

    pub(crate) fn stop_dj_preview_playback(&mut self) {
        let preview_path = self
            .dj_mix_modal
            .as_ref()
            .and_then(|modal| modal.preview_track_path.clone());
        if preview_path.is_some() {
            self.stop();
        }
        if let Some(modal) = self.dj_mix_modal.as_mut() {
            modal.preview_track_path = None;
            modal.preview_stop_seconds = None;
        }
    }

    fn dj_preview_waveform_for_path(&self, path: &Path) -> (Vec<f32>, Option<f32>) {
        if let Some(playback) = self
            .last_playback
            .as_ref()
            .filter(|playback| same_path(&playback.path, path))
        {
            return (
                playback.waveform.clone(),
                Some(playback.original_duration_seconds),
            );
        }

        self.state
            .playlists
            .iter()
            .flat_map(|playlist| playlist.tracks.iter())
            .find(|track| same_path(&track.path, path))
            .map(|track| {
                (
                    track.waveform.clone(),
                    track
                        .metadata
                        .duration_seconds
                        .filter(|seconds| seconds.is_finite() && *seconds > 0.0),
                )
            })
            .unwrap_or_default()
    }

    pub(crate) fn open_dj_mix_builder_for_current_playlist(&mut self) {
        if self
            .current_playlist()
            .map(|playlist| playlist.kind == PlaylistKind::Temporary)
            .unwrap_or(false)
        {
            self.error_message = Some("Temporary playback is read-only.".to_owned());
            return;
        }

        let tracks = self
            .current_playlist()
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .filter(|track| !track.missing && track.path.is_file())
                    .map(|track| DjMixTrack::new(track.path.clone(), track.title.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.open_dj_mix_builder(tracks);
    }

    pub(crate) fn open_dj_mix_builder_for_selection(&mut self, context_index: usize) {
        if self
            .current_playlist()
            .map(|playlist| playlist.kind == PlaylistKind::Temporary)
            .unwrap_or(false)
        {
            self.error_message = Some("Temporary playback is read-only.".to_owned());
            return;
        }

        let selected_paths = self
            .action_track_paths_for_context(context_index)
            .into_iter()
            .map(|path| path_key(&path))
            .collect::<BTreeSet<_>>();
        let tracks = self
            .current_playlist()
            .map(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .filter(|track| {
                        selected_paths.contains(&path_key(&track.path))
                            && !track.missing
                            && track.path.is_file()
                    })
                    .map(|track| DjMixTrack::new(track.path.clone(), track.title.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.open_dj_mix_builder(tracks);
    }

    fn open_dj_mix_builder(&mut self, tracks: Vec<DjMixTrack>) {
        if self.dj_mix_is_running() {
            self.error_message = Some("DJ mix export already running.".to_owned());
            return;
        }
        if tracks.len() < 2 {
            self.error_message = Some("DJ mix requires at least two available tracks.".to_owned());
            return;
        }
        self.active_panel_modal = None;
        self.panel_modal_history.clear();
        self.dj_mix_modal = Some(DjMixModalState {
            tracks,
            options: DjMixOptions::default(),
            custom_bridge_path: None,
            stage: "Ready".to_owned(),
            progress: 0.0,
            output_path: None,
            report_path: None,
            diagnostics_summary: None,
            professional_tool_status: dj_mix::professional_tool_status_summary(),
            completed: false,
            started_at: None,
            last_progress_at: None,
            preview_track_path: None,
            preview_stop_seconds: None,
        });
    }

    pub(crate) fn process_dj_mix_events(&mut self) {
        let (events, disconnected) = match &self.dj_mix_event_receiver {
            Some(receiver) => {
                let mut events = Vec::new();
                let mut disconnected = false;
                loop {
                    match receiver.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                (events, disconnected)
            }
            None => return,
        };

        for event in events {
            match event {
                DjMixEvent::Progress { stage, progress } => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = stage;
                        modal.progress = progress;
                        modal.last_progress_at = Some(Instant::now());
                    }
                }
                DjMixEvent::Completed {
                    output_path,
                    report_path,
                    track_count,
                    duration_seconds,
                    integrated_lufs,
                    true_peak_dbfs,
                    diagnostics_summary,
                } => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = format!(
                            "Complete — {track_count} tracks, {:.1} min",
                            duration_seconds / 60.0
                        );
                        modal.progress = 1.0;
                        modal.output_path = Some(output_path.clone());
                        modal.report_path = Some(report_path);
                        modal.diagnostics_summary = Some(diagnostics_summary);
                        modal.completed = true;
                        modal.last_progress_at = Some(Instant::now());
                    }
                    self.status_message = format!(
                        "DJ mix saved: {}{}{}",
                        output_path.display(),
                        integrated_lufs
                            .map(|value| format!(" — {value:.1} LUFS"))
                            .unwrap_or_default(),
                        true_peak_dbfs
                            .map(|value| format!(", {value:.1} dBTP"))
                            .unwrap_or_default(),
                    );
                    self.dj_mix_event_receiver = None;
                    self.dj_mix_cancel_flag = None;
                }
                DjMixEvent::Cancelled => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = "Cancelled".to_owned();
                        modal.progress = 0.0;
                    }
                    self.status_message = "DJ mix export cancelled.".to_owned();
                    self.dj_mix_event_receiver = None;
                    self.dj_mix_cancel_flag = None;
                }
                DjMixEvent::Failed(error) => {
                    if let Some(modal) = self.dj_mix_modal.as_mut() {
                        modal.stage = "Failed".to_owned();
                        modal.progress = 0.0;
                    }
                    self.error_message = Some(error);
                    self.dj_mix_event_receiver = None;
                    self.dj_mix_cancel_flag = None;
                }
            }
        }

        if disconnected && self.dj_mix_event_receiver.is_some() {
            self.dj_mix_event_receiver = None;
            self.dj_mix_cancel_flag = None;
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.stage = "Failed".to_owned();
                modal.progress = 0.0;
            }
            self.error_message = Some("DJ mix worker stopped before completing export.".to_owned());
        }
    }

    pub(crate) fn cancel_dj_mix_export(&mut self) {
        if let Some(cancel) = &self.dj_mix_cancel_flag {
            cancel.store(true, Ordering::Relaxed);
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.stage = "Cancelling...".to_owned();
            }
        }
    }

    fn start_dj_mix_export(&mut self) {
        if self.dj_mix_is_running() {
            return;
        }
        let Some(modal) = self.dj_mix_modal.as_ref() else {
            return;
        };
        if modal.tracks.len() < 2 {
            self.error_message = Some("DJ mix requires at least two tracks.".to_owned());
            return;
        }

        let default_name = self.default_dj_mix_file_name();
        let Some(output_path) = FileDialog::new()
            .add_filter("MP3 audio", &["mp3"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        if modal.options.bridge_mode == DjBridgeMode::Custom
            && modal
                .custom_bridge_path
                .as_ref()
                .map(|path| !path.is_file())
                .unwrap_or(true)
        {
            self.error_message = Some("Choose an available custom bridge audio file.".to_owned());
            return;
        }

        let request = dj_mix::ExportRequest {
            tracks: modal.tracks.clone(),
            options: modal.options,
            custom_bridge_path: modal.custom_bridge_path.clone(),
            output_path,
        };
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.dj_mix_event_receiver = Some(receiver);
        self.dj_mix_cancel_flag = Some(cancel);
        if let Some(modal) = self.dj_mix_modal.as_mut() {
            modal.stage = "Starting analysis...".to_owned();
            modal.progress = 0.0;
            modal.output_path = None;
            modal.report_path = None;
            modal.diagnostics_summary = None;
            modal.completed = false;
            modal.started_at = Some(Instant::now());
            modal.last_progress_at = Some(Instant::now());
        }
        let spawn_result = thread::Builder::new()
            .name("audio-orbit-dj-export".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dj_mix::export_mix(request, sender.clone(), worker_cancel)
                }));
                if result.is_err() {
                    let _ = sender.send(DjMixEvent::Failed(
                        "DJ mix worker crashed unexpectedly.".to_owned(),
                    ));
                }
            });
        if let Err(error) = spawn_result {
            self.dj_mix_event_receiver = None;
            self.dj_mix_cancel_flag = None;
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.stage = "Failed".to_owned();
                modal.started_at = None;
            }
            self.error_message = Some(format!("Failed to start DJ mix worker: {error}"));
        }
    }

    fn default_dj_mix_file_name(&self) -> String {
        let playlist_name = self
            .current_playlist()
            .map(|playlist| playlist.name.as_str())
            .unwrap_or("playlist");
        let safe_name = playlist_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim()
            .to_owned();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        format!(
            "{}-dj-mix-{timestamp}.mp3",
            if safe_name.is_empty() {
                "playlist"
            } else {
                &safe_name
            }
        )
    }

    pub(crate) fn render_dj_mix_modal(&mut self, context: &egui::Context) {
        let Some(mut modal) = self.dj_mix_modal.clone() else {
            return;
        };
        let running = self.dj_mix_is_running();
        self.render_modal_backdrop(context, "dj_mix_modal_backdrop");
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let body_scroll_height = (content_size.y - 96.0).max(120.0);
        let track_scroll_height = (content_size.y * 0.38).clamp(120.0, 360.0);
        let mut close_requested = false;
        let mut start_requested = false;
        let mut cancel_requested = false;
        let mut reveal_path = None;
        let mut play_path = None;
        let mut choose_bridge_requested = false;
        let mut clear_bridge_requested = false;
        let mut preview_play_request: Option<(PathBuf, f32, f32)> = None;
        let mut preview_seek_request: Option<(f32, f32)> = None;
        let mut preview_stop_update_request: Option<(PathBuf, f32)> = None;
        let mut preview_pause_resume_requested = false;
        let mut stop_preview_requested = false;

        egui::Area::new(egui::Id::new("dj_mix_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);
                    if Self::render_modal_header(
                        ui,
                        outer_padding.x,
                        Icon::Music,
                        "DJ Mix Builder",
                        "Build an offline DJ set with phrase-aware planning, stem-aware handoffs, professional analysis/time-stretch tools when installed, and deterministic fallbacks.",
                    ) {
                        if running {
                            cancel_requested = true;
                        } else {
                            close_requested = true;
                        }
                    }

                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(outer_padding.x as i8, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .id_salt("dj_mix_modal_body")
                                .max_height(body_scroll_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    Self::render_modal_section(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Mix engine:");
                                            ui.add_enabled_ui(!running, |ui| {
                                                ui.selectable_value(
                                                    &mut modal.options.style,
                                                    DjMixStyle::Crossfade,
                                                    "Crossfade",
                                                );
                                                ui.selectable_value(
                                                    &mut modal.options.style,
                                                    DjMixStyle::SmartDj,
                                                    "Smart DJ",
                                                );
                                            });
                                        });
                                        ui.small(match modal.options.style {
                                            DjMixStyle::Crossfade => "Simple equal-power overlap. No tempo change, loop roll, echo, or filter performance.",
                                            DjMixStyle::SmartDj => "Human-style transition planner. Uses Essentia for beat/key analysis, Rubber Band R3 for studio-quality tempo matching, and Demucs stems when those tools are installed; otherwise it falls back safely.",
                                        });
                                        ui.add_space(6.0);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.add_enabled_ui(!running, |ui| {
                                                ui.checkbox(&mut modal.options.smart_order, "Smart BPM order");
                                                ui.checkbox(&mut modal.options.normalize_loudness, "Loudness leveling");
                                                ui.checkbox(&mut modal.options.bass_swap, "Bass swap");
                                                ui.checkbox(&mut modal.options.professional_tools, "Professional tools");
                                                ui.checkbox(&mut modal.options.stem_separation, "Stem-aware mixing");
                                            });
                                        });
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Transition:");
                                            for bars in [8, 16, 32] {
                                                ui.add_enabled_ui(!running, |ui| {
                                                    ui.selectable_value(
                                                        &mut modal.options.transition_bars,
                                                        bars,
                                                        format!("{bars} bars"),
                                                    );
                                                });
                                            }
                                            ui.separator();
                                            ui.label("MP3:");
                                            for bitrate in [192, 256, 320] {
                                                ui.add_enabled_ui(!running, |ui| {
                                                    ui.selectable_value(
                                                        &mut modal.options.bitrate_kbps,
                                                        bitrate,
                                                        format!("{bitrate} kbps"),
                                                    );
                                                });
                                            }
                                        });
                                        if modal.options.style == DjMixStyle::SmartDj {
                                            ui.add_space(6.0);
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("Bridge:");
                                                ui.add_enabled_ui(!running, |ui| {
                                                    ui.selectable_value(&mut modal.options.bridge_mode, DjBridgeMode::Auto, "Auto human");
                                                    ui.selectable_value(&mut modal.options.bridge_mode, DjBridgeMode::DrumSwap, "Drum swap");
                                                    ui.selectable_value(&mut modal.options.bridge_mode, DjBridgeMode::HarmonicBridge, "Harmonic bridge");
                                                    ui.selectable_value(&mut modal.options.bridge_mode, DjBridgeMode::EchoDrop, "Echo drop");
                                                    ui.selectable_value(&mut modal.options.bridge_mode, DjBridgeMode::StemMashup, "Stem mashup");
                                                    ui.selectable_value(&mut modal.options.bridge_mode, DjBridgeMode::Custom, "Custom audio");
                                                });
                                            });
                                            ui.small("Auto chooses a phrase-level recipe. With Demucs, drums, bass, vocals, and accompaniment are handed over separately instead of fading two complete songs together.");
                                            ui.horizontal_wrapped(|ui| {
                                                ui.small(&modal.professional_tool_status);
                                                if ui.add_enabled(!running, egui::Button::new("Refresh tools")).clicked() {
                                                    modal.professional_tool_status = dj_mix::professional_tool_status_summary();
                                                }
                                            });
                                            if modal.options.bridge_mode == DjBridgeMode::Custom {
                                                ui.horizontal_wrapped(|ui| {
                                                    let label = modal.custom_bridge_path.as_ref()
                                                        .and_then(|path| path.file_name())
                                                        .map(|name| name.to_string_lossy().into_owned())
                                                        .unwrap_or_else(|| "No custom bridge selected".to_owned());
                                                    ui.label(ellipsize_chars(&label, 54));
                                                    if ui.add_enabled(!running, egui::Button::new("Choose MP3/audio...")).clicked() {
                                                        choose_bridge_requested = true;
                                                    }
                                                    if modal.custom_bridge_path.is_some()
                                                        && ui.add_enabled(!running, egui::Button::new("Clear")).clicked()
                                                    {
                                                        clear_bridge_requested = true;
                                                    }
                                                });
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.label("Loop from:");
                                                    ui.add_enabled(!running, egui::DragValue::new(&mut modal.options.bridge_start_seconds).range(0.0..=86_400.0).speed(0.5).suffix(" s"));
                                                    ui.label("Loop length:");
                                                    ui.add_enabled(!running, egui::DragValue::new(&mut modal.options.bridge_loop_seconds).range(0.25..=60.0).speed(0.25).suffix(" s"));
                                                });
                                            }
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label("Bridge level:");
                                                ui.add_enabled(!running, egui::Slider::new(&mut modal.options.bridge_level, 0.15..=1.0).show_value(true));
                                            });
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label("Target length:");
                                            ui.add_enabled(
                                                !running,
                                                egui::DragValue::new(&mut modal.options.target_minutes)
                                                    .range(1.0..=180.0)
                                                    .speed(1.0)
                                                    .suffix(" min"),
                                            );
                                            ui.small("Auto highlight sections are shortened to approach target.");
                                        });
                                        ui.small("Export runs on a background worker. External tools are optional and never bundled; missing tools trigger deterministic built-in fallbacks.");
                                    });

                                    ui.add_space(8.0);
                                    Self::render_modal_section(ui, |ui| {
                                        let show_progress = running
                                            || modal.completed
                                            || modal.stage != "Ready";
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(ui.available_width(), 48.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                if show_progress {
                                                    ui.horizontal(|ui| {
                                                        if running {
                                                            ui.spinner();
                                                        }
                                                        ui.label(&modal.stage);
                                                        if let Some(started_at) = modal.started_at {
                                                            let elapsed = started_at.elapsed().as_secs();
                                                            ui.small(format!("{}:{:02}", elapsed / 60, elapsed % 60));
                                                        }
                                                    });
                                                    ui.add(
                                                        egui::ProgressBar::new(modal.progress)
                                                            .animate(running)
                                                            .show_percentage(),
                                                    );
                                                    if running {
                                                        ui.small("Background worker active — playback and library remain usable.");
                                                    }
                                                }
                                            },
                                        );
                                        ui.small("Choose target length and section mode per track, then export MP3. The .dj-plan.json report records tool backends and transition recipes.");
                                        if let Some(summary) = &modal.diagnostics_summary {
                                            ui.small(summary);
                                        }
                                        if modal.tracks.len() < 2 {
                                            ui.small("Choose at least two available tracks.");
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            if running {
                                                if ui.button(ui_icons::label(Icon::X, "Cancel export")).clicked() {
                                                    cancel_requested = true;
                                                }
                                            } else if modal.completed {
                                                if let Some(path) = modal.output_path.clone() {
                                                    if ui.button(ui_icons::label(Icon::Play, "Play")).clicked() {
                                                        play_path = Some(path.clone());
                                                    }
                                                    if ui.button(ui_icons::label(Icon::FolderOpen, "Show MP3")).clicked() {
                                                        reveal_path = Some(path);
                                                    }
                                                }
                                                if let Some(path) = modal.report_path.clone() {
                                                    if ui.button("Show diagnostics").clicked() {
                                                        reveal_path = Some(path);
                                                    }
                                                }
                                                if ui
                                                    .add_enabled(
                                                        modal.tracks.len() >= 2,
                                                        egui::Button::new(ui_icons::label(
                                                            Icon::Music,
                                                            "Export another...",
                                                        )),
                                                    )
                                                    .clicked()
                                                {
                                                    start_requested = true;
                                                }
                                                if ui.button("Close").clicked() {
                                                    close_requested = true;
                                                }
                                            } else {
                                                if ui
                                                    .add_enabled(
                                                        modal.tracks.len() >= 2,
                                                        egui::Button::new(ui_icons::label(Icon::Music, "Export DJ mix...")),
                                                    )
                                                    .clicked()
                                                {
                                                    start_requested = true;
                                                }
                                                if ui.button("Close").clicked() {
                                                    close_requested = true;
                                                }
                                            }
                                        });
                                    });

                                    ui.add_space(8.0);
                                    Self::render_modal_section(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.heading(format!("Tracks ({})", modal.tracks.len()));
                                            if modal.options.smart_order {
                                                ui.small("Export order optimized after BPM analysis");
                                            } else {
                                                ui.small("Manual order");
                                            }
                                        });
                                        egui::ScrollArea::vertical()
                                            .id_salt("dj_mix_tracks")
                                            .max_height(track_scroll_height)
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                let mut move_action = None;
                                                let mut remove_index = None;
                                                let track_count = modal.tracks.len();
                                                for index in 0..track_count {
                                                    let title = modal.tracks[index].title.clone();
                                                    let path = modal.tracks[index].path.clone();
                                                    let previous_section_mode = modal.tracks[index].section_mode;
                                                    let mut section_mode = previous_section_mode;
                                                    let mut favorite_start = modal.tracks[index].favorite_start_seconds;
                                                    let mut favorite_end = modal.tracks[index].favorite_end_seconds;
                                                    let (waveform, known_duration) =
                                                        self.dj_preview_waveform_for_path(&path);
                                                    let duration_seconds = known_duration
                                                        .unwrap_or_else(|| favorite_end.max(60.0))
                                                        .max(0.25)
                                                        .round()
                                                        .max(1.0);
                                                    let preview_is_active = self
                                                        .active_track_path
                                                        .as_ref()
                                                        .map(|active| same_path(active, &path))
                                                        .unwrap_or(false);
                                                    let preview_is_playing = preview_is_active
                                                        && self
                                                            .player
                                                            .as_ref()
                                                            .map(AudioPlayer::is_playing)
                                                            .unwrap_or(false);
                                                    let preview_is_paused = preview_is_active
                                                        && self
                                                            .player
                                                            .as_ref()
                                                            .map(AudioPlayer::is_paused)
                                                            .unwrap_or(false);
                                                    let playhead_seconds = preview_is_active
                                                        .then(|| self.displayed_playback_position_seconds());
                                                    ui.horizontal(|ui| {
                                                        ui.label(format!("{}.", index + 1));
                                                        let track_content_width =
                                                            (ui.available_width() - 104.0).max(220.0);
                                                        ui.vertical(|ui| {
                                                            ui.set_max_width(track_content_width);
                                                            ui.label(ellipsize_chars(&title, 72));
                                                            ui.small(ellipsize_chars(&path.display().to_string(), 96));
                                                            ui.horizontal_wrapped(|ui| {
                                                                ui.add_enabled_ui(!running, |ui| {
                                                                    ui.selectable_value(
                                                                        &mut section_mode,
                                                                        DjTrackSectionMode::AutoHighlight,
                                                                        "Auto highlight",
                                                                    );
                                                                    ui.selectable_value(
                                                                        &mut section_mode,
                                                                        DjTrackSectionMode::FullTrack,
                                                                        "Full track",
                                                                    );
                                                                    ui.selectable_value(
                                                                        &mut section_mode,
                                                                        DjTrackSectionMode::FavoriteRange,
                                                                        "Favorite range",
                                                                    );
                                                                });
                                                            });
                                                            if section_mode == DjTrackSectionMode::FavoriteRange {
                                                                let mut favorite_start_changed = false;
                                                                let mut favorite_end_changed = false;
                                                                let waveform_id = egui::Id::new((
                                                                    "dj_favorite_range_waveform",
                                                                    index,
                                                                    path_key(&path),
                                                                ));
                                                                let waveform_action = draw_dj_favorite_range_waveform(
                                                                    ui,
                                                                    waveform_id,
                                                                    &waveform,
                                                                    duration_seconds,
                                                                    &mut favorite_start,
                                                                    &mut favorite_end,
                                                                    playhead_seconds,
                                                                    !running && known_duration.is_some(),
                                                                );
                                                                favorite_start_changed |= waveform_action.start_changed;
                                                                favorite_end_changed |= waveform_action.end_changed;
                                                                if let Some(seek_seconds) = waveform_action.seek_seconds {
                                                                    let stop_seconds = if seek_seconds < favorite_end {
                                                                        favorite_end
                                                                    } else {
                                                                        duration_seconds
                                                                    };
                                                                    if preview_is_active {
                                                                        preview_seek_request = Some((seek_seconds, stop_seconds));
                                                                    } else {
                                                                        preview_play_request = Some((
                                                                            path.clone(),
                                                                            seek_seconds,
                                                                            stop_seconds,
                                                                        ));
                                                                    }
                                                                }
                                                                ui.horizontal_wrapped(|ui| {
                                                                    let preview_label = if preview_is_playing {
                                                                        ui_icons::label(Icon::Pause, "Pause preview")
                                                                    } else if preview_is_paused {
                                                                        ui_icons::label(Icon::Play, "Resume preview")
                                                                    } else {
                                                                        ui_icons::label(Icon::Play, "Play range")
                                                                    };
                                                                    if ui
                                                                        .add_enabled(!running, egui::Button::new(preview_label))
                                                                        .clicked()
                                                                    {
                                                                        if preview_is_playing || preview_is_paused {
                                                                            preview_pause_resume_requested = true;
                                                                        } else {
                                                                            preview_play_request = Some((
                                                                                path.clone(),
                                                                                favorite_start,
                                                                                favorite_end,
                                                                            ));
                                                                        }
                                                                    }
                                                                    if preview_is_active {
                                                                        if ui
                                                                            .add_enabled(
                                                                                !running,
                                                                                egui::Button::new("Start = playhead"),
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            let playhead = snap_favorite_range_second(
                                                                                self.displayed_playback_position_seconds(),
                                                                                duration_seconds,
                                                                            );
                                                                            favorite_start = playhead.min(
                                                                                (favorite_end - 1.0).max(0.0),
                                                                            );
                                                                            favorite_start_changed = true;
                                                                        }
                                                                        if ui
                                                                            .add_enabled(
                                                                                !running,
                                                                                egui::Button::new("End = playhead"),
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            let playhead = snap_favorite_range_second(
                                                                                self.displayed_playback_position_seconds(),
                                                                                duration_seconds,
                                                                            );
                                                                            favorite_end = playhead.max(
                                                                                (favorite_start + 1.0)
                                                                                    .min(duration_seconds),
                                                                            );
                                                                            favorite_end_changed = true;
                                                                        }
                                                                    }
                                                                    ui.small(format!(
                                                                        "{} – {} · {}",
                                                                        format_duration(favorite_start),
                                                                        format_duration(favorite_end),
                                                                        format_duration(
                                                                            (favorite_end - favorite_start).max(0.0),
                                                                        ),
                                                                    ));
                                                                });
                                                                ui.horizontal_wrapped(|ui| {
                                                                    ui.label("Start:");
                                                                    let start_response = ui.add_enabled(
                                                                        !running,
                                                                        egui::DragValue::new(&mut favorite_start)
                                                                            .range(0.0..=duration_seconds)
                                                                            .speed(1.0)
                                                                            .fixed_decimals(0)
                                                                            .suffix(" s"),
                                                                    );
                                                                    favorite_start_changed |= start_response.changed();
                                                                    ui.label("End:");
                                                                    let end_response = ui.add_enabled(
                                                                        !running,
                                                                        egui::DragValue::new(&mut favorite_end)
                                                                            .range(0.0..=duration_seconds)
                                                                            .speed(1.0)
                                                                            .fixed_decimals(0)
                                                                            .suffix(" s"),
                                                                    );
                                                                    favorite_end_changed |= end_response.changed();
                                                                });
                                                                favorite_start = snap_favorite_range_second(
                                                                    favorite_start,
                                                                    duration_seconds,
                                                                )
                                                                .clamp(0.0, (duration_seconds - 1.0).max(0.0));
                                                                favorite_end = snap_favorite_range_second(
                                                                    favorite_end,
                                                                    duration_seconds,
                                                                )
                                                                .clamp(
                                                                    (favorite_start + 1.0).min(duration_seconds),
                                                                    duration_seconds,
                                                                );
                                                                if favorite_start_changed {
                                                                    if preview_is_active {
                                                                        preview_seek_request = Some((
                                                                            favorite_start,
                                                                            favorite_end,
                                                                        ));
                                                                    } else {
                                                                        preview_play_request = Some((
                                                                            path.clone(),
                                                                            favorite_start,
                                                                            favorite_end,
                                                                        ));
                                                                    }
                                                                } else if favorite_end_changed && preview_is_active {
                                                                    preview_stop_update_request = Some((
                                                                        path.clone(),
                                                                        favorite_end,
                                                                    ));
                                                                }
                                                                if waveform.is_empty() {
                                                                    ui.small(
                                                                        "Play preview first. Background decoding loads the real duration and waveform without blocking the DJ window.",
                                                                    );
                                                                } else {
                                                                    ui.small(
                                                                        "Left click jumps playback. Right click moves the start and seeks there. Drag either blue edge. Values use whole seconds.",
                                                                    );
                                                                }
                                                                if preview_is_playing {
                                                                    ui.ctx().request_repaint_after(Duration::from_millis(50));
                                                                }
                                                            }
                                                        });
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui
                                                                .add_enabled(!running && track_count > 2, egui::Button::new(ui_icons::icon(Icon::X)))
                                                                .on_hover_text("Remove from mix")
                                                                .clicked()
                                                            {
                                                                remove_index = Some(index);
                                                            }
                                                            if ui
                                                                .add_enabled(!running && index + 1 < track_count, egui::Button::new(ui_icons::icon(Icon::ArrowDown)))
                                                                .on_hover_text("Move down")
                                                                .clicked()
                                                            {
                                                                move_action = Some((index, index + 1));
                                                            }
                                                            if ui
                                                                .add_enabled(!running && index > 0, egui::Button::new(ui_icons::icon(Icon::ArrowUp)))
                                                                .on_hover_text("Move up")
                                                                .clicked()
                                                            {
                                                                move_action = Some((index, index - 1));
                                                            }
                                                        });
                                                    });
                                                    if previous_section_mode
                                                        == DjTrackSectionMode::FavoriteRange
                                                        && section_mode
                                                            != DjTrackSectionMode::FavoriteRange
                                                        && modal
                                                            .preview_track_path
                                                            .as_ref()
                                                            .map(|preview| same_path(preview, &path))
                                                            .unwrap_or(false)
                                                    {
                                                        stop_preview_requested = true;
                                                    }
                                                    favorite_start = snap_favorite_range_second(
                                                        favorite_start,
                                                        duration_seconds,
                                                    )
                                                    .clamp(
                                                        0.0,
                                                        (duration_seconds - 1.0).max(0.0),
                                                    );
                                                    favorite_end = snap_favorite_range_second(
                                                        favorite_end,
                                                        duration_seconds,
                                                    )
                                                    .clamp(
                                                        (favorite_start + 1.0)
                                                            .min(duration_seconds),
                                                        duration_seconds,
                                                    );
                                                    modal.tracks[index].section_mode = section_mode;
                                                    modal.tracks[index].favorite_start_seconds = favorite_start.max(0.0);
                                                    modal.tracks[index].favorite_end_seconds = favorite_end.max(0.0);
                                                    if index + 1 < track_count {
                                                        ui.separator();
                                                    }
                                                }
                                                if let Some((from, to)) = move_action {
                                                    modal.tracks.swap(from, to);
                                                    modal.options.smart_order = false;
                                                }
                                                if let Some(index) = remove_index {
                                                    if modal
                                                        .tracks
                                                        .get(index)
                                                        .zip(modal.preview_track_path.as_ref())
                                                        .map(|(track, preview)| {
                                                            same_path(&track.path, preview)
                                                        })
                                                        .unwrap_or(false)
                                                    {
                                                        stop_preview_requested = true;
                                                    }
                                                    modal.tracks.remove(index);
                                                }
                                            });
                                    });
                                });
                        });
                });
            });
        self.render_modal_info_footer_fixed(context, "dj_mix_modal_info_footer", screen_rect);

        if clear_bridge_requested {
            modal.custom_bridge_path = None;
        }
        if choose_bridge_requested {
            if let Some(path) = FileDialog::new()
                .add_filter("Audio bridge", &["mp3", "wav", "flac", "ogg"])
                .pick_file()
            {
                modal.custom_bridge_path = Some(path);
            }
        }
        self.dj_mix_modal = Some(modal);
        if stop_preview_requested {
            self.stop_dj_preview_playback();
        }
        if let Some((path, stop_seconds)) = preview_stop_update_request {
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                if modal
                    .preview_track_path
                    .as_ref()
                    .map(|preview| same_path(preview, &path))
                    .unwrap_or(false)
                {
                    modal.preview_stop_seconds = Some(stop_seconds);
                }
            }
        }
        if preview_pause_resume_requested {
            self.pause_or_resume();
        } else if let Some((seconds, stop_seconds)) = preview_seek_request {
            let active_preview_path = self.active_track_path.clone();
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.preview_track_path = active_preview_path;
                modal.preview_stop_seconds = Some(stop_seconds);
            }
            self.seek_current(seconds);
        } else if let Some((path, seconds, stop_seconds)) = preview_play_request {
            let track_index = self.current_playlist().and_then(|playlist| {
                playlist
                    .tracks
                    .iter()
                    .position(|track| same_path(&track.path, &path))
            });
            if let Some(modal) = self.dj_mix_modal.as_mut() {
                modal.preview_track_path = Some(path.clone());
                modal.preview_stop_seconds = Some(stop_seconds);
            }
            self.play_path_with_crossfade(path, track_index, seconds, 0.0);
        }
        if cancel_requested {
            self.cancel_dj_mix_export();
        }
        if let Some(path) = play_path {
            self.stop_dj_preview_playback();
            self.dj_mix_modal = None;
            self.open_audio_files_in_temporary_playlist(vec![path], true);
            return;
        }
        if let Some(path) = reveal_path {
            if let Err(error) = reveal_in_file_manager(&path) {
                self.error_message = Some(error.to_string());
            }
        }
        if start_requested {
            self.start_dj_mix_export();
        }
        if close_requested && !self.dj_mix_is_running() {
            self.stop_dj_preview_playback();
            self.dj_mix_modal = None;
        }
    }
}
