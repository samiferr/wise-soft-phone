# Call operation test coverage

Run the default suite from the project folder:

```sh
cargo test --locked
cargo fmt --check
```

Tests use local UDP sockets, so run them where local networking is permitted. The SIP tests create their own endpoints and synthetic provider. They do not call Twilio or the PSTN, use saved credentials, or open a microphone. The phone actor's hardware-audio startup is disabled only in those test fixtures.

Latest local result: **35 passed, 0 failed, 2 optional external-PBX/audio-device tests ignored**.

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
| Audio selection | System defaults, named devices, disconnected input/output |
| UI | Actual Hang up pointer click while another operation is pending, duplicate request suppression |
| Diagnostics | Append across sessions and bounded rotation |

The actor integration cases are in `src/call_operations_tests.rs`. The synthetic provider packet checks are in `src/sip_routing_tests.rs`. Existing unit and loopback tests cover validation, privacy, and repeated SIP teardown.

## Additional acceptance checks

The optional PBX test is run with:

```sh
cargo test --locked pbx_tests -- --ignored --nocapture
```

See `RELIABILITY_REVIEW.md` for its test-account environment and dialplan requirements. This test deliberately places calls to a supplied test PBX.

The default suite does not prove physical microphone/speaker audibility, device unplug recovery, TLS certificate behavior on the intended provider, SRTP media continuity, or all possible network/NAT failures. These require device/provider acceptance tests. Hold, transfer, conference, and multiple accounts are outside the implemented phone operations.

Hostname-proxy regressions use `localhost` as the edge name, in addition to numeric address tests. They verify that a hostname Route cannot fall back to the private Contact, both with a private Record-Route and with no Record-Route. The synthetic provider sends 407, 180, 200, and receiver BYE.

To verify an explicitly configured audio output, run `cargo test --locked ringtone_device_stream_starts_and_stops -- --ignored --nocapture`. Use `ALSA_CONFIG_PATH` to select a virtual ALSA output when testing without a physical speaker.
