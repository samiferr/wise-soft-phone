//! Codec negotiation regressions on loopback SIP endpoints. Endpoints use the
//! same offered-codec lists as the app (`CodecPreference`), exchange real
//! Opus-encoded RTP, and inspect the SDP the stack puts on the wire.
use std::time::Duration;

use rvoip_sip::{AudioFrame, Config, Endpoint, EndpointCall, EndpointProfile};
use tokio::{net::UdpSocket, time::timeout};

use crate::account::CodecPreference;
use crate::audio::bridge_format;
use crate::sip_routing_tests::header;

const WAIT: Duration = Duration::from_secs(5);

fn unused_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn endpoint(name: &str, port: u16, codecs: CodecPreference) -> Endpoint {
    let offered = codecs.payload_types();
    Endpoint::builder()
        .profile(EndpointProfile::Custom(Config::local(name, port)))
        .configure(move |config| config.offered_codecs = offered)
        .build()
        .await
        .unwrap()
}

/// Place a call from `caller` to `callee` preferences; return both legs.
async fn connect(
    caller: CodecPreference,
    callee: CodecPreference,
) -> (EndpointCall, EndpointCall, Endpoint, Endpoint) {
    let callee_port = unused_port();
    let alice = endpoint("alice", unused_port(), caller).await;
    let mut bob = endpoint("bob", callee_port, callee).await;
    let target = format!("sip:bob@127.0.0.1:{callee_port}");
    let (outgoing, incoming) = tokio::join!(alice.call_and_wait(&target, Some(WAIT)), async {
        let incoming = timeout(WAIT, bob.wait_for_incoming())
            .await
            .unwrap()
            .unwrap();
        incoming.answer().await.unwrap()
    });
    (outgoing.unwrap(), incoming, alice, bob)
}

async fn negotiated(call: &EndpointCall) -> rvoip_sip::NegotiatedAudio {
    timeout(WAIT, crate::audio::negotiated_media(call))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opus_endpoints_negotiate_opus_and_carry_wideband_audio() {
    let (caller, callee, _alice, _bob) =
        connect(CodecPreference::OpusFirst, CodecPreference::OpusFirst).await;
    let caller_media = negotiated(&caller).await;
    let callee_media = negotiated(&callee).await;
    for media in [&caller_media, &callee_media] {
        assert!(media.codec.eq_ignore_ascii_case("opus"), "{media:?}");
        assert_eq!((media.sample_rate, media.payload_type), (48_000, 111));
    }

    // Send what the device bridge would send: 20 ms frames in the negotiated
    // shape, carrying a 6 kHz tone that G.711 could not represent.
    let format = bridge_format(&caller_media);
    let channels = usize::from(format.channels);
    let frame_samples = format.frame_samples();
    let (sender, _caller_rx) = caller.as_session_handle().audio().await.unwrap().split();
    let (_callee_tx, mut receiver) = callee.as_session_handle().audio().await.unwrap().split();
    let send = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(20));
        for frame in 0..100u32 {
            ticker.tick().await;
            let start = frame as usize * frame_samples;
            let samples = (start..start + frame_samples)
                .flat_map(|n| {
                    let phase = 2.0 * std::f32::consts::PI * 6_000.0 * n as f32 / 48_000.0;
                    std::iter::repeat_n((phase.sin() * 8_000.0) as i16, channels)
                })
                .collect();
            let timestamp = frame * frame_samples as u32;
            let frame = AudioFrame::new(samples, format.sample_rate, format.channels, timestamp);
            if sender.send(frame).await.is_err() {
                break;
            }
        }
    });

    let mut loud_frames = 0;
    let deadline = tokio::time::Instant::now() + WAIT;
    while loud_frames < 10 && tokio::time::Instant::now() < deadline {
        let Ok(Some(frame)) = timeout(WAIT, receiver.recv()).await else {
            break;
        };
        assert_eq!(frame.sample_rate, 48_000);
        assert_eq!(frame.channels, callee_media.channels);
        let power = frame
            .samples
            .iter()
            .map(|s| f64::from(*s).powi(2))
            .sum::<f64>()
            / frame.samples.len().max(1) as f64;
        if power.sqrt() > 1_000.0 {
            loud_frames += 1;
        }
    }
    send.abort();
    assert!(
        loud_frames >= 10,
        "a 6 kHz tone must survive the Opus path ({loud_frames} loud frames)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opus_offer_falls_back_to_g711_for_g711_only_peer() {
    let (caller, callee, _alice, _bob) =
        connect(CodecPreference::OpusFirst, CodecPreference::G711Only).await;
    for media in [negotiated(&caller).await, negotiated(&callee).await] {
        assert_eq!(
            (media.codec.as_str(), media.sample_rate, media.channels),
            ("PCMU", 8_000, 1)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g711_only_offer_is_answered_with_g711() {
    let (caller, callee, _alice, _bob) =
        connect(CodecPreference::G711Only, CodecPreference::OpusFirst).await;
    for media in [negotiated(&caller).await, negotiated(&callee).await] {
        assert_eq!(media.codec, "PCMU");
        assert_eq!(bridge_format(&media).frame_samples(), 160);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_offer_lists_opus_first_with_fmtp() {
    let pbx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let pbx_port = pbx.local_addr().unwrap().port();
    let alice = endpoint("alice", unused_port(), CodecPreference::OpusFirst).await;
    let send = tokio::spawn(async move {
        let _ = alice
            .invite(&format!("sip:pbx@127.0.0.1:{pbx_port}"))
            .unwrap()
            .send()
            .await;
        alice
    });
    let mut buffer = vec![0; 8192];
    let (size, _) = timeout(WAIT, pbx.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let invite = String::from_utf8_lossy(&buffer[..size]).into_owned();
    assert!(invite.starts_with("INVITE "), "{invite}");
    let m_line = invite.lines().find(|l| l.starts_with("m=audio")).unwrap();
    assert!(m_line.ends_with("RTP/AVP 111 0 8 101"), "{m_line}");
    assert!(invite.contains("a=rtpmap:111 opus/48000/2"), "{invite}");
    assert!(
        invite.contains("a=fmtp:111 minptime=10; useinbandfec=1"),
        "{invite}"
    );
    send.abort();
}

/// Asterisk numbers Opus 107, which RVoIP itself uses for AMR-NB. The answer
/// must keep the peer's number and must not carry AMR framing parameters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_opus_on_peer_payload_type_is_answered_as_opus() {
    let bob_port = unused_port();
    let mut bob = endpoint("bob", bob_port, CodecPreference::OpusFirst).await;
    let pbx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let pbx_port = pbx.local_addr().unwrap().port();
    let sdp = "v=0\r\no=pbx 1 1 IN IP4 127.0.0.1\r\ns=Asterisk\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 36000 RTP/AVP 107 0 101\r\na=rtpmap:107 opus/48000/2\r\na=fmtp:107 useinbandfec=1\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=ptime:20\r\na=sendrecv\r\n";
    let invite = format!(
        "INVITE sip:bob@127.0.0.1:{bob_port} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pbx_port};branch=z9hG4bK-opus107;rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:pbx@127.0.0.1:{pbx_port}>;tag=pbx-opus\r\n\
         To: <sip:bob@127.0.0.1:{bob_port}>\r\n\
         Call-ID: opus107-{pbx_port}@127.0.0.1\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:pbx@127.0.0.1:{pbx_port}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n{sdp}",
        sdp.len()
    );
    pbx.send_to(invite.as_bytes(), ("127.0.0.1", bob_port))
        .await
        .unwrap();
    let incoming = timeout(WAIT, bob.wait_for_incoming())
        .await
        .unwrap()
        .unwrap();
    let answer = tokio::spawn(async move { incoming.answer().await });

    let mut buffer = vec![0; 8192];
    let ok = timeout(WAIT, async {
        loop {
            let (size, _) = pbx.recv_from(&mut buffer).await.unwrap();
            let packet = String::from_utf8_lossy(&buffer[..size]).into_owned();
            if packet.starts_with("SIP/2.0 200") {
                return packet;
            }
        }
    })
    .await
    .expect("200 OK");
    let m_line = ok.lines().find(|l| l.starts_with("m=audio")).unwrap();
    assert!(m_line.contains("RTP/AVP 107"), "{m_line}");
    assert!(ok.contains("a=rtpmap:107 opus/48000/2"), "{ok}");
    assert!(
        ok.contains("a=fmtp:107 minptime=10; useinbandfec=1"),
        "{ok}"
    );
    assert!(
        !ok.contains("octet-align") && !ok.contains("max-red"),
        "{ok}"
    );

    let ack = format!(
        "ACK sip:bob@127.0.0.1:{bob_port} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{pbx_port};branch=z9hG4bK-opus107-ack;rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:pbx@127.0.0.1:{pbx_port}>;tag=pbx-opus\r\n\
         To: {}\r\n\
         Call-ID: opus107-{pbx_port}@127.0.0.1\r\n\
         CSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n",
        header(&ok, "To")
    );
    pbx.send_to(ack.as_bytes(), ("127.0.0.1", bob_port))
        .await
        .unwrap();
    let call = timeout(WAIT, answer).await.unwrap().unwrap().unwrap();
    let media = negotiated(&call).await;
    assert!(media.codec.eq_ignore_ascii_case("opus"), "{media:?}");
    assert_eq!((media.payload_type, media.sample_rate), (107, 48_000));
}
