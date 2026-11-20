use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use rvoip_audio_device::{AudioDirection, AudioFormat, DeviceBridge, DeviceOptions, RunningAudio};
use rvoip_sip::{AudioSender, EndpointCall, NegotiatedAudio};

/// How long to wait for SDP negotiation to be recorded after a call connects.
const NEGOTIATION_WAIT: Duration = Duration::from_secs(2);

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

/// Read the codec negotiated for a connected call. Frames sent to RVoIP must
/// match its rate and channel count exactly, so audio does not start without it.
pub async fn negotiated_media(call: &EndpointCall) -> Result<NegotiatedAudio, String> {
    let handle = call.as_session_handle();
    let deadline = Instant::now() + NEGOTIATION_WAIT;
    loop {
        match handle.negotiated_audio().await {
            Ok(Some(media)) => return Ok(media),
            Ok(None) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(None) | Err(_) => {
                return Err("Could not read the negotiated call audio codec.".into());
            }
        }
    }
}

/// PCM shape the device bridge must produce for `media`.
pub fn bridge_format(media: &NegotiatedAudio) -> AudioFormat {
    AudioFormat {
        sample_rate: media.sample_rate,
        channels: media.channels.clamp(1, 2),
    }
}

/// Short user-facing codec name.
pub fn codec_label(media: &NegotiatedAudio) -> String {
    let codec = media.codec.as_str();
    if codec.eq_ignore_ascii_case("opus") {
        "Opus wideband".into()
    } else if codec.eq_ignore_ascii_case("PCMU") {
        "G.711 µ-law".into()
    } else if codec.eq_ignore_ascii_case("PCMA") {
        "G.711 A-law".into()
    } else {
        codec.to_owned()
    }
}

pub struct CallAudio {
    _bridge: RunningAudio,
    heartbeat: Arc<AtomicU64>,
    sender: AudioSender,
    started: Instant,
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
        media: &NegotiatedAudio,
        selected: &AudioSelection,
        muted: Arc<AtomicBool>,
    ) -> Result<Self, String> {
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
            .with_format(bridge_format(media))
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
        let sender = stream.sender.clone();
        let bridge = DeviceBridge::start(stream, options)
            .map_err(|_| "Could not start the microphone and speaker.")?;
        Ok(Self {
            _bridge: bridge,
            heartbeat,
            sender,
            started,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(codec: &str, sample_rate: u32, channels: u8, payload_type: u8) -> NegotiatedAudio {
        NegotiatedAudio {
            codec: codec.into(),
            sample_rate,
            channels,
            payload_type,
        }
    }

    #[test]
    fn bridge_format_follows_negotiated_codec() {
        let opus = bridge_format(&media("opus", 48_000, 2, 111));
        assert_eq!((opus.sample_rate, opus.channels), (48_000, 2));
        assert_eq!(opus.frame_samples(), 960);
        let pcmu = bridge_format(&media("PCMU", 8_000, 1, 0));
        assert_eq!((pcmu.sample_rate, pcmu.channels), (8_000, 1));
        assert_eq!(pcmu.frame_samples(), 160);
        assert_eq!(bridge_format(&media("opus", 48_000, 0, 107)).channels, 1);
    }

    #[test]
    fn codec_labels_are_readable() {
        assert_eq!(codec_label(&media("opus", 48_000, 2, 107)), "Opus wideband");
        assert_eq!(codec_label(&media("PCMU", 8_000, 1, 0)), "G.711 µ-law");
        assert_eq!(codec_label(&media("PCMA", 8_000, 1, 8)), "G.711 A-law");
    }
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
