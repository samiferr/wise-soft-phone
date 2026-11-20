# Call operation test coverage

Run the default suite from the project folder:

```sh
cargo test --locked
cargo fmt --check
```

Tests use local UDP sockets, so run them where local networking is permitted. SIP tests create local endpoints and a simulated provider. They do not call Twilio or the PSTN, use saved credentials, or open a microphone. Only these fixtures disable the phone actor's audio hardware startup.

Latest recorded local result: **44 passed, 0 failed, 2 optional external-PBX/audio-device tests ignored**. `shutdown_while_incoming_call_rings_ends_peer` fails intermittently under full-suite load; see `RELIABILITY_REVIEW.md`.

The vendored audio bridge has its own unit tests: `cargo test --locked -p rvoip-audio-device --lib` (4 passed).

## Covered operations

| Operation | Assertions |
| --- | --- |
| Registration | Digest 407 challenge, successful registration, refresh, rejected credentials, unregister/disconnect, registration deadline |
| Outgoing calls | Dial, ringing, answer, busy response, cancellation, no-answer deadline, reject a second active call |
| Incoming calls | Caller display sanitization, answer, decline, remote cancel, reject a second caller as busy |
| Connected controls | Mute/unmute state, negotiated DTMF received at peer, reject invalid tones, reject device changes during call |
| Hang-up | Local, remote, and simultaneous hang-up, outgoing and incoming calls, lifecycle/state release, repeated calls after hang-up |
| Shutdown | Connected call, incoming ringing call, stopped actor rejects commands, disconnect prevents redial |
| Failure recovery | Expired registration/teardown, stale call controls, invalid account/dial target, invalid secure destination |
| Provider routing | Private Contact with public Record-Route, outbound proxy without Record-Route, 407 retry, private Record-Route followed by receiver hang-up |
| Incoming ringtone | Tone cadence and level bounds, immediate silence on cancellation, speaker-release handshake; optional real/virtual output smoke test |
| Codecs | Opus ↔ Opus negotiation with a 6 kHz tone carried end to end at 48 kHz; Opus-first ↔ G.711-only fallback in both directions; offer lists `111 0 8 101` with Opus fmtp; Asterisk-style Opus on PT 107 answered as Opus without AMR fmtp; codec preference order and saved-settings default |
| Audio bridge | 20 ms frame shape per format, stereo interleave and mono mixdown, Opus pass-through at device rate, telephone band limit for G.711 only, drift- and click-free 44.1 → 48 kHz streaming |
| Audio selection | System defaults, named devices, disconnected input/output |
| UI | Actual Hang up pointer click while another operation is pending, duplicate request suppression |
| Diagnostics | Append across sessions and bounded rotation |

Actor integration tests are in `src/call_operations_tests.rs`; provider packet checks are in `src/sip_routing_tests.rs`; codec negotiation checks are in `src/codec_tests.rs`. Unit and loopback tests cover validation, privacy, and repeated call teardown.

## Additional acceptance checks

The optional PBX test is run with:

```sh
cargo test --locked pbx_tests -- --ignored --nocapture
```

See `RELIABILITY_REVIEW.md` for test-account variables and dialplan setup. This test places calls to the configured test PBX.

Test real devices and your provider separately for audible two-way voice (with Opus and with *G.711 only*), unplug recovery, TLS certificate handling, sustained SRTP, and network/NAT failures. Default tests do not establish these. Hold, transfer, conferencing, and multiple accounts are not implemented.

Proxy tests cover numeric addresses and `localhost`. They check that a hostname Route never falls back to private Contact, with or without a private Record-Route. The simulated provider sends 407, 180, 200, and receiver BYE.

To verify an explicitly configured audio output, run `cargo test --locked ringtone_device_stream_starts_and_stops -- --ignored --nocapture`. Use `ALSA_CONFIG_PATH` to select a virtual ALSA output when testing without a physical speaker.
