//! Call-recording playback on the selected speaker. Like the ringer, the
//! device stream lives on its own OS thread so opening or decoding never
//! blocks the UI or SIP tasks.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering},
        mpsc,
    },
};

const LOADING: u8 = 0;
const PLAYING: u8 = 1;
const FINISHED: u8 = 2;
const FAILED: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerState {
    Loading,
    Playing,
    Paused,
    Finished,
    Failed,
}

#[derive(Default)]
struct Shared {
    state: AtomicU8,
    paused: AtomicBool,
    /// Sample rate of the recording, set once it is loaded.
    rate: AtomicU32,
    /// Playback position and length in recording frames.
    position: AtomicUsize,
    total: AtomicUsize,
    /// Set by `seek` and consumed by the audio callback.
    seek: AtomicUsize,
    seeking: AtomicBool,
}

pub struct Player {
    shared: Arc<Shared>,
    stop: mpsc::Sender<()>,
}

impl Player {
    pub fn start(path: PathBuf, output: Option<String>) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let (stop, stopped) = mpsc::channel();
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("softphone-player".into())
            .spawn(move || {
                if let Err(message) = play(&path, output, &worker, stopped) {
                    worker.state.store(FAILED, Ordering::Release);
                    tracing::warn!(reason = %message, "Recording playback failed");
                }
            })
            .map_err(|_| "Could not start recording playback.".to_owned())?;
        Ok(Self { shared, stop })
    }

    pub fn state(&self) -> PlayerState {
        match self.shared.state.load(Ordering::Acquire) {
            LOADING => PlayerState::Loading,
            PLAYING if self.shared.paused.load(Ordering::Acquire) => PlayerState::Paused,
            PLAYING => PlayerState::Playing,
            FINISHED => PlayerState::Finished,
            _ => PlayerState::Failed,
        }
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Release);
    }

    pub fn resume(&self) {
        // A finished recording plays again from the beginning.
        if self.state() == PlayerState::Finished {
            self.seek(0.);
        }
        self.shared.paused.store(false, Ordering::Release);
    }

    fn rate(&self) -> usize {
        self.shared.rate.load(Ordering::Acquire).max(1) as usize
    }

    pub fn position_seconds(&self) -> u64 {
        (self.shared.position.load(Ordering::Acquire) / self.rate()) as u64
    }

    pub fn total_seconds(&self) -> u64 {
        (self.shared.total.load(Ordering::Acquire) / self.rate()) as u64
    }

    /// Fraction played, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let total = self.shared.total.load(Ordering::Acquire);
        if total == 0 {
            return 0.;
        }
        (self.shared.position.load(Ordering::Acquire) as f32 / total as f32).clamp(0., 1.)
    }

    /// Jumps to a fraction of the recording, from 0 to 1.
    pub fn seek(&self, fraction: f32) {
        let total = self.shared.total.load(Ordering::Acquire);
        let target = (fraction.clamp(0., 1.) * total as f32) as usize;
        self.shared.seek.store(target.min(total), Ordering::Release);
        self.shared.seeking.store(true, Ordering::Release);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Silence the callback immediately; the thread closes the device.
        self.shared.paused.store(true, Ordering::Release);
        let _ = self.stop.send(());
    }
}

/// Mixes both call sides to mono so one speaker plays the whole conversation.
fn load(path: &PathBuf) -> Result<(Vec<i16>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|_| "Could not read the recording.")?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err("The recording format is not supported.".into());
    }
    let channels = usize::from(spec.channels.max(1));
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Could not read the recording.")?;
    Ok((downmix(&samples, channels), spec.sample_rate))
}

fn downmix(samples: &[i16], channels: usize) -> Vec<i16> {
    samples
        .chunks_exact(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&sample| i32::from(sample)).sum();
            (sum / channels as i32) as i16
        })
        .collect()
}

fn play(
    path: &PathBuf,
    output: Option<String>,
    shared: &Arc<Shared>,
    stopped: mpsc::Receiver<()>,
) -> Result<(), String> {
    let (audio, source_rate) = load(path)?;
    if audio.is_empty() {
        return Err("The recording is empty.".into());
    }
    shared.rate.store(source_rate, Ordering::Release);
    shared.total.store(audio.len(), Ordering::Release);
    if stopped.try_recv().is_ok() {
        return Ok(());
    }
    let host = cpal::default_host();
    let device = match output {
        Some(name) => host
            .output_devices()
            .map_err(|_| "Could not enumerate playback outputs.")?
            .find(|device| device.name().is_ok_and(|value| value == name)),
        None => host.default_output_device(),
    }
    .ok_or("The playback speaker is unavailable.")?;
    let supported = device
        .default_output_config()
        .map_err(|_| "The playback speaker has no default format.")?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let audio = Arc::new(audio);
    let stream = match format {
        cpal::SampleFormat::F32 => stream::<f32>(&device, &config, audio, source_rate, shared),
        cpal::SampleFormat::F64 => stream::<f64>(&device, &config, audio, source_rate, shared),
        cpal::SampleFormat::I16 => stream::<i16>(&device, &config, audio, source_rate, shared),
        cpal::SampleFormat::U16 => stream::<u16>(&device, &config, audio, source_rate, shared),
        cpal::SampleFormat::I32 => stream::<i32>(&device, &config, audio, source_rate, shared),
        cpal::SampleFormat::U32 => stream::<u32>(&device, &config, audio, source_rate, shared),
        _ => return Err("The playback speaker uses an unsupported format.".into()),
    }?;
    stream.play().map_err(|_| "Could not play the recording.")?;
    shared.state.store(PLAYING, Ordering::Release);
    let _ = stopped.recv();
    drop(stream);
    Ok(())
}

fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    audio: Arc<Vec<i16>>,
    source_rate: u32,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, String> {
    let channels = usize::from(config.channels).max(1);
    let step = f64::from(source_rate) / f64::from(config.sample_rate.0.max(1));
    let mut cursor = shared.position.load(Ordering::Acquire) as f64;
    let data = shared.clone();
    let failed = shared.clone();
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _| {
                if data.seeking.swap(false, Ordering::AcqRel) {
                    cursor = data.seek.load(Ordering::Acquire) as f64;
                    if data.state.load(Ordering::Acquire) == FINISHED {
                        data.state.store(PLAYING, Ordering::Release);
                    }
                }
                for frame in out.chunks_mut(channels) {
                    let value = if data.paused.load(Ordering::Acquire)
                        || data.state.load(Ordering::Acquire) != PLAYING
                    {
                        0.0
                    } else {
                        let (value, done) = sample_at(&audio, cursor);
                        if done {
                            data.state.store(FINISHED, Ordering::Release);
                            data.position.store(audio.len(), Ordering::Release);
                        } else {
                            cursor += step;
                            data.position.store(cursor as usize, Ordering::Release);
                        }
                        value
                    };
                    for sample in frame {
                        *sample = T::from_sample(value);
                    }
                }
            },
            move |_| {
                failed.state.store(FAILED, Ordering::Release);
                tracing::warn!("Recording playback stream failed");
            },
            None,
        )
        .map_err(|_| "Could not open the playback stream.".into())
}

/// Linearly interpolated sample at `cursor`, and whether playback has ended.
fn sample_at(audio: &[i16], cursor: f64) -> (f32, bool) {
    let index = cursor as usize;
    if index >= audio.len() {
        return (0.0, true);
    }
    let fraction = (cursor - index as f64) as f32;
    let current = f32::from(audio[index]);
    let next = audio
        .get(index + 1)
        .map_or(current, |&sample| f32::from(sample));
    (
        (current + (next - current) * fraction) / f32::from(i16::MAX),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_recordings_are_mixed_to_mono() {
        assert_eq!(downmix(&[100, 300, -50, -150, 7, 7], 2), vec![200, -100, 7]);
        assert_eq!(downmix(&[5, 6, 7], 1), vec![5, 6, 7]);
    }

    #[test]
    fn interpolation_ends_exactly_at_the_last_sample() {
        let audio = [0i16, i16::MAX];
        assert_eq!(sample_at(&audio, 0.0), (0.0, false));
        let (middle, done) = sample_at(&audio, 0.5);
        assert!(!done && (middle - 0.5).abs() < 0.001);
        assert_eq!(sample_at(&audio, 1.0), (1.0, false));
        assert!(sample_at(&audio, 2.0).1);
    }

    #[test]
    fn loads_the_recorder_output_format() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("call.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..4 {
            writer.write_sample(100i16).unwrap();
            writer.write_sample(-100i16).unwrap();
        }
        writer.finalize().unwrap();
        let (audio, rate) = load(&path).unwrap();
        assert_eq!((audio, rate), (vec![0; 4], 8_000));
    }

    #[test]
    fn unreadable_recordings_report_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.wav");
        std::fs::write(&path, b"not a wav").unwrap();
        assert!(load(&path).is_err());
    }
}
