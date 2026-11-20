# Plan: Opus codec support

Status: implemented on this branch except for the manual interop checks in step 15. See "Implementation notes" at the end for where the work departed from the plan.

## Goal

Offer and accept Opus (`opus/48000/2`, RFC 7587) on calls. Put Opus first, with
PCMU and PCMA as fallbacks. Wideband audio should reach the speaker when the
PBX negotiates Opus. Calls to G.711-only peers must keep working as they do today.

## What the dependencies already provide (checked against the 0.4.0 sources)

| Layer | State |
|---|---|
| `rvoip-sip` 0.4.0 | Has an `opus` cargo feature. It turns on `rvoip-media-core/opus` and `rvoip-core/opus`. With the feature on, `Config::offered_codecs` accepts PT `111`; without it, PT `111` gives a config error. |
| SDP offer/answer (`adapters/media_adapter.rs`) | Writes `a=rtpmap:111 opus/48000/2`. When the peer uses another dynamic PT for Opus (Asterisk often uses 107, others use 96), it matches on the rtpmap name. It sends **no fmtp**; a code comment says production setups may want `useinbandfec=1; minptime=10`. |
| Codec (`rvoip-media-core`/`rvoip-codec-core`) | `OpusCodec` wraps the `opus` 0.3 crate (libopus through `audiopus_sys`). The encoder accepts only frames that match its sample rate, channel count, and frame size exactly (48 kHz, 20 ms = 960 samples per channel). |
| Inbound media (`media_stream.rs`) | `InboundPcmFramer` cuts decoded Opus into 20 ms frames at 48 kHz. |
| **`rvoip-audio-device` 0.4.0** | **Blocker.** Capture is fixed at `SAMPLE_RATE = 8_000` and runs through a `TELEPHONE_CUTOFF_HZ = 3400` low-pass. It sends 160-sample 8 kHz frames, which the Opus encoder rejects as a sample-rate mismatch. Playback already resamples using `frame.sample_rate`, but the 3.4 kHz low-pass would still remove the wideband content. |

Conclusion: signalling and the codec already exist upstream and only need
turning on. The real work is the device bridge, plus settings, UI, tests, and
build dependencies.

## Work plan

### Phase 0 – Build prerequisites
1. Add `features = ["opus"]` to `rvoip-sip` in `Cargo.toml`. If needed, also add
   `rvoip-media-core` with the same feature so its codec types can be named.
2. Native libopus: `audiopus_sys` uses the system `libopus` through pkg-config,
   or builds the bundled copy with CMake (CMake is present here; libopus is
   not). Choose one:
   - Dynamic: `sudo apt install libopus-dev`. Add it to the README dependency
     line and `docs/07-deployment-view.md`.
   - Static/bundled: use the bundled build (set `LIBOPUS_STATIC=1` or the crate's
     static feature) so the release binary is self-contained, and list `cmake`
     as a build dependency instead. **Recommended:** this matches the
     single-binary deployment story.
3. Run `cargo update -p rvoip-sip` only if needed and keep the `=0.4.0` pins.
   Check that `cargo build --locked` and clippy still pass. Commit the new
   `Cargo.lock`.

### Phase 1 – Codec preference in the endpoint
4. In `src/account.rs` (`Account::endpoint` → `Endpoint::builder()`), add
   `.configure(|c| c.offered_codecs = codec_list(self.codec_pref))`:
   - Default order: `[111, 0, 8, 101]` (Opus, PCMU, PCMA, DTMF).
   - "G.711 only": `[0, 8, 101]`, which matches today's behaviour.
5. Add a `codecs` field to `Account`, an enum such as `CodecPreference
   { OpusFirst, G711Only }`, with `#[serde(default)]` so existing
   `account.json` files still load. Default to `OpusFirst`. Keep `G711Only` as
   a fallback switch for PBXs with broken Opus.
6. Optional, in a vendored patch of `rvoip-sip`: emit
   `a=fmtp:111 useinbandfec=1; minptime=10; maxplaybackrate=16000; sprop-maxcapturerate=16000; stereo=0`.
   Without this, libopus defaults still interoperate. Leave it out of the
   first version unless an Asterisk test shows a need. Don't take on another
   vendored crate without a reason.

### Phase 2 – Wideband-capable audio bridge (main work)
The bridge must produce frames at the negotiated codec rate and channel count,
and must not band-limit Opus to telephone bandwidth.

7. Read the negotiated codec in `CallAudio::start` (`src/audio.rs`) before the
   bridge starts, using the session's media info (`MediaInfo.codec`, plus rate
   and channels from the negotiated config). Map it to
   `MediaFormat { rate: 8_000 | 48_000, channels: 1 | 2 }`.
   Stereo is unnecessary: always encode mono for Opus. If upstream requires
   the frame's channel count to match the SDP's `/2`, send identical L/R
   samples, and check this in the tests in step 13.
8. Replace or extend `rvoip_audio_device::DeviceBridge`. Two options:
   - **A. Vendor and patch `rvoip-audio-device`.** Use the same
     `[patch.crates-io]` mechanism already used for `rvoip-sip-dialog`. Make
     `SAMPLE_RATE`/`FRAME_SAMPLES` follow a `DeviceOptions::with_format(rate,
     channels)` setting, and make the 3.4 kHz low-pass apply only when
     `rate == 8_000`. For 48 kHz, use an anti-alias low-pass near 7–8 kHz when
     resampling from devices above 48 kHz, or none when the device already
     runs at 48 kHz.
     The patch is small, keeps the mute flag, level callback, and heartbeat
     logic, and is easy to upstream. **Recommended.**
   - B. Write a small app-owned CPAL bridge in `src/audio.rs`. This gives full
     control but duplicates jitter buffering, pre-roll, mute, and level code.
     It is only worth it if the upstream bridge needs bigger changes.
9. Resampling quality: the upstream `resample_linear` is fine for 8 kHz, but
   linear interpolation to or from 48 kHz audibly aliases. Use a
   band-limited resampler (e.g. `rubato`) when device rate ≠ codec rate. Many
   devices run at 48 kHz natively, so that case should be a pass-through.
10. Playback buffer sizes (`prime_samples`, `cap_samples`) already scale with
    the device rate. Confirm that the 50 ms pre-roll and 300 ms cap still hold
    at 48 kHz.
11. Keep `microphone_healthy()` and `log_health()` working: the heartbeat logic
    does not depend on the codec.

### Phase 3 – UI and diagnostics
12. `src/ui.rs`: show the negotiated codec in the connected-call view (e.g.
    "Opus · wideband" / "G.711 µ-law"). Put the codec preference under
    **Connection options**, and only allow changes between calls, the same
    rule as device selection. In `src/diagnostics.rs` / `phone.rs`, log the
    negotiated codec, rate, and PT at `attach_audio`.

### Phase 4 – Tests
13. Unit tests:
    - `codec_list()` gives the expected payload-type order for each preference;
      `G711Only` never contains 111.
    - Account JSON without the new field deserialises to `OpusFirst`.
    - Bridge framing: at 48 kHz mono, frames are 960 samples every 20 ms; at
      8 kHz they are still 160. The low-pass is off at 48 kHz (a 6 kHz tone
      survives) and on at 8 kHz.
14. Loopback integration (extend `src/call_operations_tests.rs` /
    `sip_routing_tests.rs`, which already create a local `Endpoint` with
    `Config::local`):
    - Opus ↔ Opus: the answer carries `opus/48000/2` and audio frames flow both
      ways.
    - Opus-first caller ↔ G.711-only callee: PCMU is negotiated.
    - Incoming offer with Opus on PT 107 or 96: the answer keeps the peer's PT.
    - SRTP + Opus (TLS profile) negotiates.
    - Add `a=rtpmap:111 opus/48000/2` variants of the SDP fixture in
      `sip_routing_tests.rs:132`.
15. Manual interop: test against Asterisk PJSIP with `allow=opus,ulaw,alaw`
    (needs `codec_opus`), with a real headset, on both UDP and TLS/SRTP. Also
    check DTMF (RFC 4733 at 8000 Hz clock alongside the Opus 48000 clock).
    Record the results in `RELIABILITY_REVIEW.md`.

### Phase 5 – Docs
16. Update `README.md`: the codec list in the intro, the Asterisk section
    (`allow=opus,ulaw,alaw`), and build dependencies. Also update
    `PROJECT_DESCRIPTION.md`, `TESTING.md`, and the architecture, data, and
    deployment views under `docs/` (codec preference field in the account data
    model, libopus/CMake in the deployment view).

## Risks and open questions
- **Upstream maturity:** rvoip labels its codec support "beta". Opus comes
  from an `opus` 0.3 wrapper using default `OpusConfig`. Check the defaults
  (VoIP application mode, bitrate around 24–32 kbit/s, FEC off) and decide
  whether to patch the defaults.
- **Telephone-event clock:** DTMF PT 101 runs at 8000 Hz while Opus uses
  48000 Hz. Confirm rvoip sends RFC 4733 events with the right timestamps when
  the primary codec is Opus.
- **Bridge format timing:** the negotiated codec has to be known before
  `DeviceBridge::start`. If a re-INVITE or UPDATE changes the codec mid-call,
  the bridge must restart. In the first version, log a warning and keep the
  initial format.
- **CPU and latency:** Opus encode/decode at 48 kHz is cheap on desktop
  hardware. Measure it anyway in the existing timing logs.

## Suggested order and size
Phases 0–1 (half a day) work end to end on the signalling side only. The calls
are not usable until Phase 2 lands, so ship 0–2 together behind the
`OpusFirst` default. Phase 2 is most of the effort (1–2 days). Phases 3–5 take
another day including interop testing.

## Implementation notes

- **Format discovery (step 7):** rvoip-sip 0.4.0 has no public accessor for the negotiated codec, and SIP traces strip SDP bodies. The work therefore vendors `rvoip-sip` with a small `SessionHandle::negotiated_audio()` patch (`vendor/rvoip-sip/SOFTPHONE_PATCH.md`). This matters more than expected: the first frame of the wrong shape ends the call's send task for the rest of the call. Audio therefore never starts without a negotiated format.
- **fmtp (step 6):** done, because rvoip-sip had to be vendored anyway. The change found an upstream bug: Opus offered on Asterisk's PT 107 was answered with AMR-NB `octet-align`/`max-red` parameters. Fixed and covered by a test.
- **Stereo:** `opus/48000/2` negotiates 2 channels, and the encoder requires interleaved stereo. The bridge duplicates the mono microphone signal, and playback now mixes received stereo down to mono. Upstream would have played it at half speed.
- **Resampling (step 9):** instead of adding `rubato`, the upstream linear interpolator became a streaming, phase-continuous resampler with the existing Butterworth band limit. Opus at a 48 kHz device rate is a pass-through. Tests cover drift, clicks, and the band limit.
- **libopus (step 2):** bundled and static through `.cargo/config.toml` (`LIBOPUS_NO_PKG`, `LIBOPUS_STATIC`). The build needs CMake, but no system libopus.
- **SRTP + Opus (step 14):** not covered by an automated test, because there is no loopback TLS fixture. It belongs to the manual TLS check in step 15.
- **Mid-call codec change:** not handled. A re-INVITE that changes the codec leaves the bridge in its initial format, as planned for the first version.

