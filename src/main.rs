#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;

use crate::audio::{AudioController, VolumeIntensity};
use eframe::egui;
use std::f32::consts::PI;
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_ORBIT_RATE_HZ: f32 = 0.65;
const ORBIT_UPDATES_PER_SECOND: f32 = 60.0;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 360.0])
            .with_min_inner_size([460.0, 320.0])
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        &format!("Audio Orbit v{}", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(|_creation_context| Ok(Box::new(BalanceApp::new()))),
    )
}

struct BalanceApp {
    controller: Option<AudioController>,
    interface_name: String,
    error_message: Option<String>,
    left_percent: u8,
    right_percent: u8,
    max_percent: u8,
    orbit_depth_percent: u8,
    orbit_speed_percent: u8,
    is_orbit_enabled: bool,
    worker_running: Arc<AtomicBool>,
    worker_max_percent: Arc<AtomicU8>,
    worker_depth_percent: Arc<AtomicU8>,
    worker_speed_percent: Arc<AtomicU8>,
    worker_handle: Option<JoinHandle<()>>,
}

impl BalanceApp {
    fn new() -> Self {
        let worker_running = Arc::new(AtomicBool::new(false));
        let worker_max_percent = Arc::new(AtomicU8::new(100));
        let worker_depth_percent = Arc::new(AtomicU8::new(100));
        let worker_speed_percent = Arc::new(AtomicU8::new(100));

        match AudioController::new() {
            Ok(controller) => {
                let balance = controller
                    .get_balance()
                    .unwrap_or_else(|_| VolumeIntensity::new(100, 100));
                let interface_name = controller.interface_name();

                Self {
                    controller: Some(controller),
                    interface_name,
                    error_message: None,
                    left_percent: balance.left_percent,
                    right_percent: balance.right_percent,
                    max_percent: 100,
                    orbit_depth_percent: 100,
                    orbit_speed_percent: 100,
                    is_orbit_enabled: false,
                    worker_running,
                    worker_max_percent,
                    worker_depth_percent,
                    worker_speed_percent,
                    worker_handle: None,
                }
            }
            Err(error) => Self {
                controller: None,
                interface_name: "No supported audio endpoint".to_owned(),
                error_message: Some(error.to_string()),
                left_percent: 100,
                right_percent: 100,
                max_percent: 100,
                orbit_depth_percent: 100,
                orbit_speed_percent: 100,
                is_orbit_enabled: false,
                worker_running,
                worker_max_percent,
                worker_depth_percent,
                worker_speed_percent,
                worker_handle: None,
            },
        }
    }

    fn apply_manual_balance(&mut self) {
        if self.is_orbit_enabled {
            return;
        }

        if let Some(controller) = &self.controller {
            if let Err(error) = controller.set_balance(VolumeIntensity::new(
                self.left_percent,
                self.right_percent,
            )) {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn start_orbit(&mut self) {
        if self.controller.is_none() || self.worker_running.load(Ordering::SeqCst) {
            return;
        }

        self.worker_running.store(true, Ordering::SeqCst);
        self.worker_max_percent
            .store(self.max_percent, Ordering::SeqCst);
        self.worker_depth_percent
            .store(self.orbit_depth_percent, Ordering::SeqCst);
        self.worker_speed_percent
            .store(self.orbit_speed_percent, Ordering::SeqCst);

        let worker_running = Arc::clone(&self.worker_running);
        let worker_max_percent = Arc::clone(&self.worker_max_percent);
        let worker_depth_percent = Arc::clone(&self.worker_depth_percent);
        let worker_speed_percent = Arc::clone(&self.worker_speed_percent);

        self.worker_handle = Some(thread::spawn(move || {
            let Ok(controller) = AudioController::new() else {
                worker_running.store(false, Ordering::SeqCst);
                return;
            };

            let interval = Duration::from_secs_f32(1.0 / ORBIT_UPDATES_PER_SECOND);
            let mut elapsed = 0.0_f32;

            while worker_running.load(Ordering::SeqCst) {
                let max_percent = worker_max_percent.load(Ordering::SeqCst);
                let depth = worker_depth_percent.load(Ordering::SeqCst) as f32 / 100.0;
                let speed = (worker_speed_percent.load(Ordering::SeqCst) as f32 / 100.0).max(0.05);
                let pan = (2.0 * PI * DEFAULT_ORBIT_RATE_HZ * speed * elapsed).sin() * depth;

                let _ = controller.set_balance(balance_style_pan(pan, max_percent));

                elapsed += interval.as_secs_f32();
                thread::sleep(interval);
            }
        }));

        self.is_orbit_enabled = true;
    }

    fn stop_orbit(&mut self) {
        self.worker_running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }

        self.is_orbit_enabled = false;
    }
}

impl Drop for BalanceApp {
    fn drop(&mut self) {
        self.stop_orbit();
    }
}

impl eframe::App for BalanceApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("Audio Orbit");
            ui.label("Windows left/right balance controller with stronger stereo orbit panning.");
            ui.label("Best tested with headphones and stereo output devices.");
            ui.separator();

            ui.horizontal(|ui| {
                ui.strong("Interface:");
                ui.label(&self.interface_name);
            });

            if let Some(error_message) = &self.error_message {
                ui.colored_label(egui::Color32::RED, error_message);
            }

            ui.add_space(8.0);

            ui.add_enabled_ui(!self.is_orbit_enabled && self.controller.is_some(), |ui| {
                let left_changed = ui
                    .add(egui::Slider::new(&mut self.left_percent, 0u8..=100u8).text("Left (%)"))
                    .changed();
                let right_changed = ui
                    .add(egui::Slider::new(&mut self.right_percent, 0u8..=100u8).text("Right (%)"))
                    .changed();

                if left_changed || right_changed {
                    self.apply_manual_balance();
                }
            });

            ui.separator();
            ui.add_enabled_ui(self.controller.is_some(), |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut self.max_percent, 0u8..=100u8)
                            .text("Orbit Max Volume (%)"),
                    )
                    .changed()
                {
                    self.worker_max_percent
                        .store(self.max_percent, Ordering::SeqCst);
                }

                if ui
                    .add(
                        egui::Slider::new(&mut self.orbit_depth_percent, 0u8..=100u8)
                            .text("Orbit Strength (%)"),
                    )
                    .changed()
                {
                    self.worker_depth_percent
                        .store(self.orbit_depth_percent, Ordering::SeqCst);
                }

                if ui
                    .add(
                        egui::Slider::new(&mut self.orbit_speed_percent, 10u8..=200u8)
                            .text("Orbit Speed (%)"),
                    )
                    .changed()
                {
                    self.worker_speed_percent
                        .store(self.orbit_speed_percent, Ordering::SeqCst);
                }
            });

            ui.add_space(12.0);

            ui.horizontal(|ui| {
                let button_text = if self.is_orbit_enabled {
                    "Disable Orbit Mode"
                } else {
                    "Enable Orbit Mode"
                };

                if ui
                    .add_enabled(self.controller.is_some(), egui::Button::new(button_text))
                    .clicked()
                {
                    if self.is_orbit_enabled {
                        self.stop_orbit();
                    } else {
                        self.start_orbit();
                    }
                }

                let status = if self.is_orbit_enabled { "Orbit ON" } else { "Orbit OFF" };
                ui.label(status);
            });
        });
    }
}

fn balance_style_pan(pan: f32, max_percent: u8) -> VolumeIntensity {
    let pan = pan.clamp(-1.0, 1.0);
    let max = max_percent.min(100) as f32;

    let (left, right) = if pan < 0.0 {
        (max, max * (1.0 + pan))
    } else {
        (max * (1.0 - pan), max)
    };

    VolumeIntensity::new(left.round() as u8, right.round() as u8)
}
