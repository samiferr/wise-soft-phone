# Softphone

A Rust desktop app for voice calls over SIP, tested on Linux. It uses **egui / eframe 0.37** with OpenGL for the interface and CPAL for the microphone and speaker. **RVoIP 0.4** handles SIP Digest registration, SDP, G.711 (PCMU / PCMA), RTP / SRTP, and RFC 4733 keypad tones.

## System documentation

The [documentation index](docs/README.md) covers the app's purpose, context, design, behavior, data, development, and deployment. Small Mermaid and SVG diagrams explain each part using the supplied description method.

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

Cargo builds the whole app from `src/` and the root `Cargo.toml`. No JavaScript, webview, Tauri CLI, or Node tools are needed.

## Android APK

The ARM64 distribution runs on Android 9 or later. Open the APK on the phone and allow microphone access for calls. Keep the app open to receive incoming calls. Android may suspend the SIP connection in the background.

To rebuild on Linux or macOS, install JDK 17, Android SDK Platform 35, Build Tools 35.0.1, and NDK r28c, and set `JAVA_HOME`, `ANDROID_HOME`, and `ANDROID_NDK_HOME` to those installations. Step-by-step setup commands are in the [Android APK build guide](docs/android-build.md). From the repository root, run:

```sh
rustup target add aarch64-linux-android
android/package-apk.sh
```

The script builds and verifies `dist/wise-softphone-0.2.0-arm64-v8a.apk`. It creates the signing key and password in the private, Git-ignored `.android-signing/` directory on the first build. Back up that directory securely to sign subsequent updates with the same identity. Saved SIP passwords on Android use an Android Keystore key.

## Connect to a PBX

1. Enter the server hostname or IPv4 address, SIP username, and password. Append `:port` for a custom server port.
2. Under **Connection options**, enter a separate authentication username if required. Select **TLS + required SRTP** for a compatible secure PBX. Default ports are UDP 5060 and TLS 5061. Secure mode validates the server certificate against the system trust store and requires SDES-SRTP.
3. Choose **Sign In**. History opens when registration succeeds. Open **Keypad** to dial an extension or `sip:1002@pbx.example.com`.
4. Use **Answer**, **Decline**, **Hang up**, and **Mute** as appropriate. During a connected call, the keypad sends DTMF. Choose your microphone and speaker between calls under **Menu → Settings**.

The phone-sized window supports light and dark appearances. Search or filter recent calls in **History**. Selecting a call or a saved contact fills the keypad. Press **Call** to place the call. **Menu → Profile** lets you edit your display name and description or log out. The latest 500 calls, contacts, and profile are stored locally per SIP account.

To save both sides of connected calls, turn on the **Record calls** switch in **Menu → Settings**. Recording is off by default, applies to the next call, and shows a red "● Recording" label during the call. In **Call history**, tap the record icon on a call to open its recording controls: play, pause, stop, share, and delete (with confirmation). Share opens the Android share sheet, or shows the file in your file manager on desktop. On Android the app fills the whole screen and keeps controls clear of system bars.

The app supports one account and one call. RVoIP refreshes registration. After a connection failure, register again. If registration fails while the endpoint is still open, choose **Disconnect** first. Calls are not redialed automatically.

**Remember in system keyring** saves the password in the OS credential store. Leave the password field empty to use the saved password for that account. The form clears passwords after submission; account JSON never contains them. Existing Linux settings still work at `$XDG_CONFIG_HOME/com.softphone.desktop/account.json` or `~/.config/com.softphone.desktop/account.json`.

Incoming calls request desktop attention, show answer controls, and ring through the selected speaker. Ringing stops when you answer, decline, disconnect, or the caller cancels. Keep the app running to receive calls. Closing the window ends the call and unregisters, with a time limit on shutdown.

## Asterisk interoperability

Use a reachable PJSIP extension with Digest authentication, `allow=ulaw,alaw`, `dtmf_mode=rfc4733`, and `direct_media=no`. Configure `rewrite_contact=yes` and `force_rport=yes` as needed. For secure calls, enable TLS with a trusted certificate and `media_encryption=sdes`.

This build targets a PBX reachable over an IPv4 LAN or VPN. IPv6, ICE/TURN configuration, hold, transfer, conferencing, video, and messaging are outside this implementation.

See [TESTING.md](TESTING.md) for covered operations and test limits.

## Verification

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover account and destination checks, commands for ended calls, caller label cleanup, and shutdown. [RELIABILITY_REVIEW.md](RELIABILITY_REVIEW.md) records fixes, integration results, and remaining limits. Test a physical headset and your provider before daily use.

## Diagnostic log

Each run adds records to `logs/softphone.log` in this project folder. The startup message shows the full path. Logs include actions, state changes, timings, registration updates, and the direction and numeric response codes of SIP REGISTER / INVITE / ACK / BYE / CANCEL messages. Separate records for clicks and backend receipt help locate delays.

At 5 MiB, the log rotates and keeps the previous file as `logs/softphone.log.1`. Share both files after reproducing a problem. Set `SOFTPHONE_LOG_DIR` to change the directory. Git ignores logs. They exclude passwords, dialed digits, caller addresses, authentication headers, SDP, and full SIP messages. Socket addresses and internal call IDs help match related events.

Call controls are fixed above the scrolling account/dialer area. Escape hangs up an active call or declines a ringing call.

Twilio accounts use their SIP domain as the outbound proxy so ACK and BYE reach the provider even when Contact contains a private address. Set a custom proxy under **Connection options → Outbound proxy**. Other PBX accounts use direct routing unless you set a proxy. The bundled `rvoip-sip-dialog` patch keeps this route in 2xx ACKs, including after authentication retries; see `vendor/rvoip-sip-dialog/SOFTPHONE_PATCH.md`.

The log startup marker `routing_revision="ack-hostname-v3"` identifies the build containing hostname-proxy ACK resolution.
