//! Tests execute the real phone actor and SIP endpoints. Only microphone /
//! speaker startup is disabled so the suite never opens personal devices.
use super::*;
use rvoip_sip::{Config, Endpoint, EndpointProfile};

struct Pair {
    phone: PhoneHandle,
    remote: Endpoint,
    local_target: String,
    remote_target: String,
    actor: tokio::task::JoinHandle<()>,
}

fn unused_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Pair {
    async fn new() -> Self {
        let local_port = unused_port();
        let remote_port = unused_port();
        let local = Endpoint::builder()
            .profile(EndpointProfile::Custom(Config::local("alice", local_port)))
            .build()
            .await
            .unwrap();
        let remote = Endpoint::builder()
            .profile(EndpointProfile::Custom(Config::local("bob", remote_port)))
            .build()
            .await
            .unwrap();
        let (control, events) = local.split();
        let status = Status {
            account: Account::default(),
            registration: Registration::Registered,
            registration_error: None,
            call: None,
            audio: AudioSelection::default(),
            notice: None,
        };
        let (publish, receive) = watch::channel(status.clone());
        let (requests, inbox) = mpsc::channel(16);
        let actor = tokio::spawn(
            Phone {
                account_path: PathBuf::new(),
                link: Some(Link {
                    control,
                    events,
                    registration_deadline: None,
                }),
                call: None,
                teardown: None,
                call_end: None,
                ringer: None,
                skip_audio: true,
                publish,
                status,
            }
            .run(inbox),
        );
        Self {
            phone: PhoneHandle {
                requests,
                status: receive,
            },
            remote,
            actor,
            local_target: format!("sip:alice@127.0.0.1:{local_port}"),
            remote_target: format!("sip:bob@127.0.0.1:{remote_port}"),
        }
    }

    async fn action(&self, action: Action) -> Result<(), String> {
        timeout(Duration::from_secs(4), self.phone.perform(action))
            .await
            .expect("actor stalled")
    }
    async fn wait(&self, predicate: impl Fn(&Status) -> bool) -> Status {
        let mut updates = self.phone.status.clone();
        timeout(Duration::from_secs(3), async {
            loop {
                let value = updates.borrow_and_update().clone();
                if predicate(&value) {
                    return value;
                }
                updates
                    .changed()
                    .await
                    .expect("actor stopped before transition");
            }
        })
        .await
        .expect("call state did not update")
    }
    async fn phase(&self, phase: CallPhase) -> CallStatus {
        self.wait(|s| s.call.as_ref().is_some_and(|c| c.phase == phase))
            .await
            .call
            .unwrap()
    }
    async fn outgoing(&mut self) -> (CallStatus, EndpointCall) {
        self.action(Action::Dial {
            target: self.remote_target.clone(),
        })
        .await
        .unwrap();
        let incoming = timeout(Duration::from_secs(2), self.remote.wait_for_incoming())
            .await
            .unwrap()
            .unwrap();
        let remote_call = incoming.answer().await.unwrap();
        let call = self.phase(CallPhase::Connected).await;
        (call, remote_call)
    }
    async fn incoming(&self) -> (CallStatus, EndpointCall) {
        let id = self
            .remote
            .invite(&self.local_target)
            .unwrap()
            .send()
            .await
            .unwrap();
        let call = self.remote.wrap_call(id);
        (self.phase(CallPhase::Incoming).await, call)
    }
    async fn close(self) {
        timeout(Duration::from_secs(10), self.phone.shutdown())
            .await
            .unwrap();
        self.actor.await.unwrap();
        self.remote.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_remote_hangup_clears_actor_and_allows_another_call() {
    let mut pair = Pair::new().await;
    for _ in 0..3 {
        let (_, remote) = pair.outgoing().await;
        remote
            .hangup_and_wait(Some(Duration::from_secs(2)))
            .await
            .unwrap();
        pair.wait(|s| s.call.is_none()).await;
    }
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_local_hangup_repeated_calls_mute_dtmf_and_stale_controls() {
    let mut pair = Pair::new().await;
    let mut previous = String::new();
    for _ in 0..3 {
        let (call, remote) = pair.outgoing().await;
        assert!(
            pair.action(Action::Dial {
                target: pair.remote_target.clone()
            })
            .await
            .is_err()
        );
        assert!(
            pair.action(Action::Answer {
                call_id: call.id.clone()
            })
            .await
            .is_err()
        );
        assert!(
            pair.action(Action::Decline {
                call_id: call.id.clone()
            })
            .await
            .is_err()
        );
        assert!(
            pair.action(Action::SelectAudio {
                input: None,
                output: None
            })
            .await
            .is_err()
        );
        assert!(
            pair.action(Action::HangUp {
                call_id: previous.clone()
            })
            .await
            .is_err()
        );
        assert!(
            pair.action(Action::SendDtmf {
                call_id: call.id.clone(),
                digit: 'X'
            })
            .await
            .is_err()
        );
        for muted in [true, false] {
            pair.action(Action::SetMute {
                call_id: call.id.clone(),
                muted,
            })
            .await
            .unwrap();
            assert_eq!(
                pair.phone.status.borrow().call.as_ref().unwrap().muted,
                muted
            );
        }
        let mut events = pair.remote.events().await.unwrap();
        pair.action(Action::SendDtmf {
            call_id: call.id.clone(),
            digit: '5',
        })
        .await
        .unwrap();
        timeout(Duration::from_secs(2), async {
            loop {
                if matches!(
                    events.next().await.unwrap(),
                    Some(EndpointEvent::DtmfReceived { digit: '5', .. })
                ) {
                    break;
                }
            }
        })
        .await
        .expect("negotiated DTMF did not reach peer");
        pair.action(Action::HangUp {
            call_id: call.id.clone(),
        })
        .await
        .unwrap();
        pair.wait(|s| s.call.is_none()).await;
        remote
            .wait_for_end(Some(Duration::from_secs(2)))
            .await
            .unwrap();
        previous = call.id;
    }
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_decline_and_remote_cancellation_clear_ringing() {
    let pair = Pair::new().await;
    let (incoming, remote) = pair.incoming().await;
    pair.action(Action::Decline {
        call_id: incoming.id.clone(),
    })
    .await
    .unwrap();
    pair.wait(|s| s.call.is_none()).await;
    remote
        .wait_for_end(Some(Duration::from_secs(2)))
        .await
        .unwrap();
    assert!(
        pair.action(Action::Answer {
            call_id: incoming.id
        })
        .await
        .is_err()
    );
    let mut events = pair.remote.events().await.unwrap();
    let (_, remote) = pair.incoming().await;
    // This case is cancellation during ringing, so wait until the caller
    // has actually observed 180 rather than racing the first provisional.
    timeout(Duration::from_secs(2), async {
        loop {
            if matches!(events.next().await.unwrap(), Some(EndpointEvent::CallProgress { call_id, status_code: 180, .. }) if call_id == remote.id()) { break; }
        }
    }).await.unwrap();
    remote
        .hangup_and_wait(Some(Duration::from_secs(2)))
        .await
        .unwrap();
    pair.wait(|s| s.call.is_none()).await;
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_answer_can_end_from_either_side() {
    let pair = Pair::new().await;
    for local_end in [true, false] {
        let (incoming, remote) = pair.incoming().await;
        pair.action(Action::Answer {
            call_id: incoming.id.clone(),
        })
        .await
        .unwrap();
        let connected = pair.phase(CallPhase::Connected).await;
        assert_eq!(connected.id, incoming.id);
        remote
            .wait_for_answered(Some(Duration::from_secs(2)))
            .await
            .unwrap();
        if local_end {
            pair.action(Action::HangUp {
                call_id: connected.id,
            })
            .await
            .unwrap();
        } else {
            remote
                .hangup_and_wait(Some(Duration::from_secs(2)))
                .await
                .unwrap();
        }
        pair.wait(|s| s.call.is_none()).await;
        remote
            .wait_for_end(Some(Duration::from_secs(2)))
            .await
            .unwrap();
    }
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_cancellation_and_busy_response_release_call() {
    let mut pair = Pair::new().await;
    pair.action(Action::Dial {
        target: pair.remote_target.clone(),
    })
    .await
    .unwrap();
    let incoming = pair.remote.wait_for_incoming().await.unwrap();
    let ringing = pair.phase(CallPhase::Ringing).await;
    assert!(
        pair.action(Action::SetMute {
            call_id: ringing.id.clone(),
            muted: true
        })
        .await
        .is_err()
    );
    assert!(
        pair.action(Action::SendDtmf {
            call_id: ringing.id.clone(),
            digit: '1'
        })
        .await
        .is_err()
    );
    pair.action(Action::HangUp {
        call_id: ringing.id,
    })
    .await
    .unwrap();
    pair.wait(|s| s.call.is_none()).await;
    drop(incoming);
    pair.action(Action::Dial {
        target: pair.remote_target.clone(),
    })
    .await
    .unwrap();
    pair.remote
        .wait_for_incoming()
        .await
        .unwrap()
        .busy()
        .await
        .unwrap();
    pair.wait(|s| s.call.is_none() && s.notice.as_deref() == Some("The peer is busy."))
        .await;
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_incoming_call_is_busy_and_keeps_original_call() {
    let mut pair = Pair::new().await;
    let (active, remote) = pair.outgoing().await;
    let id = pair
        .remote
        .invite(&pair.local_target)
        .unwrap()
        .send()
        .await
        .unwrap();
    pair.remote
        .wrap_call(id)
        .wait_for_end(Some(Duration::from_secs(2)))
        .await
        .unwrap();
    assert_eq!(
        pair.phone.status.borrow().call.as_ref().unwrap().id,
        active.id
    );
    pair.action(Action::HangUp { call_id: active.id })
        .await
        .unwrap();
    pair.wait(|s| s.call.is_none()).await;
    remote
        .wait_for_end(Some(Duration::from_secs(2)))
        .await
        .unwrap();
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_during_call_releases_peer_and_rejects_redial() {
    let mut pair = Pair::new().await;
    let (_, remote) = pair.outgoing().await;
    pair.action(Action::Disconnect).await.unwrap();
    pair.wait(|s| s.call.is_none() && s.registration == Registration::Offline)
        .await;
    remote
        .wait_for_end(Some(Duration::from_secs(3)))
        .await
        .unwrap();
    assert!(
        pair.action(Action::Dial {
            target: pair.remote_target.clone()
        })
        .await
        .is_err()
    );
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_while_connected_ends_peer_and_stops_actor() {
    let mut pair = Pair::new().await;
    let (_, remote) = pair.outgoing().await;
    timeout(Duration::from_secs(10), pair.phone.shutdown())
        .await
        .unwrap();
    remote
        .wait_for_end(Some(Duration::from_secs(3)))
        .await
        .unwrap();
    assert!(pair.phone.perform(Action::Disconnect).await.is_err());
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_digest_refresh_unregister_and_auth_failure() {
    use crate::sip_routing_tests::{header, reply};
    for rejected in [false, true] {
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let directory = std::env::temp_dir().join(format!(
            "softphone-registration-{}-{}",
            std::process::id(),
            address.port()
        ));
        let phone = PhoneHandle::start(directory.join("account.json"));
        let (refreshed, confirmation) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let mut refreshed = Some(refreshed);
            let mut authenticated_registrations = 0;
            let mut buffer = [0u8; 16384];
            loop {
                let (size, client) =
                    timeout(Duration::from_secs(10), server.recv_from(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]).into_owned();
                assert!(request.starts_with("REGISTER "));
                let unregister = header(&request, "Expires") == "0"
                    || header(&request, "Contact").contains("expires=0");
                let response = if unregister {
                    reply(&request, "", "Expires: 0\r\n")
                } else if header(&request, "Proxy-Authorization").is_empty() {
                    reply(&request,"","Proxy-Authenticate: Digest realm=\"test-register\", nonce=\"abcdef0123456789\", algorithm=MD5, qop=\"auth\"\r\n")
                        .replacen("200 OK","407 Proxy Authentication Required",1)
                } else if rejected {
                    reply(&request, "", "").replacen("200 OK", "403 Forbidden", 1)
                } else {
                    authenticated_registrations += 1;
                    let contact = header(&request, "Contact");
                    let response = reply(
                        &request,
                        "",
                        &format!("Contact: {contact};expires=4\r\nExpires: 4\r\n"),
                    );
                    if authenticated_registrations >= 2
                        && let Some(done) = refreshed.take()
                    {
                        let _ = done.send(());
                    }
                    response
                };
                server.send_to(response.as_bytes(), client).await.unwrap();
                if unregister || (rejected && !header(&request, "Proxy-Authorization").is_empty()) {
                    break;
                }
            }
        });
        phone
            .perform(Action::Register {
                account: Account {
                    registrar: address.to_string(),
                    username: "alice".into(),
                    ..Default::default()
                },
                password: Some("test-password".into()),
            })
            .await
            .unwrap();
        let mut status = phone.status.clone();
        timeout(Duration::from_secs(3), async {
            loop {
                let state = status.borrow_and_update().registration;
                if state
                    == if rejected {
                        Registration::Failed
                    } else {
                        Registration::Registered
                    }
                {
                    break;
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .expect("registration did not reach expected state");
        if !rejected {
            timeout(Duration::from_secs(8), confirmation)
                .await
                .expect("registration refresh was not sent")
                .unwrap();
        }
        timeout(Duration::from_secs(8), phone.perform(Action::Disconnect))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(phone.status.borrow().registration, Registration::Offline);
        phone.shutdown().await;
        peer.await.unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}

async fn direct_phone() -> (Phone, Endpoint) {
    let local = Endpoint::builder()
        .profile(EndpointProfile::Custom(Config::local(
            "alice",
            unused_port(),
        )))
        .build()
        .await
        .unwrap();
    let remote = Endpoint::builder()
        .profile(EndpointProfile::Custom(Config::local("bob", unused_port())))
        .build()
        .await
        .unwrap();
    let (control, events) = local.split();
    let status = Status {
        account: Account::default(),
        registration: Registration::Registered,
        registration_error: None,
        call: None,
        audio: AudioSelection::default(),
        notice: None,
    };
    let (publish, _) = watch::channel(status.clone());
    (
        Phone {
            account_path: PathBuf::new(),
            link: Some(Link {
                control,
                events,
                registration_deadline: None,
            }),
            call: None,
            teardown: None,
            call_end: None,
            ringer: None,
            skip_audio: true,
            publish,
            status,
        },
        remote,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_registration_deadline_resets_account_with_error() {
    let (mut phone, remote) = direct_phone().await;
    phone.link.as_mut().unwrap().registration_deadline =
        Some(Instant::now() - Duration::from_secs(1));
    phone.status.registration = Registration::Registering;
    phone.tick().await;
    assert_eq!(phone.status.registration, Registration::Offline);
    assert!(phone.link.is_none());
    assert!(
        phone
            .status
            .registration_error
            .as_deref()
            .unwrap()
            .contains("timed out")
    );
    remote.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_teardown_deadline_resets_endpoint_before_another_call() {
    let (mut phone, remote) = direct_phone().await;
    let sink = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let id = phone
        .link
        .as_ref()
        .unwrap()
        .control
        .invite(&format!("sip:nobody@{}", sink.local_addr().unwrap()))
        .unwrap()
        .send()
        .await
        .unwrap();
    let id = phone.link.as_ref().unwrap().control.wrap_call(id).id();
    phone.call = Some(Call::Ending {
        id,
        peer: "test".into(),
        deadline: Instant::now() - Duration::from_secs(1),
    });
    phone.tick().await;
    assert!(phone.call.is_none() && phone.link.is_none());
    assert_eq!(phone.status.registration, Registration::Offline);
    assert!(phone.status.notice.as_deref().unwrap().contains("teardown"));
    assert!(phone.dial("1001").await.is_err());
    remote.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_while_incoming_call_rings_ends_peer() {
    let pair = Pair::new().await;
    let (_, remote) = pair.incoming().await;
    timeout(Duration::from_secs(10), pair.phone.shutdown())
        .await
        .unwrap();
    remote
        .wait_for_end(Some(Duration::from_secs(3)))
        .await
        .unwrap();
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_no_answer_deadline_sends_cancel() {
    use crate::sip_routing_tests::reply;
    let (mut phone, remote) = direct_phone().await;
    let edge = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    phone
        .dial(&format!("sip:bob@{}", edge.local_addr().unwrap()))
        .await
        .unwrap();
    let mut buffer = [0u8; 16384];
    let (size, client) = timeout(Duration::from_secs(2), edge.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let invite = String::from_utf8_lossy(&buffer[..size]).into_owned();
    edge.send_to(
        reply(&invite, "", "")
            .replacen("200 OK", "180 Ringing", 1)
            .as_bytes(),
        client,
    )
    .await
    .unwrap();
    let Some(Call::Outgoing { deadline, .. }) = &mut phone.call else {
        panic!("expected outgoing call")
    };
    *deadline = Instant::now() - Duration::from_secs(1);
    phone.tick().await;
    assert_eq!(
        phone.status.notice.as_deref(),
        Some("The peer did not answer.")
    );
    let (size, _) = timeout(Duration::from_secs(3), edge.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let cancel = String::from_utf8_lossy(&buffer[..size]).into_owned();
    assert!(cancel.starts_with("CANCEL "));
    edge.send_to(reply(&cancel, "", "").as_bytes(), client)
        .await
        .unwrap();
    edge.send_to(
        reply(&invite, "", "")
            .replacen("200 OK", "487 Request Terminated", 1)
            .as_bytes(),
        client,
    )
    .await
    .unwrap();
    assert!(
        timeout(Duration::from_secs(3), teardown_result(&mut phone.teardown))
            .await
            .unwrap()
    );
    phone.disconnect().await;
    remote.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_hangup_releases_call_without_disconnecting_account() {
    let mut pair = Pair::new().await;
    let (call, remote) = pair.outgoing().await;
    let (local_result, remote_result) = tokio::join!(
        pair.action(Action::HangUp { call_id: call.id }),
        remote.hangup_and_wait(Some(Duration::from_secs(3)))
    );
    local_result.unwrap();
    remote_result.unwrap();
    pair.wait(|s| s.call.is_none()).await;
    assert_eq!(
        pair.phone.status.borrow().registration,
        Registration::Registered
    );
    pair.close().await;
}
