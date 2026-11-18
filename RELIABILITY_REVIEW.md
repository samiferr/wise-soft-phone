# Reliability review

The application has been migrated from Tauri to egui / eframe. SIP and audio remain in Rust using RVoIP 0.4. This review distinguishes implemented safeguards from remaining release work.

## Fixed

- Replaced string call states with an enum and preserved call-ID checks. Delayed controls cannot operate on a replacement call.
- Bounded dial, answer, decline, busy response, device enumeration, credential access, and registration queries. Failed dial or answer operations reset the endpoint to clear uncertain dialogs.
- Added a 30-second registration watchdog, retained the outgoing-call deadline and bounded teardown, and surfaced network errors.
- Replaced the one-time audio-start check with an ongoing microphone heartbeat. Audio queue counters are logged on the first health warning, without SIP credentials or packet contents.
- Fixed caller-ID parsing for valid From headers with SIP tags. Displayed URI passwords and routing parameters are removed.
- Rejected malformed destination hostnames and ports, header injection, and insecure use of a sips destination.
- Serialized credential saves and staged non-secret settings before changing keyring entries. A timed-out keyring operation cannot race another save in this process.
- Kept network and device work off the GUI thread. Status changes wake the native interface, including while hidden. Errors appear above the scrolling controls.

## Verification evidence

The default Cargo test suite passes twenty-nine tests, including a loopback SIP test that confirms the remote party sees hang-up and a second call succeeds. Local UDP sockets must be permitted to run that test. An additional explicitly ignored PBX test is included in `src/phone.rs` and requires an isolated test server.

During the migration session, the UDP PBX test passed registration, busy rejection, cancellation, three consecutive connected calls, mute/unmute, DTMF, rejection of stale controls, and shutdown while ringing. The isolated Asterisk log confirmed DTMF delivery. Native-window inspection confirmed rendering and incoming call answering. Audio used virtual ALSA devices, not a physical headset.

The TLS/SRTP test reached a connected call but failed the sustained microphone-health assertion after six seconds. The cause is unresolved. The temporary PBX fixture and logs were unavailable when the session resumed. **Secure audio is not release-validated.** The earlier short secure-call checks do not establish sustained audio reliability.

## Hang-up correction after user testing

The submitted log ended at `Call connected` and did not include a hang-up request or outcome. ALSA/JACK device-enumeration messages alone do not establish a signaling failure.

Hang up and Disconnect now bypass the UI's ordinary pending-operation gate. Duplicate teardown requests are prevented separately. Hang-up runs in a background task while the actor continues consuming SIP events. It allows the default UDP transaction lifetime of 32 seconds, with a 34-second outer deadline. If completion cannot be confirmed, the app closes the endpoint and explicitly warns that the remote call may still be active. Outgoing cancellation retains SIP timing but exposes a Disconnect now control. Request, pending cancellation, and reset outcomes are logged without caller or credential data.

Regression tests cover teardown submission during another pending operation and two successive real loopback SIP calls whose remote endpoints observe termination. The external PBX test remains opt-in.

## Remaining limitations

- Investigate the TLS audio warning with the included PBX test and audio queue diagnostics. Check transport readiness, outbound queue closure/overload, and physical-device behavior before declaring the secure profile ready.
- The RVoIP audio bridge starts a background thread without a startup acknowledgement. The heartbeat detects absent capture but cannot prove speaker playback or remote audibility. Device recovery requires ending the call and selecting a working device.
- System keyring work cannot be forcibly canceled. A timed-out worker may finish later. Filesystem and keyring updates are not one atomic transaction; a crash or final rename failure may require re-entering the password.
- Shutdown is bounded and best effort. Process termination or a failed network can leave a PBX registration until it expires.
- Reconnection is manual. Audible ringing, hold, transfer, multiple accounts, IPv6, and configurable ICE/TURN are not implemented.
- Validate real headset audio, unplug/replug, network loss, certificate rejection, missing SRTP, and the intended provider before daily use.

## Repeating the PBX test

Configure an isolated Asterisk account with G.711 and RFC 4733. Extension 600 must answer with an IVR/echo, 601 must ring, and 602 must return busy. Supply test credentials through these environment variables:

- `SOFTPHONE_TEST_SERVER` — IPv4 address/hostname and optional port
- `SOFTPHONE_TEST_USER` and `SOFTPHONE_TEST_PASSWORD`
- `SOFTPHONE_TEST_TLS=1` for TLS with required SDES-SRTP
- `ALSA_CONFIG_PATH` for an isolated virtual audio configuration, if used

Run `cargo test --locked pbx_tests -- --ignored --nocapture`. Use only a dedicated test account, since the test places calls and unregisters it.

## Follow-up PBX hang-up logs

The user's next test confirmed that incoming and outgoing hang-up reached the backend, both hit the previous five-second deadline, and the remote call stayed active. Resetting the local endpoint did not solve remote termination. The exact PBX failure is still unresolved.

The revised build keeps processing events during teardown, retains a Disconnect now control, and logs BYE/CANCEL direction, transport, destination socket, and numeric response status. Full SIP messages, request URIs, authentication headers, and SDP are not printed. The next provider test should establish whether BYE is emitted and whether its response returns. Loopback SIP regression tests pass; this does not establish interoperability with the user's PBX.

## Concrete SIP port and persistent diagnostics

A new regression exercised local and remote termination over four successive calls. With a port-zero SIP identity, remote termination failed in the synthetic endpoint test. Selecting a concrete listening port before building the endpoint made both directions complete in under one second. The production account builder now uses that concrete port for UDP and TLS. A small bind handoff exists between choosing the port and RVoIP opening it; a collision produces an endpoint startup error.

Call controls now sit above the scrolling area, Escape ends the call, and an actual egui pointer-click regression checks submission while another action is pending. A lifecycle observer complements endpoint terminal events and is canceled when the call is released.

The latest supplied Twilio log showed an incoming BYE acknowledged in under one millisecond and UI call release about four milliseconds after receipt. It did not contain a local Hang-up request. The port correction and UI changes require another Twilio test to confirm behavior on that network; the local regression does not establish provider interoperability.

Persistent diagnostics append to `logs/softphone.log`, rotate at 5 MiB with one previous file, and include action receipt/completion, durations, state changes, registration states, and filtered SIP method/status/direction/socket data. Full SIP messages and credential-bearing fields are excluded. Logs are ignored by Git. The rotation/reopen behavior is tested.

## Twilio private-Contact routing correction

The persistent log establishes the failure: INVITE/REGISTER went to public edge `54.172.60.0:5060`, but the 2xx ACK and local BYE went to private `172.18.61.86:5060`. Repeated 200 INVITE responses show the ACK was not reaching the provider; local BYE retransmitted for 32 seconds. This is distinct from the previously fixed local port-zero issue.

Twilio domains now automatically configure an outbound proxy. The vendored RVoIP ACK patch retains the initial INVITE proxy routes for the 2xx ACK when Record-Route is absent. RVoIP already applies the configured proxy to BYE. Dialog route sets remain unchanged to avoid adding duplicate proxy entries. Learned Record-Route takes precedence and the private Contact stays in the request URI.

Wire tests use a synthetic UDP provider with a private Contact, with and without Record-Route and with a 407 Digest challenge. They check ACK/BYE arrival at the edge and subsecond local teardown. Another Twilio call is required to confirm the live provider path. The existing diagnostic log is preserved.

Reference: https://www.twilio.com/docs/sip-trunking/troubleshooting

Validation after the final ACK patch: 13 tests passed and one external-PBX test remained opt-in. All three synthetic edge-routing cases passed, including 407 authentication and exactly one Route hop in ACK/BYE. Formatting and the native debug build passed. The live Twilio path still requires a fresh user call.

## Expanded call-operation suite and receiver termination

The default suite now has 29 passing tests, with one additional PBX test opt-in. `TESTING.md` maps supported operations to test coverage. Tests exercise the actual phone actor over local SIP endpoints and synthetic provider packets; hardware capture/playback is disabled only in those fixtures.

The receiver-hang-up regression reproduced a private Record-Route case not covered by the earlier no-Record-Route tests. The ACK patch now prefixes the configured edge for a private IPv4 Record-Route hop while retaining the provider's routing and Contact values. Wire tests verify that ACK reaches the edge and remote BYE is acknowledged and observed by the call lifecycle.

The suite also exposed and fixed a shutdown race: disconnect now allows a bounded two-second wait for the background BYE/CANCEL before closing its transport. Completion receivers are cleared after consumption, retaining cancellation safety when an unrelated actor select branch wins. All registration, refresh, mute, DTMF, cancellation, busy, repeat-call, timeout, and shutdown tests pass locally. Live Twilio and physical audio acceptance are still separate checks.

Final expanded-suite validation: 29 passed, 0 failed, 1 external-PBX test ignored. Native build, formatting, and Clippy with warnings denied for application targets passed. The vendored upstream dependency emits existing deprecation warnings on this compiler.

## Hostname ACK routing and incoming ringtone

The next live trace still sent ACK to private provider addresses while BYE used the public edge and completed quickly. The ACK transport code converted only numeric next hops and fell back to Contact when its Route contained a hostname. New localhost-proxy tests reproduced this. The patch now reuses the INVITE's exact selected edge/flow for an unchanged next hop, or explicitly resolves a different route hostname. Startup diagnostics identify this as `routing_revision="ack-hostname-v3"`.

Incoming calls now generate a repeating tone on the selected output device. The tone is silenced immediately on answer/decline/cancel/disconnect; answering allows a bounded speaker-release handshake before call audio opens. The ringer has cadence, stop-signal, and output-release tests. A virtual ALSA output smoke test passed and produced nonzero PCM samples; physical-speaker audibility is not established by that test.

Validation: 35 default tests passed; external PBX and audio-device tests remain opt-in. The virtual audio-device test was additionally run and passed. Live Twilio revalidation still requires a call with the rebuilt app.

Final hostname/ringtone validation: native build, formatting, and application Clippy checks passed. The built binary contains `ack-hostname-v3`. Default suite: 35 passed, 2 optional tests ignored; the virtual ringtone device test additionally passed.
