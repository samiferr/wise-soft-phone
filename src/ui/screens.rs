use super::*;
use chrono::{Local, TimeZone};
use egui::{
    Align, Align2, Button, FontId, Frame, Label, Layout, Rect, ScrollArea, Sense, Stroke, Ui,
    UiBuilder, pos2, vec2,
};

fn region(parent: &mut Ui, id: &'static str, rect: Rect) -> Ui {
    let mut ui = parent.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    ui.set_clip_rect(rect.intersect(parent.clip_rect()));
    ui
}

fn inset(rect: Rect, x: f32) -> Rect {
    rect.shrink2(vec2(x, 0.))
}

impl SoftphoneApp {
    fn navigate(&mut self, page: Page) {
        self.nav.page = page;
        self.nav.drawer = false;
        self.nav.searching = false;
        self.nav.query.clear();
        if page == Page::Profile {
            self.nav.profile_name = self.nav.book.name.clone();
            self.nav.profile_description = self.nav.book.description.clone();
        }
        if page == Page::AddContact {
            self.nav.draft = Contact::default();
        }
    }

    fn load_line(&mut self, status: &Status) {
        let path =
            library::line_directory(&self.account_path, &status.account).join("profile.json");
        self.nav = Navigation {
            signed_in: true,
            ..Default::default()
        };
        match library::load(&path) {
            Ok(book) => {
                self.nav.book = book;
                self.nav.book_path = Some(path);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn save_book(&mut self, book: AddressBook) -> bool {
        let result = self
            .nav
            .book_path
            .as_ref()
            .ok_or_else(|| "Saved profile data is unavailable.".to_owned())
            .and_then(|path| library::save(path, &book));
        match result {
            Ok(()) => {
                self.nav.book = book;
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn set_theme(&mut self, ui: &Ui, dark: bool) {
        ui.ctx().set_theme(if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        if let Err(e) = library::save(&self.account_path.with_file_name("appearance.json"), &dark) {
            self.error = Some(e);
        }
    }

    fn display_name<'a>(&'a self, status: &'a Status) -> &'a str {
        if self.nav.book.name.trim().is_empty() {
            &status.account.username
        } else {
            &self.nav.book.name
        }
    }

    pub(super) fn show(&mut self, ui: &mut Ui, status: &Status) {
        if status.registration == Registration::Registered && !self.nav.signed_in {
            self.load_line(status);
        }
        if status.registration == Registration::Offline && self.nav.signed_in {
            self.nav = Default::default();
            self.target.clear();
        }
        if status.call.is_some() {
            self.nav.drawer = false;
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if let Some(call) = &status.call {
                let call_id = call.id.clone();
                self.dispatch(
                    ui.ctx(),
                    if call.phase == CallPhase::Incoming {
                        Action::Decline { call_id }
                    } else {
                        Action::HangUp { call_id }
                    },
                );
            } else if self.nav.drawer {
                self.nav.drawer = false;
            } else if self.nav.searching {
                self.nav.searching = false;
                self.nav.query.clear();
            } else if self.nav.page == Page::AddContact {
                self.navigate(Page::Contacts);
            } else {
                self.navigate(Page::History);
            }
        }

        let c = Colors::of(ui);
        let outer = ui.max_rect();
        let bounds = Rect::from_center_size(outer.center(), outer.size().min(PHONE_SIZE));
        ui.painter().rect_filled(outer, 0, c.line);
        ui.painter().rect_filled(bounds, 14, c.bg);
        ui.painter().rect_stroke(
            bounds,
            14,
            Stroke::new(1., c.line),
            egui::StrokeKind::Inside,
        );
        let mut shell = region(ui, "phone", bounds.shrink(2.));
        let progress = shell.ctx().animate_bool_with_time(
            shell.scope_id().with("side menu"),
            self.nav.drawer,
            0.18,
        );
        let mut content = region(&mut shell, "content", bounds.shrink(2.));
        if progress > 0. || self.nav.drawer {
            content.disable();
        }
        let main = self.nav.signed_in || status.call.is_some();
        let calling = status.call.is_some();
        let tabs = main
            && !calling
            && matches!(self.nav.page, Page::History | Page::Contacts | Page::Keypad);
        let body = inset(content.max_rect(), 15.);
        let mut top = region(
            &mut content,
            "bar",
            Rect::from_min_max(
                body.min + vec2(0., 5.),
                pos2(body.right(), body.top() + 45.),
            ),
        );
        self.bar(&mut top, main, calling);
        let mut area = Rect::from_min_max(
            pos2(body.left(), body.top() + 53.),
            pos2(body.right(), body.bottom() - 9.),
        );
        if tabs {
            let nav = Rect::from_min_max(
                pos2(bounds.left() + 2., bounds.bottom() - 60.),
                bounds.max - vec2(2., 2.),
            );
            self.tabs(&mut content, nav);
            area.max.y = nav.top() - 6.;
        }
        let mut body_ui = region(&mut content, "body", area);
        // Call controls stay above scrolling messages and the tone pad.
        if let Some(call) = &status.call {
            heading(&mut body_ui, call.phase.label());
            body_ui.add(Label::new(RichText::new(&call.peer).size(18.)).truncate());
            if let Some((_, start)) = &self.connected {
                caption(&mut body_ui, &duration(start.elapsed().as_secs()));
            }
            self.call_controls(&mut body_ui, call);
            body_ui.add_space(8.);
        }
        if main && !calling {
            if self.nav.page == Page::History {
                body_ui.horizontal(|ui| {
                    heading(ui, "Call history");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (label, color) = if status.registration == Registration::Registered {
                            ("Online", c.accent)
                        } else {
                            ("Offline", c.muted)
                        };
                        ui.label(RichText::new(label).size(11.).color(color));
                        let (r, _) = ui.allocate_exact_size(vec2(6., 6.), Sense::hover());
                        ui.painter().circle_filled(r.center(), 2.5, color);
                    });
                });
            } else {
                heading(&mut body_ui, self.nav.page.title());
            }
            if self.nav.searching {
                let edit = body_ui.add(
                    egui::TextEdit::singleline(&mut self.nav.query)
                        .hint_text("Search names or numbers")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 7)),
                );
                #[cfg(target_os = "android")]
                crate::android_input::sync(&edit, &mut self.nav.query, false);
                if self.nav.query.is_empty() && body_ui.memory(|m| m.focused().is_none()) {
                    edit.request_focus();
                }
            }
            if self.nav.page == Page::History {
                self.chips(&mut body_ui);
            }
            body_ui.add_space(6.);
        }
        ScrollArea::vertical()
            .id_salt(("screen", main, calling, self.nav.page))
            .auto_shrink([false, false])
            .show(&mut body_ui, |ui| {
                ui.set_width(ui.available_width());
                self.messages(ui, status);
                if calling {
                    if status
                        .call
                        .as_ref()
                        .is_some_and(|c| c.phase == CallPhase::Connected)
                    {
                        self.keypad(ui, status);
                    }
                } else if !main {
                    self.login(ui, status);
                } else {
                    match self.nav.page {
                        Page::History => self.history(ui, status),
                        Page::Keypad => self.keypad(ui, status),
                        Page::Contacts => self.contacts(ui),
                        Page::Profile => self.profile(ui, status),
                        Page::Settings => self.settings(ui, status),
                        Page::AddContact => self.add_contact(ui, status),
                    }
                }
            });
        if progress > 0. && main && !calling {
            self.drawer(&mut shell, status, progress);
        }
    }

    fn messages(&self, ui: &mut Ui, status: &Status) {
        let c = Colors::of(ui);
        for (message, color) in [
            (
                self.closing.then_some("Ending calls and disconnecting…"),
                c.muted,
            ),
            (
                self.error
                    .as_deref()
                    .or(status.registration_error.as_deref()),
                c.danger,
            ),
            (status.notice.as_deref(), c.muted),
        ] {
            if let Some(text) = message {
                ui.label(RichText::new(text).size(13.).color(color));
            }
        }
    }

    fn bar(&mut self, ui: &mut Ui, main: bool, calling: bool) {
        let rect = ui.max_rect();
        brand(ui, rect.center());
        ui.horizontal(|ui| {
            if main && !calling {
                if matches!(
                    self.nav.page,
                    Page::Profile | Page::Settings | Page::AddContact
                ) {
                    if icon_button(ui, Icon::Back, "Back").clicked() {
                        self.navigate(if self.nav.page == Page::AddContact {
                            Page::Contacts
                        } else {
                            Page::History
                        });
                    }
                } else if icon_button(ui, Icon::Menu, "Open menu").clicked() {
                    self.nav.drawer = true;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !main {
                    let dark = ui.visuals().dark_mode;
                    if icon_button(
                        ui,
                        if dark { Icon::Sun } else { Icon::Moon },
                        "Switch appearance",
                    )
                    .clicked()
                    {
                        self.set_theme(ui, !dark);
                    }
                } else if !calling
                    && matches!(self.nav.page, Page::History | Page::Contacts)
                    && icon_button(
                        ui,
                        if self.nav.searching {
                            Icon::Close
                        } else {
                            Icon::Search
                        },
                        if self.nav.searching {
                            "Close search"
                        } else {
                            "Search"
                        },
                    )
                    .clicked()
                {
                    self.nav.searching = !self.nav.searching;
                    self.nav.query.clear();
                }
            });
        });
    }

    fn tabs(&mut self, ui: &mut Ui, rect: Rect) {
        let c = Colors::of(ui);
        ui.painter().rect_filled(rect, 12, c.surface);
        ui.painter()
            .hline(rect.x_range(), rect.top(), Stroke::new(1., c.line));
        for (i, (page, glyph)) in [
            (Page::Keypad, Icon::Keypad),
            (Page::History, Icon::History),
            (Page::Contacts, Icon::Contacts),
        ]
        .into_iter()
        .enumerate()
        {
            let width = rect.width() / 3.;
            let r = Rect::from_min_size(
                rect.min + vec2(width * i as f32, 1.),
                vec2(width, rect.height() - 2.),
            );
            let response = ui.interact(r, ui.scope_id().with(page), Sense::click());
            let active = self.nav.page == page;
            let center = pos2(r.center().x, r.top() + 17.);
            let color = if active { c.accent } else { c.muted };
            if active || response.has_focus() || response.hovered() {
                ui.painter().rect_filled(
                    Rect::from_center_size(center, vec2(44., 25.)),
                    12,
                    c.tint,
                );
            }
            icon(ui.painter(), center, 18., glyph, color);
            let label = if page == Page::History {
                "History"
            } else {
                page.title()
            };
            ui.painter().text(
                pos2(center.x, r.top() + 39.),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(12.),
                color,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Tab, ui.is_enabled(), active, label)
            });
            if response.clicked() {
                self.navigate(page);
            }
        }
    }

    fn drawer(&mut self, ui: &mut Ui, status: &Status, progress: f32) {
        let c = Colors::of(ui);
        let r = ui.max_rect();
        ui.painter()
            .rect_filled(r, 12, Color32::from_black_alpha((85. * progress) as u8));
        let w = (r.width() * 0.82).min(252.);
        let panel = Rect::from_min_size(r.min - vec2(w * (1. - progress), 0.), vec2(w, r.height()));
        let outside = Rect::from_min_max(pos2(panel.right(), r.top()), r.max);
        if ui
            .interact(
                outside,
                ui.scope_id().with("dismiss drawer"),
                Sense::click(),
            )
            .clicked()
        {
            self.nav.drawer = false;
        }
        ui.painter().rect_filled(panel, 12, c.surface);
        let mut menu = region(ui, "drawer", panel.shrink2(vec2(16., 12.)));
        menu.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, Icon::Close, "Close menu").clicked() {
                    self.nav.drawer = false;
                }
            })
        });
        let (r, _) = menu.allocate_exact_size(vec2(54., 54.), Sense::hover());
        avatar(
            menu.painter(),
            r.center(),
            26.,
            self.display_name(status),
            c,
        );
        menu.add(
            Label::new(
                RichText::new(self.display_name(status))
                    .size(21.)
                    .family(semibold()),
            )
            .truncate(),
        );
        menu.add(Label::new(RichText::new(&status.account.registrar).color(c.muted)).truncate());
        menu.add_space(12.);
        menu.separator();
        menu.add_space(8.);
        for (page, glyph) in [
            (Page::Profile, Icon::User),
            (Page::Settings, Icon::Settings),
        ] {
            let (r, res) =
                menu.allocate_exact_size(vec2(menu.available_width(), 44.), Sense::click());
            if res.hovered() || res.has_focus() {
                menu.painter().rect_filled(r, 10, c.tint);
            }
            icon(
                menu.painter(),
                pos2(r.left() + 13., r.center().y),
                18.,
                glyph,
                c.accent,
            );
            menu.painter().text(
                pos2(r.left() + 33., r.center().y),
                Align2::LEFT_CENTER,
                page.title(),
                FontId::proportional(15.),
                c.ink,
            );
            icon(
                menu.painter(),
                pos2(r.right() - 10., r.center().y),
                13.,
                Icon::Next,
                c.muted,
            );
            res.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, page.title()));
            if res.clicked() {
                self.navigate(page);
            }
        }
    }

    fn login(&mut self, ui: &mut Ui, status: &Status) {
        let offline = status.registration == Registration::Offline;
        let available = self.can_dispatch(false) && offline;
        ui.add_space(18.);
        waveform(ui);
        ui.add_space(10.);
        ui.vertical_centered(|ui| {
            heading(ui, "Sign in");
            caption(ui, "Connect your SIP account to start calling.");
        });
        ui.add_space(12.);
        let mut submit = false;
        ui.add_enabled_ui(available, |ui| {
            field(
                ui,
                "SIP server",
                &mut self.account.registrar,
                "pbx.example.com",
                false,
            );
            ui.add_space(2.);
            field(
                ui,
                "Username",
                &mut self.account.username,
                "Name or extension",
                false,
            );
            ui.add_space(2.);
            let hint = if self.account.remember_password {
                "Use saved password"
            } else {
                "Enter your password"
            };
            let password = field(ui, "Password", &mut self.password, hint, true);
            submit = password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.checkbox(
                &mut self.account.remember_password,
                "Remember in system keyring",
            );
        });
        ui.add_space(6.);
        let label = if offline && !self.pending {
            "Sign In"
        } else if status.registration == Registration::Failed {
            "Registration failed"
        } else {
            "Signing in…"
        };
        if (primary(ui, label, available) || submit) && available {
            let password = (!self.password.is_empty()).then(|| std::mem::take(&mut self.password));
            self.dispatch(
                ui.ctx(),
                Action::Register {
                    account: self.account.clone(),
                    password,
                },
            );
        }
        if !offline
            && ui
                .add_enabled(self.can_dispatch(true), Button::new("Disconnect"))
                .clicked()
        {
            self.dispatch(ui.ctx(), Action::Disconnect);
        }
        ui.add_space(4.);
        ui.add_enabled_ui(available, |ui| {
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
                caption(ui, "Secure mode requires a trusted PBX certificate.");
            });
        });
    }

    fn chips(&mut self, ui: &mut Ui) {
        let c = Colors::of(ui);
        ui.add_space(3.);
        ScrollArea::horizontal()
            .id_salt("history filters")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    ui.spacing_mut().button_padding = vec2(8., 5.);
                    for filter in [
                        None,
                        Some(CallType::Missed),
                        Some(CallType::Incoming),
                        Some(CallType::Outgoing),
                    ] {
                        let active = self.nav.filter == filter;
                        let text = filter.map_or("All", CallType::label);
                        let chip = Button::new(RichText::new(text).size(12.).color(if active {
                            c.on_primary
                        } else {
                            c.muted
                        }))
                        .fill(if active { c.primary } else { c.surface })
                        .stroke(Stroke::new(1., if active { c.primary } else { c.line }))
                        .min_size(vec2(35., 30.))
                        .corner_radius(15);
                        if ui.add(chip).clicked() {
                            self.nav.filter = filter;
                        }
                    }
                });
            });
    }

    fn history(&mut self, ui: &mut Ui, status: &Status) {
        let c = Colors::of(ui);
        let query = self.nav.query.trim().to_lowercase();
        let today = Local::now().date_naive();
        let mut last_day = None;
        let mut any = false;
        let mut destination = None;
        for record in status.history.iter() {
            if self.nav.filter.is_some_and(|kind| kind != record.kind) {
                continue;
            }
            let target = record.target.as_deref().unwrap_or("");
            let name = self
                .nav
                .book
                .contacts
                .iter()
                .find(|contact| contact.target == target)
                .map_or(record.peer.as_str(), |contact| contact.name.as_str());
            if !matches_query(&query, name, target) && !record.peer.to_lowercase().contains(&query)
            {
                continue;
            }
            let Some(time) = Local.timestamp_opt(record.timestamp, 0).single() else {
                continue;
            };
            any = true;
            let day = time.date_naive();
            if last_day != Some(day) {
                let group = if day == today {
                    "Today".into()
                } else if Some(day) == today.pred_opt() {
                    "Yesterday".into()
                } else {
                    day.format("%b %-d, %Y").to_string()
                };
                ui.add_space(3.);
                ui.label(
                    RichText::new(group)
                        .size(13.)
                        .family(semibold())
                        .color(c.muted),
                );
                last_day = Some(day);
            }
            let (glyph, color) = match record.kind {
                CallType::Incoming => (Icon::Incoming, c.accent),
                CallType::Outgoing => (Icon::Outgoing, c.accent),
                CallType::Missed => (Icon::Missed, c.danger),
            };
            let meta = record.seconds.map_or_else(
                || record.kind.label().to_owned(),
                |secs| format!("{} · {}", record.kind.label(), duration(secs)),
            );
            let time = time.format("%H:%M").to_string();
            if person_row(
                ui,
                name,
                &meta,
                Some((glyph, color)),
                &time,
                !target.is_empty(),
            )
            .clicked()
                && !target.is_empty()
            {
                destination = Some(target.to_owned());
            }
        }
        if !any {
            let empty = status.history.is_empty();
            empty_state(
                ui,
                Icon::History,
                if empty {
                    "No calls yet"
                } else {
                    "No matching calls"
                },
                if empty {
                    "Your incoming, outgoing, and missed calls will appear here."
                } else {
                    "Try a different filter or search."
                },
            );
            if empty && primary(ui, "Open keypad", true) {
                self.navigate(Page::Keypad);
            }
        }
        if let Some(target) = destination {
            self.target = target;
            self.navigate(Page::Keypad);
        }
    }

    fn keypad(&mut self, ui: &mut Ui, status: &Status) {
        let c = Colors::of(ui);
        let call = status.call.as_ref();
        let mut enter = false;
        if call.is_none() {
            caption(ui, "Enter a number or SIP address.");
            ui.add_space(10.);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.target)
                    .hint_text("Number or SIP address")
                    .align(Align2::CENTER_CENTER)
                    .font(FontId::proportional(21.))
                    .desired_width(f32::INFINITY)
                    .frame(
                        Frame::new()
                            .inner_margin(egui::Margin::symmetric(8, 10))
                            .fill(c.surface)
                            .corner_radius(12)
                            .stroke(Stroke::new(1., c.line)),
                    ),
            );
            #[cfg(target_os = "android")]
            crate::android_input::sync(&response, &mut self.target, false);
            enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.);
        }
        let available = self.can_dispatch(false);
        ui.add_enabled_ui(available, |ui| {
            for row in [
                [("1", ""), ("2", "ABC"), ("3", "DEF")],
                [("4", "GHI"), ("5", "JKL"), ("6", "MNO")],
                [("7", "PQRS"), ("8", "TUV"), ("9", "WXYZ")],
                [("*", ""), ("0", ""), ("#", "")],
            ] {
                ui.horizontal(|ui| {
                    let w =
                        ((ui.available_width() - ui.spacing().item_spacing.x * 2.) / 3.).floor();
                    for (digit, letters) in row {
                        let (r, res) = ui.allocate_exact_size(vec2(w, 48.), Sense::click());
                        let p = ui.painter();
                        p.rect_filled(
                            r,
                            13,
                            if res.hovered() || res.has_focus() {
                                c.tint
                            } else {
                                c.surface
                            },
                        );
                        p.rect_stroke(r, 13, Stroke::new(1., c.line), egui::StrokeKind::Inside);
                        p.text(
                            r.center() - vec2(0., if letters.is_empty() { 0. } else { 5. }),
                            Align2::CENTER_CENTER,
                            digit,
                            FontId::proportional(22.),
                            c.ink,
                        );
                        if !letters.is_empty() {
                            p.text(
                                r.center() + vec2(0., 12.),
                                Align2::CENTER_CENTER,
                                letters,
                                FontId::proportional(9.),
                                c.muted,
                            );
                        }
                        res.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), digit)
                        });
                        if res.clicked() {
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
        });
        if call.is_none() {
            ui.add_space(8.);
            let can = available
                && !self.target.trim().is_empty()
                && status.registration == Registration::Registered;
            if (primary(ui, "Call", can) || enter) && can {
                self.dispatch(
                    ui.ctx(),
                    Action::Dial {
                        target: self.target.clone(),
                    },
                );
            }
            ui.vertical_centered(|ui| {
                if ui
                    .add_enabled(
                        !self.target.is_empty(),
                        Button::new("Clear number").frame(false),
                    )
                    .clicked()
                {
                    self.target.clear();
                }
            });
        }
    }

    fn contacts(&mut self, ui: &mut Ui) {
        let query = self.nav.query.trim().to_lowercase();
        let mut destination = None;
        let mut found = false;
        for contact in &self.nav.book.contacts {
            if matches_query(&query, &contact.name, &contact.target) {
                found = true;
                if person_row(ui, &contact.name, &contact.target, None, "", true).clicked() {
                    destination = Some(contact.target.clone());
                }
            }
        }
        if !found {
            let empty = self.nav.book.contacts.is_empty();
            empty_state(
                ui,
                Icon::Contacts,
                if empty {
                    "No contacts yet"
                } else {
                    "No matching contacts"
                },
                if empty {
                    "Save a name and number for the people you call."
                } else {
                    "Try another name or number."
                },
            );
        }
        ui.add_space(6.);
        if primary(ui, "Add contact", true) {
            self.navigate(Page::AddContact);
        }
        if let Some(target) = destination {
            self.target = target;
            self.navigate(Page::Keypad);
        }
    }

    fn add_contact(&mut self, ui: &mut Ui, status: &Status) {
        field(ui, "Name", &mut self.nav.draft.name, "Contact name", false);
        field(
            ui,
            "Number or SIP address",
            &mut self.nav.draft.target,
            "Extension or SIP address",
            false,
        );
        ui.add_space(8.);
        if primary(
            ui,
            "Save contact",
            !self.nav.draft.name.trim().is_empty() && !self.nav.draft.target.trim().is_empty(),
        ) {
            let target = self.nav.draft.target.trim();
            if let Err(error) = crate::phone::validate_target(target, status.account.secure) {
                self.error = Some(error);
                return;
            }
            let mut book = self.nav.book.clone();
            let contact = Contact {
                name: self.nav.draft.name.trim().into(),
                target: target.into(),
            };
            book.contacts.retain(|item| item.target != target);
            book.contacts.push(contact);
            book.contacts
                .sort_by_cached_key(|row| row.name.to_lowercase());
            if self.save_book(book) {
                self.navigate(Page::Contacts);
            }
        }
    }

    fn profile(&mut self, ui: &mut Ui, status: &Status) {
        let c = Colors::of(ui);
        Frame::new()
            .fill(c.surface)
            .stroke(Stroke::new(1., c.line))
            .corner_radius(16)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(48., 48.), Sense::hover());
                    avatar(ui.painter(), r.center(), 24., self.display_name(status), c);
                    ui.add(
                        Label::new(
                            RichText::new(self.display_name(status))
                                .size(19.)
                                .family(semibold()),
                        )
                        .truncate(),
                    );
                    caption(
                        ui,
                        if status.registration == Registration::Registered {
                            "Online"
                        } else {
                            "Offline"
                        },
                    );
                    if !self.nav.book.description.is_empty() {
                        ui.add(Label::new(&self.nav.book.description).truncate());
                    }
                });
            });
        ui.add_space(7.);
        ui.label(RichText::new("Profile info").size(17.).family(semibold()));
        field(
            ui,
            "Display name",
            &mut self.nav.profile_name,
            &status.account.username,
            false,
        );
        field(
            ui,
            "About",
            &mut self.nav.profile_description,
            "Add a short description",
            false,
        );
        let changed = self.nav.profile_name != self.nav.book.name
            || self.nav.profile_description != self.nav.book.description;
        if primary(ui, "Save changes", changed) {
            let mut book = self.nav.book.clone();
            book.name = self.nav.profile_name.trim().to_owned();
            book.description = self.nav.profile_description.trim().to_owned();
            if self.save_book(book) {
                self.navigate(Page::Profile);
            }
        }
        ui.add_space(9.);
        ui.separator();
        if ui
            .add_enabled(
                self.can_dispatch(true),
                Button::new(RichText::new("Log out").color(c.danger).family(semibold()))
                    .min_size(vec2(ui.available_width(), 39.))
                    .fill(c.surface)
                    .stroke(Stroke::new(1., c.line)),
            )
            .clicked()
        {
            self.dispatch(ui.ctx(), Action::Disconnect);
        }
    }

    fn settings(&mut self, ui: &mut Ui, status: &Status) {
        ui.label(RichText::new("Appearance").size(17.).family(semibold()));
        let dark = ui.visuals().dark_mode;
        ui.horizontal(|ui| {
            for (label, value) in [("Light", false), ("Dark", true)] {
                if ui.selectable_label(dark == value, label).clicked() {
                    self.set_theme(ui, value);
                }
            }
        });
        ui.add_space(10.);
        ui.separator();
        ui.add_space(6.);
        ui.label(RichText::new("Audio devices").size(17.).family(semibold()));
        self.audio_devices(ui, status);
        ui.add_space(12.);
        ui.separator();
        caption(
            ui,
            if status.account.secure {
                "TLS signaling · SRTP voice · G.711"
            } else {
                "UDP signaling · RTP voice · G.711"
            },
        );
    }
}

fn duration(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn matches_query(query: &str, name: &str, target: &str) -> bool {
    name.to_lowercase().contains(query) || target.to_lowercase().contains(query)
}

fn empty_state(ui: &mut Ui, glyph: Icon, title: &str, detail: &str) {
    let c = Colors::of(ui);
    ui.add_space(18.);
    Frame::new()
        .fill(c.surface)
        .corner_radius(18)
        .stroke(Stroke::new(1., c.line))
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(6.);
                let (r, _) = ui.allocate_exact_size(vec2(60., 60.), Sense::hover());
                ui.painter()
                    .circle_stroke(r.center(), 28., Stroke::new(1., c.line));
                ui.painter().circle_filled(r.center(), 22., c.tint);
                icon(ui.painter(), r.center(), 23., glyph, c.accent);
                ui.add_space(3.);
                ui.label(RichText::new(title).size(19.).family(semibold()));
                caption(ui, detail);
                ui.add_space(6.);
            });
        });
    ui.add_space(6.);
}

fn person_row(
    ui: &mut Ui,
    name: &str,
    detail: &str,
    glyph: Option<(Icon, Color32)>,
    time: &str,
    enabled: bool,
) -> egui::Response {
    let c = Colors::of(ui);
    let (r, res) = ui.allocate_exact_size(
        vec2(ui.available_width(), 55.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    ui.painter().rect_filled(
        r,
        12,
        if res.hovered() && enabled || res.has_focus() {
            c.tint
        } else {
            c.surface
        },
    );
    ui.painter()
        .rect_stroke(r, 12, Stroke::new(1., c.line), egui::StrokeKind::Inside);
    avatar(
        ui.painter(),
        pos2(r.left() + 25., r.center().y),
        16.,
        name,
        c,
    );
    let x = r.left() + 49.;
    let title_right = r.right() - if time.is_empty() { 20. } else { 44. };
    let mut title = region(
        ui,
        "row name",
        Rect::from_min_max(pos2(x, r.top() + 7.), pos2(title_right, r.top() + 27.)),
    );
    title.add(
        Label::new(RichText::new(name).size(15.).family(semibold()))
            .truncate()
            .selectable(false),
    );
    let mut metadata = region(
        ui,
        "row detail",
        Rect::from_min_max(
            pos2(x + if glyph.is_some() { 15. } else { 0. }, r.top() + 29.),
            pos2(r.right() - 10., r.bottom() - 3.),
        ),
    );
    let color = glyph.map_or(c.muted, |(_, color)| color);
    metadata.add(
        Label::new(RichText::new(detail).size(12.).color(color))
            .truncate()
            .selectable(false),
    );
    if let Some((glyph, color)) = glyph {
        icon(ui.painter(), pos2(x + 6., r.top() + 36.), 13., glyph, color);
    }
    if time.is_empty() {
        icon(
            ui.painter(),
            pos2(r.right() - 12., r.center().y),
            12.,
            Icon::Next,
            c.muted,
        );
    } else {
        ui.painter().text(
            pos2(r.right() - 9., r.top() + 16.),
            Align2::RIGHT_CENTER,
            time,
            FontId::proportional(11.),
            c.muted,
        );
    }
    res.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            enabled,
            format!("{name}, {detail} {time}"),
        )
    });
    res
}
