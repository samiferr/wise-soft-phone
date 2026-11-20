# Local RVoIP 0.4.0 patch

Source: https://crates.io/crates/rvoip-sip/0.4.0
Upstream: https://github.com/eisenzopf/rvoip
License: MIT.

Only what the library build needs is vendored: `src`, `config`, `state_tables`, the manifest, and the README. Upstream test, bench, and example targets were removed from `Cargo.toml` because their sources are not included. `SOFTPHONE.patch` shows the exact source change relative to crates.io 0.4.0.

## Negotiated audio accessor

`SessionHandle::negotiated_audio()` returns the call's negotiated codec name, sample rate, channel count, and payload type (`NegotiatedAudio`, re-exported from the crate root). It reads the existing internal `negotiated_media_config_exact`.

The application needs this before it starts the device bridge. RVoIP rejects any frame whose rate or channel count differs from the negotiated codec, and the first rejected frame ends the call's send task. Opus is negotiated as 48 kHz with one or two channels, so the bridge cannot assume 8 kHz mono.

## Opus fmtp

- Offers: PT 111 carries `a=fmtp:111 minptime=10; useinbandfec=1` (RFC 7587 §7). Upstream sent no fmtp.
- Answers: a dynamic payload type (≥ 96) takes its fmtp from the codec named in the offer's rtpmap, not from this stack's own numbering. Asterisk offers Opus on 107, which RVoIP uses for AMR-NB octet-aligned. Upstream therefore answered Opus with `octet-align=1; max-red=0`. A telephone-event payload on a non-101 number now gets `0-15`, and other unknown dynamic payloads get no fmtp.

`src/codec_tests.rs` in the application covers both: the outgoing offer's m-line order and fmtp, and an Asterisk-style INVITE with Opus on PT 107.

Keep this patch until an upstream release exposes the negotiated format and keys answer fmtp on the codec name.
