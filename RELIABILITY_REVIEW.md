# Reliability review

Softphone moved from Tauri to egui/eframe. SIP and audio still use Rust and RVoIP 0.4. This review records fixes, test results, and remaining work. Test counts below belong to each stage; the latest results appear at the end.

## Fixed

- Replaced string call states with an enum and kept call-ID checks. Delayed controls cannot affect a new call.
- Added time limits for dial, answer, decline, busy responses, device lookup, credential access, and registration queries. Failed dial or answer operations reset the endpoint when the dialog state is uncertain.
- Added a 30-second registration watchdog, kept outgoing-call and teardown deadlines, and made network errors visible.
- Replaced the one-time audio-start check with a continuing microphone heartbeat. The first health warning logs audio queue counters, excluding SIP credentials and packet contents.
- Fixed caller-ID parsing for valid From headers with SIP tags. Displayed labels remove URI passwords and routing parameters.
- Rejected malformed destination names/ports, header injection, and insecure use of a sips destination.
- Made credential saves run one at a time and staged non-secret settings before keyring changes. A timed-out keyring worker cannot overlap another save in this process.
- Kept network and device work off the GUI thread. Status changes wake the UI even when hidden. Errors appear above the scrolling controls.

## Early verification

The default Cargo suite passed 29 tests, including a loopback SIP test that checked remote hang-up and a successful second call. The test needs local UDP sockets. An ignored PBX test in `src/phone.rs` requires an isolated server.

During migration, the UDP PBX test passed registration, busy rejection, cancellation, three connected calls, mute/unmute, DTMF, stale-control rejection, and shutdown while ringing. Asterisk logs confirmed DTMF delivery. Native-window checks confirmed rendering and incoming-call answering. Audio used virtual ALSA devices rather than a physical headset.

The TLS/SRTP test connected but failed the sustained microphone-health check after six seconds. The cause remains unresolved; the temporary PBX and logs were unavailable when work resumed. **Secure audio is not release-validated.** Earlier short secure-call tests do not prove sustained audio reliability.

## Hang-up fix after user testing

The supplied log stopped at `Call connected` with no hang-up request or result. ALSA/JACK device-lookup messages alone did not show a signaling failure.

Hang up and Disconnect now work while another UI operation is pending, with separate checks against duplicate requests. Hang-up runs in the background so the actor can keep handling SIP events. It allows the default 32-second UDP transaction lifetime within a 34-second outer deadline. If the end cannot be confirmed, the app closes the endpoint and warns that the remote call may remain active. Outgoing cancellation keeps SIP timing and offers Disconnect now. Logs record requests, pending cancellation, and resets without caller or credential data.

Tests cover teardown during another pending operation and two successive loopback calls whose remote endpoints confirm termination. The external PBX test remains optional.

## Remaining limitations

- Investigate the TLS audio warning using the PBX test and audio queue logs. Check transport readiness, outbound queue closure/overload, and physical devices before approving secure calls.
- The RVoIP audio bridge starts a thread without confirming startup. Its heartbeat detects missing capture but cannot confirm speaker playback or remote audibility. To recover a device, end the call and select a working one.
- Keyring work cannot be forcibly canceled; a timed-out worker may finish later. File and keyring changes are not one atomic operation. A crash or failed final rename may require entering the password again.
- Shutdown has time limits and makes a best effort. A forced stop or network failure can leave PBX registration active until expiry.
- Reconnection is manual. Hold, transfer, multiple accounts, IPv6, and configurable ICE/TURN remain unsupported. Audible ringing was also absent at this stage; the later ringtone work is recorded below.
- Before daily use, test a real headset, unplug/replug, network loss, certificate rejection, missing SRTP, and the intended provider.

## Repeat the PBX test

Use an isolated Asterisk account with G.711 and RFC 4733. Extension 600 must answer with an IVR/echo, 601 must ring, and 602 must return busy. Set:

- `SOFTPHONE_TEST_SERVER` — IPv4 address/hostname and optional port
- `SOFTPHONE_TEST_USER` and `SOFTPHONE_TEST_PASSWORD`
- `SOFTPHONE_TEST_TLS=1` for TLS with required SDES-SRTP
- `ALSA_CONFIG_PATH` for an isolated virtual audio setup, if used

Run `cargo test --locked pbx_tests -- --ignored --nocapture`. Use a dedicated test account: the test places calls and unregisters it.

## Follow-up PBX hang-up logs

The next user test showed incoming and outgoing hang-up reaching the backend. Both hit the earlier five-second deadline, leaving the remote call active. Resetting the endpoint did not end the remote call. The exact PBX failure remained unresolved at this stage.

The revised build kept handling events during teardown, offered Disconnect now, and logged BYE/CANCEL direction, transport, destination socket, and numeric response status. It excluded full SIP messages, request URIs, authentication headers, and SDP. The next provider test needed to show whether BYE was sent and its response received. Passing loopback tests did not prove compatibility with the user's PBX.

## Concrete SIP port and persistent logs

A four-call regression reproduced failed remote termination when the SIP identity used port zero. Choosing a listening port before building the endpoint made local and remote termination complete in under one second. Production setup now uses that port for UDP and TLS. Another process can take it before RVoIP binds; a collision causes endpoint startup to fail.

Call controls now stay above the scrolling area, and Escape ends the call. An egui pointer-click test checks submission while another action is pending. A call-lifecycle observer supplements endpoint end events and is canceled when the call is released.

The latest Twilio log at this stage showed an incoming BYE acknowledged in under one millisecond and UI call release about four milliseconds later. It had no local Hang-up request. The port and UI fixes still needed a Twilio test; local results did not prove provider compatibility.

Logs now append to `logs/softphone.log`, rotate at 5 MiB, and keep one previous file. They record action receipt/completion, timings, state and registration changes, and filtered SIP method/status/direction/socket data. They exclude full SIP and credential-bearing fields. Git ignores logs. Tests cover rotation and reopening.

## Twilio private-Contact routing fix

Logs showed INVITE/REGISTER reaching public edge `54.172.60.0:5060`, while 2xx ACK and local BYE went to private `172.18.61.86:5060`. Repeated INVITE 200 responses showed ACK was not reaching the provider. BYE retried for 32 seconds. This was separate from the port-zero problem.

Twilio domains now set an outbound proxy automatically. The vendored RVoIP patch keeps initial INVITE proxy routes in 2xx ACK when Record-Route is absent. RVoIP already uses the proxy for BYE. Dialog route sets stay unchanged to avoid duplicate proxy entries. At this stage, learned Record-Route took precedence, and private Contact stayed in the request URI.

Synthetic UDP provider tests used private Contact with and without Record-Route, including a 407 Digest challenge. They checked ACK/BYE arrival at the edge and local teardown in under one second. Live Twilio still needed another call. The existing diagnostic log was kept.

Reference: https://www.twilio.com/docs/sip-trunking/troubleshooting

After this ACK patch, 13 tests passed and one external-PBX test remained optional. All three simulated edge-routing cases passed, including 407 authentication and exactly one Route hop in ACK/BYE. Formatting and the native debug build passed. Live Twilio still needed a fresh call.

## Broader call tests and receiver hang-up

The expanded default suite passed 29 tests, with one optional PBX test. `TESTING.md` maps operations to coverage. Tests use the real Phone actor with local SIP endpoints and simulated provider packets. Only these fixtures disable audio hardware startup.

A receiver-hang-up test reproduced a private Record-Route case missed by earlier tests. The ACK patch now puts the configured edge before a private IPv4 Record-Route hop, keeping provider routing and Contact values. Packet tests confirm ACK reaches the edge and remote BYE is acknowledged and seen by the call lifecycle.

The suite also found a shutdown race. Disconnect now waits up to two seconds for background BYE/CANCEL before closing transport. Completion receivers are cleared after use, keeping cancellation safe if another actor select branch wins. Local registration, refresh, mute, DTMF, cancellation, busy, repeat-call, timeout, and shutdown tests passed. Live Twilio and physical audio still needed separate checks.

Validation at this stage: 29 passed, 0 failed, 1 external-PBX test ignored. The native build, formatting, and application Clippy checks with warnings denied passed. The vendored dependency emitted existing compiler deprecation warnings.

## Hostname ACK routing and incoming ringtone

The next live trace still sent ACK to private provider addresses, while BYE used the public edge and finished quickly. ACK transport handled only numeric next hops and fell back to Contact for a hostname Route. Tests using a localhost proxy reproduced this. The patch now reuses the INVITE's selected edge/flow when the next hop is unchanged, or resolves a different route hostname. Startup logs identify the fix as `routing_revision="ack-hostname-v3"`.

Incoming calls now play a repeating tone on the selected output. Answer, decline, cancel, and disconnect silence it immediately. Answer waits briefly for speaker release before opening call audio. Tests cover cadence, stopping, and output release. A virtual ALSA test passed and produced nonzero PCM samples; it did not prove physical-speaker audibility.

Final recorded results: native build, formatting, and application Clippy checks passed. The binary contains `ack-hostname-v3`. The default suite had 35 passed and 2 optional tests ignored. The virtual ringtone device test also passed. Live Twilio still needs a call with the rebuilt app; external PBX and audio-device tests remain optional.
