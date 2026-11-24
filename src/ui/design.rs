use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, Response, RichText, Sense, Stroke, Ui,
    Vec2, pos2, vec2,
};

pub const PHONE_SIZE: Vec2 = vec2(78.0 * 96.0 / 25.4, 163.4 * 96.0 / 25.4);
const fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Color32,
    pub surface: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub tint: Color32,
    pub accent: Color32,
    pub primary: Color32,
    pub on_primary: Color32,
    pub danger: Color32,
}

impl Colors {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                bg: rgb(0x101a17),
                surface: rgb(0x192520),
                ink: rgb(0xf1f5ed),
                muted: rgb(0xa4b6ab),
                line: rgb(0x35453c),
                tint: rgb(0x273d32),
                accent: rgb(0xc3eaa3),
                primary: rgb(0xc3eaa3),
                on_primary: rgb(0x152d21),
                danger: rgb(0xffa49a),
            }
        } else {
            Self {
                bg: rgb(0xf7f8f3),
                surface: rgb(0xffffff),
                ink: rgb(0x172a21),
                muted: rgb(0x586c60),
                line: rgb(0xd7e0d7),
                tint: rgb(0xe6efdf),
                accent: rgb(0x315d40),
                primary: rgb(0x244e38),
                on_primary: rgb(0xf4ffe9),
                danger: rgb(0xa53e36),
            }
        }
    }
    pub fn of(ui: &Ui) -> Self {
        Self::new(ui.visuals().dark_mode)
    }
}

pub fn semibold() -> egui::FontFamily {
    egui::FontFamily::Name("Barlow Semibold".into())
}

pub fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let base = egui::FontFamily::Proportional;
    let fallbacks = fonts.families[&base].clone();
    for (name, bytes, family) in [
        (
            "Barlow",
            include_bytes!("../../assets/fonts/Barlow-Regular.ttf").as_slice(),
            base,
        ),
        (
            "Barlow Semibold",
            include_bytes!("../../assets/fonts/Barlow-SemiBold.ttf").as_slice(),
            semibold(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), egui::FontData::from_static(bytes).into());
        let mut chain = fallbacks.clone();
        chain.insert(0, name.into());
        fonts.families.insert(family, chain);
    }
    ctx.set_fonts(fonts);
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let c = Colors::new(theme == egui::Theme::Dark);
        let mut style = (*ctx.style_of(theme)).clone();
        style.spacing.item_spacing = vec2(6., 5.);
        style.spacing.button_padding = vec2(9., 6.);
        style.visuals.override_text_color = Some(c.ink);
        style.visuals.panel_fill = c.bg;
        style.visuals.window_fill = c.surface;
        style.visuals.extreme_bg_color = c.surface;
        style.visuals.weak_text_color = Some(c.muted);
        style.visuals.selection.bg_fill = c.tint;
        style.visuals.selection.stroke = Stroke::new(1.5, c.accent);
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
        ] {
            widget.corner_radius = 10.into();
            widget.fg_stroke = Stroke::new(1.4, c.ink);
        }
        style.visuals.widgets.inactive.weak_bg_fill = c.surface;
        style.visuals.widgets.hovered.weak_bg_fill = c.tint;
        style.visuals.widgets.active.weak_bg_fill = c.tint;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1., c.line);
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.));
        ctx.set_style_of(theme, style);
    }
}

#[derive(Clone, Copy)]
pub enum Icon {
    Wave,
    Menu,
    Search,
    History,
    Keypad,
    Contacts,
    User,
    Settings,
    Back,
    Close,
    Sun,
    Moon,
    Incoming,
    Outgoing,
    Missed,
    Next,
}

pub fn icon(p: &Painter, center: Pos2, size: f32, glyph: Icon, color: Color32) {
    let point = |x: f32, y: f32| center + vec2(x - 12., y - 12.) * (size / 24.);
    let stroke = Stroke::new((size / 13.).max(1.2), color);
    let path = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x, y, r| {
        p.circle_stroke(point(x, y), size * r / 24., stroke);
    };
    match glyph {
        Icon::Wave => {
            for (x, h) in [(3., 3.), (7.5, 7.), (12., 10.), (16.5, 5.), (21., 2.)] {
                p.rect_filled(
                    Rect::from_center_size(point(x, 12.), vec2(size * 0.115, size * h / 12.)),
                    3,
                    color,
                );
            }
        }
        Icon::Menu => {
            path(&[(3., 6.), (21., 6.)]);
            path(&[(3., 12.), (17., 12.)]);
            path(&[(3., 18.), (21., 18.)]);
        }
        Icon::Search => {
            circle(10.5, 10.5, 6.5);
            path(&[(15.5, 15.5), (21., 21.)]);
        }
        Icon::History => {
            circle(12., 12., 8.8);
            path(&[(12., 6.5), (12., 12.3), (16., 14.8)]);
        }
        Icon::Keypad => {
            for x in [6., 12., 18.] {
                for y in [5., 11.5, 18.] {
                    p.circle_filled(point(x, y), size * 0.083, color);
                }
            }
        }
        Icon::Contacts => {
            p.rect_stroke(
                Rect::from_min_max(point(4., 3.), point(20., 21.)),
                3,
                stroke,
                egui::StrokeKind::Middle,
            );
            circle(12., 9., 2.5);
            path(&[
                (7.5, 17.),
                (8., 15.5),
                (10., 14.),
                (14., 14.),
                (16., 15.5),
                (16.5, 17.),
            ]);
        }
        Icon::User => {
            circle(12., 8., 4.);
            path(&[
                (4., 20.),
                (4.8, 17.),
                (7., 15.),
                (10., 14.),
                (14., 14.),
                (17., 15.),
                (19.2, 17.),
                (20., 20.),
            ]);
        }
        Icon::Settings => {
            for (y, x) in [(6., 8.), (12., 16.), (18., 10.)] {
                path(&[(3., y), (x - 2., y)]);
                circle(x, y, 2.);
                path(&[(x + 2., y), (21., y)]);
            }
        }
        Icon::Back => path(&[(15., 5.), (8., 12.), (15., 19.)]),
        Icon::Next => path(&[(9., 6.), (15., 12.), (9., 18.)]),
        Icon::Close => {
            path(&[(6., 6.), (18., 18.)]);
            path(&[(18., 6.), (6., 18.)]);
        }
        Icon::Sun => {
            circle(12., 12., 4.);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([center + d * size * 0.32, center + d * size * 0.45], stroke);
            }
        }
        Icon::Moon => {
            path(&[
                (19.8, 15.),
                (17.8, 18.5),
                (14.8, 20.),
                (11.4, 20.4),
                (7.8, 19.2),
                (5., 16.6),
                (3.7, 13.),
                (4., 9.6),
                (5.6, 6.4),
                (9.2, 3.8),
                (8.3, 7.2),
                (8.8, 10.8),
                (10.8, 13.9),
                (14., 15.8),
                (17.3, 16.),
            ]);
        }
        Icon::Incoming => {
            path(&[(18., 5.), (6., 17.), (6., 9.)]);
            path(&[(6., 17.), (14., 17.)]);
        }
        Icon::Outgoing => {
            path(&[(6., 18.), (18., 6.), (10., 6.)]);
            path(&[(18., 6.), (18., 14.)]);
        }
        Icon::Missed => {
            path(&[(21., 7.), (13., 15.), (4., 6.), (4., 12.)]);
            path(&[(4., 6.), (10., 6.)]);
        }
    }
}

pub fn icon_button(ui: &mut Ui, glyph: Icon, label: &str) -> Response {
    let c = Colors::of(ui);
    let (r, res) = ui.allocate_exact_size(vec2(36., 36.), Sense::click());
    if res.hovered() || res.has_focus() {
        ui.painter().rect_filled(r, 12, c.tint);
    }
    icon(ui.painter(), r.center(), 18., glyph, c.ink);
    res.widget_info(|| {
        egui::WidgetInfo::labeled(egui::accesskit::Role::Button, ui.is_enabled(), label)
    });
    res.on_hover_text(label)
}

pub fn brand(ui: &Ui, center: Pos2) {
    let c = Colors::of(ui);
    let tile = Rect::from_center_size(center - vec2(23., 0.), vec2(25., 25.));
    ui.painter().rect_filled(tile, 8, c.primary);
    icon(ui.painter(), tile.center(), 17., Icon::Wave, c.on_primary);
    ui.painter().text(
        center - vec2(6., 0.),
        Align2::LEFT_CENTER,
        "Wise",
        FontId::new(20., semibold()),
        c.ink,
    );
}

// A clipped bank of voice waves gives the small sign-in screen a distinct identity.
pub fn waveform(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 73.), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 17, rgb(0x173d31));
    for i in 0..21 {
        let x = r.left() + 15. + (r.width() - 30.) * i as f32 / 20.;
        for j in 0..5 {
            p.circle_filled(pos2(x, r.top() + 12. + j as f32 * 12.), 0.7, rgb(0x426054));
        }
    }
    for i in 0..13 {
        let t = i as f32 / 12.;
        let points = (0..=120)
            .map(|n| {
                let x = n as f32 / 120.;
                let envelope = (std::f32::consts::PI * x).sin();
                let y = (x * 10.8 + t * 1.7 - 1.2).sin() * 0.22 + (x * 6.3 - t * 2.6).sin() * 0.12;
                pos2(
                    r.left() + 3. + x * (r.width() - 6.),
                    r.center().y + r.height() * envelope * (y + (t - 0.5) * 0.28),
                )
            })
            .collect();
        p.add(egui::Shape::line(
            points,
            Stroke::new(
                0.9,
                Color32::from_rgb(
                    (93. + 106. * t) as u8,
                    (155. + 75. * t) as u8,
                    (119. + 39. * t) as u8,
                ),
            ),
        ));
    }
}

pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(27.).family(semibold()));
}
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(13.).color(Colors::of(ui).muted));
}

pub fn field(ui: &mut Ui, label: &str, value: &mut String, hint: &str, secret: bool) -> Response {
    let c = Colors::of(ui);
    let label_response = ui.label(RichText::new(label).size(13.).family(semibold()));
    let response = ui.add(
        egui::TextEdit::singleline(value)
            .password(secret)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(10, 9))
                    .corner_radius(9)
                    .fill(c.surface)
                    .stroke(Stroke::new(1., c.line)),
            ),
    );
    #[cfg(target_os = "android")]
    crate::android_input::sync(&response, value, secret);
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            9,
            Stroke::new(1.5, c.accent),
            egui::StrokeKind::Inside,
        );
    }
    response.labelled_by(label_response.id)
}

pub fn primary(ui: &mut Ui, label: &str, enabled: bool) -> bool {
    let c = Colors::of(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new((
            egui::Atom::grow(),
            RichText::new(label)
                .family(semibold())
                .size(15.)
                .color(c.on_primary),
            egui::Atom::grow(),
        ))
        .fill(c.primary)
        .corner_radius(12)
        .min_size(vec2(ui.available_width(), 42.)),
    )
    .clicked()
}

pub fn avatar(p: &Painter, center: Pos2, radius: f32, name: &str, c: Colors) {
    p.circle_filled(center, radius, c.tint);
    let initials: String = name
        .trim_start_matches("sips:")
        .trim_start_matches("sip:")
        .split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if initials.is_empty() || initials.chars().any(|ch| !ch.is_alphabetic()) {
        icon(p, center, radius * 1.05, Icon::User, c.accent);
    } else {
        p.text(
            center,
            Align2::CENTER_CENTER,
            initials,
            FontId::proportional(radius * 0.86),
            c.accent,
        );
    }
}
