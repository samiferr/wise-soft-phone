//! Wire regression for a provider edge with a private dialog Contact.
use crate::account::Account;
use std::time::Duration;
use tokio::{net::UdpSocket, time::timeout};

pub(crate) fn header<'a>(packet: &'a str, name: &str) -> &'a str {
    packet
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then_some(value.trim())
        })
        .unwrap_or("")
}
fn edge_route_count(packet: &str, address: &str) -> usize {
    packet
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("route"))
        .map(|(_, value)| value.matches(address).count())
        .sum()
}

pub(crate) fn reply(request: &str, body: &str, extra: &str) -> String {
    let to = header(request, "To");
    let to = if to.contains(";tag=") {
        to.to_owned()
    } else {
        format!("{to};tag=test-edge")
    };
    format!(
        "SIP/2.0 200 OK\r\nVia: {}\r\nFrom: {}\r\nTo: {to}\r\nCall-ID: {}\r\nCSeq: {}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
        header(request, "Via"),
        header(request, "From"),
        header(request, "Call-ID"),
        header(request, "CSeq"),
        body.len()
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_route_reaches_provider_edge_with_private_contact() {
    check_provider_routing(true, false, false, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outbound_proxy_reaches_provider_edge_without_record_route() {
    check_provider_routing(false, true, false, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_auth_retry_preserves_one_edge_route_for_ack_and_bye() {
    check_provider_routing(false, true, true, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_hangup_after_private_record_route_updates_call_lifecycle() {
    check_provider_routing(true, true, true, true, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostname_proxy_delivers_ack_and_receives_remote_hangup() {
    check_provider_routing(true, true, true, true, true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostname_proxy_without_record_route_delivers_ack_and_bye() {
    check_provider_routing(false, true, true, false, false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostname_record_route_resolves_without_falling_back_to_contact() {
    check_provider_routing(true, false, true, false, true, true).await;
}

async fn check_provider_routing(
    record_route: bool,
    outbound_proxy: bool,
    challenge: bool,
    private_route: bool,
    receiver_hangup: bool,
    hostname_proxy: bool,
) {
    let edge = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = edge.local_addr().unwrap();
    let proxy_address = if hostname_proxy {
        format!("localhost:{}", address.port())
    } else {
        address.to_string()
    };
    let route_address = proxy_address.clone();
    let account = Account {
        registrar: address.to_string(),
        username: "alice".into(),
        outbound_proxy: if outbound_proxy {
            proxy_address
        } else {
            String::new()
        },
        ..Default::default()
    };
    let local = account.endpoint("test-password".into()).await.unwrap();
    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 16384];
        let (size, client) = timeout(Duration::from_secs(3), edge.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        let mut invite = String::from_utf8_lossy(&buffer[..size]).into_owned();
        assert!(invite.starts_with("INVITE "));
        if challenge {
            let challenge = reply(&invite, "", "Proxy-Authenticate: Digest realm=\"test-edge\", nonce=\"abcdef0123456789\", algorithm=MD5, qop=\"auth\"\r\n")
                .replacen("SIP/2.0 200 OK", "SIP/2.0 407 Proxy Authentication Required", 1);
            edge.send_to(challenge.as_bytes(), client).await.unwrap();
            let (size, _) = timeout(Duration::from_secs(2), edge.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert!(String::from_utf8_lossy(&buffer[..size]).starts_with("ACK "));
            let (size, _) = timeout(Duration::from_secs(2), edge.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            invite = String::from_utf8_lossy(&buffer[..size]).into_owned();
            assert!(invite.starts_with("INVITE "));
            assert!(!header(&invite, "Proxy-Authorization").is_empty());
            assert_eq!(
                header(&invite, "Route").matches(&route_address).count(),
                usize::from(outbound_proxy)
            );
        }
        let sdp = "v=0\r\no=bob 1 1 IN IP4 127.0.0.1\r\ns=Test\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 35000 RTP/AVP 0 8 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:8 PCMA/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n";
        let routes = if record_route {
            if private_route {
                "Record-Route: <sip:172.18.61.86:5060;lr>\r\n".into()
            } else {
                format!("Record-Route: <sip:{route_address};lr>\r\n")
            }
        } else {
            String::new()
        };
        let extra = format!(
            "Contact: <sip:bob@172.18.61.86:5060>\r\n{routes}Content-Type: application/sdp\r\n"
        );
        // Exercise the early-dialog path used by real providers, not just a
        // direct final answer. Keep the same remote tag for 180 and 200.
        edge.send_to(
            reply(&invite, "", "")
                .replacen("200 OK", "180 Ringing", 1)
                .as_bytes(),
            client,
        )
        .await
        .unwrap();
        edge.send_to(reply(&invite, sdp, &extra).as_bytes(), client)
            .await
            .unwrap();
        let (size, _) = timeout(Duration::from_secs(2), edge.recv_from(&mut buffer))
            .await
            .expect("ACK must reach public edge")
            .unwrap();
        let ack = String::from_utf8_lossy(&buffer[..size]).into_owned();
        assert!(
            ack.starts_with("ACK sip:bob@172.18.61.86:5060 SIP/2.0"),
            "preserve private Contact as request URI"
        );
        assert_eq!(
            edge_route_count(&ack, &route_address),
            1,
            "one edge route in ACK"
        );
        if receiver_hangup {
            let from = header(&invite, "To");
            let bye = format!(
                "BYE {} SIP/2.0\r\nVia: SIP/2.0/UDP {address};branch=z9hG4bK-remote-end\r\nFrom: {from};tag=test-edge\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: 900 BYE\r\nMax-Forwards: 70\r\nContent-Length: 0\r\n\r\n",
                header(&invite, "Contact").trim_matches(['<', '>']),
                header(&invite, "From"),
                header(&invite, "Call-ID")
            );
            edge.send_to(bye.as_bytes(), client).await.unwrap();
            let (size, _) = timeout(Duration::from_secs(2), edge.recv_from(&mut buffer))
                .await
                .expect("client must acknowledge receiver hang-up")
                .unwrap();
            assert!(String::from_utf8_lossy(&buffer[..size]).starts_with("SIP/2.0 200"));
            return;
        }
        let (size, _) = timeout(Duration::from_secs(2), edge.recv_from(&mut buffer))
            .await
            .expect("BYE must reach public edge")
            .unwrap();
        let bye = String::from_utf8_lossy(&buffer[..size]).into_owned();
        assert!(bye.starts_with("BYE sip:bob@172.18.61.86:5060 SIP/2.0"));
        assert_eq!(
            edge_route_count(&bye, &route_address),
            1,
            "one edge route in BYE"
        );
        edge.send_to(reply(&bye, "", "").as_bytes(), client)
            .await
            .unwrap();
    });
    let call = local
        .call_and_wait(&format!("sip:bob@{address}"), Some(Duration::from_secs(2)))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    timeout(Duration::from_secs(3), async {
        if receiver_hangup {
            call.wait_for_end(Some(Duration::from_secs(2))).await
        } else {
            call.hangup_and_wait(Some(Duration::from_secs(2))).await
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    server.await.unwrap();
    local.shutdown().await.unwrap();
}
