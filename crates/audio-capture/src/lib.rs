//! Audio capture via cpal. Supports regular inputs (mic / line-in) and
//! loopback capture of system audio: opening an input stream on an output
//! device transparently enables loopback on WASAPI (and macOS via an
//! aggregate device in cpal 0.18+).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("no supported config for device {0}")]
    NoConfig(String),
    #[error("cpal error: {0}")]
    Cpal(String),
}

pub type Result<T> = std::result::Result<T, CaptureError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// Output device captured via loopback (system audio).
    Loopback,
    /// Regular capture device (mic / line-in).
    Input,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioDeviceInfo {
    /// Stable id: "loopback:<name>" or "input:<name>".
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub is_default: bool,
}

pub fn list_devices() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let mut out = Vec::new();

    let default_out = host.default_output_device().map(|d| d.to_string());
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            let name = d.to_string();
            out.push(AudioDeviceInfo {
                id: format!("loopback:{name}"),
                is_default: Some(&name) == default_out.as_ref(),
                name,
                kind: DeviceKind::Loopback,
            });
        }
    }
    let default_in = host.default_input_device().map(|d| d.to_string());
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            let name = d.to_string();
            out.push(AudioDeviceInfo {
                id: format!("input:{name}"),
                is_default: Some(&name) == default_in.as_ref(),
                name,
                kind: DeviceKind::Input,
            });
        }
    }
    out
}

/// Running capture stream. Dropping stops capture.
pub struct CaptureHandle {
    _stream: cpal::Stream,
    pub sample_rate: u32,
}

/// Start capturing the device identified by `device_id` (see `list_devices`;
/// `None` = default loopback). `on_samples` receives mono f32 samples on the
/// audio thread — it must not block.
pub fn start_capture(
    device_id: Option<&str>,
    mut on_samples: impl FnMut(&[f32]) + Send + 'static,
) -> Result<CaptureHandle> {
    let host = cpal::default_host();

    let (kind, name) = match device_id {
        None => (DeviceKind::Loopback, None),
        Some(id) => match id.split_once(':') {
            Some(("loopback", n)) => (DeviceKind::Loopback, Some(n.to_string())),
            Some(("input", n)) => (DeviceKind::Input, Some(n.to_string())),
            _ => (DeviceKind::Loopback, Some(id.to_string())),
        },
    };

    let device = match (kind, &name) {
        (DeviceKind::Loopback, None) => host.default_output_device(),
        (DeviceKind::Loopback, Some(n)) => host
            .output_devices()
            .map_err(|e| CaptureError::Cpal(e.to_string()))?
            .find(|d| &d.to_string() == n),
        (DeviceKind::Input, Some(n)) => host
            .input_devices()
            .map_err(|e| CaptureError::Cpal(e.to_string()))?
            .find(|d| &d.to_string() == n),
        (DeviceKind::Input, None) => host.default_input_device(),
    }
    .ok_or_else(|| CaptureError::DeviceNotFound(name.clone().unwrap_or_default()))?;

    let device_name = device.to_string();
    // For loopback devices the *output* config describes the format we
    // will receive on the input stream.
    let config = match kind {
        DeviceKind::Loopback => device.default_output_config(),
        DeviceKind::Input => device.default_input_config(),
    }
    .map_err(|_| CaptureError::NoConfig(device_name.clone()))?;

    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    tracing::info!(
        device = %device_name,
        ?kind,
        sample_rate,
        channels,
        ?sample_format,
        "starting audio capture"
    );

    let err_fn = |e| tracing::error!("audio stream error: {e}");
    let mut mono = Vec::<f32>::with_capacity(4096);

    macro_rules! build {
        ($t:ty, $to_f32:expr) => {
            device
                .build_input_stream(
                    stream_config,
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        mono.clear();
                        for frame in data.chunks_exact(channels) {
                            let sum: f32 = frame.iter().map(|&s| ($to_f32)(s)).sum();
                            mono.push(sum / channels as f32);
                        }
                        on_samples(&mono);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| CaptureError::Cpal(e.to_string()))?
        };
    }

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build!(f32, |s: f32| s),
        cpal::SampleFormat::I16 => build!(i16, |s: i16| s as f32 / 32768.0),
        cpal::SampleFormat::U16 => build!(u16, |s: u16| (s as f32 - 32768.0) / 32768.0),
        cpal::SampleFormat::I32 => build!(i32, |s: i32| s as f32 / 2147483648.0),
        other => {
            return Err(CaptureError::Cpal(format!(
                "unsupported sample format {other:?}"
            )))
        }
    };
    stream.play().map_err(|e| CaptureError::Cpal(e.to_string()))?;

    Ok(CaptureHandle {
        _stream: stream,
        sample_rate,
    })
}
