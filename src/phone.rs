use std::future::pending;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use rvoip_sip::{
    EndpointCall, EndpointCallId, EndpointControl, EndpointEvent, EndpointEvents,
    EndpointIncomingCall, EndpointRegistrationInfo, EndpointRegistrationStatus,
};
use rvoip_sip_core::types::From as SipFrom;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, interval, timeout};

use crate::account::Account;
use crate::audio::{AudioSelection, CallAudio, DeviceList, VoiceEnhancements};
use crate::library::{self, CallLog, Record};
use crate::recording;
use crate::ringer::Ringer;

const ACTION_TIMEOUT: Duration = Duration::from_secs(5);
const DIAL_TIMEOUT: Duration = Duration::from_secs(45);

pub enum Action {
    Register {
        account: Account,
        password: Option<String>,
    },
    Disconnect,
    Dial {
        target: String,
    },
    Answer {
        call_id: String,
    },
    Decline {
        call_id: String,
    },
    HangUp {
        call_id: String,
    },
    SetMute {
        call_id: String,
        muted: bool,
    },
    SendDtmf {
        call_id: String,
        digit: char,
    },
    SetCallRecording(bool),
    SetVoiceEnhancements(VoiceEnhancements),
    DeleteRecording {
        filename: String,
    },
    SelectAudio {
        input: Option<String>,
        output: Option<String>,
    },
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Register { .. } => "register",
            Self::Disconnect => "disconnect",
            Self::Dial { .. } => "dial",
            Self::Answer { .. } => "answer",
            Self::Decline { .. } => "decline",
            Self::HangUp { .. } => "hang_up",
            Self::SetMute { .. } => "set_mute",
            Self::SendDtmf { .. } => "send_dtmf",
            Self::SetCallRecording(_) => "set_call_recording",
            Self::SetVoiceEnhancements(_) => "set_voice_enhancements",
            Self::DeleteRecording { .. } => "delete_recording",
            Self::SelectAudio { .. } => "select_audio",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    Offline,
    Registering,
    Registered,
    Failed,
}

#[derive(Clone)]
pub struct Status {
    pub account: Account,
    pub registration: Registration,
    pub registration_error: Option<String>,
    pub history: Arc<Vec<Record>>,
    pub call: Option<CallStatus>,
    pub audio: AudioSelection,
    pub notice: Option<String>,
    pub record_calls: bool,
    pub enhancements: VoiceEnhancements,
    pub recording_error: Option<String>,
}

impl Status {
    pub fn is_ringing(&self) -> bool {
        self.call
            .as_ref()
            .is_some_and(|call| call.phase == CallPhase::Incoming)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPhase {
    Incoming,
    Dialing,
    Ringing,
    Connected,
    Ending,
}
impl CallPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Incoming => "Incoming call",
            Self::Dialing => "Calling",
            Self::Ringing => "Ringing",
            Self::Connected => "Connected",
            Self::Ending => "Ending call",
        }
    }
}

#[derive(Clone)]
pub struct CallStatus {
    pub id: String,
    pub peer: String,
    pub phase: CallPhase,
    pub muted: bool,
    pub recording: bool,
}

enum Call {
    Incoming(EndpointIncomingCall),
    Outgoing {
        call: EndpointCall,
        peer: String,
        ringing: bool,
        deadline: Instant,
    },
    Connected {
        call: EndpointCall,
        peer: String,
        muted: Arc<AtomicBool>,
        audio: Option<CallAudio>,
        audio_started: Instant,
    },
    Ending {
        id: EndpointCallId,
        peer: String,
        deadline: Instant,
    },
}

impl Call {
    fn id(&self) -> EndpointCallId {
        match self {
            Self::Incoming(incoming) => incoming.id(),
            Self::Outgoing { call, .. } | Self::Connected { call, .. } => call.id(),
            Self::Ending { id, .. } => id.clone(),
        }
    }

    fn status(&self) -> CallStatus {
        let (peer, phase, muted) = match self {
            Self::Incoming(incoming) => {
                return CallStatus {
                    id: incoming.id().to_string(),
                    peer: caller_label(incoming.from()),
                    phase: CallPhase::Incoming,
                    muted: false,
                    recording: false,
                };
            }
            Self::Outgoing { peer, ringing, .. } => (
                peer.as_str(),
                if *ringing {
                    CallPhase::Ringing
                } else {
                    CallPhase::Dialing
                },
                false,
            ),
            Self::Connected { peer, muted, .. } => (
                peer.as_str(),
                CallPhase::Connected,
                muted.load(Ordering::Relaxed),
            ),
            Self::Ending { peer, .. } => (peer.as_str(), CallPhase::Ending, false),
        };
        CallStatus {
            id: self.id().to_string(),
            peer: peer.into(),
            phase,
            muted,
            recording: matches!(self, Self::Connected { audio: Some(audio), .. }
                if audio.recording.as_ref().is_some_and(|recording| !recording.failed())),
        }
    }
}

struct Link {
    control: EndpointControl,
    events: EndpointEvents,
    registration_deadline: Option<Instant>,
}

enum Request {
    Action(Action, oneshot::Sender<Result<(), String>>),
    Shutdown(oneshot::Sender<()>),
}

pub struct PhoneHandle {
    requests: mpsc::Sender<Request>,
    pub status: watch::Receiver<Status>,
}

impl PhoneHandle {
    pub fn start(path: PathBuf) -> Self {
        let (account, notice) = match Account::load(&path) {
            Ok(account) => (account, None),
            Err(err) => (Account::default(), Some(err)),
        };
        let recording_setting = library::load::<bool>(&path.with_file_name("call-recording.json"));
        let (enhancements, enhancements_error) = match library::load::<VoiceEnhancements>(
            &path.with_file_name("voice-enhancements.json"),
        ) {
            Ok(enhancements) => (enhancements, None),
            Err(error) => (VoiceEnhancements::default(), Some(error)),
        };
        let notice = notice.or(enhancements_error);
        let (record_calls, recording_error) = match recording_setting {
            Ok(enabled) => (enabled, None),
            Err(error) => (false, Some(error)),
        };
        let status = Status {
            account,
            registration: Registration::Offline,
            registration_error: None,
            history: Default::default(),
            call: None,
            audio: AudioSelection::default(),
            notice,
            record_calls,
            enhancements,
            recording_error,
        };
        let (publish, receive) = watch::channel(status.clone());
        let (requests, inbox) = mpsc::channel(16);
        tokio::spawn(
            Phone {
                account_path: path,
                log: None,
                history_path: None,
                link: None,
                call: None,
                teardown: None,
                call_end: None,
                ringer: None,
                #[cfg(test)]
                skip_audio: false,
                publish,
                status,
            }
            .run(inbox),
        );
        Self {
            requests,
            status: receive,
        }
    }

    pub async fn perform(&self, action: Action) -> Result<(), String> {
        tracing::debug!(operation = action.name(), "Phone action queued");
        let (reply, result) = oneshot::channel();
        self.requests
            .send(Request::Action(action, reply))
            .await
            .map_err(|_| "The SIP service is unavailable.")?;
        result
            .await
            .map_err(|_| "The SIP service stopped.".to_owned())?
    }

    pub async fn shutdown(&self) {
        let (reply, done) = oneshot::channel();
        if self.requests.send(Request::Shutdown(reply)).await.is_ok() {
            let _ = done.await;
        }
    }
}

struct Phone {
    log: Option<CallLog>,
    history_path: Option<PathBuf>,
    account_path: PathBuf,
    link: Option<Link>,
    call: Option<Call>,
    teardown: Option<oneshot::Receiver<bool>>,
    call_end: Option<oneshot::Receiver<bool>>,
    ringer: Option<Ringer>,
    #[cfg(test)]
    skip_audio: bool,
    publish: watch::Sender<Status>,
    status: Status,
}

impl Phone {
    async fn run(mut self, mut inbox: mpsc::Receiver<Request>) {
        let mut ticker = interval(Duration::from_secs(2));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                request = inbox.recv() => match request {
                    Some(Request::Action(action, reply)) => {
                        let operation = action.name();
                        let started = Instant::now();
                        tracing::debug!(operation, "Phone action received");
                        let result = self.perform(action).await;
                        tracing::debug!(operation, success = result.is_ok(), elapsed_ms = started.elapsed().as_millis(), "Phone action completed");
                        self.update();
                        let _ = reply.send(result);
                    }
                    Some(Request::Shutdown(reply)) => {
                        self.disconnect().await;
                        let _ = reply.send(());
                        return;
                    }
                    None => {
                        self.disconnect().await;
                        return;
                    }
                },
                event = next_event(&mut self.link) => {
                    match event {
                        Some(event) => self.handle_event(event).await,
                        None => {
                            self.disconnect().await;
                            self.status.notice = Some("The SIP endpoint stopped. Register again to reconnect.".into());
                            self.update();
                        }
                    }
                }
                ended = teardown_result(&mut self.call_end) => {
                    self.call_end = None;
                    if ended && matches!(self.call, Some(Call::Connected { .. })) {
                        tracing::debug!("Remote termination observed through SIP lifecycle");
                        self.finish_call("Call ended by peer.");
                        self.update();
                    }
                }
                result = teardown_result(&mut self.teardown) => {
                    self.teardown = None;
                    if matches!(self.call, Some(Call::Ending { .. })) {
                        if result {
                            self.finish_call("Call ended.");
                        } else {
                            self.close_link().await;
                            self.status.notice = Some("The peer did not confirm call termination. Disconnected locally; the remote call may remain active.".into());
                        }
                        self.update();
                    }
                }
                _ = ticker.tick() => self.tick().await,
            }
        }
    }

    fn update(&mut self) {
        if matches!(self.call, Some(Call::Incoming(_))) && self.audio_enabled() {
            if self.ringer.is_none() {
                match Ringer::start(self.status.audio.output.clone()) {
                    Ok(ringer) => self.ringer = Some(ringer),
                    Err(error) => self.status.notice = Some(error),
                }
            }
        } else {
            self.ringer = None;
        }
        let call = self.call.as_ref().map(Call::status);
        if self.status.call.as_ref().map(|c| (&c.id, c.phase))
            != call.as_ref().map(|c| (&c.id, c.phase))
        {
            tracing::debug!(previous = ?self.status.call.as_ref().map(|c| c.phase), next = ?call.as_ref().map(|c| c.phase), call_id = ?call.as_ref().map(|c| &c.id), "Call state changed");
        }
        if call.is_none()
            && let Some(log) = self.log.take()
        {
            self.record(log);
        }
        self.status.call = call;
        self.publish.send_replace(self.status.clone());
    }

    fn record(&mut self, log: CallLog) {
        let history = Arc::make_mut(&mut self.status.history);
        let record = log.finish();
        let index = history.partition_point(|other| other.timestamp > record.timestamp);
        history.insert(index, record);
        history.truncate(500);
        if let Some(path) = &self.history_path
            && let Err(error) = library::save(path, history)
        {
            self.status.notice = Some(error);
        }
    }

    async fn perform(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Register { account, password } => self.register(account, password).await,
            Action::Disconnect => {
                self.disconnect().await;
                Ok(())
            }
            Action::Dial { target } => self.dial(&target).await,
            Action::Answer { call_id } => self.answer(&call_id).await,
            Action::Decline { call_id } => {
                self.require_incoming(&call_id)?;
                self.ringer = None;
                if let Some(Call::Incoming(incoming)) = self.call.take() {
                    if !matches!(
                        timeout(ACTION_TIMEOUT, incoming.decline()).await,
                        Ok(Ok(()))
                    ) {
                        self.disconnect().await;
                        return Err(
                            "Declining failed. The account was disconnected to clear the call."
                                .into(),
                        );
                    }
                    self.status.notice = Some("Call declined.".into());
                }
                Ok(())
            }
            Action::HangUp { call_id } => {
                self.require_call(&call_id)?;
                tracing::info!("Hang-up requested");
                self.hang_up().await;
                Ok(())
            }
            Action::SetMute { call_id, muted } => {
                self.require_call(&call_id)?;
                let Some(Call::Connected { muted: flag, .. }) = &self.call else {
                    return Err("Mute is available during a connected call.".into());
                };
                flag.store(muted, Ordering::Relaxed);
                Ok(())
            }
            Action::SendDtmf { call_id, digit } => {
                self.require_call(&call_id)?;
                let Some(Call::Connected { call, .. }) = &self.call else {
                    return Err("Keypad tones are available during a connected call.".into());
                };
                if !"0123456789*#".contains(digit) {
                    return Err("Choose a valid keypad tone.".into());
                }
                timeout(ACTION_TIMEOUT, call.send_dtmf(digit))
                    .await
                    .map_err(|_| "Sending the keypad tone timed out.")?
                    .map_err(|_| "The peer did not negotiate keypad tones.".into())
            }
            Action::SetCallRecording(enabled) => {
                if self.call.is_some() {
                    return Err("Change call recording between calls.".into());
                }
                library::save(
                    &self.account_path.with_file_name("call-recording.json"),
                    &enabled,
                )?;
                self.status.record_calls = enabled;
                self.status.recording_error = None;
                Ok(())
            }
            Action::SetVoiceEnhancements(enhancements) => {
                if self.call.is_some() {
                    return Err("Change voice settings between calls.".into());
                }
                library::save(
                    &self.account_path.with_file_name("voice-enhancements.json"),
                    &enhancements,
                )?;
                self.status.enhancements = enhancements;
                Ok(())
            }
            Action::DeleteRecording { filename } => {
                recording::delete(&self.account_path.with_file_name("recordings"), &filename)?;
                library::forget_recording(
                    Arc::make_mut(&mut self.status.history).as_mut_slice(),
                    &filename,
                );
                if let Some(path) = &self.history_path {
                    library::save(path, self.status.history.as_ref())?;
                }
                self.update();
                Ok(())
            }
            Action::SelectAudio { input, output } => {
                if self.call.is_some() {
                    return Err("Change audio devices between calls.".into());
                }
                let selection = AudioSelection { input, output };
                timeout(
                    ACTION_TIMEOUT,
                    tokio::task::spawn_blocking(DeviceList::scan),
                )
                .await
                .map_err(|_| "Listing audio devices timed out.")?
                .map_err(|_| "Could not list audio devices.")?
                .validate(&selection)?;
                self.status.audio = selection;
                Ok(())
            }
        }
    }

    fn require_call(&self, call_id: &str) -> Result<(), String> {
        if self
            .call
            .as_ref()
            .is_some_and(|call| call.id().to_string() == call_id)
        {
            Ok(())
        } else {
            Err("This call has already ended.".into())
        }
    }

    fn require_incoming(&self, call_id: &str) -> Result<(), String> {
        self.require_call(call_id)?;
        match self.call {
            Some(Call::Incoming(_)) => Ok(()),
            _ => Err("This call is no longer ringing.".into()),
        }
    }

    async fn register(
        &mut self,
        mut account: Account,
        password: Option<String>,
    ) -> Result<(), String> {
        if self.link.is_some() {
            return Err("Disconnect the current account before registering again.".into());
        }
        account.validate()?;
        self.status.registration = Registration::Registering;
        self.status.registration_error = None;
        self.status.notice = None;
        self.update();

        let result = self.open_link(account, password).await;
        if let Err(error) = &result {
            self.status.registration = Registration::Offline;
            self.status.registration_error = Some(error.clone());
        }
        result
    }

    async fn open_link(
        &mut self,
        account: Account,
        password: Option<String>,
    ) -> Result<(), String> {
        let path = self.account_path.clone();
        let saved = account.clone();
        let password = timeout(
            Duration::from_secs(10),
            tokio::task::spawn_blocking(move || saved.store(&path, password)),
        )
        .await
        .map_err(|_| "The credential store timed out. Unlock it and try again.")?
        .map_err(|_| "Could not load SIP credentials.")??;
        self.status.account = account.clone();
        let history_path =
            library::line_directory(&self.account_path, &account).join("history.json");
        self.history_path = None;
        self.status.history = Arc::new(match library::load(&history_path) {
            Ok(history) => {
                self.history_path = Some(history_path);
                history
            }
            Err(error) => {
                self.status.notice = Some(error);
                Vec::new()
            }
        });

        let endpoint = timeout(ACTION_TIMEOUT + ACTION_TIMEOUT, account.endpoint(password))
            .await
            .map_err(|_| "Starting the SIP endpoint timed out.")??;
        let (control, events) = endpoint.split();
        if !matches!(
            timeout(ACTION_TIMEOUT, control.register()).await,
            Ok(Ok(()))
        ) {
            let _ = timeout(ACTION_TIMEOUT, control.shutdown()).await;
            return Err(
                "Could not start registration. Check the PBX address and transport.".into(),
            );
        }
        self.link = Some(Link {
            control,
            events,
            registration_deadline: Some(Instant::now() + Duration::from_secs(30)),
        });
        tracing::info!("SIP registration started");
        Ok(())
    }

    async fn disconnect(&mut self) {
        self.ringer = None;
        if !matches!(self.call, Some(Call::Ending { .. })) {
            self.hang_up().await;
        }
        // Do not close the transport before the spawned BYE/CANCEL has had
        // a chance to reach the peer. Keep total window-close time bounded.
        if self.teardown.is_some() {
            let confirmed =
                timeout(Duration::from_secs(2), teardown_result(&mut self.teardown)).await;
            tracing::debug!(
                confirmed = matches!(confirmed, Ok(true)),
                "Disconnect call teardown wait completed"
            );
        }
        self.close_link().await;
    }

    async fn close_link(&mut self) {
        self.teardown = None;
        self.call_end = None;
        if let Some(mut call) = self.call.take() {
            self.save_recording(&mut call);
        }
        if let Some(link) = self.link.take() {
            let _ = timeout(
                Duration::from_secs(3),
                link.control
                    .unregister_and_wait(Some(Duration::from_secs(2))),
            )
            .await;
            let _ = timeout(Duration::from_secs(3), link.control.shutdown()).await;
        }
        self.status.registration = Registration::Offline;
        self.status.registration_error = None;
        self.status.notice = None;
        self.update();
    }

    async fn dial(&mut self, target: &str) -> Result<(), String> {
        if self.call.is_some() {
            return Err("End the current call before dialing.".into());
        }
        let Some(link) = &self.link else {
            return Err("Register a SIP account first.".into());
        };
        if self.status.registration != Registration::Registered {
            return Err("Wait for SIP registration before dialing.".into());
        }
        let target = target.trim();
        validate_target(target, self.status.account.secure)?;
        let invite = link
            .control
            .invite(target)
            .map_err(|_| "Enter a valid SIP address or extension.")?;
        // A canceled send can still create a SIP dialog. Reset the endpoint on
        // failure so an untracked call cannot survive and overlap the next call.
        let call_id = match timeout(ACTION_TIMEOUT, invite.send()).await {
            Ok(Ok(id)) => id,
            _ => {
                self.disconnect().await;
                return Err("Starting the call failed. Register again to reconnect.".into());
            }
        };
        self.log = Some(CallLog::new(target.into(), Some(target.into()), false));
        self.call = Some(Call::Outgoing {
            call: link.control.wrap_call(call_id),
            peer: target.into(),
            ringing: false,
            deadline: Instant::now() + DIAL_TIMEOUT,
        });
        self.status.notice = None;
        tracing::info!("Outgoing call started");
        Ok(())
    }

    async fn answer(&mut self, call_id: &str) -> Result<(), String> {
        self.require_incoming(call_id)?;
        let Some(Call::Incoming(incoming)) = self.call.take() else {
            return Ok(());
        };
        if let Some(ringer) = self.ringer.take() {
            ringer.stop().await;
        }
        let peer = caller_label(incoming.from());
        let call = match timeout(ACTION_TIMEOUT, incoming.answer()).await {
            Ok(Ok(call)) => call,
            _ => {
                self.disconnect().await;
                return Err("Answering failed. Register again to reconnect.".into());
            }
        };
        self.attach_audio(call, peer).await;
        Ok(())
    }

    fn watch_call_end(&mut self, call: &EndpointCall) {
        let call = call.clone();
        let (mut done, receive) = oneshot::channel();
        self.call_end = Some(receive);
        tokio::spawn(async move {
            tokio::select! {
                result = call.wait_for_end(None) => { let _ = done.send(result.is_ok()); }
                _ = done.closed() => {}
            }
        });
    }

    fn audio_enabled(&self) -> bool {
        #[cfg(test)]
        {
            !self.skip_audio
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    async fn attach_audio(&mut self, call: EndpointCall, peer: String) {
        if let Some(log) = &mut self.log {
            log.answer();
        }
        self.watch_call_end(&call);
        let muted = Arc::new(AtomicBool::new(false));
        self.status.notice = None;
        self.status.recording_error = None;
        let recordings = self.account_path.with_file_name("recordings");
        let record_to = self.status.record_calls.then_some(recordings.as_path());
        let audio = if self.audio_enabled() {
            match timeout(
                ACTION_TIMEOUT,
                CallAudio::start(
                    &call,
                    &self.status.audio,
                    muted.clone(),
                    record_to,
                    self.status.enhancements,
                ),
            )
            .await
            {
                Ok(Ok((audio, recording_error))) => {
                    self.status.recording_error = recording_error;
                    Some(audio)
                }
                Ok(Err(error)) => {
                    self.status.notice = Some(error);
                    self.status.recording_error = self.status.record_calls.then(|| {
                        "This call is not being recorded because call audio could not start.".into()
                    });
                    None
                }
                Err(_) => {
                    self.status.notice = Some("Opening the call audio timed out.".into());
                    self.status.recording_error = self.status.record_calls.then(|| {
                        "This call is not being recorded because call audio could not start.".into()
                    });
                    None
                }
            }
        } else {
            None
        };

        self.call = Some(Call::Connected {
            call,
            peer,
            muted,
            audio,
            audio_started: Instant::now(),
        });
        tracing::info!("Call connected");
    }

    async fn hang_up(&mut self) {
        self.ringer = None;
        let Some(mut current) = self.call.take() else {
            return;
        };
        if let Some(log) = &mut self.log {
            log.stop_timer();
        }
        self.save_recording(&mut current);
        let id = current.id();
        let peer = current.status().peer;
        let call = match current {
            Call::Outgoing { call, .. } | Call::Connected { call, .. } => call,
            Call::Incoming(incoming) => {
                let _ = timeout(ACTION_TIMEOUT, incoming.decline()).await;
                return;
            }
            Call::Ending { .. } => {
                tracing::warn!("Forced disconnect requested during call teardown");
                self.close_link().await;
                self.status.notice =
                    Some("Disconnected to clear the call. Register again to reconnect.".into());
                return;
            }
        };
        self.call = Some(Call::Ending {
            id,
            peer,
            deadline: Instant::now() + Duration::from_secs(35),
        });
        self.update();
        // Keep consuming endpoint events while the stack waits for the BYE or
        // CANCEL transaction. SIP's default UDP transaction lifetime is 32 s.
        let (done, result) = oneshot::channel();
        self.teardown = Some(result);
        self.status.notice = Some("Ending call. Waiting for SIP confirmation…".into());
        tokio::spawn(async move {
            let success = match timeout(
                Duration::from_secs(34),
                call.hangup_and_wait(Some(Duration::from_secs(33))),
            )
            .await
            {
                Ok(Ok(_)) => {
                    tracing::info!("SIP teardown confirmed");
                    true
                }
                Ok(Err(error)) => {
                    tracing::warn!(error_kind = ?std::mem::discriminant(&error), "SIP teardown failed");
                    false
                }
                Err(_) => {
                    tracing::warn!("SIP teardown exceeded transaction deadline");
                    false
                }
            };
            let _ = done.send(success);
        });
    }

    fn save_recording(&mut self, call: &mut Call) {
        if let Call::Connected {
            audio: Some(audio), ..
        } = call
            && let Some(recording) = audio.recording.take()
        {
            match recording.finish() {
                Ok(Some(filename)) => {
                    if let Some(log) = &mut self.log {
                        log.set_recording(filename);
                    }
                }
                Ok(None) => {
                    self.status.recording_error = Some("No call audio was recorded.".into())
                }
                Err(error) => self.status.recording_error = Some(error),
            }
        }
    }

    fn finish_call(&mut self, notice: &str) {
        self.ringer = None;
        self.teardown = None;
        self.call_end = None;
        if let Some(mut call) = self.call.take() {
            self.save_recording(&mut call);
        }
        self.status.notice = Some(notice.into());
        tracing::info!("Call ended");
    }

    async fn handle_event(&mut self, event: EndpointEvent) {
        let active = self.call.as_ref().map(Call::id);
        match event {
            EndpointEvent::IncomingCall(incoming) => {
                let (name, target) = caller_identity(incoming.from());
                let log = CallLog::new(name, target, true);
                if self.call.is_some() {
                    self.record(log);
                    self.update();
                    let _ = timeout(ACTION_TIMEOUT, incoming.busy()).await;
                    return;
                }
                self.log = Some(log);
                self.call = Some(Call::Incoming(incoming));
                self.status.notice = None;
                tracing::info!("Incoming call ringing");
            }
            EndpointEvent::CallProgress {
                call_id,
                status_code: code @ (180 | 183),
                ..
            } if active.as_ref() == Some(&call_id) => {
                tracing::debug!(status = code, "SIP call progress");
                if let Some(Call::Outgoing { ringing, .. }) = &mut self.call {
                    *ringing = true;
                }
            }
            EndpointEvent::CallAnswered { call, .. } if active.as_ref() == Some(&call.id()) => {
                if let Some(Call::Outgoing { peer, .. }) = &self.call {
                    self.attach_audio(call, peer.clone()).await;
                }
            }
            EndpointEvent::CallEnded { call_id, .. } | EndpointEvent::CallCancelled { call_id }
                if active.as_ref() == Some(&call_id) =>
            {
                self.finish_call("Call ended.");
            }
            EndpointEvent::CallFailed {
                call_id,
                status_code,
                ..
            } if active.as_ref() == Some(&call_id) => {
                let notice = match status_code {
                    408 | 480 => "The peer did not answer.",
                    486 | 600 => "The peer is busy.",
                    487 => "Call canceled.",
                    603 => "Call declined.",
                    _ => "The SIP call could not be established.",
                };
                self.finish_call(notice);
            }
            EndpointEvent::SipTrace(trace) => {
                log_teardown_trace(&trace);
                return;
            }
            EndpointEvent::RegistrationChanged(info) => self.registration_status(info),
            EndpointEvent::NetworkError { call_id, .. }
                if call_id.is_none() || call_id.as_ref() == active.as_ref() =>
            {
                tracing::warn!("Endpoint reported a network error");
                self.disconnect().await;
                self.status.notice =
                    Some("The SIP connection failed. Register again to reconnect.".into());
            }
            _ => return,
        }
        self.update();
    }

    fn registration_status(&mut self, info: EndpointRegistrationInfo) {
        use EndpointRegistrationStatus as State;
        let previous = self.status.registration;
        self.status.registration = match info.status {
            State::Registered => Registration::Registered,
            State::Registering => Registration::Registering,
            State::Unregistered | State::Unregistering | State::Failed => Registration::Failed,
        };
        if self.status.registration != previous {
            tracing::debug!(status = ?self.status.registration, "SIP registration update");
        }
        if let Some(link) = &mut self.link {
            match self.status.registration {
                Registration::Registering => {
                    link.registration_deadline
                        .get_or_insert(Instant::now() + Duration::from_secs(30));
                }
                _ => link.registration_deadline = None,
            }
        }
        self.status.registration_error = if self.status.registration == Registration::Failed {
            Some("Registration failed. Check the SIP account and PBX connection.".into())
        } else {
            None
        };
    }

    async fn tick(&mut self) {
        if self.ringer.as_ref().is_some_and(Ringer::failed) {
            const NOTICE: &str = "Incoming ringtone could not play. Check your selected speaker.";
            if self.status.notice.as_deref() != Some(NOTICE) {
                self.status.notice = Some(NOTICE.into());
                self.update();
            }
        }
        if self
            .link
            .as_ref()
            .and_then(|link| link.registration_deadline)
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.disconnect().await;
            self.status.registration_error = Some(
                "Registration timed out. Check the PBX address and network, then try again.".into(),
            );
            self.update();
            return;
        }
        if let Some(link) = &self.link {
            let previous = self.status.registration;
            if let Ok(Ok(Some(info))) =
                timeout(Duration::from_secs(1), link.control.registration_info()).await
            {
                self.registration_status(info);
                if self.status.registration != previous {
                    self.update();
                }
            }
        }

        if matches!(&self.call, Some(Call::Connected { audio: Some(audio), .. })
            if audio.recording.as_ref().is_some_and(|recording| recording.failed()))
            && self.status.recording_error.is_none()
        {
            self.status.recording_error =
                Some("Call recording stopped because audio could not be saved.".into());
            self.update();
        }

        match &mut self.call {
            Some(Call::Outgoing { deadline, .. }) if Instant::now() >= *deadline => {
                self.hang_up().await;
                self.status.notice = Some("The peer did not answer.".into());
                self.update();
            }
            Some(Call::Ending { deadline, .. }) if Instant::now() >= *deadline => {
                // The endpoint owns SIP teardown. Close it before allowing a new call.
                self.disconnect().await;
                self.status.notice =
                    Some("The call timed out during teardown. Register again to reconnect.".into());
                self.update();
            }
            Some(Call::Connected {
                audio: Some(audio),
                audio_started,
                ..
            }) if audio_started.elapsed() > Duration::from_secs(5)
                && !audio.microphone_healthy() =>
            {
                const AUDIO_WARNING: &str = "Microphone audio stopped or did not start. End the call and check your audio device.";
                if self.status.notice.as_deref() != Some(AUDIO_WARNING) {
                    audio.log_health();
                    self.status.notice = Some(AUDIO_WARNING.into());
                    self.update();
                }
            }
            _ => {}
        }
    }
}

async fn teardown_result(receiver: &mut Option<oneshot::Receiver<bool>>) -> bool {
    let result = match receiver.as_mut() {
        Some(receiver) => receiver.await.unwrap_or(false),
        None => pending().await,
    };
    // Consume only after completion; keeping the receiver in place across
    // await makes this safe when another select branch wins and cancels us.
    *receiver = None;
    result
}

fn log_teardown_trace(trace: &rvoip_sip::EndpointSipTrace) {
    // Only inspect a method token and numeric status. Never print a request
    // URI, raw header, reason phrase, SDP, or authentication data.
    let method = trace.raw_message.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("cseq")
            .then(|| value.split_whitespace().nth(1))
            .flatten()
    });
    if !matches!(
        method,
        Some("REGISTER" | "INVITE" | "ACK" | "BYE" | "CANCEL")
    ) {
        return;
    }
    let status = trace
        .start_line
        .strip_prefix("SIP/2.0 ")
        .and_then(|line| line.split_whitespace().next())
        .and_then(|code| code.parse::<u16>().ok());
    tracing::info!(direction = ?trace.direction, transport = %trace.transport,
        local = %trace.local_addr, remote = %trace.remote_addr, method, status, "SIP signaling packet");
}

async fn next_event(link: &mut Option<Link>) -> Option<EndpointEvent> {
    match link {
        Some(link) => link.events.next().await.ok().flatten(),
        None => pending().await,
    }
}

fn caller_label(from: &str) -> String {
    caller_identity(from).0
}

fn caller_identity(from: &str) -> (String, Option<String>) {
    // FromStr on SipFrom parses an address only and rejects a trailing SIP tag.
    let Ok((remaining, SipFrom(address))) =
        rvoip_sip_core::parser::headers::parse_from(from.trim().as_bytes())
    else {
        return ("Unknown caller".into(), None);
    };
    if !remaining.is_empty() {
        return ("Unknown caller".into(), None);
    }
    let mut uri = address.uri;
    uri.password = None;
    uri.parameters.clear();
    uri.headers.clear();
    let target = uri.to_string();
    let name = address
        .display_name
        .map(|s| {
            s.chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| target.clone());
    (name, Some(target))
}

pub(crate) fn validate_target(target: &str, secure: bool) -> Result<(), String> {
    let address = target
        .strip_prefix("sip:")
        .or_else(|| target.strip_prefix("sips:"))
        .unwrap_or(target);
    if address.is_empty()
        || target.len() > 256
        || !address
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+*@:".contains(&byte))
        || address.matches('@').count() > 1
        || (address.contains(':') && !address.contains('@'))
        || (target.starts_with("sips:") && !secure)
    {
        return Err("Enter an extension or a SIP address such as sip:1002@pbx.example.com.".into());
    }
    if let Some((user, host)) = address.split_once('@')
        && (user.is_empty() || user.contains(':') || !crate::account::valid_server(host))
    {
        return Err("Enter a SIP address with a username and PBX hostname.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn call_recording_defaults_to_off_and_persists_both_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("account.json");
        let phone = PhoneHandle::start(path.clone());
        assert!(!phone.status.borrow().record_calls);

        for enabled in [true, false] {
            phone
                .perform(Action::SetCallRecording(enabled))
                .await
                .unwrap();
            assert_eq!(phone.status.borrow().record_calls, enabled);
            let restarted = PhoneHandle::start(path.clone());
            assert_eq!(restarted.status.borrow().record_calls, enabled);
            restarted.shutdown().await;
        }
        phone.shutdown().await;
    }

    #[tokio::test]
    async fn voice_enhancements_default_on_and_persist() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("account.json");
        let phone = PhoneHandle::start(path.clone());
        assert_eq!(
            phone.status.borrow().enhancements,
            VoiceEnhancements::default()
        );
        let changed = VoiceEnhancements {
            echo_cancellation: false,
            noise_suppression: true,
            voice_enhancement: false,
        };
        phone
            .perform(Action::SetVoiceEnhancements(changed))
            .await
            .unwrap();
        let restarted = PhoneHandle::start(path.clone());
        assert_eq!(restarted.status.borrow().enhancements, changed);
        restarted.shutdown().await;
        phone.shutdown().await;
    }

    #[test]
    fn validates_dial_targets_and_secure_transport() {
        for target in [
            "1001",
            "*98",
            "+14165550100",
            "sip:alice@pbx.example.com",
            "bob@127.0.0.1:5060",
        ] {
            assert!(validate_target(target, false).is_ok(), "{target}");
        }
        for target in [
            "",
            "sip:",
            "a@@host",
            "a@host:0",
            "a@host:99999",
            "a@-host",
            "a@host:abc",
            "a@host;transport=tcp",
            "a:secret@host",
            "1001\r\nHeader: injected",
            "sips:a@host",
        ] {
            assert!(validate_target(target, false).is_err(), "{target}");
        }
        assert!(validate_target("sips:a@host", true).is_ok());
    }

    #[test]
    fn caller_labels_hide_credentials_and_routing_parameters() {
        assert_eq!(
            caller_label("Alice <sip:1001@pbx.example.com>;tag=secret"),
            "Alice"
        );
        let label =
            caller_label("<sip:1001:password@pbx.example.com;transport=udp?Subject=secret>");
        assert!(!label.contains("password") && !label.contains("secret"));
        assert_eq!(caller_label("not a sip address"), "Unknown caller");
    }

    #[tokio::test]
    async fn offline_actor_rejects_stale_call_actions_and_shuts_down() {
        let phone = PhoneHandle::start(std::env::temp_dir().join(format!(
            "softphone-missing-{}/account.json",
            std::process::id()
        )));
        for action in [
            Action::Dial {
                target: "1001".into(),
            },
            Action::Answer {
                call_id: "stale".into(),
            },
            Action::Decline {
                call_id: "stale".into(),
            },
            Action::HangUp {
                call_id: "stale".into(),
            },
            Action::SetMute {
                call_id: "stale".into(),
                muted: true,
            },
            Action::SendDtmf {
                call_id: "stale".into(),
                digit: '5',
            },
        ] {
            assert!(
                timeout(Duration::from_secs(1), phone.perform(action))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        timeout(Duration::from_secs(1), phone.shutdown())
            .await
            .unwrap();
        assert!(phone.perform(Action::Disconnect).await.is_err());
    }
}

#[cfg(test)]
mod pbx_tests {
    use super::*;

    async fn wait_for(phone: &PhoneHandle, predicate: impl Fn(&Status) -> bool) -> Status {
        let mut status = phone.status.clone();
        timeout(Duration::from_secs(15), async {
            loop {
                let snapshot = status.borrow_and_update().clone();
                if predicate(&snapshot) {
                    return snapshot;
                }
                status.changed().await.expect("SIP actor stopped");
            }
        })
        .await
        .expect("PBX state transition timed out")
    }

    async fn action(phone: &PhoneHandle, action: Action) -> Result<(), String> {
        timeout(Duration::from_secs(20), phone.perform(action))
            .await
            .expect("SIP command stalled")
    }

    /// Requires an isolated PBX: 600 answers with an IVR/echo, 601 rings,
    /// 602 returns busy. Use virtual audio or an explicitly selected test device.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires SOFTPHONE_TEST_SERVER, USER, PASSWORD and a test PBX"]
    async fn pbx_registration_call_controls_and_repeated_teardown() {
        use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer().with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("softphone", tracing::Level::INFO)
                        .with_target("rvoip_audio_device", tracing::Level::ERROR),
                ),
            )
            .try_init();
        let directory = std::env::temp_dir().join(format!("softphone-pbx-{}", std::process::id()));
        let phone = PhoneHandle::start(directory.join("account.json"));
        let account = Account {
            registrar: std::env::var("SOFTPHONE_TEST_SERVER").expect("test server"),
            username: std::env::var("SOFTPHONE_TEST_USER").expect("test username"),
            secure: std::env::var("SOFTPHONE_TEST_TLS").is_ok_and(|value| value == "1"),
            ..Default::default()
        };
        let password = std::env::var("SOFTPHONE_TEST_PASSWORD").expect("test password");
        action(
            &phone,
            Action::Register {
                account,
                password: Some(password),
            },
        )
        .await
        .unwrap();
        wait_for(&phone, |status| {
            status.registration == Registration::Registered
        })
        .await;
        action(
            &phone,
            Action::Dial {
                target: "602".into(),
            },
        )
        .await
        .unwrap();
        wait_for(&phone, |status| {
            status.call.is_none() && status.notice.as_deref() == Some("The peer is busy.")
        })
        .await;
        action(
            &phone,
            Action::Dial {
                target: "601".into(),
            },
        )
        .await
        .unwrap();
        let ringing = wait_for(&phone, |status| {
            status
                .call
                .as_ref()
                .is_some_and(|call| call.phase == CallPhase::Ringing)
        })
        .await;
        action(
            &phone,
            Action::HangUp {
                call_id: ringing.call.unwrap().id,
            },
        )
        .await
        .unwrap();
        wait_for(&phone, |status| status.call.is_none()).await;
        for _ in 0..3 {
            action(
                &phone,
                Action::Dial {
                    target: "600".into(),
                },
            )
            .await
            .unwrap();
            let connected = wait_for(&phone, |status| {
                status
                    .call
                    .as_ref()
                    .is_some_and(|call| call.phase == CallPhase::Connected)
            })
            .await;
            let id = connected.call.unwrap().id;
            assert!(
                action(
                    &phone,
                    Action::HangUp {
                        call_id: "stale".into()
                    }
                )
                .await
                .is_err()
            );
            assert!(
                action(
                    &phone,
                    Action::Dial {
                        target: "601".into()
                    }
                )
                .await
                .is_err()
            );
            assert!(
                action(
                    &phone,
                    Action::SelectAudio {
                        input: None,
                        output: None
                    }
                )
                .await
                .is_err()
            );
            assert!(
                action(
                    &phone,
                    Action::SendDtmf {
                        call_id: id.clone(),
                        digit: 'X'
                    }
                )
                .await
                .is_err()
            );
            action(
                &phone,
                Action::SetMute {
                    call_id: id.clone(),
                    muted: true,
                },
            )
            .await
            .unwrap();
            assert!(phone.status.borrow().call.as_ref().unwrap().muted);
            // Exercise the continuous input watchdog, including while muted.
            tokio::time::sleep(Duration::from_secs(6)).await;
            assert!(
                phone.status.borrow().notice.is_none(),
                "unexpected media warning: {:?}",
                phone.status.borrow().notice
            );
            action(
                &phone,
                Action::SetMute {
                    call_id: id.clone(),
                    muted: false,
                },
            )
            .await
            .unwrap();
            action(
                &phone,
                Action::SendDtmf {
                    call_id: id.clone(),
                    digit: '5',
                },
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            action(&phone, Action::HangUp { call_id: id })
                .await
                .unwrap();
            wait_for(&phone, |status| status.call.is_none()).await;
        }
        action(
            &phone,
            Action::Dial {
                target: "601".into(),
            },
        )
        .await
        .unwrap();
        wait_for(&phone, |status| {
            status
                .call
                .as_ref()
                .is_some_and(|call| call.phase == CallPhase::Ringing)
        })
        .await;
        timeout(Duration::from_secs(10), phone.shutdown())
            .await
            .unwrap();
        assert!(phone.status.borrow().registration == Registration::Offline);
        assert!(phone.status.borrow().call.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod hangup_tests {
    use super::*;
    use rvoip_sip::{Config, Endpoint, EndpointProfile};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_hangup_reaches_remote_peer_and_allows_another_call() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let mut remote = Endpoint::builder()
            .name("test-remote")
            .enable_sip_trace()
            .profile(EndpointProfile::Custom(Config::local("bob", port)))
            .build()
            .await
            .unwrap();
        let mut remote_events = remote.events().await.unwrap();
        let local = Account {
            registrar: format!("127.0.0.1:{port}"),
            username: "alice".into(),
            ..Default::default()
        }
        .endpoint("test-password".into())
        .await
        .unwrap();
        let (control, events) = local.split();
        let status = Status {
            account: Account::default(),
            registration: Registration::Registered,
            registration_error: None,
            history: Default::default(),
            call: None,
            audio: AudioSelection::default(),
            notice: None,
            record_calls: false,
            enhancements: Default::default(),
            recording_error: None,
        };
        let (publish, _receive) = watch::channel(status.clone());
        let mut phone = Phone {
            account_path: PathBuf::new(),
            log: None,
            history_path: None,
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
        };
        for attempt in 0..4 {
            let target = format!("sip:bob@127.0.0.1:{port}");
            phone.dial(&target).await.unwrap();
            let incoming = timeout(ACTION_TIMEOUT, remote.wait_for_incoming())
                .await
                .unwrap()
                .unwrap();
            let remote_call = incoming.answer().await.unwrap();
            let Some(Call::Outgoing { call, .. }) = phone.call.take() else {
                panic!("outgoing call missing")
            };
            let call = call.wait_for_answered(Some(ACTION_TIMEOUT)).await.unwrap();
            // Ensure the remote has received the ACK before testing an
            // established remote hang-up, rather than racing call setup.
            timeout(ACTION_TIMEOUT, async {
                loop {
                    if let Some(EndpointEvent::SipTrace(trace)) =
                        remote_events.next().await.unwrap()
                        && trace.start_line.starts_with("ACK ")
                    {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            let id = call.id().to_string();
            // Signaling-only regression: no microphone or PBX is required.
            phone.watch_call_end(&call);
            phone.call = Some(Call::Connected {
                call,
                peer: target,
                muted: Arc::new(AtomicBool::new(false)),
                audio: None,
                audio_started: Instant::now(),
            });
            let started = Instant::now();
            if attempt % 2 == 0 {
                phone.perform(Action::HangUp { call_id: id }).await.unwrap();
                assert!(matches!(phone.call, Some(Call::Ending { .. })));
                assert!(
                    timeout(ACTION_TIMEOUT, teardown_result(&mut phone.teardown))
                        .await
                        .unwrap()
                );
                phone.finish_call("Call ended.");
            } else {
                remote_call
                    .hangup_and_wait(Some(ACTION_TIMEOUT))
                    .await
                    .unwrap();
                assert!(
                    timeout(ACTION_TIMEOUT, teardown_result(&mut phone.call_end))
                        .await
                        .unwrap()
                );
                phone.finish_call("Call ended by peer.");
            }
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "healthy SIP teardown exceeded one second: {:?}",
                started.elapsed()
            );
            assert!(phone.call.is_none());
            assert!(
                phone.link.is_some(),
                "successful hang-up must preserve the account"
            );
            timeout(
                ACTION_TIMEOUT,
                remote_call.wait_for_end(Some(ACTION_TIMEOUT)),
            )
            .await
            .unwrap()
            .unwrap();
        }
        phone.disconnect().await;
        remote.shutdown().await.unwrap();
    }
}

#[cfg(test)]
#[path = "call_operations_tests.rs"]
mod operations_tests;
