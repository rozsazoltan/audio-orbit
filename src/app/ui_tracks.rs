use crate::*;

const TRACK_ROW_HEIGHT: f32 = 32.0;
const TRACK_SEPARATOR_HEIGHT: f32 = 1.0;
const TRACK_GROUP_TOP_GAP: f32 = 6.0;
const TRACK_GROUP_HEADER_HEIGHT: f32 = 24.0;
const TRACK_GROUP_BLOCK_HEIGHT: f32 = TRACK_GROUP_TOP_GAP + TRACK_GROUP_HEADER_HEIGHT + TRACK_SEPARATOR_HEIGHT;
const TRACK_LIST_OVERSCAN_ROWS: f32 = 4.0;
const PLAYLIST_SUBMENU_MAX_HEIGHT: f32 = 320.0;
const PLAYLIST_SUBMENU_GAP: f32 = 1.0;
const PLAYLIST_SUBMENU_HOVER_GRACE_SECONDS: f64 = 0.35;
const PLAYLIST_SUBMENU_STALE_SECONDS: f64 = 0.50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PlaylistSubmenuKind {
    Add,
    Search,
}

#[derive(Clone, Debug)]
enum PlaylistSubmenuAction {
    Add { playlist_index: usize },
    New,
    Search {
        playlist_index: usize,
        track_path: PathBuf,
    },
}

#[derive(Clone, Debug)]
struct PlaylistSubmenuEntry {
    action: PlaylistSubmenuAction,
    rect: egui::Rect,
}

#[derive(Clone, Debug)]
struct PlaylistSubmenuState {
    owner_path: PathBuf,
    action_paths: Vec<PathBuf>,
    kind: PlaylistSubmenuKind,
    popup_rect: egui::Rect,
    entries: Vec<PlaylistSubmenuEntry>,
    last_hovered_at: f64,
    last_rendered_at: f64,
}

fn playlist_submenu_state_id() -> egui::Id {
    egui::Id::new("audio_orbit_track_playlist_submenu")
}

fn load_playlist_submenu_state(context: &egui::Context) -> Option<PlaylistSubmenuState> {
    context.data_mut(|data| {
        data.get_temp::<Option<PlaylistSubmenuState>>(playlist_submenu_state_id())
            .flatten()
    })
}

fn store_playlist_submenu_state(
    context: &egui::Context,
    state: Option<PlaylistSubmenuState>,
) {
    context.data_mut(|data| data.insert_temp(playlist_submenu_state_id(), state));
}

fn menu_style(ui: &mut egui::Ui) {
    ui.style_mut().spacing.button_padding = egui::vec2(2.0, 0.0);
    ui.visuals_mut().widgets.active.bg_stroke = egui::Stroke::NONE;
    ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;
    ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
}

fn render_playlist_submenu(
    ui: &mut egui::Ui,
    owner_path: &Path,
    action_paths: &[PathBuf],
    kind: PlaylistSubmenuKind,
    trigger_label: String,
    entries: Vec<(String, PlaylistSubmenuAction)>,
    disabled_hover_text: Option<&str>,
) {
    let frame = egui::Frame::menu(ui.style());
    let frame_margin = frame.total_margin();
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let text_color = ui.visuals().widgets.inactive.fg_stroke.color;
    let button_padding = ui.spacing().button_padding;
    let longest_text_width = entries
        .iter()
        .map(|(label, _)| text_width(ui, label, font_id.clone(), text_color))
        .fold(0.0, f32::max);
    let content_width = (longest_text_width + button_padding.x * 2.0).ceil().max(1.0);
    let outer_width = content_width + frame_margin.left + frame_margin.right;
    let gap = ui.spacing().menu_spacing.max(PLAYLIST_SUBMENU_GAP);
    let screen_rect = ui.ctx().screen_rect();
    let trigger_hint = ui.available_rect_before_wrap();
    let available_right = screen_rect.right() - trigger_hint.right() - gap;
    let available_left = trigger_hint.left() - screen_rect.left() - gap;
    let open_left = available_right < outer_width && available_left > available_right;
    let arrow = if open_left { "◀" } else { "▶" };
    let button = egui::Button::new(trigger_label).shortcut_text(arrow);

    if entries.is_empty() {
        let response = ui.add_enabled(false, button);
        if let Some(hover_text) = disabled_hover_text {
            response.on_hover_text(hover_text);
        }
        if load_playlist_submenu_state(ui.ctx())
            .as_ref()
            .is_some_and(|state| state.owner_path == owner_path && state.kind == kind)
        {
            store_playlist_submenu_state(ui.ctx(), None);
        }
        return;
    }

    let response = ui.add(button);
    let now = ui.input(|input| input.time);
    let pointer_position = ui.input(|input| input.pointer.hover_pos());
    let mut state = load_playlist_submenu_state(ui.ctx());

    if response.hovered() {
        let preserve_popup = state
            .as_ref()
            .filter(|state| state.owner_path == owner_path && state.kind == kind)
            .map(|state| (state.popup_rect, state.entries.clone()))
            .unwrap_or((egui::Rect::NOTHING, Vec::new()));

        state = Some(PlaylistSubmenuState {
            owner_path: owner_path.to_path_buf(),
            action_paths: action_paths.to_vec(),
            kind,
            popup_rect: preserve_popup.0,
            entries: preserve_popup.1,
            last_hovered_at: now,
            last_rendered_at: now,
        });
    }

    let Some(mut active_state) = state else {
        return;
    };
    if active_state.owner_path != owner_path || active_state.kind != kind {
        return;
    }

    let pointer_in_popup = pointer_position
        .map(|position| active_state.popup_rect.contains(position))
        .unwrap_or(false);
    if response.hovered() || pointer_in_popup {
        active_state.last_hovered_at = now;
    } else if now - active_state.last_hovered_at > PLAYLIST_SUBMENU_HOVER_GRACE_SECONDS {
        store_playlist_submenu_state(ui.ctx(), None);
        return;
    }

    let available_right = screen_rect.right() - response.rect.right() - gap;
    let available_left = response.rect.left() - screen_rect.left() - gap;
    let open_left = available_right < outer_width && available_left > available_right;
    let (pivot, position) = if open_left {
        (
            egui::Align2::RIGHT_TOP,
            egui::pos2(response.rect.left() - gap, response.rect.top() - frame_margin.top),
        )
    } else {
        (
            egui::Align2::LEFT_TOP,
            egui::pos2(response.rect.right() + gap, response.rect.top() - frame_margin.top),
        )
    };

    let available_content_height =
        (screen_rect.bottom() - response.rect.top() - frame_margin.bottom).max(ui.spacing().interact_size.y);
    let max_height = PLAYLIST_SUBMENU_MAX_HEIGHT.min(available_content_height);
    let popup_entries = entries;
    let mut rendered_entries = Vec::with_capacity(popup_entries.len());
    let area_id = egui::Id::new(("track_playlist_submenu_area", owner_path, kind));
    let area_response = egui::Area::new(area_id)
        .kind(egui::UiKind::Menu)
        .order(egui::Order::Foreground)
        .pivot(pivot)
        .fixed_pos(position)
        .default_width(content_width)
        .constrain(false)
        .fade_in(false)
        .sense(egui::Sense::hover())
        .show(ui.ctx(), |ui| {
            menu_style(ui);
            frame.show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.set_min_width(content_width);
                    egui::ScrollArea::vertical()
                        .id_salt(("track_playlist_submenu_scroll", owner_path, kind))
                        .max_height(max_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_min_width(content_width);
                            for (label, action) in popup_entries {
                                let item_response = ui.button(&label);
                                rendered_entries.push(PlaylistSubmenuEntry {
                                    action,
                                    rect: item_response.rect,
                                });
                            }
                        });
                });
            });
        });

    active_state.popup_rect = area_response.response.rect;
    active_state.entries = rendered_entries;
    active_state.last_rendered_at = now;
    let pointer_in_rendered_popup = pointer_position
        .map(|position| active_state.popup_rect.contains(position))
        .unwrap_or(false);
    if pointer_in_rendered_popup {
        active_state.last_hovered_at = now;
    }
    store_playlist_submenu_state(ui.ctx(), Some(active_state));
    ui.ctx().request_repaint_after(Duration::from_millis(50));
}

#[derive(Clone, Debug)]
enum VirtualTrackEntry {
    GroupHeader {
        group: String,
        top: f32,
        bottom: f32,
    },
    TrackRow {
        index: usize,
        group: String,
        visible_row_index: usize,
        next_visible_track_index: Option<usize>,
        top: f32,
        bottom: f32,
        has_separator_after: bool,
    },
}

#[derive(Clone, Debug)]
struct PlaylistTrackTarget {
    playlist_index: usize,
    track_path: PathBuf,
    playlist_name: String,
    kind: PlaylistKind,
}

fn find_track_by_exact_path_or_file_name(playlist: &Playlist, path: &Path) -> Option<usize> {
    let source_path = normalized_path(path);
    let source_file_name = normalized_file_name(path);
    let mut exact_file_name_match = None;

    for (track_index, track) in playlist.tracks.iter().enumerate() {
        if normalized_path(&track.path) == source_path {
            return Some(track_index);
        }

        if exact_file_name_match.is_none() {
            let candidate_file_name = normalized_file_name(&track.path);
            if let (Some(source), Some(candidate)) =
                (source_file_name.as_deref(), candidate_file_name.as_deref())
            {
                if source == candidate {
                    exact_file_name_match = Some(track_index);
                }
            }
        }
    }

    exact_file_name_match
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn normalized_file_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
}

impl VirtualTrackEntry {
    fn top(&self) -> f32 {
        match self {
            Self::GroupHeader { top, .. } | Self::TrackRow { top, .. } => *top,
        }
    }

    fn bottom(&self) -> f32 {
        match self {
            Self::GroupHeader { bottom, .. } | Self::TrackRow { bottom, .. } => *bottom,
        }
    }
}

impl AudioOrbitApp {
    pub(crate) fn render_track_panel(&mut self, ui: &mut egui::Ui) {
        self.process_playlist_submenu_input(ui.ctx());
        let Some(playlist) = self.current_playlist() else {
            ui.heading("No playlist");
            return;
        };

        let playlist_name = playlist.name.clone();
        let is_favorites = playlist.kind == PlaylistKind::Favorites;
        let selected_playlist_label = format!("{} {}", playlist.kind.icon(), playlist_name);
        let selected_group_label = playlist.selected_group.clone().unwrap_or_default();
        let folder_group_count = playlist.folder_groups().len();
        let show_group_headers = playlist.selected_group.is_none() && folder_group_count > 1;
        let query = self.track_search_query.trim().to_owned();
        let visible_indexes = self.visible_track_indexes();
        let visible_count = visible_indexes.len();

        let playlist_options: Vec<(usize, String)> = self
            .state
            .playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                (
                    index,
                    format!("{} {}", playlist.kind.icon(), playlist.name),
                )
            })
            .collect();

        ui.horizontal(|ui| {
            let compact_controls = ui.available_width() < 520.0;
            let selector_width = if compact_controls { 96.0 } else { 120.0 };
            let selected_playlist_short_label = ellipsize_chars(
                &selected_playlist_label,
                if compact_controls { 18 } else { 24 },
            );

            egui::ComboBox::from_id_salt("track_panel_playlist_selector")
                .selected_text(selected_playlist_short_label)
                .width(selector_width)
                .height(520.0)
                .show_ui(ui, |ui| {
                    for (index, label) in playlist_options {
                        if ui
                            .selectable_label(self.state.selected_playlist_index == index, label)
                            .clicked()
                        {
                            self.select_playlist(index);
                        }
                    }
                });
            if folder_group_count > 1 && !selected_group_label.is_empty() && !compact_controls {
                ui.small(ellipsize_chars(&selected_group_label, 24));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search_label = if self.show_track_search {
                    if compact_controls { ui_icons::icon(Icon::X) } else { self.control_label(Icon::X, "Close search") }
                } else if compact_controls {
                    ui_icons::icon(Icon::Search)
                } else {
                    self.control_label(Icon::Search, "Search")
                };
                if ui.button(search_label)
                    .on_hover_text(if self.show_track_search { "Close search" } else { "Search tracks" })
                    .clicked()
                {
                    self.show_track_search = !self.show_track_search;
                    self.focus_track_search = self.show_track_search;
                    self.state.ui.show_track_search = self.show_track_search;
                    if !self.show_track_search {
                        self.track_search_query.clear();
                        self.search_cursor = 0;
                    }
                    self.save_state_silently();
                }

                if ui.small_button("Z-A").on_hover_text("Sort current playlist Z to A").clicked() {
                    self.sort_current_playlist_by_name(false);
                }
                if ui.small_button("A-Z").on_hover_text("Sort current playlist A to Z").clicked() {
                    self.sort_current_playlist_by_name(true);
                }
                if is_favorites
                    && ui
                        .small_button("Added")
                        .on_hover_text("Sort Favorites by when tracks were added, newest first")
                        .clicked()
                {
                    self.sort_current_favorites_by_added();
                }

                let has_active_source = self.active_track_path.is_some() || self.active_radio_index.is_some();
                let now_playing_label = if compact_controls {
                    ui_icons::icon(Icon::Music)
                } else {
                    ui_icons::label(Icon::Music, "Now playing")
                };
                if ui
                    .add_enabled(has_active_source, egui::Button::new(now_playing_label))
                    .on_hover_text("Switch to the active source and center the currently playing item")
                    .clicked()
                {
                    self.jump_to_now_playing();
                }
            });
        });

        if self.show_track_search {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(ui_icons::icon(Icon::Search));

                let search_input_width = ui.available_width().max(120.0);
                let response = ui.add_sized(
                    egui::vec2(search_input_width, ui.spacing().interact_size.y),
                    egui::TextEdit::singleline(&mut self.track_search_query).hint_text("Search tracks or folders"),
                );
                if self.focus_track_search {
                    response.request_focus();
                    self.focus_track_search = false;
                }
                if response.changed() {
                    self.search_cursor = 0;
                }
            });

            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                let compact_item_gap = ui.spacing().item_spacing.x.min(6.0);
                ui.spacing_mut().item_spacing.x = compact_item_gap;
                let can_jump = !visible_indexes.is_empty() && !self.track_search_query.trim().is_empty();
                if search_icon_text_button(ui, can_jump, Icon::ArrowDown, "Next").clicked() {
                    let next = visible_indexes[self.search_cursor % visible_indexes.len()];
                    self.select_only_track_for_action(next);
                    self.search_cursor = (self.search_cursor + 1) % visible_indexes.len().max(1);
                }

                if search_icon_text_button(ui, true, Icon::X, "Clear").clicked() {
                    self.track_search_query.clear();
                    self.search_cursor = 0;
                }

                if ui
                    .checkbox(&mut self.search_playback_filtered_only, "Play results")
                    .on_hover_text("When enabled, Next/auto-play stays inside the current search results until search is closed. Turn it off to keep normal playlist playback while searching.")
                    .changed()
                {
                    self.state.ui.search_playback_filtered_only = self.search_playback_filtered_only;
                    self.save_state_silently();
                }
            });
            if !query.is_empty() {
                ui.add_space(2.0);
                let mode = if self.search_playback_filtered_only { "playback is limited to search results" } else { "playback keeps normal playlist order" };
                ui.small(format!("Filtering tracks by: {query} · {mode}"));
            }
        }

        if self.state.playback.repeat_mode == RepeatMode::Selection {
            ui.add_space(4.0);
            let repeat_order = if self.state.playback.shuffle_enabled { "random playback" } else { "playlist order" };
            let helper = format!("Repeat selection mode: tick tracks or whole folders for {repeat_order}.");
            let helper_width = ui.available_width().max(32.0);
            let _ = render_ellipsized_single_line(
                ui,
                &helper,
                helper_width,
                egui::TextStyle::Small.resolve(ui.style()),
                ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.78),
            );
        }

        if self.multi_selected_track_indexes.len() > 1 {
            ui.add_space(4.0);
            ui.small(format!(
                "{} tracks selected. Right-click selection to copy, add to playlist, or delete.",
                self.multi_selected_track_indexes.len()
            ));
        }

        ui.add_space(4.0);
        ui.separator();

        if visible_count == 0 {
            self.scroll_to_track_path_requested = None;
            ui.centered_and_justified(|ui| {
                if self.show_track_search && !self.track_search_query.trim().is_empty() {
                    ui.label("No tracks match the current search.");
                } else {
                    ui.label("No tracks in this view. Add files or import a music folder.");
                }
            });
            return;
        }

        let playlist_index = self.state.selected_playlist_index;
        let track_count = self
            .state
            .playlists
            .get(playlist_index)
            .map(|playlist| playlist.tracks.len())
            .unwrap_or(0);
        let mut group_track_indexes: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        if let Some(playlist) = self.state.playlists.get(playlist_index) {
            for index in visible_indexes.iter().copied() {
                if let Some(track) = playlist.tracks.get(index) {
                    group_track_indexes.entry(track.group.clone()).or_default().push(index);
                }
            }
        }

        let row_width = (ui.available_width() - 22.0).max(320.0);
        let scroll_height = ui.available_height();
        let mut reorder_track: Option<(usize, usize)> = None;
        let mut next_track_drop_target_index: Option<usize> = self.track_drop_target_index;
        let scroll_output = egui::ScrollArea::vertical()
            .id_salt("track_list_scroll")
            .vertical_scroll_offset(self.current_playlist_scroll_offset_y())
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show_viewport(ui, |ui, _viewport| {
                ui.set_width(row_width);
                let visible_rect = ui.clip_rect();
                let sticky_top = visible_rect.top();
                let pointer_position = ui.input(|input| input.pointer.hover_pos().or(input.pointer.interact_pos()));
                let fast_scroll_paint = self.dragging_track_index.is_none()
                    && ui.input(|input| {
                        input.raw_scroll_delta.length_sq() > 0.0
                            || input.smooth_scroll_delta.length_sq() > 0.0
                    });
                let content_top = ui.cursor().min.y;
                let mut logical_entries: Vec<VirtualTrackEntry> = Vec::new();
                let mut last_group: Option<String> = None;
                let mut displayed_track_indexes: Vec<usize> = Vec::new();

                if let Some(playlist) = self.state.playlists.get(playlist_index) {
                    let mut visible_row_index = 0usize;
                    for index in visible_indexes.iter().copied() {
                        let Some(track) = playlist.tracks.get(index) else {
                            continue;
                        };
                        if show_group_headers && last_group.as_deref() != Some(track.group.as_str()) {
                            logical_entries.push(VirtualTrackEntry::GroupHeader {
                                group: track.group.clone(),
                                top: 0.0,
                                bottom: 0.0,
                            });
                            last_group = Some(track.group.clone());
                        }
                        if show_group_headers && self.collapsed_groups.contains(&track.group) {
                            continue;
                        }
                        logical_entries.push(VirtualTrackEntry::TrackRow {
                            index,
                            group: track.group.clone(),
                            visible_row_index,
                            next_visible_track_index: None,
                            top: 0.0,
                            bottom: 0.0,
                            has_separator_after: false,
                        });
                        displayed_track_indexes.push(index);
                        visible_row_index += 1;
                    }
                }

                let mut displayed_track_position = 0usize;
                let mut y = content_top;
                for entry in &mut logical_entries {
                    match entry {
                        VirtualTrackEntry::GroupHeader { top, bottom, .. } => {
                            *top = y;
                            y += TRACK_GROUP_BLOCK_HEIGHT;
                            *bottom = y;
                        }
                        VirtualTrackEntry::TrackRow {
                            next_visible_track_index,
                            top,
                            bottom,
                            has_separator_after,
                            ..
                        } => {
                            *top = y;
                            *next_visible_track_index = displayed_track_indexes
                                .get(displayed_track_position + 1)
                                .copied();
                            *has_separator_after = next_visible_track_index.is_some();
                            y += TRACK_ROW_HEIGHT + if *has_separator_after { TRACK_SEPARATOR_HEIGHT } else { 0.0 };
                            *bottom = y;
                            displayed_track_position += 1;
                        }
                    }
                }
                let content_bottom = y;
                let overscan = TRACK_ROW_HEIGHT * TRACK_LIST_OVERSCAN_ROWS;
                // Entry positions are calculated in egui screen coordinates. The
                // viewport rectangle passed by ScrollArea is not stable enough to mix
                // with those coordinates across scroll/resize, so use the actual clipped
                // visible rect for virtualization bounds. Mixing coordinate spaces caused
                // skipped rows and growing blank gaps while scrolling large playlists.
                let render_top = visible_rect.top() - overscan;
                let render_bottom = visible_rect.bottom() + overscan;
                let first_rendered = logical_entries
                    .iter()
                    .position(|entry| entry.bottom() >= render_top)
                    .unwrap_or(logical_entries.len());
                let last_rendered_exclusive = logical_entries
                    .iter()
                    .rposition(|entry| entry.top() <= render_bottom)
                    .map(|index| index + 1)
                    .unwrap_or(first_rendered);

                if let Some(requested_path) = self.scroll_to_track_path_requested.clone() {
                    let requested_entry = logical_entries.iter().find(|entry| {
                        let VirtualTrackEntry::TrackRow { index, .. } = entry else {
                            return false;
                        };
                        self.state
                            .playlists
                            .get(playlist_index)
                            .and_then(|playlist| playlist.tracks.get(*index))
                            .map(|track| same_path(&track.path, &requested_path))
                            .unwrap_or(false)
                    });

                    if let Some(entry) = requested_entry {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(visible_rect.left(), entry.top()),
                            egui::vec2(row_width, TRACK_ROW_HEIGHT),
                        );
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }

                    self.scroll_to_track_path_requested = None;
                }

                if self.scroll_to_active_track_requested {
                    let active_entry = self.active_track_path.as_ref().and_then(|active_path| {
                        logical_entries.iter().find(|entry| {
                            let VirtualTrackEntry::TrackRow { index, .. } = entry else {
                                return false;
                            };
                            self.state
                                .playlists
                                .get(playlist_index)
                                .and_then(|playlist| playlist.tracks.get(*index))
                                .map(|track| same_path(&track.path, active_path))
                                .unwrap_or(false)
                        })
                    });

                    if let Some(entry) = active_entry {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(visible_rect.left(), entry.top()),
                            egui::vec2(row_width, TRACK_ROW_HEIGHT),
                        );
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }

                    // A failed one-shot request should not keep the app in fast repaint mode
                    // forever. The explicit Now playing action clears filters before setting
                    // this flag, so hidden-by-search/folder cases can safely end here.
                    self.scroll_to_active_track_requested = false;
                }

                if let Some(requested_group) = self.scroll_to_folder_group_requested.clone() {
                    if let Some(entry) = logical_entries.iter().find(|entry| {
                        matches!(entry, VirtualTrackEntry::GroupHeader { group, .. } if group == &requested_group)
                    }) {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(visible_rect.left(), entry.top()),
                            egui::vec2(row_width, TRACK_GROUP_HEADER_HEIGHT),
                        );
                        ui.scroll_to_rect(rect, Some(egui::Align::Min));
                        self.scroll_to_folder_group_requested = None;
                    }
                }

                let mut viewport_group: Option<String> = None;
                let mut next_group_header_top: Option<f32> = None;
                for entry in &logical_entries {
                    match entry {
                        VirtualTrackEntry::GroupHeader { group, top, .. } => {
                            if *top <= sticky_top {
                                viewport_group = Some(group.clone());
                            } else {
                                next_group_header_top = Some(
                                    next_group_header_top
                                        .map(|current| current.min(*top))
                                        .unwrap_or(*top),
                                );
                                break;
                            }
                        }
                        VirtualTrackEntry::TrackRow { group, top, bottom, .. } => {
                            if viewport_group.is_none() && *bottom >= visible_rect.top() && *top <= visible_rect.bottom() {
                                viewport_group = Some(group.clone());
                            }
                        }
                    }
                }

                if first_rendered > 0 {
                    let next_top = logical_entries
                        .get(first_rendered)
                        .map(VirtualTrackEntry::top)
                        .unwrap_or(content_bottom);
                    let skipped = (next_top - ui.cursor().min.y).max(0.0);
                    if skipped > 0.0 {
                        ui.add_space(skipped);
                    }
                }

                for entry_index in first_rendered..last_rendered_exclusive {
                    let entry = logical_entries[entry_index].clone();
                    match entry {
                        VirtualTrackEntry::GroupHeader { group, .. } => {
                            ui.add_space(TRACK_GROUP_TOP_GAP);
                            let collapsed = self.collapsed_groups.contains(&group);
                            let mut toggle_group = false;
                            let repeat_selection_mode = self.state.playback.repeat_mode == RepeatMode::Selection;
                            let group_indexes = group_track_indexes.get(&group).cloned().unwrap_or_default();
                            let header_response = ui.allocate_ui_with_layout(
                                egui::vec2(row_width, TRACK_GROUP_HEADER_HEIGHT),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let icon = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
                                    if ui.small_button(ui_icons::icon(icon)).on_hover_text("Collapse/expand folder").clicked() {
                                        toggle_group = true;
                                    }
                                    if repeat_selection_mode {
                                        let mut checked = !group_indexes.is_empty()
                                            && group_indexes.iter().all(|index| self.selected_track_indexes.contains(index));
                                        if ui.checkbox(&mut checked, "").on_hover_text("Include this whole folder in repeat selection").changed() {
                                            if checked {
                                                for index in &group_indexes {
                                                    self.selected_track_indexes.insert(*index);
                                                }
                                            } else {
                                                for index in &group_indexes {
                                                    self.selected_track_indexes.remove(index);
                                                }
                                            }
                                            self.persist_repeat_selection_for_current_playlist();
                                            self.save_state_silently();
                                        }
                                    }
                                    ui.label(egui::RichText::new(group.as_str()).size(13.0).strong());
                                },
                            );
                            header_response.response.context_menu(|ui| {
                                if self.state.playback.repeat_mode == RepeatMode::Selection {
                                    if ui.button("Select folder for repeat").clicked() {
                                        for index in &group_indexes {
                                            self.selected_track_indexes.insert(*index);
                                        }
                                        self.persist_repeat_selection_for_current_playlist();
                                        self.save_state_silently();
                                        ui.close_menu();
                                    }
                                    if ui.button("Remove folder from repeat").clicked() {
                                        for index in &group_indexes {
                                            self.selected_track_indexes.remove(index);
                                        }
                                        self.persist_repeat_selection_for_current_playlist();
                                        self.save_state_silently();
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                }
                                if ui.button("Move folder up").clicked() {
                                    self.move_folder_group_in_current_playlist(&group, -1);
                                    ui.close_menu();
                                }
                                if ui.button("Move folder down").clicked() {
                                    self.move_folder_group_in_current_playlist(&group, 1);
                                    ui.close_menu();
                                }
                            });
                            if toggle_group {
                                self.toggle_folder_group_collapsed_and_focus(&group);
                            } else if header_response.response.double_clicked() {
                                self.request_folder_group_top(&group);
                            }
                            paint_list_separator(ui, row_width, false);
                        }
                        VirtualTrackEntry::TrackRow {
                            index,
                            visible_row_index,
                            next_visible_track_index,
                            has_separator_after,
                            ..
                        } => {
                            let Some(track) = self
                                .state
                                .playlists
                                .get(playlist_index)
                                .and_then(|playlist| playlist.tracks.get(index))
                                .cloned()
                            else {
                                continue;
                            };
                            let is_selected = if self.multi_selected_track_indexes.is_empty() {
                                self.selected_track_index == Some(index)
                            } else {
                                self.multi_selected_track_indexes.contains(&index)
                            };
                            let is_active = self
                                .active_track_path
                                .as_ref()
                                .map(|active| same_path(active, &track.path))
                                .unwrap_or(false);
                            let favorite = self.is_favorite(&track.path);
                            let missing = track.missing;
                            let track_metadata = if self.player_only_mode {
                                format_track_metadata_player_only(&track)
                            } else {
                                format_track_metadata_compact(&track)
                            };
                            let metadata = if missing {
                                if track_metadata.is_empty() {
                                    "Missing".to_owned()
                                } else {
                                    format!("Missing · {track_metadata}")
                                }
                            } else {
                                track_metadata
                            };
                            let title = if is_active {
                                format!("{} {}", ui_icons::icon(Icon::Play), track.title)
                            } else {
                                track.title.clone()
                            };
                            let path = track.path.clone();
                            let repeat_selection_mode = self.state.playback.repeat_mode == RepeatMode::Selection;

                            if fast_scroll_paint {
                                let row_rect = paint_track_row_fast_scroll(
                                    ui,
                                    row_width,
                                    &title,
                                    &metadata,
                                    is_selected,
                                    is_active,
                                    favorite,
                                    missing,
                                    repeat_selection_mode,
                                    self.selected_track_indexes.contains(&index),
                                );
                                if has_separator_after {
                                    paint_list_separator(ui, row_width, false);
                                } else if next_track_drop_target_index == Some(track_count) {
                                    paint_list_edge_separator(ui, row_rect, row_width, true);
                                }
                                continue;
                            }

                            let row_hovered = next_row_pointer_hovered(ui, row_width, TRACK_ROW_HEIGHT);
                            let mut body_clicked = false;
                            let mut body_double_clicked = false;
                            let row_response = ui.allocate_ui_with_layout(
                                egui::vec2(row_width, TRACK_ROW_HEIGHT),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if repeat_selection_mode {
                                        let mut checked = self.selected_track_indexes.contains(&index);
                                        if ui.checkbox(&mut checked, "").on_hover_text("Include in repeat selection").changed() {
                                            if checked {
                                                self.selected_track_indexes.insert(index);
                                            } else {
                                                self.selected_track_indexes.remove(&index);
                                            }
                                            self.persist_repeat_selection_for_current_playlist();
                                            self.save_state_silently();
                                        }
                                    }

                                    let heart = if favorite {
                                        let color = egui::Color32::from_rgb(230, 70, 95);
                                        egui::RichText::new("♥")
                                            .color(if missing { color.linear_multiply(0.42) } else { color })
                                            .size(15.0)
                                    } else {
                                        let color = ui.visuals().widgets.inactive.fg_stroke.color;
                                        egui::RichText::new("♡")
                                            .color(if missing { color.linear_multiply(0.42) } else { color })
                                            .size(15.0)
                                    };
                                    if ui
                                        .add_sized(egui::vec2(28.0, 24.0), egui::Button::new(heart))
                                        .on_hover_text("Toggle favorite")
                                        .clicked()
                                    {
                                        self.toggle_favorite(path.clone());
                                    }

                                    let body_width = ui.available_width().max(160.0);
                                    let (body_rect, response) = ui.allocate_exact_size(
                                        egui::vec2(body_width, 24.0),
                                        egui::Sense::click(),
                                    );

                                    if is_selected {
                                        let fill = ui.visuals().selection.bg_fill;
                                        ui.painter().rect_filled(
                                            body_rect,
                                            5.0,
                                            if missing { fill.linear_multiply(0.42) } else { fill },
                                        );
                                    }

                                    let body_padding = 8.0;
                                    let mut text_color = if is_selected {
                                        ui.visuals().selection.stroke.color
                                    } else if is_active {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        ui.visuals().widgets.inactive.fg_stroke.color
                                    };
                                    if missing {
                                        text_color = text_color.linear_multiply(0.42);
                                    }
                                    let title_font = egui::FontId::proportional(14.0);
                                    let metadata_font = egui::FontId::proportional(12.0);
                                    let mut metadata_color = if is_active || row_hovered {
                                        ui.visuals().widgets.inactive.fg_stroke.color
                                    } else {
                                        ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.50)
                                    };
                                    if missing {
                                        metadata_color = metadata_color.linear_multiply(0.42);
                                    }
                                    let metadata_width = if metadata.is_empty() {
                                        0.0
                                    } else {
                                        text_width(ui, &metadata, metadata_font.clone(), metadata_color).ceil()
                                    };
                                    let metadata_gap = if metadata.is_empty() { 0.0 } else { 6.0 };
                                    let title_left = body_rect.left() + body_padding;
                                    let title_right = (body_rect.right() - body_padding - metadata_width - metadata_gap)
                                        .max(title_left + 24.0);
                                    let title_rect = egui::Rect::from_min_max(
                                        egui::pos2(title_left, body_rect.top()),
                                        egui::pos2(title_right, body_rect.bottom()),
                                    );

                                    let title = ellipsize_to_width_exact(ui, &title, title_rect.width(), title_font.clone(), text_color);
                                    ui.painter().with_clip_rect(title_rect).text(
                                        egui::pos2(title_rect.left(), title_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        title,
                                        title_font,
                                        text_color,
                                    );

                                    if !metadata.is_empty() {
                                        let metadata_rect = egui::Rect::from_min_max(
                                            egui::pos2(body_rect.right() - body_padding - metadata_width, body_rect.top()),
                                            egui::pos2(body_rect.right() - body_padding, body_rect.bottom()),
                                        );
                                        ui.painter().with_clip_rect(metadata_rect).text(
                                            egui::pos2(metadata_rect.right(), metadata_rect.center().y),
                                            egui::Align2::RIGHT_CENTER,
                                            metadata,
                                            metadata_font,
                                            metadata_color,
                                        );
                                    }

                                    let response = if missing {
                                        response.on_hover_text(format!("File not found: {}", path.display()))
                                    } else {
                                        response
                                    };
                                    if response.double_clicked() {
                                        body_double_clicked = true;
                                        self.select_only_track_for_action(index);
                                        self.play_path(path.clone(), Some(index), 0.0);
                                    } else if response.clicked() {
                                        body_clicked = true;
                                        let modifiers = ui.input(|input| input.modifiers);
                                        self.select_track_from_pointer(index, modifiers);
                                    }
                                    response.context_menu(|ui| {
                                        self.ensure_track_selected_for_context(index);
                                        self.render_track_row_context_menu(ui, index, path.clone());
                                    });
                                },
                            );
                            let mut context_rect = row_response.response.rect.expand(2.0);
                            context_rect.min.x += if repeat_selection_mode { 62.0 } else { 34.0 };
                            let context_response = ui.interact(
                                context_rect,
                                ui.make_persistent_id(("track_context", index)),
                                egui::Sense::click_and_drag(),
                            );
                            if !body_double_clicked && context_response.double_clicked() {
                                self.select_only_track_for_action(index);
                                self.play_path(path.clone(), Some(index), 0.0);
                            } else if !body_clicked && context_response.clicked() {
                                let modifiers = ui.input(|input| input.modifiers);
                                self.select_track_from_pointer(index, modifiers);
                            }
                            if context_response.drag_started() {
                                self.dragging_track_index = Some(index);
                                self.track_drop_target_index = None;
                            }
                            let pointer_over_row = pointer_position
                                .map(|position| row_response.response.rect.expand(2.0).contains(position))
                                .unwrap_or(false);
                            if let Some(from) = self.dragging_track_index {
                                if pointer_over_row {
                                    let pointer_y = pointer_position
                                        .map(|position| position.y)
                                        .unwrap_or(row_response.response.rect.center().y);
                                    let drop_after = pointer_y >= row_response.response.rect.center().y;
                                    let to = if drop_after {
                                        next_visible_track_index.unwrap_or(track_count)
                                    } else {
                                        index
                                    };
                                    next_track_drop_target_index = Some(to);
                                    if ui.input(|input| input.pointer.any_released()) && self.valid_track_drop_target(from, to) {
                                        reorder_track = Some((from, to));
                                        self.dragging_track_index = None;
                                    }
                                }
                            }
                            let track_drop_target_for_paint = next_track_drop_target_index
                                .or(self.track_drop_target_index)
                                .filter(|to| self.dragging_track_index.map(|from| self.valid_track_drop_target(from, *to)).unwrap_or(false));
                            if self.dragging_track_index == Some(index) && track_drop_target_for_paint.is_some() {
                                paint_dragged_row_fade(ui, row_response.response.rect);
                            }
                            if visible_row_index == 0 && track_drop_target_for_paint == Some(index) {
                                paint_list_edge_separator(ui, row_response.response.rect, row_width, false);
                            }
                            if row_response.response.secondary_clicked() || context_response.secondary_clicked() {
                                self.ensure_track_selected_for_context(index);
                            }
                            context_response.context_menu(|ui| {
                                self.ensure_track_selected_for_context(index);
                                self.render_track_row_context_menu(ui, index, path.clone());
                            });
                            if has_separator_after {
                                let separator_drop_target = next_visible_track_index.unwrap_or(index + 1);
                                paint_list_separator(ui, row_width, track_drop_target_for_paint == Some(separator_drop_target));
                            } else if track_drop_target_for_paint == Some(track_count) {
                                paint_list_edge_separator(ui, row_response.response.rect, row_width, true);
                            }
                        }
                    }
                }

                let remaining = (content_bottom - ui.cursor().min.y).max(0.0);
                if remaining > 0.0 {
                    ui.add_space(remaining);
                }

                if show_group_headers && self.current_playlist_scroll_offset_y() > 2.0 {
                    if let Some(group) = viewport_group {
                        let sticky_height = TRACK_GROUP_HEADER_HEIGHT;
                        let push_offset_y = next_group_header_top
                            .map(|next_top| (next_top - sticky_top - sticky_height).min(0.0))
                            .unwrap_or(0.0);
                        let collapsed = self.collapsed_groups.contains(&group);
                        let (sticky_rect, sticky_icon_rect) = paint_sticky_folder_header(
                            ui,
                            visible_rect,
                            &group,
                            collapsed,
                            push_offset_y,
                        );
                        let sticky_response = ui.interact(
                            sticky_rect,
                            ui.make_persistent_id(("sticky_folder_header", group.as_str())),
                            egui::Sense::click(),
                        );
                        let clicked_icon = sticky_response.clicked()
                            && ui
                                .input(|input| input.pointer.interact_pos())
                                .map(|position| sticky_icon_rect.contains(position))
                                .unwrap_or(false);
                        if clicked_icon {
                            self.toggle_folder_group_collapsed_and_focus(&group);
                        } else if sticky_response.double_clicked() {
                            self.request_folder_group_top(&group);
                        }
                    }
                }
            });
        self.remember_current_playlist_scroll_offset(scroll_output.state.offset.y);
        self.track_drop_target_index = next_track_drop_target_index;
        if !ui.input(|input| input.pointer.primary_down()) {
            self.dragging_track_index = None;
            self.track_drop_target_index = None;
        }
        if let Some((from, to)) = reorder_track {
            self.track_drop_target_index = None;
            self.move_track_to_index_in_current_playlist(from, to);
        }
    }
    fn process_playlist_submenu_input(&mut self, context: &egui::Context) {
        let Some(state) = load_playlist_submenu_state(context) else {
            return;
        };
        let now = context.input(|input| input.time);
        if now - state.last_rendered_at > PLAYLIST_SUBMENU_STALE_SECONDS {
            store_playlist_submenu_state(context, None);
            return;
        }

        let pressed_position = context.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            })
        });
        let Some(position) = pressed_position else {
            return;
        };
        let Some(action) = state
            .entries
            .iter()
            .find(|entry| entry.rect.contains(position))
            .map(|entry| entry.action.clone())
        else {
            return;
        };

        store_playlist_submenu_state(context, None);
        match action {
            PlaylistSubmenuAction::Add { playlist_index } => {
                self.add_tracks_to_playlist(state.action_paths, playlist_index);
            }
            PlaylistSubmenuAction::New => {
                self.open_new_playlist_modal(state.action_paths);
            }
            PlaylistSubmenuAction::Search {
                playlist_index,
                track_path,
            } => {
                self.jump_to_track_in_playlist(playlist_index, track_path);
            }
        }
    }

    fn matching_playlist_tracks(&self, path: &Path) -> Vec<PlaylistTrackTarget> {
        let current_playlist_index = self.state.selected_playlist_index;

        self.state
            .playlists
            .iter()
            .enumerate()
            .filter(|(playlist_index, _)| *playlist_index != current_playlist_index)
            .filter_map(|(playlist_index, playlist)| {
                find_track_by_exact_path_or_file_name(playlist, path).map(|track_index| {
                    PlaylistTrackTarget {
                        playlist_index,
                        track_path: playlist.tracks[track_index].path.clone(),
                        playlist_name: playlist.name.clone(),
                        kind: playlist.kind.clone(),
                    }
                })
            })
            .collect()
    }

    pub(crate) fn render_track_row_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        path: PathBuf,
    ) {
        let missing = self
            .current_playlist()
            .and_then(|playlist| playlist.tracks.get(index))
            .map(|track| track.missing)
            .unwrap_or(true);
        let action_paths = self.action_track_paths_for_context(index);
        let selected_count = action_paths.len();
        let available_dj_track_count = action_paths.iter().filter(|path| path.is_file()).count();
        let file_operation_idle = self.track_file_operations_idle();

        if selected_count > 1 {
            ui.small(format!("{selected_count} tracks selected"));
            ui.separator();
        }

        let play_response = ui.button(ui_icons::label(Icon::Play, "Play now"));
        let play_response = if missing {
            play_response.on_hover_text("Checks whether file has returned before playback.")
        } else {
            play_response
        };
        if play_response.clicked() {
            self.select_only_track_for_action(index);
            self.play_path(path.clone(), Some(index), 0.0);
            ui.close_menu();
        }
        if ui.button(ui_icons::label(Icon::Info, "Details")).clicked() {
            self.details_modal = Some(DetailsModal::Track(path.clone()));
            ui.close_menu();
        }
        let can_move_up = selected_count <= 1 && self.can_move_track_in_current_playlist(index, -1);
        if ui.add_enabled(can_move_up, egui::Button::new("Move up")).clicked() {
            self.move_track_in_current_playlist(index, -1);
            ui.close_menu();
        }
        let can_move_down = selected_count <= 1 && self.can_move_track_in_current_playlist(index, 1);
        if ui.add_enabled(can_move_down, egui::Button::new("Move down")).clicked() {
            self.move_track_in_current_playlist(index, 1);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                !missing,
                egui::Button::new(ui_icons::label(Icon::ExternalLink, "Show in File Explorer")),
            )
            .on_disabled_hover_text("File is missing.")
            .clicked()
        {
            self.reveal_track_in_file_manager(path.clone());
            ui.close_menu();
        }

        let mut add_targets = self
            .state
            .playlists
            .iter()
            .enumerate()
            .filter(|(_, playlist)| playlist.accepts_manual_tracks())
            .map(|(playlist_index, playlist)| {
                (
                    format!("{} {}", playlist.kind.icon(), playlist.name),
                    PlaylistSubmenuAction::Add { playlist_index },
                )
            })
            .collect::<Vec<_>>();
        add_targets.push(("New".to_owned(), PlaylistSubmenuAction::New));
        render_playlist_submenu(
            ui,
            &path,
            &action_paths,
            PlaylistSubmenuKind::Add,
            ui_icons::label(Icon::ListPlus, "Add to playlist"),
            add_targets,
            None,
        );

        let playlist_matches = self
            .matching_playlist_tracks(&path)
            .into_iter()
            .map(|target| {
                (
                    format!("{} {}", target.kind.icon(), target.playlist_name),
                    PlaylistSubmenuAction::Search {
                        playlist_index: target.playlist_index,
                        track_path: target.track_path,
                    },
                )
            })
            .collect::<Vec<_>>();
        let search_action_paths = vec![path.clone()];
        render_playlist_submenu(
            ui,
            &path,
            &search_action_paths,
            PlaylistSubmenuKind::Search,
            ui_icons::label(Icon::Search, "Search in playlist"),
            playlist_matches,
            Some("Track is not present in another playlist."),
        );

        if ui
            .add_enabled(
                available_dj_track_count >= 2 && !self.dj_mix_is_running(),
                egui::Button::new(ui_icons::label(Icon::Music, "Create DJ mix...")),
            )
            .on_disabled_hover_text("Select at least two available tracks, or wait for current DJ mix export.")
            .clicked()
        {
            self.open_dj_mix_builder_for_selection(index);
            ui.close_menu();
        }

        let copy_label = if selected_count > 1 {
            "Copy selected to folder..."
        } else {
            "Copy to folder..."
        };
        if ui
            .add_enabled(
                selected_count > 0 && file_operation_idle,
                egui::Button::new(ui_icons::label(Icon::FolderOpen, copy_label)),
            )
            .on_disabled_hover_text("Another track file operation is running.")
            .clicked()
        {
            self.copy_track_selection_to_folder(index);
            ui.close_menu();
        }

        if selected_count > 1 {
            if ui
                .add_enabled(
                    file_operation_idle,
                    egui::Button::new(ui_icons::label(Icon::Trash2, "Delete selected from disk...")),
                )
                .on_disabled_hover_text("Another track file operation is running.")
                .clicked()
            {
                self.request_delete_track_selection(index);
                ui.close_menu();
            }
        } else if !missing {
            let can_remove_from_playlist = self
                .current_playlist()
                .map(|playlist| playlist.kind != PlaylistKind::Folder)
                .unwrap_or(false);
            if ui
                .add_enabled(can_remove_from_playlist, egui::Button::new(ui_icons::label(Icon::ListMinus, "Remove from playlist")))
                .clicked()
            {
                self.remove_track_from_current_playlist(index);
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    file_operation_idle,
                    egui::Button::new(ui_icons::label(Icon::Trash2, "Delete from disk")),
                )
                .on_disabled_hover_text("Another track file operation is running.")
                .clicked()
            {
                self.delete_track_from_disk(path);
                ui.close_menu();
            }
        } else if ui
            .button(ui_icons::label(Icon::Trash2, "Remove missing entry"))
            .on_hover_text("Remove this unavailable item from current playlist. No disk operation is performed.")
            .clicked()
        {
            self.remove_missing_track_from_current_playlist(index);
            ui.close_menu();
        }
    }
}

fn paint_track_row_fast_scroll(
    ui: &mut egui::Ui,
    row_width: f32,
    title: &str,
    metadata: &str,
    is_selected: bool,
    is_active: bool,
    favorite: bool,
    missing: bool,
    repeat_selection_mode: bool,
    repeat_selected: bool,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(row_width, TRACK_ROW_HEIGHT),
        egui::Sense::hover(),
    );

    // Keep the cheap scroll renderer visually identical to the fully interactive
    // row layout. During scroll we deliberately skip egui widgets/context menus,
    // but spacing and text clipping must not change between the two modes, or the
    // list appears to vibrate while scrolling large playlists.
    let item_gap = ui.spacing().item_spacing.x;
    let button_size = egui::vec2(28.0, 24.0);
    let body_height = 24.0;
    let mut left = rect.left();

    let mut text_color = if is_selected {
        ui.visuals().selection.stroke.color
    } else if is_active {
        ui.visuals().selection.bg_fill
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    if missing {
        text_color = text_color.linear_multiply(0.42);
    }
    let title_font = egui::FontId::proportional(14.0);
    let metadata_font = egui::FontId::proportional(12.0);
    let mut metadata_color = ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.50);
    if missing {
        metadata_color = metadata_color.linear_multiply(0.42);
    }

    if repeat_selection_mode {
        let checkbox_side = ui.spacing().interact_size.y.min(TRACK_ROW_HEIGHT).max(18.0);
        let checkbox_rect = egui::Rect::from_center_size(
            egui::pos2(left + checkbox_side * 0.5, rect.center().y),
            egui::vec2(checkbox_side, checkbox_side),
        );
        let marker = if repeat_selected { "☑" } else { "☐" };
        ui.painter().text(
            checkbox_rect.center(),
            egui::Align2::CENTER_CENTER,
            marker,
            metadata_font.clone(),
            metadata_color,
        );
        left += checkbox_side + item_gap;
    }

    let heart = if favorite { "♥" } else { "♡" };
    let mut heart_color = if favorite {
        egui::Color32::from_rgb(230, 70, 95)
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    if missing {
        heart_color = heart_color.linear_multiply(0.42);
    }
    let heart_rect = egui::Rect::from_min_size(
        egui::pos2(left, rect.center().y - button_size.y * 0.5),
        button_size,
    );
    let heart_visuals = ui.visuals().widgets.inactive;
    ui.painter().rect(
        heart_rect,
        4.0,
        heart_visuals.bg_fill,
        heart_visuals.bg_stroke,
        egui::StrokeKind::Outside,
    );
    ui.painter().text(
        heart_rect.center(),
        egui::Align2::CENTER_CENTER,
        heart,
        egui::FontId::proportional(15.0),
        heart_color,
    );
    left = heart_rect.right() + item_gap;

    let body_rect = egui::Rect::from_min_max(
        egui::pos2(left, rect.center().y - body_height * 0.5),
        egui::pos2(rect.right(), rect.center().y + body_height * 0.5),
    );

    if is_selected {
        let fill = ui.visuals().selection.bg_fill;
        ui.painter().rect_filled(
            body_rect,
            5.0,
            if missing { fill.linear_multiply(0.42) } else { fill },
        );
    }

    let body_padding = 8.0;
    let metadata_width = if metadata.is_empty() {
        0.0
    } else {
        text_width(ui, metadata, metadata_font.clone(), metadata_color).ceil()
    };
    let metadata_gap = if metadata.is_empty() { 0.0 } else { 6.0 };
    let title_left = body_rect.left() + body_padding;
    let title_right = (body_rect.right() - body_padding - metadata_width - metadata_gap)
        .max(title_left + 24.0);
    let title_rect = egui::Rect::from_min_max(
        egui::pos2(title_left, body_rect.top()),
        egui::pos2(title_right, body_rect.bottom()),
    );
    let clipped_title = ellipsize_to_width_exact(ui, title, title_rect.width(), title_font.clone(), text_color);
    ui.painter().with_clip_rect(title_rect).text(
        egui::pos2(title_rect.left(), title_rect.center().y),
        egui::Align2::LEFT_CENTER,
        clipped_title,
        title_font,
        text_color,
    );

    if !metadata.is_empty() {
        let metadata_rect = egui::Rect::from_min_max(
            egui::pos2(body_rect.right() - body_padding - metadata_width, body_rect.top()),
            egui::pos2(body_rect.right() - body_padding, body_rect.bottom()),
        );
        ui.painter().with_clip_rect(metadata_rect).text(
            egui::pos2(metadata_rect.right(), metadata_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            metadata,
            metadata_font,
            metadata_color,
        );
    }

    rect
}

#[cfg(test)]
mod playlist_search_tests {
    use super::*;

    #[test]
    fn playlist_search_prefers_exact_path_over_earlier_file_name_match() {
        let requested = PathBuf::from("C:/Music/Artist/Track.mp3");
        let mut playlist = Playlist::new("Target");
        playlist.tracks = vec![
            Track::from_path(PathBuf::from("D:/Archive/Track.mp3"), None, 0),
            Track::from_path(requested.clone(), None, 0),
        ];

        assert_eq!(find_track_by_exact_path_or_file_name(&playlist, &requested), Some(1));
    }

    #[test]
    fn playlist_search_falls_back_to_exact_file_name() {
        let requested = PathBuf::from("C:/Music/Artist/Track.mp3");
        let mut playlist = Playlist::new("Target");
        playlist.tracks = vec![
            Track::from_path(PathBuf::from("D:/Archive/Other Track.mp3"), None, 0),
            Track::from_path(PathBuf::from("D:/Archive/TRACK.MP3"), None, 0),
        ];

        assert_eq!(find_track_by_exact_path_or_file_name(&playlist, &requested), Some(1));
    }

    #[test]
    fn playlist_search_rejects_partial_file_name_matches() {
        let requested = PathBuf::from("C:/Music/Artist/Track.mp3");
        let mut playlist = Playlist::new("Target");
        playlist.tracks = vec![Track::from_path(
            PathBuf::from("D:/Archive/Track (Remix).mp3"),
            None,
            0,
        )];

        assert_eq!(find_track_by_exact_path_or_file_name(&playlist, &requested), None);
    }
}
