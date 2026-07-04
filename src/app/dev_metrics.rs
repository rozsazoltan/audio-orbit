use crate::*;

#[cfg(debug_assertions)]
use std::sync::{atomic::Ordering, Arc};

#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub(crate) struct DevMetricsPanelState {
    pub(crate) snapshot: DevMetricsSnapshot,
    last_sample: Option<ProcessMetricsSample>,
    last_sample_at: Option<Instant>,
    last_frame_at: Option<Instant>,
    last_refresh_at: Instant,
    logical_processors: usize,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub(crate) struct DevMetricsSnapshot {
    pub(crate) cpu_percent: Option<f32>,
    pub(crate) working_set_bytes: Option<u64>,
    pub(crate) peak_working_set_bytes: Option<u64>,
    pub(crate) pagefile_bytes: Option<u64>,
    pub(crate) frame_delta_ms: f32,
    pub(crate) estimated_fps: f32,
    pub(crate) repaint_interval_ms: u64,
    pub(crate) uptime_seconds: f32,
    pub(crate) background_jobs: Vec<&'static str>,
    pub(crate) player_state: &'static str,
    pub(crate) gpu_usage_label: &'static str,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug, Default)]
pub(crate) struct DevMetricsCounters {
    pub(crate) playlists: usize,
    pub(crate) tracks: usize,
    pub(crate) radio_stations: usize,
    pub(crate) undo_stack: usize,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
struct ProcessMetricsSample {
    process_time_100ns: u64,
    working_set_bytes: u64,
    peak_working_set_bytes: u64,
    pagefile_bytes: u64,
}

#[cfg(debug_assertions)]
impl Default for DevMetricsSnapshot {
    fn default() -> Self {
        Self {
            cpu_percent: None,
            working_set_bytes: None,
            peak_working_set_bytes: None,
            pagefile_bytes: None,
            frame_delta_ms: 0.0,
            estimated_fps: 0.0,
            repaint_interval_ms: 0,
            uptime_seconds: 0.0,
            background_jobs: Vec::new(),
            player_state: "idle",
            gpu_usage_label: "not sampled",
        }
    }
}

#[cfg(debug_assertions)]
impl Default for DevMetricsPanelState {
    fn default() -> Self {
        Self {
            snapshot: DevMetricsSnapshot::default(),
            last_sample: None,
            last_sample_at: None,
            last_frame_at: None,
            last_refresh_at: Instant::now(),
            logical_processors: std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1)
                .max(1),
        }
    }
}

#[cfg(debug_assertions)]
impl AudioOrbitApp {
    pub(crate) fn update_dev_metrics(&mut self, context: &egui::Context, repaint_interval: Duration) {
        if !self.show_dev_metrics_window {
            return;
        }

        let now = Instant::now();
        let frame_delta = self
            .dev_metrics
            .last_frame_at
            .replace(now)
            .map(|previous| now.saturating_duration_since(previous))
            .unwrap_or_default();

        self.dev_metrics.snapshot.frame_delta_ms = duration_ms(frame_delta);
        self.dev_metrics.snapshot.estimated_fps = if frame_delta.as_secs_f32() > 0.0 {
            1.0 / frame_delta.as_secs_f32()
        } else {
            0.0
        };
        self.dev_metrics.snapshot.repaint_interval_ms = repaint_interval.as_millis() as u64;
        self.dev_metrics.snapshot.background_jobs = self.active_dev_background_jobs();
        self.dev_metrics.snapshot.player_state = self.dev_player_state_label();
        self.dev_metrics.snapshot.uptime_seconds = context.input(|input| input.time as f32);

        if now.saturating_duration_since(self.dev_metrics.last_refresh_at) < Duration::from_millis(750) {
            return;
        }
        self.dev_metrics.last_refresh_at = now;

        let Some(sample) = collect_process_metrics_sample() else {
            self.dev_metrics.snapshot.cpu_percent = None;
            self.dev_metrics.snapshot.working_set_bytes = None;
            self.dev_metrics.snapshot.peak_working_set_bytes = None;
            self.dev_metrics.snapshot.pagefile_bytes = None;
            return;
        };

        if let (Some(previous), Some(previous_at)) = (self.dev_metrics.last_sample, self.dev_metrics.last_sample_at) {
            let elapsed = now.saturating_duration_since(previous_at).as_secs_f64();
            if elapsed > 0.0 {
                let process_seconds = sample
                    .process_time_100ns
                    .saturating_sub(previous.process_time_100ns) as f64
                    / 10_000_000.0;
                let normalized = process_seconds / elapsed / self.dev_metrics.logical_processors as f64 * 100.0;
                self.dev_metrics.snapshot.cpu_percent = Some(normalized.clamp(0.0, 100.0) as f32);
            }
        }

        self.dev_metrics.snapshot.working_set_bytes = Some(sample.working_set_bytes);
        self.dev_metrics.snapshot.peak_working_set_bytes = Some(sample.peak_working_set_bytes);
        self.dev_metrics.snapshot.pagefile_bytes = Some(sample.pagefile_bytes);
        self.dev_metrics.last_sample = Some(sample);
        self.dev_metrics.last_sample_at = Some(now);
    }

    fn active_dev_background_jobs(&self) -> Vec<&'static str> {
        let mut jobs = Vec::new();
        if self.pending_prepared_track_receiver.is_some() {
            jobs.push("audio prepare");
        }
        if self.pending_folder_scan_receiver.is_some() {
            jobs.push("folder scan");
        }
        if self.pending_track_switch.is_some() {
            jobs.push("crossfade switch");
        }
        if self.update_check_receiver.is_some() {
            jobs.push("update check");
        }
        if self.update_install_receiver.is_some() {
            jobs.push("update install");
        }
        if self.radio_title_receiver.is_some() {
            jobs.push("radio metadata");
        }
        if self.pending_profile_apply_at.is_some() {
            jobs.push("profile countdown");
        }
        jobs
    }

    fn dev_player_state_label(&self) -> &'static str {
        if self.active_radio_index.is_some() {
            "radio"
        } else if self.player.as_ref().map(AudioPlayer::is_playing).unwrap_or(false) {
            "music playing"
        } else if self.active_track_path.is_some() {
            "music selected"
        } else {
            "idle"
        }
    }


    pub(crate) fn render_dev_metrics_window(&mut self, context: &egui::Context) {
        let viewport_id = egui::ViewportId::from_hash_of("audio_orbit_dev_metrics_window");

        if !self.show_dev_metrics_window {
            return;
        }

        let close_requested_from_child = !self.dev_metrics_window_open_flag.load(Ordering::Relaxed);
        let builder = egui::ViewportBuilder::default()
            .with_title("Audio Orbit - Dev metrics")
            .with_inner_size([520.0, 640.0])
            .with_min_inner_size([360.0, 260.0]);

        if close_requested_from_child {
            // Keep the viewport registered for this frame while sending the close command.
            // Some native backends ignore a close command if the child viewport is no
            // longer scheduled, which leaves an empty native window behind.
            context.show_viewport_deferred(viewport_id, builder, move |context, _class| {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            });
            context.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
            self.show_dev_metrics_window = false;
            return;
        }

        self.dev_metrics_window_open_flag.store(true, Ordering::Relaxed);
        let open_flag = Arc::clone(&self.dev_metrics_window_open_flag);
        let snapshot = self.dev_metrics.snapshot.clone();
        let counters = self.dev_metrics_counters();

        context.show_viewport_deferred(viewport_id, builder, move |context, _class| {
            context.set_visuals(egui::Visuals::dark());

            if context.input(|input| input.viewport().close_requested()) {
                open_flag.store(false, Ordering::Relaxed);
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }

            let snapshot = snapshot.clone();
            let counters = counters.clone();
            egui::CentralPanel::default().show(context, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("dev_metrics_native_window_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        render_dev_metrics_panel_content(ui, &snapshot, &counters);
                    });
            });
        });
    }

    fn dev_metrics_counters(&self) -> DevMetricsCounters {
        DevMetricsCounters {
            playlists: self.state.playlists.len(),
            tracks: self.total_track_count(),
            radio_stations: self.state.radio_stations.len(),
            undo_stack: self.order_undo_stack.len(),
        }
    }

    fn total_track_count(&self) -> usize {
        self.state
            .playlists
            .iter()
            .map(|playlist| playlist.tracks.len())
            .sum()
    }

}

#[cfg(debug_assertions)]
fn render_dev_metrics_panel_content(ui: &mut egui::Ui, snapshot: &DevMetricsSnapshot, counters: &DevMetricsCounters) {
    AudioOrbitApp::render_modal_section(ui, |ui| {
        ui.heading("Dev runtime metrics");
        ui.small("Debug-only process metrics for checking CPU, memory, repaint cadence, and active background work while profiling Audio Orbit.");
    });
    ui.add_space(8.0);

    AudioOrbitApp::render_modal_section(ui, |ui| {
        ui.heading("Process");
        metric_row(ui, "CPU", snapshot.cpu_percent.map(|value| format!("{value:.1}%")).unwrap_or_else(|| "Waiting for sample".to_owned()));
        metric_row(ui, "RAM", snapshot.working_set_bytes.map(format_bytes).unwrap_or_else(|| "Unavailable".to_owned()));
        metric_row(ui, "Peak RAM", snapshot.peak_working_set_bytes.map(format_bytes).unwrap_or_else(|| "Unavailable".to_owned()));
        metric_row(ui, "Commit", snapshot.pagefile_bytes.map(format_bytes).unwrap_or_else(|| "Unavailable".to_owned()));
        metric_row(ui, "GPU", snapshot.gpu_usage_label.to_owned());
        ui.small("GPU usage is not polled directly here to avoid adding a high-overhead Windows performance-counter loop to normal profiling runs. Use Task Manager or GPUView for exact per-adapter GPU counters.");
    });
    ui.add_space(8.0);

    AudioOrbitApp::render_modal_section(ui, |ui| {
        ui.heading("UI / repaint");
        metric_row(ui, "Frame delta", format!("{:.1} ms", snapshot.frame_delta_ms));
        metric_row(ui, "Estimated FPS", format!("{:.1}", snapshot.estimated_fps));
        metric_row(ui, "Next repaint", format!("{} ms", snapshot.repaint_interval_ms));
        metric_row(ui, "App uptime", format_duration(snapshot.uptime_seconds));
        metric_row(ui, "Player state", snapshot.player_state.to_owned());
        let jobs = if snapshot.background_jobs.is_empty() {
            "none".to_owned()
        } else {
            snapshot.background_jobs.join(", ")
        };
        metric_row(ui, "Background work", jobs);
    });
    ui.add_space(8.0);

    AudioOrbitApp::render_modal_section(ui, |ui| {
        ui.heading("Counters");
        metric_row(ui, "Playlists", counters.playlists.to_string());
        metric_row(ui, "Tracks", counters.tracks.to_string());
        metric_row(ui, "Radio stations", counters.radio_stations.to_string());
        metric_row(ui, "Undo stack", counters.undo_stack.to_string());
    });
    ui.add_space(2.0);
}

#[cfg(debug_assertions)]
fn metric_row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.horizontal_wrapped(|ui| {
        ui.set_width(ui.available_width());
        ui.strong(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.monospace(value);
        });
    });
}

#[cfg(debug_assertions)]
fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

#[cfg(debug_assertions)]
fn duration_ms(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1000.0
}

#[cfg(all(debug_assertions, windows))]
fn collect_process_metrics_sample() -> Option<ProcessMetricsSample> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{GetCurrentProcess, GetProcessTimes},
        },
    };

    unsafe {
        let process = GetCurrentProcess();
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
            return None;
        }

        let mut memory: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(process, &mut memory, memory.cb) == 0 {
            return None;
        }

        Some(ProcessMetricsSample {
            process_time_100ns: filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)),
            working_set_bytes: memory.WorkingSetSize as u64,
            peak_working_set_bytes: memory.PeakWorkingSetSize as u64,
            pagefile_bytes: memory.PagefileUsage as u64,
        })
    }
}

#[cfg(all(debug_assertions, windows))]
fn filetime_to_u64(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

#[cfg(all(debug_assertions, not(windows)))]
fn collect_process_metrics_sample() -> Option<ProcessMetricsSample> {
    None
}
