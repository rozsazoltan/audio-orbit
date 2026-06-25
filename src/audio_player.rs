use crate::dsp::{render_orbit_to_stereo, DspSettings, RenderInfo};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{buffer::SamplesBuffer, Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

pub struct PlaybackInfo {
    pub path: PathBuf,
    pub duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub output_samples: usize,
}

pub struct AudioPlayer {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Option<Sink>,
    output_device_name: String,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let output_device_name = default_output_device_name();
        let (_stream, stream_handle) = OutputStream::try_default()
            .context("failed to open the default audio output device")?;

        Ok(Self {
            _stream,
            stream_handle,
            sink: None,
            output_device_name,
        })
    }

    pub fn output_device_name(&self) -> &str {
        &self.output_device_name
    }

    pub fn play_file_with_orbit(&mut self, path: &Path, settings: DspSettings) -> Result<PlaybackInfo> {
        self.stop();

        let file = File::open(path)
            .with_context(|| format!("failed to open audio file: {}", path.display()))?;
        let decoder = Decoder::new(BufReader::new(file))
            .with_context(|| format!("failed to decode audio file: {}", path.display()))?;

        let input_channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let input_samples: Vec<f32> = decoder.convert_samples().collect();

        if input_samples.is_empty() {
            anyhow::bail!("the selected audio file did not contain any decoded samples");
        }

        let (processed_samples, render_info) =
            render_orbit_to_stereo(&input_samples, input_channels, sample_rate, settings);

        self.play_processed_samples(processed_samples, sample_rate)?;

        Ok(playback_info(path, render_info))
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
    }

    pub fn pause_or_resume(&mut self) {
        if let Some(sink) = &self.sink {
            if sink.is_paused() {
                sink.play();
            } else {
                sink.pause();
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        self.sink
            .as_ref()
            .map(|sink| !sink.empty() && !sink.is_paused())
            .unwrap_or(false)
    }

    pub fn is_paused(&self) -> bool {
        self.sink
            .as_ref()
            .map(|sink| !sink.empty() && sink.is_paused())
            .unwrap_or(false)
    }

    pub fn has_finished(&self) -> bool {
        self.sink.as_ref().map(|sink| sink.empty()).unwrap_or(false)
    }

    fn play_processed_samples(&mut self, samples: Vec<f32>, sample_rate: u32) -> Result<()> {
        let sink = Sink::try_new(&self.stream_handle)
            .context("failed to create audio playback sink")?;
        let source = SamplesBuffer::new(2, sample_rate, samples);

        sink.append(source);
        sink.play();
        self.sink = Some(sink);

        Ok(())
    }
}

fn playback_info(path: &Path, render_info: RenderInfo) -> PlaybackInfo {
    PlaybackInfo {
        path: path.to_path_buf(),
        duration_seconds: render_info.duration_seconds,
        input_channels: render_info.input_channels,
        sample_rate: render_info.sample_rate,
        output_samples: render_info.output_samples,
    }
}

fn default_output_device_name() -> String {
    let host = cpal::default_host();

    host.default_output_device()
        .and_then(|device| device.name().ok())
        .unwrap_or_else(|| "Default output device".to_owned())
}
