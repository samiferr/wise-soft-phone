//! OS audio-device bridge for [`rvoip-sip`](rvoip_sip).
//!
//! rvoip-sip exposes a call's audio as a frame-based
//! [`AudioStream`] and intentionally does **not** own OS
//! devices. This crate is the missing other half: an optional [`cpal`] bridge
//! that captures the microphone and plays back the speaker against that stream,
//! so a softphone author doesn't have to re-derive real-time audio.
//!
//! What it handles (the parts easy to get wrong):
//!
//! - **Drift-free 20 ms pacing.** Mic frames are emitted from a
//!   [`tokio::time::interval`] (one frame per tick, silence on underrun), not a
//!   per-frame `sleep` — `sleep` accumulates latency and drifts to ~22 ms,
//!   starving the far end.
//! - **Negotiated PCM format.** Mic frames are produced at the call's
//!   negotiated sample rate and channel count ([`DeviceOptions::with_format`]):
//!   8 kHz mono for G.711, 48 kHz for Opus. Stereo frames carry the mono mic
//!   on both channels; received stereo is mixed to mono for playback.
//! - **Band-limited resampling.** A 4th-order Butterworth low-pass band-limits
//!   before decimation and after interpolation, so linear interpolation doesn't
//!   alias. G.711 keeps the telephone passband (3.4 kHz); wideband codecs keep
//!   up to 45 % of the lower rate. The interpolator carries its phase across
//!   blocks, so non-integer ratios (44.1 ↔ 48 kHz) neither drift nor click.
//! - **Click-free jitter buffer.** A small playback buffer (~50 ms pre-roll,
//!   ~300 ms cap) decays toward silence on underrun instead of hard-zeroing.
//! - **Mute-as-silence and VU metering.**
//!
//! # Quick start
//!
//! ```rust,no_run
//! # async fn example(handle: rvoip_sip::SessionHandle) -> anyhow::Result<()> {
//! use rvoip_audio_device::DeviceBridge;
//! let audio = handle.audio().await?;
//! let _running = DeviceBridge::attach_default(audio)?; // capture/play until dropped
//! # Ok(())
//! # }
//! ```
//!
//! ## Threading
//!
//! cpal `Stream`s are `!Send`, so the whole bridge runs on a dedicated OS thread
//! with its own current-thread tokio runtime. Only the `Send`
//! [`AudioSender`]/`AudioReceiver` halves and the
//! optional level callback cross the thread boundary.

#![deny(missing_docs)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::{mpsc, oneshot};

use rvoip_sip::{AudioFrame, AudioSender, AudioStream};

/// Default PCM format: 8 kHz mono (G.711).
const DEFAULT_SAMPLE_RATE: u32 = 8_000;
const FRAME_MS: u32 = 20;
/// Low-pass cutoff: top of the telephone passband (G.711 is band-limited anyway).
const TELEPHONE_CUTOFF_HZ: f32 = 3400.0;

/// PCM shape of the frames exchanged with rvoip for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// Sample rate in Hz (8000 for G.711, 48000 for Opus).
    pub sample_rate: u32,
    /// Interleaved channel count, 1 or 2.
    pub channels: u8,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: 1,
        }
    }
}

impl AudioFormat {
    /// Samples per channel in one 20 ms frame.
    pub fn frame_samples(self) -> usize {
        (self.sample_rate as usize * FRAME_MS as usize) / 1_000
    }
}

/// Audio bandwidth worth keeping for a stream at `rate` Hz.
///
/// Narrowband (G.711) keeps the telephone passband; wider streams keep 45 %
/// of the rate, capped at 20 kHz.
fn audio_band_hz(rate: u32) -> f32 {
    if rate <= DEFAULT_SAMPLE_RATE {
        TELEPHONE_CUTOFF_HZ
    } else {
        (rate as f32 * 0.45).min(20_000.0)
    }
}

/// Direction of an audio device / stream, from the client's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDirection {
    /// Capture device (microphone) — audio sent to the remote party.
    Input,
    /// Playback device (speaker) — audio received from the remote party.
    Output,
}

/// A VU-meter level update (RMS amplitude, ~`0.0..=1.0`) for one direction.
#[derive(Debug, Clone, Copy)]
pub struct AudioLevel {
    /// Which stream the level was measured on.
    pub direction: AudioDirection,
    /// RMS amplitude of the most recent block.
    pub level: f32,
}

/// Callback invoked (~10×/s per direction) with VU-meter levels.
pub type LevelSink = Arc<dyn Fn(AudioLevel) + Send + Sync>;

/// Configuration for a [`DeviceBridge`].
///
/// Build with [`DeviceOptions::new`] and the `with_*` setters, or take
/// [`DeviceOptions::default`] for default devices, unmuted, no metering.
#[derive(Clone, Default)]
pub struct DeviceOptions {
    input_device: Option<String>,
    output_device: Option<String>,
    muted: Option<Arc<AtomicBool>>,
    level_sink: Option<LevelSink>,
    format: AudioFormat,
}

impl DeviceOptions {
    /// Default options: system default devices, unmuted, no level metering.
    pub fn new() -> Self {
        Self::default()
    }

    /// Select the input (capture) device by index or name substring.
    pub fn with_input_device(mut self, selector: impl Into<String>) -> Self {
        self.input_device = Some(selector.into());
        self
    }

    /// Select the output (playback) device by index or name substring.
    pub fn with_output_device(mut self, selector: impl Into<String>) -> Self {
        self.output_device = Some(selector.into());
        self
    }

    /// Produce mic frames in the call's negotiated PCM format. Defaults to
    /// 8 kHz mono. rvoip rejects frames whose rate or channel count differs
    /// from the negotiated codec, so pass the format the call negotiated.
    pub fn with_format(mut self, format: AudioFormat) -> Self {
        self.format = AudioFormat {
            sample_rate: format.sample_rate.max(1),
            channels: format.channels.clamp(1, 2),
        };
        self
    }

    /// Share a mute flag with the bridge. While `true`, the mic pump emits
    /// silence (RTP keeps flowing) — pair this with the rvoip `mute()` signal.
    pub fn with_mute_flag(mut self, muted: Arc<AtomicBool>) -> Self {
        self.muted = Some(muted);
        self
    }

    /// Receive [`AudioLevel`] updates for VU meters via a callback.
    pub fn with_level_callback(
        mut self,
        callback: impl Fn(AudioLevel) + Send + Sync + 'static,
    ) -> Self {
        self.level_sink = Some(Arc::new(callback));
        self
    }
}

/// Live cpal bridge for one call. Dropping it stops capture/playback.
pub struct RunningAudio {
    // Dropping the sender closes the oneshot, which unblocks the bridge thread
    // and lets the cpal streams (and tokio runtime) drop, stopping audio.
    _stop_tx: oneshot::Sender<()>,
}

/// Entry point: attach OS capture/playback to a call's [`AudioStream`].
pub struct DeviceBridge;

impl DeviceBridge {
    /// Start capturing/playing `audio` on a dedicated OS thread, configured by
    /// `opts`. The returned [`RunningAudio`] stops everything when dropped.
    pub fn start(audio: AudioStream, opts: DeviceOptions) -> anyhow::Result<RunningAudio> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("rvoip-audio-device".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        log::error!("audio runtime build failed: {e}");
                        return;
                    }
                };
                rt.block_on(async move {
                    if let Err(e) = run_bridge(audio, opts, stop_rx).await {
                        log::error!("audio bridge error: {e}");
                    }
                });
            })?;
        Ok(RunningAudio { _stop_tx: stop_tx })
    }

    /// Convenience: attach the system default devices, unmuted, no metering.
    pub fn attach_default(audio: AudioStream) -> anyhow::Result<RunningAudio> {
        Self::start(audio, DeviceOptions::default())
    }
}

async fn run_bridge(
    audio: AudioStream,
    opts: DeviceOptions,
    stop_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let DeviceOptions {
        input_device,
        output_device,
        muted,
        level_sink,
        format,
    } = opts;
    let muted = muted.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

    let (sender, mut receiver) = audio.split();
    let host = cpal::default_host();
    let input = choose_device(&host, true, input_device.as_deref())?;
    let output = choose_device(&host, false, output_device.as_deref())?;
    let input_name = input.name().unwrap_or_else(|_| "input".into());
    let output_name = output.name().unwrap_or_else(|_| "output".into());
    log::info!(
        "audio route: {input_name} -> {output_name} ({} Hz, {} ch)",
        format.sample_rate,
        format.channels
    );

    let input_config = input.default_input_config()?;
    let output_config = output.default_output_config()?;
    let input_sample_rate = input_config.sample_rate().0;
    let output_sample_rate = output_config.sample_rate().0;
    let input_channels = input_config.channels() as usize;
    let output_channels = output_config.channels() as usize;

    let (mic_tx, mut mic_rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let playback = Arc::new(Mutex::new(Playback {
        buffer: VecDeque::with_capacity(output_sample_rate as usize),
        primed: false,
        last: 0.0,
        prime_samples: (output_sample_rate as usize * 50) / 1000, // ~50 ms pre-roll
        cap_samples: (output_sample_rate as usize * 300) / 1000,  // ~300 ms cap
    }));

    // cpal streams are `!Send` and stay on this thread for their whole life.
    let input_stream =
        build_input_stream(&input, &input_config.into(), input_channels, mic_tx, muted)?;
    let output_stream = build_output_stream(
        &output,
        &output_config.into(),
        output_channels,
        playback.clone(),
    )?;
    input_stream.play()?;
    output_stream.play()?;

    // Mic pump (Send): captured f32 @ device rate -> codec-rate i16 frames -> rvoip.
    let mic_level_sink = level_sink.clone();
    let input_task = tokio::spawn(async move {
        send_microphone_frames(&mut mic_rx, input_sample_rate, format, sender, mic_level_sink)
            .await;
    });

    // Playback pump (Send): rvoip frames -> resample to device rate -> buffer.
    let out_level_sink = level_sink.clone();
    let output_task = tokio::spawn(async move {
        let mut level_tick = 0u32;
        // Rebuilt whenever the received rate changes (codec renegotiation).
        let mut resampler: Option<Resampler> = None;
        while let Some(frame) = receiver.recv().await {
            let channels = usize::from(frame.channels.max(1));
            let interleaved = frame
                .samples
                .iter()
                .map(|sample| *sample as f32 / i16::MAX as f32)
                .collect::<Vec<_>>();
            let mono = mix_to_mono(&interleaved, channels);
            // Throttle VU updates to ~10/s (see input pump).
            level_tick = level_tick.wrapping_add(1);
            if level_tick % 5 == 0 {
                emit_level(&out_level_sink, AudioDirection::Output, rms(&mono));
            }
            if resampler
                .as_ref()
                .is_none_or(|r| r.from_rate != frame.sample_rate.max(1))
            {
                let rate = frame.sample_rate.max(1);
                resampler = Some(Resampler::new(rate, output_sample_rate, audio_band_hz(rate)));
            }
            let resampled = match resampler.as_mut() {
                Some(r) => r.process(&mono),
                None => mono,
            };
            if let Ok(mut pb) = playback.lock() {
                pb.buffer.extend(resampled);
                let cap = pb.cap_samples;
                while pb.buffer.len() > cap {
                    pb.buffer.pop_front();
                }
                if !pb.primed && pb.buffer.len() >= pb.prime_samples {
                    pb.primed = true;
                }
            } else {
                break;
            }
        }
    });

    // Hold the streams until asked to stop (or the rvoip side closes).
    let _ = stop_rx.await;
    input_task.abort();
    output_task.abort();
    drop(input_stream);
    drop(output_stream);
    Ok(())
}

fn emit_level(sink: &Option<LevelSink>, direction: AudioDirection, level: f32) {
    if let Some(sink) = sink {
        sink(AudioLevel { direction, level });
    }
}

async fn send_microphone_frames(
    mic_rx: &mut mpsc::UnboundedReceiver<Vec<f32>>,
    input_sample_rate: u32,
    format: AudioFormat,
    sender: AudioSender,
    level_sink: Option<LevelSink>,
) {
    let frame_samples = format.frame_samples();
    let channels = usize::from(format.channels);
    let mut mono_buffer = Vec::<f32>::new();
    let mut timestamp = 0u32;
    let mut level_tick = 0u32;
    // Bound the accumulation buffer so latency can't grow without limit if the
    // capture clock runs slightly faster than the 20 ms send clock.
    let max_buffer = (format.sample_rate as usize) / 2; // 0.5 s at the codec rate
    let mut resampler = Resampler::new(
        input_sample_rate,
        format.sample_rate,
        audio_band_hz(format.sample_rate),
    );

    // Drift-free 20 ms send clock. `interval` ticks on absolute deadlines and
    // absorbs processing time, unlike `sleep` which accumulates drift (a
    // per-frame sleep emits frames every ~22 ms, starving the far end).
    let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS as u64));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            // Emit exactly one 20 ms frame per tick.
            _ = ticker.tick() => {
                let mono: Vec<i16> = if mono_buffer.len() >= frame_samples {
                    mono_buffer.drain(..frame_samples).map(float_to_i16).collect()
                } else {
                    // Underrun: send silence to hold the RTP cadence.
                    vec![0i16; frame_samples]
                };
                let pcm = interleave(mono, channels);
                let frame = AudioFrame::new(pcm, format.sample_rate, format.channels, timestamp);
                timestamp = timestamp.wrapping_add(frame_samples as u32);
                if sender.send(frame).await.is_err() {
                    return;
                }
            }
            // Drain whatever the mic captured into the accumulation buffer.
            maybe = mic_rx.recv() => {
                let Some(samples) = maybe else { return };
                level_tick = level_tick.wrapping_add(1);
                if level_tick % 5 == 0 {
                    emit_level(&level_sink, AudioDirection::Input, rms(&samples));
                }
                mono_buffer.extend(resampler.process(&samples));
                if mono_buffer.len() > max_buffer {
                    let overflow = mono_buffer.len() - max_buffer;
                    mono_buffer.drain(..overflow);
                }
            }
        }
    }
}

fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: mpsc::UnboundedSender<Vec<f32>>,
    muted: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream> {
    let err_fn = |err| log::error!("input stream error: {err}");
    let sample_format = device.default_input_config()?.sample_format();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| send_input_samples(data, channels, &tx, &muted),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                let converted = data
                    .iter()
                    .map(|sample| *sample as f32 / i16::MAX as f32)
                    .collect::<Vec<_>>();
                send_input_samples(&converted, channels, &tx, &muted);
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _| {
                let converted = data
                    .iter()
                    .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                    .collect::<Vec<_>>();
                send_input_samples(&converted, channels, &tx, &muted);
            },
            err_fn,
            None,
        )?,
        other => anyhow::bail!("unsupported input sample format {other:?}"),
    };
    Ok(stream)
}

fn send_input_samples(
    data: &[f32],
    channels: usize,
    tx: &mpsc::UnboundedSender<Vec<f32>>,
    muted: &AtomicBool,
) {
    if muted.load(Ordering::SeqCst) {
        // Mute = send silence so RTP keeps flowing but carries no audio.
        let _ = tx.send(vec![0.0; data.len() / channels.max(1)]);
    } else {
        let _ = tx.send(mix_to_mono(data, channels));
    }
}

/// Playback jitter-buffer state shared between the playback pump (producer) and
/// the cpal output callback (consumer), behind a single mutex.
struct Playback {
    buffer: VecDeque<f32>,
    /// Whether enough audio is buffered to start draining; re-armed after an
    /// underrun so we don't click on every empty poll.
    primed: bool,
    /// Last emitted sample, decayed toward zero on underrun to avoid clicks.
    last: f32,
    /// Start draining once this many samples are buffered (~50 ms pre-roll).
    prime_samples: usize,
    /// Hard cap (~300 ms); drop oldest beyond this so latency self-corrects.
    cap_samples: usize,
}

fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    playback: Arc<Mutex<Playback>>,
) -> anyhow::Result<cpal::Stream> {
    let err_fn = |err| log::error!("output stream error: {err}");
    let sample_format = device.default_output_config()?.sample_format();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| fill_output(data, channels, &playback, |s| s),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| fill_output(data, channels, &playback, float_to_i16),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _| {
                fill_output(data, channels, &playback, |sample| {
                    ((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
                })
            },
            err_fn,
            None,
        )?,
        other => anyhow::bail!("unsupported output sample format {other:?}"),
    };
    Ok(stream)
}

fn fill_output<T: Copy>(
    data: &mut [T],
    channels: usize,
    playback: &Arc<Mutex<Playback>>,
    convert: impl Fn(f32) -> T,
) {
    let channels = channels.max(1);
    if let Ok(mut pb) = playback.lock() {
        for frame in data.chunks_mut(channels) {
            let sample = if pb.primed {
                match pb.buffer.pop_front() {
                    Some(v) => {
                        pb.last = v;
                        v
                    }
                    None => {
                        // Underrun: stop draining until re-primed and decay the
                        // last sample toward silence (no hard-zero click).
                        pb.primed = false;
                        pb.last *= 0.85;
                        pb.last
                    }
                }
            } else {
                // Pre-roll / re-priming: ease toward silence.
                pb.last *= 0.85;
                pb.last
            };
            let converted = convert(sample);
            for out in frame {
                *out = converted;
            }
        }
    } else {
        let zero = convert(0.0);
        for out in data {
            *out = zero;
        }
    }
}

fn mix_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// One biquad section (RBJ cookbook low-pass), Direct Form I.
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn lowpass(fs: f32, fc: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * fc / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// 4th-order Butterworth low-pass (two cascaded biquads). Band-limits to the
/// telephone passband so the 8 kHz↔device-rate conversions don't alias — the
/// anti-alias / reconstruction filtering that linear interpolation alone skips.
/// Stateful, so it stays click-free across streaming blocks.
struct LowPass {
    stages: [Biquad; 2],
}

impl LowPass {
    fn new(sample_rate: u32, cutoff_hz: f32) -> Self {
        let fs = sample_rate as f32;
        // Butterworth Q values for a 4th-order (two-section) response.
        Self {
            stages: [
                Biquad::lowpass(fs, cutoff_hz, 0.541_196_1),
                Biquad::lowpass(fs, cutoff_hz, 1.306_563),
            ],
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let mut s = x;
        for stage in self.stages.iter_mut() {
            s = stage.process(s);
        }
        s
    }
}

/// Streaming mono sample-rate converter: linear interpolation with a
/// Butterworth band limit, applied at the higher of the two rates (before
/// decimation, after interpolation). Equal rates pass through, filtered only
/// when the requested band is narrower than the stream (G.711 at 8 kHz has no
/// filter; an 8 kHz codec on a 48 kHz device is still limited to 3.4 kHz).
struct Resampler {
    from_rate: u32,
    to_rate: u32,
    /// Filter applied to the input before decimation.
    pre: Option<LowPass>,
    /// Filter applied to the output after interpolation.
    post: Option<LowPass>,
    /// Position of the next output sample, in input samples relative to the
    /// start of the next block (`-1.0..0.0` points into `last`).
    position: f64,
    /// Final input sample of the previous block, for interpolation across
    /// block boundaries.
    last: f32,
}

impl Resampler {
    fn new(from_rate: u32, to_rate: u32, band_hz: f32) -> Self {
        let from_rate = from_rate.max(1);
        let to_rate = to_rate.max(1);
        let lower = from_rate.min(to_rate) as f32;
        let cutoff = band_hz.min(lower * 0.45);
        let needs_filter = |rate: u32| cutoff < rate as f32 * 0.4;
        let (pre, post) = if from_rate > to_rate {
            (Some(LowPass::new(from_rate, cutoff)), None)
        } else if from_rate < to_rate {
            (None, Some(LowPass::new(to_rate, cutoff)))
        } else if needs_filter(from_rate) {
            (Some(LowPass::new(from_rate, cutoff)), None)
        } else {
            (None, None)
        };
        Self {
            from_rate,
            to_rate,
            pre,
            post,
            position: 0.0,
            last: 0.0,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let filtered;
        let input = match self.pre.as_mut() {
            Some(lp) => {
                filtered = input.iter().map(|s| lp.process(*s)).collect::<Vec<_>>();
                filtered.as_slice()
            }
            None => input,
        };
        let mut output = if self.from_rate == self.to_rate {
            input.to_vec()
        } else {
            let step = self.from_rate as f64 / self.to_rate as f64;
            let len = input.len() as f64;
            let mut output =
                Vec::with_capacity((len / step) as usize + 2);
            // Output samples whose right-hand neighbour lies in this block.
            while self.position < len - 1.0 {
                let floor = self.position.floor();
                let frac = (self.position - floor) as f32;
                let (a, b) = if floor < 0.0 {
                    (self.last, input[0])
                } else {
                    let idx = floor as usize;
                    (input[idx], input[idx + 1])
                };
                output.push(a + (b - a) * frac);
                self.position += step;
            }
            self.position -= len;
            self.last = input[input.len() - 1];
            output
        };
        if let Some(lp) = self.post.as_mut() {
            for s in output.iter_mut() {
                *s = lp.process(*s);
            }
        }
        output
    }
}

/// Duplicate mono samples into `channels` interleaved channels.
fn interleave(mono: Vec<i16>, channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return mono;
    }
    mono.into_iter()
        .flat_map(|sample| std::iter::repeat_n(sample, channels))
        .collect()
}

fn float_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    power.sqrt()
}

fn choose_device(
    host: &cpal::Host,
    input: bool,
    selector: Option<&str>,
) -> anyhow::Result<cpal::Device> {
    if let Some(selector) = selector {
        let devices = if input {
            host.input_devices()?
        } else {
            host.output_devices()?
        }
        .collect::<Vec<_>>();

        if let Ok(index) = selector.parse::<usize>() {
            return devices
                .into_iter()
                .nth(index)
                .ok_or_else(|| anyhow::anyhow!("audio device index {index} not found"));
        }

        let needle = selector.to_ascii_lowercase();
        return devices
            .into_iter()
            .find(|device| {
                device
                    .name()
                    .map(|name| name.to_ascii_lowercase().contains(&needle))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("audio device matching '{selector}' not found"));
    }

    if input {
        host.default_input_device()
            .ok_or_else(|| anyhow::anyhow!("no default input device"))
    } else {
        host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no default output device"))
    }
}

/// Enumerate available audio devices for `direction` as `(id, display_name)`.
///
/// The id is the device name (usable as a [`DeviceOptions`] selector).
pub fn list_devices(direction: AudioDirection) -> Vec<(String, String)> {
    let host = cpal::default_host();
    let devices = match direction {
        AudioDirection::Input => host.input_devices(),
        AudioDirection::Output => host.output_devices(),
    };
    match devices {
        Ok(devs) => devs
            .filter_map(|d| d.name().ok())
            .map(|name| (name.clone(), name))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    /// Run `input` through `resampler` in 10 ms blocks and drop the first
    /// 50 ms of output (filter settling).
    fn stream(resampler: &mut Resampler, input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
        let block = from_rate as usize / 100;
        let out: Vec<f32> = input
            .chunks(block)
            .flat_map(|chunk| resampler.process(chunk))
            .collect();
        out[to_rate as usize / 20..].to_vec()
    }

    #[test]
    fn frame_shape_follows_format() {
        assert_eq!(AudioFormat::default().frame_samples(), 160);
        let opus = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        assert_eq!(opus.frame_samples(), 960);
        assert_eq!(interleave(vec![1, 2], 2), vec![1, 1, 2, 2]);
        assert_eq!(interleave(vec![1, 2], 1), vec![1, 2]);
        assert_eq!(mix_to_mono(&[0.5, -0.5, 1.0, 0.0], 2), vec![0.0, 0.5]);
    }

    #[test]
    fn opus_at_device_rate_passes_through_unfiltered() {
        let input = tone(48_000, 6_000.0, 4_800);
        let mut resampler = Resampler::new(48_000, 48_000, audio_band_hz(48_000));
        assert_eq!(resampler.process(&input), input);
    }

    #[test]
    fn wideband_keeps_six_khz_and_g711_removes_it() {
        let input = tone(48_000, 6_000.0, 48_000);
        let wide = stream(
            &mut Resampler::new(44_100, 48_000, audio_band_hz(48_000)),
            &tone(44_100, 6_000.0, 44_100),
            44_100,
            48_000,
        );
        assert!(rms(&wide) > 0.33, "wideband 6 kHz rms {}", rms(&wide));
        // Decimating to 8 kHz without the telephone band limit would alias
        // 6 kHz down to an audible 2 kHz at full level. The 4th-order 3.4 kHz
        // Butterworth (unchanged from upstream) gives about 20 dB there.
        let narrow = stream(
            &mut Resampler::new(48_000, 8_000, audio_band_hz(8_000)),
            &input,
            48_000,
            8_000,
        );
        assert!(rms(&narrow) < 0.04, "G.711 6 kHz rms {}", rms(&narrow));
    }

    #[test]
    fn streaming_resampler_neither_drifts_nor_clicks() {
        let input = tone(44_100, 440.0, 441_000); // 10 s
        let mut resampler = Resampler::new(44_100, 48_000, audio_band_hz(48_000));
        let out: Vec<f32> = input
            .chunks(441)
            .flat_map(|chunk| resampler.process(chunk))
            .collect();
        assert!((out.len() as i64 - 480_000).abs() <= 2, "len {}", out.len());
        let max_step = out
            .windows(2)
            .skip(2_400)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        // A 440 Hz, 0.5-amplitude sine moves at most ~0.029 per 48 kHz sample.
        assert!(max_step < 0.035, "discontinuity {max_step}");
    }
}
