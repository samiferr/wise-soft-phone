# Local RVoIP 0.4.0 patch

Source: https://crates.io/crates/rvoip-audio-device/0.4.0
Upstream: https://github.com/eisenzopf/rvoip
License: MIT.

`SOFTPHONE.patch` shows the exact source change relative to crates.io 0.4.0.

Upstream captures at a fixed 8 kHz mono and band-limits both directions to 3.4 kHz. The Opus encoder rejects those frames: it expects 48 kHz, 960 samples per channel. Received stereo Opus was also played as if mono, at half speed.

Changes:

- `DeviceOptions::with_format(AudioFormat { sample_rate, channels })` sets the PCM shape of microphone frames. The default is still 8 kHz mono. Stereo frames carry the mono microphone signal on both channels.
- Playback mixes interleaved frames to mono using `frame.channels`.
- `Resampler` replaces the per-block `resample_linear`. It keeps interpolation phase and the previous sample across blocks, so 44.1 ↔ 48 kHz conversion neither drifts nor clicks at block edges. The 4th-order Butterworth filter is applied before decimation and after interpolation. G.711 keeps the 3.4 kHz telephone band. Wider codecs keep 45 % of the lower rate, capped at 20 kHz, and Opus at a 48 kHz device rate passes through unfiltered.
- Unit tests cover frame shape, interleaving, pass-through, the band limit, and drift and click behaviour over 10 s of streaming.

Run them with `cargo test -p rvoip-audio-device --lib`.
