# Voice-only SIP softphone — technical description

## Scope

Softphone is a native Rust desktop application for one SIP account and one active voice call. The Linux implementation supports account registration, incoming and outgoing calls, answer, decline, cancel, hang-up, microphone mute, audio device selection, and in-call DTMF. It uses egui through eframe for the interface and RVoIP for signaling and media.

## Components

| Module | Responsibility |
| --- | --- |
| `src/main.rs` | Native window, Tokio runtime, account location, filtered diagnostics, final shutdown |
| `src/ui.rs` | egui account form, call controls, dial pad, device list, call duration, desktop attention |
| `src/phone.rs` | Serialized call actor, typed state, command validation, SIP events, operation deadlines, teardown |
| `src/account.rs` | Account validation, non-secret JSON persistence, OS keyring, transport and endpoint configuration |
| `src/audio.rs` | CPAL device enumeration, RVoIP device bridge, mute flag, microphone heartbeat |

The UI submits actions through a bounded Tokio channel. Each action has a response channel. The actor publishes immutable snapshots through a watch channel, which wakes egui even when the window is hidden. The UI allows one submitted operation at a time. Network, credentials, and device enumeration run away from the GUI thread.

The actor owns the endpoint and call. Commands for a particular call carry its identifier and are rejected if the call has ended or changed. Call states are incoming, dialing, ringing, connected, and ending. Audio is owned by the connected state and drops when the call leaves it. Late events are matched against the active identifier.

## Signaling and voice

The pinned `rvoip-sip` 0.4 Endpoint API provides Digest registration, refresh, SIP dialog handling, SDP negotiation, PCMU / PCMA, and RTP telephone events. UDP accounts use unencrypted RTP. TLS accounts require trusted certificates and SDES-SRTP. The application chooses a local IPv4 address using the operating system route to the PBX.

`rvoip-audio-device` bridges call PCM to CPAL input and output devices. Mute supplies silence while maintaining the RTP cadence. A periodic input callback updates an atomic heartbeat, allowing the actor to report a microphone stream that fails to start or stops during a call. This does not prove that a speaker is physically audible or that the remote party receives usable audio.

## Reliability and security

SIP actions have deadlines. Uncertain dial and answer failures disconnect the endpoint before another call can start. Outgoing ringing is limited to 45 seconds. An initial or refreshing registration cannot remain pending indefinitely. Teardown retains the ending state while the SIP stack resolves cancellation and late responses, then disconnects if necessary. Network errors are surfaced to the user. Recovery is explicit re-registration.

The account JSON contains no password. Optional saved credentials use `keyring`, backed by Secret Service on Linux. The application enables operational logging but excludes raw SIP traces and authentication material. The account settings path is compatible with the prior Tauri build.

## Build and acceptance

Cargo builds the entire application. eframe uses the OpenGL backend with X11 and Wayland support. Dependencies are pinned by `Cargo.lock`. The minimum compiler is Rust 1.95.

Acceptance checks include registration and refresh, incoming answer and decline, outgoing cancellation, busy rejection, repeated calls after teardown, mute, DTMF, and clean shutdown. Secure-profile checks include a trusted TLS/SRTP call and rejection of an untrusted certificate or a peer without SRTP. Virtual audio tests verify device plumbing and packets. Release acceptance additionally requires real microphone/speaker listening tests and testing against the intended PBX or provider.

References: [egui / eframe](https://github.com/emilk/egui), [RVoIP](https://github.com/eisenzopf/rvoip), [CPAL](https://github.com/RustAudio/cpal).
