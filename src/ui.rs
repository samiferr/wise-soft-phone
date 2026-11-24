use crate::{
    account::Account,
    audio::DeviceList,
    phone::{Action, CallPhase, PhoneHandle, Registration, Status},
    player::{Player, PlayerState},
};
use eframe::egui::{self, Color32, RichText};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio::runtime::Handle;

mod design;
mod screens;
use crate::library::{self, AddressBook, CallType, Contact};
pub use design::PHONE_SIZE;
use design::*;
use std::path::PathBuf;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Page {
    #[default]
    History,
    Keypad,
    Contacts,
    Profile,
    Settings,
    AddContact,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::History => "Call history",
            Self::Keypad => "Keypad",
            Self::Contacts => "Contacts",
            Self::Profile => "Profile",
            Self::Settings => "Settings",
            Self::AddContact => "New contact",
        }
    }
}

#[derive(Default)]
struct Navigation {
    page: Page,
    drawer: bool,
    searching: bool,
    query: String,
    filter: Option<CallType>,
    /// History row whose recording controls are open, by recording filename.
    recording: Option<String>,
    confirm_delete: bool,
    book_path: Option<PathBuf>,
    book: AddressBook,
    draft: Contact,
    profile_name: String,
    profile_description: String,
    signed_in: bool,
}

enum Completion {
    Action {
        result: Result<(), String>,
        ending_call: bool,
    },
    Devices(Result<DeviceList, String>),
    Shutdown,
}

pub struct SoftphoneApp {
    account_path: PathBuf,
    nav: Navigation,
    phone: Arc<PhoneHandle>,
    runtime: Handle,
    sender: mpsc::Sender<Completion>,
    replies: mpsc::Receiver<Completion>,
    account: Account,
    password: String,
    target: String,
    devices: DeviceList,
    scanning: bool,
    pending: bool,
    hangup_pending: bool,
    closing: bool,
    closed: bool,
    ringing: bool,
    error: Option<String>,
    connected: Option<(String, Instant)>,
    playback: Option<(String, Player)>,
}

impl SoftphoneApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        phone: Arc<PhoneHandle>,
        runtime: Handle,
        account_path: PathBuf,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        configure(ctx);
        let theme = library::load::<Option<bool>>(&account_path.with_file_name("appearance.json"));
        if let Ok(Some(dark)) = theme {
            ctx.set_theme(if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
        }
        let account = phone.status.borrow().account.clone();
        let (sender, replies) = mpsc::channel();
        // Wake even when minimized so incoming calls can request user attention.
        let mut status = phone.status.clone();
        let repaint = ctx.clone();
        runtime.spawn(async move {
            while status.changed().await.is_ok() {
                repaint.request_repaint();
            }
        });
        let mut app = Self {
            phone,
            account_path,
            nav: Navigation::default(),
            runtime,
            sender,
            replies,
            account,
            password: String::new(),
            target: String::new(),
            devices: DeviceList::default(),
            scanning: false,
            pending: false,
            hangup_pending: false,
            closing: false,
            closed: false,
            ringing: false,
            error: theme.err(),
            connected: None,
            playback: None,
        };
        app.scan(ctx);
        app
    }

    fn dispatch(&mut self, ctx: &egui::Context, action: Action) {
        let ending_call = matches!(action, Action::HangUp { .. } | Action::Disconnect);
        if !self.can_dispatch(ending_call) {
            return;
        }
        if ending_call {
            self.hangup_pending = true;
        } else {
            self.pending = true;
        }
        self.error = None;
        if matches!(action, Action::HangUp { .. }) {
            tracing::info!("Hang-up control activated");
        }
        let phone = self.phone.clone();
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let result = phone.perform(action).await;
            let _ = sender.send(Completion::Action {
                result,
                ending_call,
            });
            ctx.request_repaint();
        });
    }

    /// Keeps the progress bar moving, reports playback failures, and releases
    /// the speaker as soon as a call needs it.
    fn poll_playback(&mut self, ctx: &egui::Context, in_call: bool) {
        let Some((_, player)) = &self.playback else {
            return;
        };
        match player.state() {
            _ if in_call => self.playback = None,
            PlayerState::Failed => {
                self.playback = None;
                self.error = Some("Could not play the recording.".into());
            }
            PlayerState::Loading | PlayerState::Playing => {
                ctx.request_repaint_after(Duration::from_millis(100))
            }
            PlayerState::Paused | PlayerState::Finished => {}
        }
    }

    fn can_dispatch(&self, ending_call: bool) -> bool {
        !self.closing && !self.hangup_pending && (ending_call || !self.pending)
    }

    fn scan(&mut self, ctx: &egui::Context) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::task::spawn_blocking(DeviceList::scan),
            )
            .await
            .map_err(|_| "Listing audio devices timed out.".into())
            .and_then(|result| result.map_err(|_| "Could not list audio devices.".into()));
            let _ = sender.send(Completion::Devices(result));
            ctx.request_repaint();
        });
    }

    fn audio_devices(&mut self, ui: &mut egui::Ui, status: &Status) {
        let mut input = status.audio.input.clone();
        let mut output = status.audio.output.clone();
        ui.add_enabled_ui(
            status.call.is_none() && !self.pending && !self.closing,
            |ui| {
                device_picker(ui, "Microphone", &mut input, &self.devices.inputs);
                device_picker(ui, "Speaker", &mut output, &self.devices.outputs);
                if ui
                    .add_enabled(
                        !self.scanning,
                        egui::Button::new(if self.scanning {
                            "Scanning…"
                        } else {
                            "Refresh devices"
                        }),
                    )
                    .clicked()
                {
                    self.scan(ui.ctx());
                }
            },
        );
        if input != status.audio.input || output != status.audio.output {
            self.dispatch(ui.ctx(), Action::SelectAudio { input, output });
        }
    }

    fn call_controls(&mut self, ui: &mut egui::Ui, call: &crate::phone::CallStatus) -> egui::Rect {
        let mut hangup_rect = egui::Rect::NOTHING;
        let available = self.can_dispatch(false);
        ui.push_id("active_call_controls", |ui| {
            ui.horizontal(|ui| {
                if call.phase == CallPhase::Incoming {
                    if ui
                        .add_enabled(
                            available,
                            egui::Button::new(RichText::new("Answer").color(Color32::WHITE))
                                .fill(Color32::from_rgb(26, 105, 78)),
                        )
                        .clicked()
                    {
                        self.dispatch(
                            ui.ctx(),
                            Action::Answer {
                                call_id: call.id.clone(),
                            },
                        );
                    }
                    if ui
                        .add_enabled(available, egui::Button::new("Decline"))
                        .clicked()
                    {
                        self.dispatch(
                            ui.ctx(),
                            Action::Decline {
                                call_id: call.id.clone(),
                            },
                        );
                    }
                } else {
                    if call.phase == CallPhase::Connected
                        && ui
                            .add_enabled(
                                available,
                                egui::Button::new(if call.muted { "Unmute" } else { "Mute" }),
                            )
                            .clicked()
                    {
                        self.dispatch(
                            ui.ctx(),
                            Action::SetMute {
                                call_id: call.id.clone(),
                                muted: !call.muted,
                            },
                        );
                    }
                    let hangup = ui.add_enabled(
                        self.can_dispatch(true),
                        egui::Button::new(
                            RichText::new(if call.phase == CallPhase::Ending {
                                "Disconnect now"
                            } else {
                                "Hang up"
                            })
                            .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(139, 48, 60)),
                    );
                    hangup_rect = hangup.rect;
                    if hangup.clicked() {
                        self.dispatch(
                            ui.ctx(),
                            Action::HangUp {
                                call_id: call.id.clone(),
                            },
                        );
                    }
                }
            });
        });
        hangup_rect
    }
}

impl eframe::App for SoftphoneApp {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Completion::Action {
                    result,
                    ending_call,
                } => {
                    if ending_call {
                        self.hangup_pending = false;
                    } else {
                        self.pending = false;
                    }
                    self.error = result.err();
                }
                Completion::Devices(result) => {
                    self.scanning = false;
                    match result {
                        Ok(devices) => self.devices = devices,
                        Err(error) => self.error = Some(error),
                    }
                }
                Completion::Shutdown => {
                    self.closed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let status = self.phone.status.borrow().clone();
        if self.ringing != status.is_ringing() {
            self.ringing = status.is_ringing();
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                if self.ringing {
                    egui::UserAttentionType::Critical
                } else {
                    egui::UserAttentionType::Reset
                },
            ));
        }
        match status
            .call
            .as_ref()
            .filter(|c| c.phase == CallPhase::Connected)
        {
            Some(call) if self.connected.as_ref().is_none_or(|(id, _)| id != &call.id) => {
                self.connected = Some((call.id.clone(), Instant::now()))
            }
            None => self.connected = None,
            _ => {}
        }
        if self.connected.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        self.poll_playback(ctx, status.call.is_some());
        if ctx.input(|input| input.viewport().close_requested()) && !self.closed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.closing {
                self.closing = true;
                let phone = self.phone.clone();
                let sender = self.sender.clone();
                let ctx = ctx.clone();
                self.runtime.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(10), phone.shutdown()).await;
                    let _ = sender.send(Completion::Shutdown);
                    ctx.request_repaint();
                });
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let status = self.phone.status.borrow().clone();
        self.show(ui, &status);
    }
}

fn device_picker(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut Option<String>,
    devices: &[String],
) {
    ui.label(label);
    egui::ComboBox::from_id_salt(label)
        .width(ui.available_width())
        .truncate()
        .selected_text(selected.as_deref().unwrap_or("System default"))
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, "System default");
            for device in devices {
                if ui
                    .add(
                        egui::Button::selectable(selected.as_ref() == Some(device), device)
                            .truncate(),
                    )
                    .clicked()
                {
                    *selected = Some(device.clone());
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn history_drawer_and_tabs_work_inside_a_large_window() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = test_app(temp.path().join("account.json"));
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut status = app.phone.status.borrow().clone();
        status.account.username = "1001".into();
        status.account.registrar = "example.test".into();
        status.registration = Registration::Registered;
        status.history = Arc::new(vec![library::Record {
            peer: "Test caller".into(),
            target: Some("1002".into()),
            kind: CallType::Incoming,
            timestamp: chrono::Local::now().timestamp(),
            seconds: Some(31),
            recording: None,
        }]);
        let desktop = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900., 720.));
        let phone = egui::Rect::from_center_size(desktop.center(), PHONE_SIZE);
        let mut time = 0.;
        let mut frame = |app: &mut SoftphoneApp, events| {
            time += 0.25;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(desktop),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| app.show(ui, &status),
            );
            output.textures_delta.clear();
            output
                .shapes
                .into_iter()
                .filter_map(|shape| {
                    let egui::Shape::Text(text) = shape.shape else {
                        return None;
                    };
                    let center = text.visual_bounding_rect().center();
                    shape
                        .clip_rect
                        .contains(center)
                        .then(|| (text.galley.text().to_owned(), center))
                })
                .collect::<std::collections::HashMap<_, _>>()
        };
        let click = |pos| {
            [true, false].map(|pressed| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ]
            })
        };
        frame(&mut app, vec![]);
        let labels = frame(&mut app, vec![]);
        assert_eq!(app.nav.page, Page::History);
        assert!(labels.values().all(|pos| phone.contains(*pos)));
        for events in click(labels["Missed"]) {
            frame(&mut app, events);
        }
        assert_eq!(app.nav.filter, Some(CallType::Missed));
        let labels = frame(&mut app, vec![]);
        assert!(labels.contains_key("No matching calls"));
        for events in click(labels["All"]) {
            frame(&mut app, events);
        }
        let labels = frame(&mut app, vec![]);
        for events in click(labels["Test caller"]) {
            frame(&mut app, events);
        }
        assert_eq!(app.nav.page, Page::Keypad);
        assert_eq!(app.target, "1002");
        for events in click(phone.min + egui::vec2(34., 27.)) {
            frame(&mut app, events);
        }
        frame(&mut app, vec![]);
        let labels = frame(&mut app, vec![]);
        assert!(app.nav.drawer);
        for events in click(labels["Profile"]) {
            frame(&mut app, events);
        }
        assert_eq!(app.nav.page, Page::Profile);
        assert!(!app.nav.drawer);
        app.phone.shutdown().await;
    }

    fn test_app(account_path: PathBuf) -> SoftphoneApp {
        let phone = Arc::new(PhoneHandle::start(account_path.clone()));
        let (sender, replies) = mpsc::channel();
        SoftphoneApp {
            phone,
            account_path,
            nav: Navigation::default(),
            runtime: Handle::current(),
            sender,
            replies,
            account: Account::default(),
            password: String::new(),
            target: String::new(),
            devices: DeviceList::default(),
            scanning: false,
            pending: false,
            hangup_pending: false,
            closing: false,
            closed: false,
            ringing: false,
            error: None,
            connected: None,
            playback: None,
        }
    }

    #[tokio::test]
    async fn hangup_can_be_submitted_while_another_operation_is_pending() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = test_app(temp.path().join("account.json"));
        let phone = app.phone.clone();
        app.pending = true;
        assert!(!app.can_dispatch(false));
        assert!(app.can_dispatch(true));
        let ctx = egui::Context::default();
        let call = crate::phone::CallStatus {
            id: "ended".into(),
            peer: "test".into(),
            phase: CallPhase::Connected,
            muted: false,
            recording: false,
        };
        let mut rect = egui::Rect::NOTHING;
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(780.0, 680.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input(), |ui| {
            rect = app.call_controls(ui, &call);
        });
        output.textures_delta.clear();
        assert!(rect.is_positive() && rect.bottom() < 100.0);
        let pointer = rect.center();
        for pressed in [true, false] {
            let mut event = input();
            event.events = vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let mut output = ctx.run_ui(event, |ui| {
                app.call_controls(ui, &call);
            });
            output.textures_delta.clear();
        }
        assert!(app.hangup_pending);
        assert!(
            !app.can_dispatch(true),
            "prevent duplicate hang-up requests"
        );
        tokio::task::yield_now().await;
        assert!(matches!(
            app.replies.try_recv(),
            Ok(Completion::Action {
                ending_call: true,
                ..
            })
        ));
        phone.shutdown().await;
    }

    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        ui: impl FnMut(&mut egui::Ui),
    ) -> Vec<(String, egui::Pos2)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400., 700.),
                )),
                events,
                ..Default::default()
            },
            ui,
        );
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.visual_bounding_rect().center(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn switch_flips_on_click_and_is_inert_when_disabled() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut value = false;
        let rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let frame = |ctx: &egui::Context, enabled: bool, value: &mut bool, events| {
            run_frame(ctx, events, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    rect.set(switch(ui, value, "Record calls").rect);
                });
            });
        };
        frame(&ctx, true, &mut value, vec![]);
        let pointer = rect.get().center();
        let click = |pressed| {
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ]
        };
        frame(
            &ctx,
            false,
            &mut value,
            vec![egui::Event::PointerMoved(pointer)],
        );
        for pressed in [true, false] {
            frame(&ctx, false, &mut value, click(pressed));
        }
        assert!(!value, "a disabled switch must not change");
        frame(
            &ctx,
            true,
            &mut value,
            vec![egui::Event::PointerMoved(pointer)],
        );
        for pressed in [true, false] {
            frame(&ctx, true, &mut value, click(pressed));
        }
        assert!(value);
    }

    #[tokio::test]
    async fn history_row_with_a_recording_offers_controls_and_delete_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = test_app(temp.path().join("account.json"));
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut status = app.phone.status.borrow().clone();
        status.history = Arc::new(vec![library::Record {
            peer: "Test caller".into(),
            target: Some("1002".into()),
            kind: CallType::Incoming,
            timestamp: chrono::Local::now().timestamp(),
            seconds: Some(31),
            recording: Some("call.wav".into()),
        }]);
        let show = |app: &mut SoftphoneApp, ctx: &egui::Context, status: &Status| {
            run_frame(ctx, vec![], |ui| app.history(ui, status))
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        let labels = show(&mut app, &ctx, &status);
        assert!(labels.contains(&"Test caller".to_owned()));
        assert!(
            !labels.contains(&"Ready".to_owned()),
            "controls start hidden"
        );

        app.nav.recording = Some("call.wav".into());
        let labels = show(&mut app, &ctx, &status);
        assert!(labels.contains(&"Ready".to_owned()));
        assert!(!labels.contains(&"Delete this recording?".to_owned()));

        app.nav.confirm_delete = true;
        let labels = show(&mut app, &ctx, &status);
        assert!(labels.contains(&"Delete this recording?".to_owned()));

        app.close_recording();
        assert!(app.nav.recording.is_none() && !app.nav.confirm_delete);
    }

    #[tokio::test]
    async fn settings_offer_voice_quality_switches_that_lock_during_calls() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = test_app(temp.path().join("account.json"));
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut status = app.phone.status.borrow().clone();
        let show = |app: &mut SoftphoneApp, status: &Status| {
            run_frame(&ctx, vec![], |ui| app.settings(ui, status))
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        let labels = show(&mut app, &status);
        for title in [
            "Voice quality",
            "Echo cancellation",
            "Noise suppression",
            "Voice enhancement",
            "Record calls",
        ] {
            assert!(labels.contains(&title.to_owned()), "{title}");
        }
        assert!(!labels.contains(&"Change this between calls.".to_owned()));

        status.call = Some(crate::phone::CallStatus {
            id: "active".into(),
            peer: "test".into(),
            phase: CallPhase::Connected,
            muted: false,
            recording: false,
        });
        assert!(show(&mut app, &status).contains(&"Change this between calls.".to_owned()));
    }
}
