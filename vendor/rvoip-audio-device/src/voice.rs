//! Microphone voice processing built on WebRTC's audio processing module.
//!
//! The processor runs on the captured signal at the device sample rate, in
//! 10 ms frames, before the signal is band-limited and decimated to 8 kHz.
//! Processing at the device rate keeps the echo canceller's far-end reference
//! equal to what the speaker actually plays, and lets noise suppression see
//! the full band instead of an already-narrowed one.
use std::sync::Mutex;

use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression,
    NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::Playback;

/// Which microphone enhancements to apply to a call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceProcessing {
    /// Remove the far end's voice picked up from the speaker (acoustic echo).
    pub echo_cancellation: bool,
    /// Reduce steady background noise such as fans and traffic.
    pub noise_suppression: bool,
    /// Level the voice with a gentle adaptive gain and a limiter.
    pub voice_enhancement: bool,
}

impl VoiceProcessing {
    /// Whether any enhancement is enabled.
    pub fn any(self) -> bool {
        self.echo_cancellation || self.noise_suppression || self.voice_enhancement
    }

    /// The WebRTC configuration for these options.
    ///
    /// A high-pass filter is part of every enabled setup: it removes DC offset
    /// and low-frequency rumble that would otherwise confuse the echo
    /// canceller, the noise estimate, and the gain controller.
    pub(crate) fn config(self) -> Config {
        Config {
            high_pass_filter: self.any().then(HighPassFilter::default),
            echo_canceller: self.echo_cancellation.then(EchoCanceller::default),
            // High suppression is the level WebRTC uses for calls; lower levels
            // leave audible hiss on a narrowband phone channel.
            noise_suppression: self.noise_suppression.then(|| NoiseSuppression {
                level: NoiseSuppressionLevel::High,
                ..Default::default()
            }),
            // There is no OS microphone-volume control here, so only the
            // digital stage is used. The gain ceiling stays modest so quiet
            // rooms are not turned into hiss; the limiter prevents clipping.
            gain_controller2: self.voice_enhancement.then(|| GainController2 {
                input_volume_controller: false,
                adaptive_digital: Some(AdaptiveDigital {
                    max_gain_db: 30.0,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

/// Processes captured microphone audio, using the played-out audio as the
/// echo reference.
pub(crate) struct VoiceProcessor {
    apm: AudioProcessing,
    capture_frame: usize,
    render_frame: usize,
    capture_pending: Vec<f32>,
    render_pending: Vec<f32>,
}

impl VoiceProcessor {
    pub(crate) fn new(
        options: VoiceProcessing,
        capture_rate: u32,
        render_rate: u32,
    ) -> Result<Self, String> {
        let capture = StreamConfig::new(capture_rate, 1);
        let render = StreamConfig::new(render_rate, 1);
        let apm = AudioProcessing::builder()
            .config(options.config())
            .capture_config(capture)
            .render_config(render)
            .build();
        Ok(Self {
            apm,
            capture_frame: capture.num_frames(),
            render_frame: render.num_frames(),
            capture_pending: Vec::new(),
            render_pending: Vec::new(),
        })
    }

    /// Processes microphone samples, returning whole 10 ms frames. Samples
    /// that do not fill a frame are held back for the next call, so the
    /// output lags the input by less than one frame.
    pub(crate) fn process(&mut self, mic: &[f32], playback: &Mutex<Playback>) -> Vec<f32> {
        self.capture_pending.extend_from_slice(mic);
        if let Ok(mut playback) = playback.lock() {
            self.render_pending.extend(playback.render.drain(..));
        }
        let mut output = Vec::with_capacity(mic.len());
        while self.capture_pending.len() >= self.capture_frame {
            // The reference must reach the canceller before the capture frame
            // that contains its echo.
            while self.render_pending.len() >= self.render_frame {
                let frame: Vec<f32> = self.render_pending.drain(..self.render_frame).collect();
                let mut scratch = vec![0.0; self.render_frame];
                let _ = self.apm.process_render_f32(&[&frame], &mut [&mut scratch]);
            }
            let frame: Vec<f32> = self.capture_pending.drain(..self.capture_frame).collect();
            let mut processed = vec![0.0; self.capture_frame];
            match self
                .apm
                .process_capture_f32(&[&frame], &mut [&mut processed])
            {
                Ok(()) => output.extend(processed),
                // Never drop the microphone because processing failed.
                Err(_) => output.extend(frame),
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn playback() -> Mutex<Playback> {
        Mutex::new(Playback {
            buffer: VecDeque::new(),
            primed: false,
            last: 0.0,
            prime_samples: 0,
            cap_samples: 0,
            render: VecDeque::new(),
            tap: true,
        })
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    /// Deterministic white noise so the tests never flake.
    struct Noise(u32);
    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// Speech-like test signal: a few harmonics under a syllable-rate envelope.
    fn voice(rate: u32, seconds: usize, seed: u32) -> Vec<f32> {
        let mut noise = Noise(seed);
        (0..rate as usize * seconds)
            .map(|n| {
                let t = n as f32 / rate as f32;
                let envelope = 0.5 + 0.5 * (std::f32::consts::TAU * 3.0 * t).sin();
                let tone: f32 = (1..=5)
                    .map(|h| {
                        (std::f32::consts::TAU * (140.0 + seed as f32 % 40.0) * h as f32 * t)
                            .sin()
                            / h as f32
                    })
                    .sum();
                0.25 * envelope * tone + 0.01 * noise.next()
            })
            .collect()
    }

    #[test]
    fn options_map_to_webrtc_stages() {
        let none = VoiceProcessing::default().config();
        assert!(none.high_pass_filter.is_none());
        assert!(none.echo_canceller.is_none());
        assert!(none.noise_suppression.is_none());
        assert!(none.gain_controller2.is_none());

        let echo_only = VoiceProcessing {
            echo_cancellation: true,
            ..Default::default()
        }
        .config();
        assert!(echo_only.high_pass_filter.is_some());
        assert!(echo_only.echo_canceller.is_some());
        assert!(echo_only.noise_suppression.is_none());
        assert!(echo_only.gain_controller2.is_none());

        let all = VoiceProcessing {
            echo_cancellation: true,
            noise_suppression: true,
            voice_enhancement: true,
        }
        .config();
        assert!(all.noise_suppression.is_some() && all.gain_controller2.is_some());
        let gain = all.gain_controller2.unwrap();
        assert!(!gain.input_volume_controller, "no OS volume control exists");
        assert!(gain.adaptive_digital.is_some());
    }

    #[test]
    fn output_length_follows_input_in_whole_frames() {
        let options = VoiceProcessing {
            echo_cancellation: true,
            noise_suppression: true,
            voice_enhancement: true,
        };
        let mut processor = VoiceProcessor::new(options, 48_000, 44_100).unwrap();
        let playback = playback();
        let mut produced = 0;
        // Odd chunk sizes, as a real device callback delivers.
        for _ in 0..40 {
            produced += processor.process(&vec![0.0; 333], &playback).len();
        }
        assert_eq!(produced, (40 * 333 / 480) * 480);
    }

    #[test]
    fn noise_suppression_reduces_steady_noise() {
        let options = VoiceProcessing {
            noise_suppression: true,
            ..Default::default()
        };
        let rate = 16_000;
        let mut processor = VoiceProcessor::new(options, rate, rate).unwrap();
        let playback = playback();
        let mut noise = Noise(7);
        let input: Vec<f32> = (0..rate * 4).map(|_| 0.05 * noise.next()).collect();
        let mut output = Vec::new();
        for chunk in input.chunks(160) {
            output.extend(processor.process(chunk, &playback));
        }
        let tail = rate as usize * 2;
        let before = rms(&input[input.len() - tail..]);
        let after = rms(&output[output.len() - tail..]);
        assert!(after < before * 0.5, "noise {before} -> {after}");
    }

    /// RMS of the last three seconds of the microphone signal and of the
    /// processed output, when the microphone hears only the speaker.
    fn echo_levels(options: VoiceProcessing) -> (f32, f32) {
        let rate = 16_000;
        let mut processor = VoiceProcessor::new(options, rate, rate).unwrap();
        let playback = playback();
        let far_end = voice(rate, 10, 3);
        // The microphone hears only the speaker: delayed, attenuated far end.
        let delay = rate as usize / 20;
        let mut microphone = vec![0.0; delay];
        microphone.extend(far_end.iter().map(|sample| 0.5 * sample));
        microphone.truncate(far_end.len());
        let mut output = Vec::new();
        for (far, near) in far_end.chunks(160).zip(microphone.chunks(160)) {
            playback.lock().unwrap().render.extend(far.iter().copied());
            output.extend(processor.process(near, &playback));
        }
        let tail = rate as usize * 3;
        (
            rms(&microphone[microphone.len() - tail..]),
            rms(&output[output.len() - tail..]),
        )
    }

    #[test]
    fn echo_cancellation_removes_the_far_end_from_the_microphone() {
        let (echo, residual) = echo_levels(VoiceProcessing {
            echo_cancellation: true,
            ..Default::default()
        });
        assert!(
            residual < echo * 0.3,
            "echo {echo} -> residual {residual} (expected at least 10 dB of cancellation)"
        );
    }

    #[test]
    fn without_echo_cancellation_the_echo_stays() {
        let (echo, residual) = echo_levels(VoiceProcessing::default());
        assert!(residual > echo * 0.9, "control changed the signal: {echo} -> {residual}");
    }

    #[test]
    fn the_talker_passes_through_when_the_far_end_is_silent() {
        let options = VoiceProcessing {
            echo_cancellation: true,
            noise_suppression: true,
            ..Default::default()
        };
        let rate = 16_000;
        let mut processor = VoiceProcessor::new(options, rate, rate).unwrap();
        let playback = playback();
        let speech = voice(rate, 6, 11);
        let mut output = Vec::new();
        for chunk in speech.chunks(160) {
            playback
                .lock()
                .unwrap()
                .render
                .extend(std::iter::repeat_n(0.0, 160));
            output.extend(processor.process(chunk, &playback));
        }
        let tail = rate as usize * 3;
        let kept = rms(&output[output.len() - tail..]) / rms(&speech[speech.len() - tail..]);
        assert!(kept > 0.5, "near-end speech was suppressed: kept {kept}");
    }
}
