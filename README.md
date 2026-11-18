# Softphone

A native Rust desktop phone for voice calls over SIP. The interface uses **egui / eframe 0.37** with OpenGL. **RVoIP 0.4** handles SIP Digest registration, SDP, G.711 (PCMU / PCMA), RTP / SRTP, and RFC 4733 keypad tones. CPAL connects the microphone and speaker. Linux is the validated target.

## Build and run

Install Rust 1.95 or newer. On Debian / Ubuntu, install the native dependencies:

```sh
sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-dev libwayland-dev libx11-dev libxi-dev libgl1-mesa-dev
cargo run --locked
```

For an optimized binary:

```sh
cargo build --locked --release
./target/release/softphone
```

No JavaScript, webview, Tauri CLI, or Node tooling is needed. The source is now under `src/`, with a root `Cargo.toml`.

## Connect to a PBX

1. Enter the server hostname or IPv4 address, SIP username, and password. Append `:port` for a custom server port.
2. Under **Connection options**, enter a separate authentication username if required. Select **TLS + required SRTP** for a compatible secure PBX. Default ports are UDP 5060 and TLS 5061. Secure mode validates the server certificate against the system trust store and requires SDES-SRTP.
3. Choose **Register account**. Once registered, dial an extension or `sip:1002@pbx.example.com`.
4. Use **Answer**, **Decline**, **Hang up**, and **Mute microphone** as appropriate. During a connected call, the keypad sends DTMF. Microphone and speaker selection is available between calls.

One account and one call are supported. Registration refresh is handled by RVoIP. After a connection failure, register again. If registration fails while the endpoint remains open, choose **Disconnect** before retrying. There is no automatic redial.

**Remember in system keyring** stores the password in the OS credential store. An empty password field uses the saved credential for the same account. Passwords are cleared from the form after submission and never saved in account JSON. Existing Linux settings remain compatible at `$XDG_CONFIG_HOME/com.softphone.desktop/account.json` or `~/.config/com.softphone.desktop/account.json`.

Incoming calls request desktop attention and show answer controls. Incoming calls play a repeating ringtone on the selected speaker. The tone stops on answer, decline, remote cancellation, or disconnect. Keep the application running to receive calls. Closing the window ends the call and unregisters, with a bounded shutdown wait.

## Asterisk interoperability

Use a reachable PJSIP extension with Digest authentication, `allow=ulaw,alaw`, `dtmf_mode=rfc4733`, and `direct_media=no`. Configure `rewrite_contact=yes` and `force_rport=yes` as needed. For secure calls, enable TLS with a trusted certificate and `media_encryption=sdes`.

This build targets a PBX reachable over an IPv4 LAN or VPN. IPv6, ICE/TURN configuration, hold, transfer, conferencing, video, and messaging are outside this implementation.

See [TESTING.md](TESTING.md) for the complete operation coverage matrix and test boundaries.

## Verification

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The tests cover account and target validation, stale call commands, caller label sanitization, and shutdown. See [RELIABILITY_REVIEW.md](RELIABILITY_REVIEW.md) for fixes, integration evidence, and remaining limits. Physical headset audio and a real provider should be checked before daily use.

## Diagnostic log

Each run appends to `logs/softphone.log` in this project folder. The startup message prints its full path. The file records application actions, state transitions, operation durations, registration updates, and SIP REGISTER / INVITE / ACK / BYE / CANCEL directions and numeric response codes. Call-control clicks are recorded separately from backend receipt so input and signaling delays can be distinguished.

The log rotates at 5 MiB, retaining `logs/softphone.log.1`. Share both files after reproducing a problem. `SOFTPHONE_LOG_DIR` overrides the directory when needed. Logs are ignored by version control. Passwords, dialed digits, caller addresses, authentication headers, SDP, and full SIP messages are excluded. Socket addresses and internal call identifiers are included for correlation.

Call controls are fixed above the scrolling account/dialer area. Escape hangs up an active call or declines a ringing call.

Twilio accounts automatically use their SIP domain as the outbound proxy. This keeps ACK and BYE on the provider edge when a dialog Contact contains a private address. A custom proxy can be entered under **Connection options → Outbound proxy**. Other PBX accounts keep direct routing unless a proxy is configured. The bundled `rvoip-sip-dialog` patch preserves this route in 2xx ACKs, including after authentication retries; see `vendor/rvoip-sip-dialog/SOFTPHONE_PATCH.md`.

The log startup marker `routing_revision="ack-hostname-v3"` identifies the build containing hostname-proxy ACK resolution.
