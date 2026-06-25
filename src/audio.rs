#[derive(Clone, Copy, Debug)]
pub struct VolumeIntensity {
    pub left_percent: u8,
    pub right_percent: u8,
}

impl VolumeIntensity {
    pub fn new(left_percent: u8, right_percent: u8) -> Self {
        Self {
            left_percent: left_percent.min(100),
            right_percent: right_percent.min(100),
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::VolumeIntensity;
    use anyhow::{Context, Result};
    use std::ptr::null;
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };

    pub struct AudioController {
        endpoint_volume: IAudioEndpointVolume,
        channel_count: u32,
        _com: ComApartment,
    }

    struct ComApartment;

    impl ComApartment {
        fn init() -> Result<Self> {
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .context("failed to initialize COM for Windows Core Audio")?;
            }

            Ok(Self)
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }

    impl AudioController {
        pub fn new() -> Result<Self> {
            let com = ComApartment::init()?;

            let device_enumerator: IMMDeviceEnumerator = unsafe {
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("failed to create Windows audio device enumerator")?
            };

            let device = unsafe {
                device_enumerator
                    .GetDefaultAudioEndpoint(eRender, eConsole)
                    .context("failed to open the default render audio endpoint")?
            };

            let endpoint_volume: IAudioEndpointVolume = unsafe {
                device
                    .Activate(CLSCTX_ALL, None)
                    .context("failed to activate endpoint volume control")?
            };

            let channel_count = unsafe {
                endpoint_volume
                    .GetChannelCount()
                    .context("failed to read audio channel count")?
            };

            Ok(Self {
                endpoint_volume,
                channel_count,
                _com: com,
            })
        }

        pub fn set_balance(&self, intensity: VolumeIntensity) -> Result<()> {
            let left_scalar = intensity.left_percent as f32 / 100.0;
            let right_scalar = intensity.right_percent as f32 / 100.0;

            unsafe {
                if self.channel_count >= 2 {
                    self.endpoint_volume
                        .SetChannelVolumeLevelScalar(0, left_scalar, null())
                        .context("failed to set left channel volume")?;
                    self.endpoint_volume
                        .SetChannelVolumeLevelScalar(1, right_scalar, null())
                        .context("failed to set right channel volume")?;
                } else {
                    let mono_scalar = (left_scalar + right_scalar) / 2.0;
                    self.endpoint_volume
                        .SetMasterVolumeLevelScalar(mono_scalar, null())
                        .context("failed to set mono endpoint volume")?;
                }
            }

            Ok(())
        }

        pub fn get_balance(&self) -> Result<VolumeIntensity> {
            unsafe {
                if self.channel_count >= 2 {
                    let left = self
                        .endpoint_volume
                        .GetChannelVolumeLevelScalar(0)
                        .context("failed to read left channel volume")?;
                    let right = self
                        .endpoint_volume
                        .GetChannelVolumeLevelScalar(1)
                        .context("failed to read right channel volume")?;

                    Ok(VolumeIntensity::new(
                        scalar_to_percent(left),
                        scalar_to_percent(right),
                    ))
                } else {
                    let master = self
                        .endpoint_volume
                        .GetMasterVolumeLevelScalar()
                        .context("failed to read mono endpoint volume")?;
                    let percent = scalar_to_percent(master);
                    Ok(VolumeIntensity::new(percent, percent))
                }
            }
        }

        pub fn interface_name(&self) -> String {
            "Default Windows audio endpoint".to_owned()
        }
    }

    fn scalar_to_percent(value: f32) -> u8 {
        (value.clamp(0.0, 1.0) * 100.0).round() as u8
    }
}

#[cfg(not(windows))]
mod platform {
    use super::VolumeIntensity;
    use anyhow::{bail, Result};

    pub struct AudioController;

    impl AudioController {
        pub fn new() -> Result<Self> {
            bail!("Audio Orbit audio control is only supported on Windows")
        }

        pub fn set_balance(&self, _intensity: VolumeIntensity) -> Result<()> {
            bail!("Audio Orbit audio control is only supported on Windows")
        }

        pub fn get_balance(&self) -> Result<VolumeIntensity> {
            bail!("Audio Orbit audio control is only supported on Windows")
        }

        pub fn interface_name(&self) -> String {
            "Unsupported platform".to_owned()
        }
    }
}

pub use platform::AudioController;
