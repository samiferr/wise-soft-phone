//! Incoming-call alert on the selected speaker. Device streams stay on their
//! own OS thread so opening a slow audio device never blocks SIP or the UI.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    f64::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub struct Ringer {
    stop: mpsc::Sender<()>,
    active: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    finished: tokio::sync::oneshot::Receiver<()>,
}

impl Ringer {
    pub fn start(output: Option<String>) -> Result<Self, String> {
        let (stop, stopped) = mpsc::channel();
        let (finished, completion) = tokio::sync::oneshot::channel();
        let active = Arc::new(AtomicBool::new(true));
        let failed = Arc::new(AtomicBool::new(false));
        let playing = active.clone();
        let error = failed.clone();
        std::thread::Builder::new()
            .name("softphone-ringer".into())
            .spawn(move || {
                if let Err(message) = ring(output, stopped, playing, error.clone()) {
                    error.store(true, Ordering::Release);
                    tracing::warn!(reason = %message, "Incoming ringtone could not start");
                }
                let _ = finished.send(());
            })
            .map_err(|_| "Could not start the incoming ringtone.".to_owned())?;
        Ok(Self {
            stop,
            active,
            failed,
            finished: completion,
        })
    }
    pub async fn stop(mut self) {
        self.active.store(false, Ordering::Release);
        let _ = self.stop.send(());
        // Release exclusive hardware output before the call bridge opens it.
        let _ =
            tokio::time::timeout(std::time::Duration::from_millis(500), &mut self.finished).await;
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}
impl Drop for Ringer {
    fn drop(&mut self) {
        // Silence callbacks immediately, including when device startup has
        // not finished yet, then ask its owner to close the stream.
        self.active.store(false, Ordering::Release);
        let _ = self.stop.send(());
    }
}

fn ring(
    output: Option<String>,
    stopped: mpsc::Receiver<()>,
    active: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) -> Result<(), String> {
    if !active.load(Ordering::Acquire) {
        return Ok(());
    }
    let host = cpal::default_host();
    let device = match output {
        Some(name) => host
            .output_devices()
            .map_err(|_| "Could not enumerate ringtone outputs.")?
            .find(|device| device.name().is_ok_and(|value| value == name)),
        None => host.default_output_device(),
    }
    .ok_or("The ringtone speaker is unavailable.")?;
    let supported = device
        .default_output_config()
        .map_err(|_| "The ringtone speaker has no default format.")?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let stream = match format {
        cpal::SampleFormat::F32 => stream::<f32>(&device, &config, active.clone(), failed),
        cpal::SampleFormat::F64 => stream::<f64>(&device, &config, active.clone(), failed),
        cpal::SampleFormat::I16 => stream::<i16>(&device, &config, active.clone(), failed),
        cpal::SampleFormat::U16 => stream::<u16>(&device, &config, active.clone(), failed),
        cpal::SampleFormat::I32 => stream::<i32>(&device, &config, active.clone(), failed),
        cpal::SampleFormat::U32 => stream::<u32>(&device, &config, active.clone(), failed),
        _ => return Err("The ringtone speaker uses an unsupported format.".into()),
    }?;
    if active.load(Ordering::Acquire) {
        stream
            .play()
            .map_err(|_| "Could not play the incoming ringtone.")?;
        tracing::debug!("Incoming ringtone started");
        let _ = stopped.recv();
    }
    drop(stream);
    tracing::debug!("Incoming ringtone stopped");
    Ok(())
}

fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    active: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let rate = config.sample_rate.0;
    let channels = usize::from(config.channels);
    let mut frame = 0u64;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                for samples in data.chunks_mut(channels.max(1)) {
                    let value = if active.load(Ordering::Acquire) {
                        tone(frame, rate)
                    } else {
                        0.0
                    };
                    frame = frame.wrapping_add(1);
                    for sample in samples {
                        *sample = T::from_sample(value);
                    }
                }
            },
            move |_| {
                failed.store(true, Ordering::Release);
                tracing::warn!("Incoming ringtone speaker stream failed");
            },
            None,
        )
        .map_err(|_| "Could not open the incoming ringtone stream.".into())
}

fn tone(frame: u64, rate: u32) -> f32 {
    let time = frame as f64 / f64::from(rate.max(1));
    let cycle = time % 6.0;
    if cycle >= 2.0 {
        return 0.0;
    }
    let envelope = (cycle / 0.01).min((2.0 - cycle) / 0.01).clamp(0.0, 1.0);
    (0.12 * envelope * ((TAU * 440.0 * time).sin() + (TAU * 480.0 * time).sin())) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ringtone_has_audible_burst_silent_interval_and_bounded_volume() {
        let rate = 8000;
        assert!((0..rate * 2).any(|frame| tone(frame as u64, rate).abs() > 0.05));
        assert!((rate * 2..rate * 6).all(|frame| tone(frame as u64, rate) == 0.0));
        assert!((rate * 6..rate * 8).any(|frame| tone(frame as u64, rate).abs() > 0.05));
        assert!((0..rate * 8).all(|frame| tone(frame as u64, rate).abs() <= 0.241));
        assert_eq!(tone(0, rate), 0.0);
    }
    #[test]
    fn dropping_ringer_silences_even_pending_device_startup() {
        let (stop, stopped) = mpsc::channel();
        let active = Arc::new(AtomicBool::new(true));
        let ringer = Ringer {
            stop,
            active: active.clone(),
            failed: Arc::new(AtomicBool::new(false)),
            finished: tokio::sync::oneshot::channel().1,
        };
        drop(ringer);
        assert!(!active.load(Ordering::Acquire));
        assert!(stopped.try_recv().is_ok());
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    #[tokio::test]
    async fn stop_waits_for_speaker_release_before_returning() {
        let (stop, stopped) = mpsc::channel();
        let (finished, completion) = tokio::sync::oneshot::channel();
        let active = Arc::new(AtomicBool::new(true));
        let ringer = Ringer {
            stop,
            active: active.clone(),
            failed: Arc::new(AtomicBool::new(false)),
            finished: completion,
        };
        let worker = std::thread::spawn(move || {
            stopped.recv().unwrap();
            finished.send(()).unwrap();
        });
        ringer.stop().await;
        worker.join().unwrap();
        assert!(!active.load(Ordering::Acquire));
    }

    #[tokio::test]
    #[ignore = "requires an explicitly selected test output, e.g. ALSA_CONFIG_PATH pointing at virtual ALSA"]
    async fn ringtone_device_stream_starts_and_stops() {
        let ringer = Ringer::start(None).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!ringer.failed(), "ringtone output could not start");
        ringer.stop().await;
    }
}
