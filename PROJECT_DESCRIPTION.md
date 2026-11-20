# Voice-only SIP softphone — technical description

See [docs/README.md](docs/README.md) for the full system description in seven views.

## Scope

Softphone is a Rust desktop app for one SIP account and one active voice call. On Linux, it supports registration, incoming and outgoing calls, answer, decline, cancel, hang-up, mute, audio device selection, and in-call DTMF. egui/eframe provides the interface; RVoIP handles signaling and voice.

## Components

| Module | Responsibility |
| --- | --- |
| `src/main.rs` | Native window, Tokio runtime, account location, filtered diagnostics, final shutdown |
| `src/ui.rs` | egui account form, call controls, dial pad, device list, call duration, desktop attention |
| `src/phone.rs` | Serialized call actor, typed state, command validation, SIP events, operation deadlines, teardown |
| `src/account.rs` | Account validation, non-secret JSON persistence, OS keyring, transport and endpoint configuration |
| `src/audio.rs` | CPAL device enumeration, RVoIP device bridge, mute flag, microphone heartbeat |

The UI sends actions to the Phone actor through a Tokio channel with a fixed capacity. Each action has a reply channel. The actor sends complete, read-only status snapshots through a watch channel, waking egui even when hidden. Most UI operations run one at a time; Hang up and Disconnect can still run while another operation is pending. Network work, credential access, and device lookup run off the GUI thread.

The actor owns the endpoint and call. It rejects commands for a call that has ended or changed, using the call ID to check commands and late events. Calls can be incoming, dialing, ringing, connected, or ending. Call audio exists only while connected.

## Signaling and voice

The pinned `rvoip-sip` 0.4 Endpoint API provides Digest registration, refresh, SIP dialog handling, SDP negotiation, Opus, PCMU / PCMA, and RTP telephone events. Opus is offered first by default (payload type 111, `opus/48000/2`), with G.711 as fallback. Local patches in `vendor/rvoip-sip` and `vendor/rvoip-audio-device` expose the negotiated audio format and run the device bridge at that rate (48 kHz for Opus), instead of a fixed 8 kHz. UDP accounts use unencrypted RTP. TLS accounts require trusted certificates and SDES-SRTP. The application chooses a local IPv4 address using the operating system route to the PBX.

`rvoip-audio-device` connects call PCM to CPAL input and output devices. Mute sends silence while keeping RTP packets on schedule. Input callbacks update a heartbeat so the actor can report a microphone stream that never starts or later stops. The heartbeat cannot confirm speaker output or usable audio at the other end.

## Reliability and security

SIP actions have time limits. If a failed dial or answer leaves the call state uncertain, the app disconnects before allowing another call. Outgoing ringing lasts at most 45 seconds, and registration attempts and refreshes also have deadlines. A call stays in the ending state while the SIP stack handles cancellation and late responses, then disconnects if needed. The UI shows network errors; the user must register again to recover.

Account JSON contains no password. Saved passwords use `keyring`, backed by Secret Service on Linux. Logs exclude raw SIP traces and authentication data. Settings use the same path as the earlier Tauri build.

## Build and acceptance

Cargo builds the entire application. eframe uses the OpenGL backend with X11 and Wayland support. Dependencies are pinned by `Cargo.lock`. The minimum compiler is Rust 1.95.

Before release, check registration and refresh, answer and decline, cancellation, busy rejection, repeat calls, mute, DTMF, and shutdown. For secure calls, test trusted TLS/SRTP and rejection of an untrusted certificate or a peer without SRTP. Virtual audio tests check device connections and packets. Real microphone/speaker listening tests and checks against the intended PBX or provider are also required.

References: [egui / eframe](https://github.com/emilk/egui), [RVoIP](https://github.com/eisenzopf/rvoip), [CPAL](https://github.com/RustAudio/cpal).
