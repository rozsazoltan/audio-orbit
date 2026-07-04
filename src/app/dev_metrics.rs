use crate::*;

#[cfg(debug_assertions)]
use std::{
    process::Child,
};

#[cfg(debug_assertions)]
const DEV_METRICS_PROCESS_ARG: &str = "--audio-orbit-dev-metrics";

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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DevMetricsSnapshot {
    pub(crate) cpu_percent: Option<f32>,
    pub(crate) working_set_bytes: Option<u64>,
    pub(crate) peak_working_set_bytes: Option<u64>,
    pub(crate) pagefile_bytes: Option<u64>,
    pub(crate) frame_delta_ms: f32,
    pub(crate) estimated_fps: f32,
    pub(crate) repaint_interval_ms: u64,
    pub(crate) uptime_seconds: f32,
    pub(crate) background_jobs: Vec<String>,
    pub(crate) player_state: String,
    pub(crate) gpu_usage_label: String,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DevMetricsCounters {
    pub(crate) playlists: usize,
    pub(crate) tracks: usize,
    pub(crate) radio_stations: usize,
    pub(crate) undo_stack: usize,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DevMetricsNativeWindowData {
    snapshot: DevMetricsSnapshot,
    counters: DevMetricsCounters,
}

#[cfg(debug_assertions)]
pub(crate) struct DevMetricsNativeWindowHandle {
    child: Child,
    snapshot_path: PathBuf,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub(crate) struct DevMetricsProcessConfig {
    target_pid: u32,
    snapshot_path: PathBuf,
}

#[cfg(debug_assertions)]
impl DevMetricsNativeWindowHandle {
    fn spawn() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("failed to resolve Audio Orbit executable: {error}"))?;
        let snapshot_path = std::env::temp_dir().join(format!(
            "audio-orbit-dev-metrics-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&snapshot_path);

        let child = Command::new(executable)
            .arg(DEV_METRICS_PROCESS_ARG)
            .arg(std::process::id().to_string())
            .arg(&snapshot_path)
            .spawn()
            .map_err(|error| format!("failed to open Dev metrics window: {error}"))?;

        Ok(Self { child, snapshot_path })
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn update(&self, snapshot: DevMetricsSnapshot, counters: DevMetricsCounters) {
        let data = DevMetricsNativeWindowData { snapshot, counters };
        if let Ok(json) = serde_json::to_vec(&data) {
            let _ = fs::write(&self.snapshot_path, json);
        }
    }

    pub(crate) fn close(&mut self) {
        if self.is_running() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = fs::remove_file(&self.snapshot_path);
    }
}

#[cfg(debug_assertions)]
pub(crate) fn dev_metrics_process_config_from_args() -> Option<DevMetricsProcessConfig> {
    let mut args = std::env::args_os();
    let _program = args.next()?;
    let mode = args.next()?;
    if mode != DEV_METRICS_PROCESS_ARG {
        return None;
    }

    let target_pid = args.next()?.to_string_lossy().parse::<u32>().ok()?;
    let snapshot_path = PathBuf::from(args.next()?);
    Some(DevMetricsProcessConfig {
        target_pid,
        snapshot_path,
    })
}

#[cfg(debug_assertions)]
pub(crate) fn run_dev_metrics_process(config: DevMetricsProcessConfig) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Audio Orbit - Dev metrics")
        .with_inner_size([520.0, 640.0])
        .with_min_inner_size([360.0, 260.0])
        .with_resizable(true);

    if let Some(icon) = icon::load_window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Audio Orbit - Dev metrics",
        options,
        Box::new(move |creation_context| {
            ui_icons::install(&creation_context.egui_ctx);
            configure_app_style(&creation_context.egui_ctx);
            Ok(Box::new(DevMetricsStandaloneApp::new(config)))
        }),
    )
}

#[cfg(debug_assertions)]
struct DevMetricsStandaloneApp {
    config: DevMetricsProcessConfig,
    snapshot: DevMetricsSnapshot,
    counters: DevMetricsCounters,
    last_snapshot_read_at: Instant,
    last_sample: Option<ProcessMetricsSample>,
    last_sample_at: Option<Instant>,
    logical_processors: usize,
}

#[cfg(debug_assertions)]
impl DevMetricsStandaloneApp {
    fn new(config: DevMetricsProcessConfig) -> Self {
        Self {
            config,
            snapshot: DevMetricsSnapshot::default(),
            counters: DevMetricsCounters::default(),
            last_snapshot_read_at: Instant::now() - Duration::from_secs(1),
            last_sample: None,
            last_sample_at: None,
            logical_processors: std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1)
                .max(1),
        }
    }

    fn refresh(&mut self, _context: &egui::Context) {
        let now = Instant::now();

        if now.saturating_duration_since(self.last_snapshot_read_at) >= Duration::from_millis(250) {
            self.last_snapshot_read_at = now;
            if let Ok(bytes) = fs::read(&self.config.snapshot_path) {
                if let Ok(data) = serde_json::from_slice::<DevMetricsNativeWindowData>(&bytes) {
                    let cpu_percent = self.snapshot.cpu_percent;
                    let working_set_bytes = self.snapshot.working_set_bytes;
                    let peak_working_set_bytes = self.snapshot.peak_working_set_bytes;
                    let pagefile_bytes = self.snapshot.pagefile_bytes;
                    self.snapshot = data.snapshot;
                    self.snapshot.cpu_percent = cpu_percent;
                    self.snapshot.working_set_bytes = working_set_bytes;
                    self.snapshot.peak_working_set_bytes = peak_working_set_bytes;
                    self.snapshot.pagefile_bytes = pagefile_bytes;
                    self.counters = data.counters;
                }
            }
        }

        if let Some(sample) = collect_process_metrics_sample_for_pid(self.config.target_pid) {
            if let (Some(previous), Some(previous_at)) = (self.last_sample, self.last_sample_at) {
                let elapsed = now.saturating_duration_since(previous_at).as_secs_f64();
                if elapsed > 0.0 {
                    let process_seconds = sample
                        .process_time_100ns
                        .saturating_sub(previous.process_time_100ns) as f64
                        / 10_000_000.0;
                    let normalized = process_seconds / elapsed / self.logical_processors as f64 * 100.0;
                    self.snapshot.cpu_percent = Some(normalized.clamp(0.0, 100.0) as f32);
                }
            }
            self.snapshot.working_set_bytes = Some(sample.working_set_bytes);
            self.snapshot.peak_working_set_bytes = Some(sample.peak_working_set_bytes);
            self.snapshot.pagefile_bytes = Some(sample.pagefile_bytes);
            self.last_sample = Some(sample);
            self.last_sample_at = Some(now);
        } else {
            self.snapshot.cpu_percent = None;
            self.snapshot.working_set_bytes = None;
            self.snapshot.peak_working_set_bytes = None;
            self.snapshot.pagefile_bytes = None;
        }

    }
}

#[cfg(debug_assertions)]
impl eframe::App for DevMetricsStandaloneApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        context.set_visuals(egui::Visuals::dark());
        context.request_repaint_after(Duration::from_millis(500));

        if context.input(|input| input.viewport().close_requested()) {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.refresh(context);
        egui::CentralPanel::default().show(context, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("dev_metrics_process_window_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    render_dev_metrics_panel_content(ui, &self.snapshot, &self.counters);
                });
        });
    }
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
            player_state: "idle".to_owned(),
            gpu_usage_label: "not sampled".to_owned(),
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
        self.dev_metrics.snapshot.player_state = self.dev_player_state_label().to_owned();
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

    fn active_dev_background_jobs(&self) -> Vec<String> {
        let mut jobs = Vec::new();
        if self.pending_prepared_track_receiver.is_some() {
            jobs.push("audio prepare".to_owned());
        }
        if self.pending_fast_seek.is_some() {
            jobs.push("seek restart debounce".to_owned());
        }
        if self.pending_seek_prepare.is_some() {
            jobs.push("seek prepare debounce".to_owned());
        }
        if self.pending_folder_scan_receiver.is_some() {
            jobs.push("folder scan".to_owned());
        }
        if self.pending_track_switch.is_some() {
            jobs.push("crossfade switch".to_owned());
        }
        if self.update_check_receiver.is_some() {
            jobs.push("update check".to_owned());
        }
        if self.update_install_receiver.is_some() {
            jobs.push("update install".to_owned());
        }
        if self.radio_title_receiver.is_some() {
            jobs.push("radio metadata".to_owned());
        }
        if self.pending_profile_apply_at.is_some() {
            jobs.push("profile countdown".to_owned());
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


    pub(crate) fn render_dev_metrics_window(&mut self, _context: &egui::Context) {
        if !self.show_dev_metrics_window {
            return;
        }

        if let Some(handle) = &mut self.dev_metrics_window {
            if !handle.is_running() {
                handle.close();
                self.dev_metrics_window = None;
                self.show_dev_metrics_window = false;
                return;
            }
        }

        if self.dev_metrics_window.is_none() {
            match DevMetricsNativeWindowHandle::spawn() {
                Ok(handle) => self.dev_metrics_window = Some(handle),
                Err(error) => {
                    self.show_dev_metrics_window = false;
                    self.error_message = Some(error);
                    self.error_updated_at = Instant::now();
                    return;
                }
            }
        }

        let snapshot = self.dev_metrics.snapshot.clone();
        let counters = self.dev_metrics_counters();
        if let Some(handle) = &self.dev_metrics_window {
            handle.update(snapshot, counters);
        }
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
        metric_row(ui, "GPU", snapshot.gpu_usage_label.clone());
        ui.small("GPU usage is not polled directly here to avoid adding a high-overhead Windows performance-counter loop to normal profiling runs. Use Task Manager or GPUView for exact per-adapter GPU counters.");
    });
    ui.add_space(8.0);

    AudioOrbitApp::render_modal_section(ui, |ui| {
        ui.heading("UI / repaint");
        metric_row(ui, "Frame delta", format!("{:.1} ms", snapshot.frame_delta_ms));
        metric_row(ui, "Estimated FPS", format!("{:.1}", snapshot.estimated_fps));
        metric_row(ui, "Next repaint", format!("{} ms", snapshot.repaint_interval_ms));
        metric_row(ui, "App uptime", format_duration(snapshot.uptime_seconds));
        metric_row(ui, "Player state", snapshot.player_state.clone());
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
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    unsafe { collect_process_metrics_sample_for_handle(GetCurrentProcess()) }
}

#[cfg(all(debug_assertions, windows))]
fn collect_process_metrics_sample_for_pid(pid: u32) -> Option<ProcessMetricsSample> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::OpenProcess,
    };

    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;

    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if process.is_null() {
            return None;
        }
        let sample = collect_process_metrics_sample_for_handle(process);
        CloseHandle(process);
        sample
    }
}

#[cfg(all(debug_assertions, windows))]
unsafe fn collect_process_metrics_sample_for_handle(process: windows_sys::Win32::Foundation::HANDLE) -> Option<ProcessMetricsSample> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::GetProcessTimes,
        },
    };

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

#[cfg(all(debug_assertions, windows))]
fn filetime_to_u64(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

#[cfg(all(debug_assertions, not(windows)))]
fn collect_process_metrics_sample() -> Option<ProcessMetricsSample> {
    None
}

#[cfg(all(debug_assertions, not(windows)))]
fn collect_process_metrics_sample_for_pid(_pid: u32) -> Option<ProcessMetricsSample> {
    None
}
