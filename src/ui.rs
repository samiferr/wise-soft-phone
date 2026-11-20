use crate::{
    account::{Account, CodecPreference},
    audio::DeviceList,
    phone::{Action, CallPhase, PhoneHandle, Registration, Status},
};
use eframe::egui::{self, Color32, RichText};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio::runtime::Handle;

const GREEN: Color32 = Color32::from_rgb(78, 210, 163);
const RED: Color32 = Color32::from_rgb(239, 113, 122);
const MUTED: Color32 = Color32::from_rgb(157, 173, 194);

enum Completion {
    Action {
        result: Result<(), String>,
        ending_call: bool,
    },
    Devices(Result<DeviceList, String>),
    Shutdown,
}

pub struct SoftphoneApp {
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
}

impl SoftphoneApp {
    pub fn new(cc: &eframe::CreationContext<'_>, phone: Arc<PhoneHandle>, runtime: Handle) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.set_theme(egui::Theme::Dark);
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(17, 26, 40);
        style.visuals.window_fill = Color32::from_rgb(23, 35, 52);
        style.visuals.selection.bg_fill = Color32::from_rgb(38, 103, 91);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
        ctx.set_style_of(egui::Theme::Dark, style);
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
            error: None,
            connected: None,
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

    fn account_panel(&mut self, ui: &mut egui::Ui, status: &Status) {
        ui.heading("Your account");
        ui.label(RichText::new("Voice over SIP").color(MUTED));
        ui.add_space(12.0);
        ui.add_enabled_ui(
            status.registration == Registration::Offline && !self.pending && !self.closing,
            |ui| {
                field(
                    ui,
                    "SIP server",
                    &mut self.account.registrar,
                    "pbx.example.com:5060",
                    false,
                );
                field(ui, "Username", &mut self.account.username, "1001", false);
                field(
                    ui,
                    "Password",
                    &mut self.password,
                    if self.account.remember_password {
                        "Use saved password"
                    } else {
                        "SIP password"
                    },
                    true,
                );
                ui.checkbox(
                    &mut self.account.remember_password,
                    "Remember in system keyring",
                );
                ui.collapsing("Connection options", |ui| {
                    field(
                        ui,
                        "Authentication username",
                        &mut self.account.auth_username,
                        "Same as username",
                        false,
                    );
                    field(
                        ui,
                        "Outbound proxy",
                        &mut self.account.outbound_proxy,
                        "Automatic for Twilio",
                        false,
                    );
                    ui.checkbox(&mut self.account.secure, "TLS + required SRTP");
                    ui.small("Secure mode requires a trusted PBX certificate.");
                    egui::ComboBox::from_label("Audio codecs")
                        .selected_text(self.account.codecs.label())
                        .show_ui(ui, |ui| {
                            for choice in CodecPreference::ALL {
                                ui.selectable_value(
                                    &mut self.account.codecs,
                                    choice,
                                    choice.label(),
                                );
                            }
                        });
                });
            },
        );
        ui.add_space(8.0);
        let offline = status.registration == Registration::Offline;
        let label = if offline {
            "Register account"
        } else {
            "Disconnect"
        };
        if ui
            .add_enabled(
                self.can_dispatch(!offline),
                egui::Button::new(label).min_size(egui::vec2(ui.available_width(), 42.0)),
            )
            .clicked()
        {
            let action = if offline {
                let password = if self.password.is_empty() {
                    None
                } else {
                    Some(std::mem::take(&mut self.password))
                };
                Action::Register {
                    account: self.account.clone(),
                    password,
                }
            } else {
                Action::Disconnect
            };
            self.dispatch(ui.ctx(), action);
        }
        ui.add_space(18.0);
        ui.separator();
        ui.heading("Audio devices");
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
        ui.add_space(8.0);
        ui.label(
            RichText::new("Select devices between calls.\nOne account · One active call")
                .small()
                .color(MUTED),
        );
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
                            egui::Button::new("Answer").fill(Color32::from_rgb(26, 105, 78)),
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
                                egui::Button::new(if call.muted {
                                    "Unmute microphone"
                                } else {
                                    "Mute microphone"
                                }),
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
                        egui::Button::new(if call.phase == CallPhase::Ending {
                            "Disconnect now"
                        } else {
                            "Hang up"
                        })
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

    fn dialer(&mut self, ui: &mut egui::Ui, status: &Status) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            let call = status.call.as_ref();
            ui.label(
                RichText::new(call.map_or("Ready when you are", |c| c.phase.label()))
                    .size(25.0)
                    .strong(),
            );
            if let Some(call) = call {
                ui.add(
                    egui::Label::new(RichText::new(&call.peer).size(20.0).color(GREEN)).truncate(),
                );
                if let Some((_, started)) = &self.connected {
                    let seconds = started.elapsed().as_secs();
                    let elapsed = format!("{:02}:{:02}", seconds / 60, seconds % 60);
                    let text = match &call.codec {
                        Some(codec) => format!("{elapsed} · {codec}"),
                        None => elapsed,
                    };
                    ui.label(RichText::new(text).monospace().color(MUTED));
                } else {
                    ui.label(" ");
                }
            } else {
                ui.label(RichText::new("Dial an extension or SIP address").color(MUTED));
                ui.label(" ");
            }
            ui.add_space(14.0);
            let response = ui.add_enabled(
                call.is_none() && !self.closing,
                egui::TextEdit::singleline(&mut self.target)
                    .hint_text("Number or sip:user@server")
                    .font(egui::TextStyle::Heading)
                    .desired_width(360.0),
            );
            let dial =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.add_space(12.0);
            let connected = call.is_some_and(|c| c.phase == CallPhase::Connected);
            ui.add_enabled_ui(
                !self.pending && !self.closing && (call.is_none() || connected),
                |ui| {
                    for row in [
                        ["1", "2", "3"],
                        ["4", "5", "6"],
                        ["7", "8", "9"],
                        ["*", "0", "#"],
                    ] {
                        ui.horizontal(|ui| {
                            ui.add_space(((ui.available_width() - 314.0) / 2.0).max(0.0));
                            for digit in row {
                                if ui
                                    .add_sized(
                                        [98.0, 52.0],
                                        egui::Button::new(RichText::new(digit).size(24.0)),
                                    )
                                    .clicked()
                                {
                                    if let Some(call) = call {
                                        self.dispatch(
                                            ui.ctx(),
                                            Action::SendDtmf {
                                                call_id: call.id.clone(),
                                                digit: digit.chars().next().unwrap(),
                                            },
                                        );
                                    } else {
                                        self.target.push_str(digit);
                                    }
                                }
                            }
                        });
                    }
                },
            );
            ui.add_space(14.0);
            let available = !self.pending && !self.closing;
            if call.is_none() {
                let can_dial = available
                    && status.registration == Registration::Registered
                    && !self.target.trim().is_empty();
                let clicked = ui
                    .add_enabled(
                        can_dial,
                        egui::Button::new(RichText::new("Call").strong())
                            .fill(Color32::from_rgb(26, 105, 78))
                            .min_size(egui::vec2(314.0, 46.0)),
                    )
                    .clicked();
                if can_dial && (clicked || dial) {
                    self.dispatch(
                        ui.ctx(),
                        Action::Dial {
                            target: self.target.clone(),
                        },
                    );
                }
                if ui
                    .add_enabled(
                        available && !self.target.is_empty(),
                        egui::Button::new("Clear number"),
                    )
                    .clicked()
                {
                    self.target.clear();
                }
            }
        });
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
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            && let Some(call) = &status.call
        {
            let action = if call.phase == CallPhase::Incoming {
                Action::Decline {
                    call_id: call.id.clone(),
                }
            } else {
                Action::HangUp {
                    call_id: call.id.clone(),
                }
            };
            self.dispatch(ui.ctx(), action);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("SOFTPHONE").size(27.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = match status.registration {
                        Registration::Offline => ("Offline", MUTED),
                        Registration::Registering => ("Registering…", MUTED),
                        Registration::Registered => ("Registered", GREEN),
                        Registration::Failed => ("Registration failed", RED),
                    };
                    ui.label(RichText::new(label).color(color).strong());
                    if self.pending {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(12.0);
            ui.separator();
            if self.closing {
                ui.label("Ending calls and disconnecting…");
            }
            if let Some(error) = self.error.as_ref().or(status.registration_error.as_ref()) {
                ui.colored_label(RED, error);
            }
            if let Some(notice) = &status.notice {
                ui.label(notice);
            }
            if let Some(call) = &status.call {
                self.call_controls(ui, call);
                ui.separator();
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(300.0, 540.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.account_panel(ui, &status),
                    );
                    ui.add_space(12.0);
                    ui.vertical(|ui| self.dialer(ui, &status));
                });
                ui.separator();
                ui.label(
                    RichText::new(if self.account.secure {
                        "TLS signaling · SRTP voice · G.711"
                    } else {
                        "UDP signaling · RTP voice · G.711"
                    })
                    .small()
                    .color(MUTED),
                );
            });
        });
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str, password: bool) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .password(password)
            .desired_width(f32::INFINITY),
    );
}

fn device_picker(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut Option<String>,
    devices: &[String],
) {
    ui.label(label);
    egui::ComboBox::from_id_salt(label)
        .width(280.0)
        .selected_text(selected.as_deref().unwrap_or("System default"))
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, "System default");
            for device in devices {
                ui.selectable_value(selected, Some(device.clone()), device);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hangup_can_be_submitted_while_another_operation_is_pending() {
        let phone = Arc::new(PhoneHandle::start(
            std::env::temp_dir().join("softphone-ui-test/account.json"),
        ));
        let (sender, replies) = mpsc::channel();
        let mut app = SoftphoneApp {
            phone: phone.clone(),
            runtime: Handle::current(),
            sender,
            replies,
            account: Account::default(),
            password: String::new(),
            target: String::new(),
            devices: DeviceList::default(),
            scanning: false,
            pending: true,
            hangup_pending: false,
            closing: false,
            closed: false,
            ringing: false,
            error: None,
            connected: None,
        };
        assert!(!app.can_dispatch(false));
        assert!(app.can_dispatch(true));
        let ctx = egui::Context::default();
        let call = crate::phone::CallStatus {
            id: "ended".into(),
            peer: "test".into(),
            phase: CallPhase::Connected,
            muted: false,
            codec: Some("Opus wideband".into()),
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
}
