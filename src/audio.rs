use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use rvoip_audio_device::{AudioDirection, DeviceBridge, DeviceOptions, RunningAudio};
use rvoip_sip::{AudioSender, EndpointCall};
use std::path::Path;

use crate::recording::Recording;

#[derive(Clone, Default)]
pub struct AudioSelection {
    pub input: Option<String>,
    pub output: Option<String>,
}

#[derive(Default)]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

impl DeviceList {
    pub fn scan() -> Self {
        Self {
            inputs: names(AudioDirection::Input),
            outputs: names(AudioDirection::Output),
        }
    }

    pub fn validate(&self, selection: &AudioSelection) -> Result<(), String> {
        for (device, available) in [
            (&selection.input, &self.inputs),
            (&selection.output, &self.outputs),
        ] {
            if device
                .as_ref()
                .is_some_and(|name| !available.contains(name))
            {
                return Err(
                    "The selected audio device is no longer available. Refresh the device list."
                        .into(),
                );
            }
        }
        Ok(())
    }
}

fn devices(direction: AudioDirection) -> Vec<(usize, String)> {
    let host = cpal::default_host();
    let devices = match direction {
        AudioDirection::Input => host.input_devices(),
        AudioDirection::Output => host.output_devices(),
    };
    match devices {
        Ok(devices) => devices
            .enumerate()
            .filter_map(|(index, device)| device.name().ok().map(|name| (index, name)))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn names(direction: AudioDirection) -> Vec<String> {
    let mut devices: Vec<_> = devices(direction)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    devices.sort_unstable();
    devices.dedup();
    devices
}

fn selector(direction: AudioDirection, name: &str) -> Result<String, String> {
    devices(direction)
        .into_iter()
        .find(|(_, device)| device == name)
        .map(|(index, _)| index.to_string())
        .ok_or_else(|| format!("The selected audio device is unavailable: {name}."))
}

pub struct CallAudio {
    _bridge: RunningAudio,
    heartbeat: Arc<AtomicU64>,
    sender: AudioSender,
    started: Instant,
    pub recording: Option<Recording>,
}

impl CallAudio {
    pub fn microphone_healthy(&self) -> bool {
        let last = self.heartbeat.load(Ordering::Relaxed);
        last != 0
            && self
                .started
                .elapsed()
                .as_millis()
                .saturating_sub(u128::from(last - 1))
                < Duration::from_secs(4).as_millis()
    }

    pub fn log_health(&self) {
        tracing::warn!(queue = ?self.sender.metrics(), microphone_last_ms = self.heartbeat.load(Ordering::Relaxed), "Call audio is not progressing");
    }

    pub async fn start(
        call: &EndpointCall,
        selected: &AudioSelection,
        muted: Arc<AtomicBool>,
        recordings: Option<&Path>,
    ) -> Result<(Self, Option<String>), String> {
        let selected = selected.clone();
        let (input, output) = tokio::task::spawn_blocking(move || {
            Ok::<_, String>((
                selected
                    .input
                    .as_deref()
                    .map(|name| selector(AudioDirection::Input, name))
                    .transpose()?,
                selected
                    .output
                    .as_deref()
                    .map(|name| selector(AudioDirection::Output, name))
                    .transpose()?,
            ))
        })
        .await
        .map_err(|_| "Could not find the selected audio devices.")??;
        let stream = call
            .as_session_handle()
            .audio()
            .await
            .map_err(|_| "Could not open the call audio stream.")?;
        let heartbeat = Arc::new(AtomicU64::new(0));
        let pulse = heartbeat.clone();
        let started = Instant::now();
        let mut options = DeviceOptions::new()
            .with_mute_flag(muted)
            .with_level_callback(move |level| {
                if level.direction == AudioDirection::Input {
                    pulse.store(started.elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
                }
            });
        if let Some(input) = input {
            options = options.with_input_device(input);
        }
        if let Some(output) = output {
            options = options.with_output_device(output);
        }
        let (recording, warning) = match recordings.map(Recording::start).transpose() {
            Ok(Some((recording, sink))) => {
                options = options.with_frame_callback(sink);
                (Some(recording), None)
            }
            Ok(None) => (None, None),
            Err(error) => (None, Some(error)),
        };
        let sender = stream.sender.clone();
        let bridge = DeviceBridge::start(stream, options)
            .map_err(|_| "Could not start the microphone and speaker.")?;
        Ok((
            Self {
                _bridge: bridge,
                heartbeat,
                sender,
                started,
                recording,
            },
            warning,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_selection_rejects_disconnected_input_and_output() {
        let devices = DeviceList {
            inputs: vec!["microphone".into()],
            outputs: vec!["speaker".into()],
        };
        assert!(devices.validate(&AudioSelection::default()).is_ok());
        assert!(
            devices
                .validate(&AudioSelection {
                    input: Some("microphone".into()),
                    output: Some("speaker".into())
                })
                .is_ok()
        );
        assert!(
            devices
                .validate(&AudioSelection {
                    input: Some("unplugged".into()),
                    output: None
                })
                .is_err()
        );
        assert!(
            devices
                .validate(&AudioSelection {
                    input: None,
                    output: Some("unplugged".into())
                })
                .is_err()
        );
    }
}
