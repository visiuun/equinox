//! OLED look: true black, hairline borders, one accent colour. Plus the few
//! custom widgets egui doesn't have (faders, switches, chips).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind,
    Ui, pos2, vec2,
};

pub const BG: Color32 = Color32::BLACK;
pub const CARD: Color32 = Color32::from_rgb(7, 7, 8);
pub const RAISED: Color32 = Color32::from_rgb(16, 16, 18);
pub const LINE: Color32 = Color32::from_rgb(26, 26, 29);
pub const FAINT: Color32 = Color32::from_rgb(44, 44, 48);
pub const DIM: Color32 = Color32::from_rgb(120, 120, 128);
pub const TEXT: Color32 = Color32::from_rgb(228, 228, 232);
pub const WHITE: Color32 = Color32::from_rgb(250, 250, 252);
pub const VIOLET: Color32 = Color32::from_rgb(167, 139, 250);
pub const WARN: Color32 = Color32::from_rgb(255, 184, 77);
pub const BAD: Color32 = Color32::from_rgb(255, 92, 92);
pub const GOOD: Color32 = Color32::from_rgb(80, 230, 150);

pub const ACCENTS: [[u8; 3]; 6] = [
    [125, 249, 255],
    [94, 240, 200],
    [180, 140, 255],
    [255, 120, 170],
    [255, 196, 87],
    [235, 235, 240],
];

/// Distinct, soft colours for band nodes.
pub fn band_color(i: usize) -> Color32 {
    const C: [Color32; 10] = [
        Color32::from_rgb(255, 138, 101),
        Color32::from_rgb(255, 202, 87),
        Color32::from_rgb(160, 230, 110),
        Color32::from_rgb(90, 220, 190),
        Color32::from_rgb(100, 180, 255),
        Color32::from_rgb(160, 140, 255),
        Color32::from_rgb(230, 120, 240),
        Color32::from_rgb(255, 110, 150),
        Color32::from_rgb(200, 200, 140),
        Color32::from_rgb(140, 210, 255),
    ];
    C[i % C.len()]
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let mut load = |key: &str, files: &[&str]| -> bool {
        for f in files {
            if let Ok(bytes) = std::fs::read(dir.join(f)) {
                fonts.font_data.insert(key.to_string(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let ui_font = load("ui", &["segoeui.ttf"]);
    let semi = load("ui-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let mono = load("mono", &["CascadiaMono.ttf", "consola.ttf"]);
    let symbols = load("symbols", &["seguisym.ttf"]);
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    if ui_font {
        prop.insert(0, "ui".into());
    }
    if symbols {
        prop.push("symbols".into());
    }
    let mut semi_list = Vec::new();
    if semi {
        semi_list.push("ui-semibold".to_string());
    }
    semi_list.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(semibold(), semi_list);
    if mono {
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    }
    ctx.set_fonts(fonts);
}

pub fn style(ctx: &egui::Context, accent: Color32) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = Color32::from_rgb(10, 10, 11);
    v.faint_bg_color = Color32::from_rgb(9, 9, 10);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = accent;
    v.selection.bg_fill = accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.popup_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(200),
    };
    v.window_shadow = v.popup_shadow;
    let r = CornerRadius::same(7);
    for (w, fill, fg) in [
        (&mut v.widgets.noninteractive, BG, DIM),
        (&mut v.widgets.inactive, RAISED, TEXT),
        (&mut v.widgets.hovered, Color32::from_rgb(30, 30, 34), WHITE),
        (&mut v.widgets.active, Color32::from_rgb(42, 42, 48), WHITE),
        (&mut v.widgets.open, RAISED, TEXT),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, fg);
        w.corner_radius = r;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, FAINT);
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.slider_rail_height = 4.0;
        s.spacing.combo_width = 160.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.5));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(12.5));
    });
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(14)
        .inner_margin(16.0)
}

/// Small uppercase section label.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(10.5).color(DIM).extra_letter_spacing(1.4));
}

pub fn heading(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).size(15.5).family(semibold()).color(WHITE))
}

/// Filled call-to-action button.
pub fn primary(ui: &mut Ui, accent: Color32, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::BLACK).family(semibold()))
            .fill(accent)
            .corner_radius(8),
    )
}

/// Quiet bordered button.
pub fn ghost(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(TEXT))
            .fill(BG)
            .stroke(Stroke::new(1.0, FAINT))
            .corner_radius(8),
    )
}

/// Borderless icon/text button.
pub fn flat(ui: &mut Ui, text: impl Into<RichText>) -> Response {
    ui.add(egui::Button::new(text.into()).frame(false))
}

/// iOS-style switch.
pub fn switch(ui: &mut Ui, on: &mut bool, accent: Color32) -> Response {
    let size = vec2(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let p = ui.painter();
    let bg = lerp_color(Color32::from_rgb(38, 38, 42), accent, t);
    p.rect_filled(rect, 10.0, bg);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
    p.circle_filled(
        pos2(x, rect.center().y),
        7.0,
        if *on { Color32::BLACK } else { Color32::from_rgb(170, 170, 176) },
    );
    resp
}

/// Label + description on the left, switch on the right.
pub fn switch_row(ui: &mut Ui, on: &mut bool, accent: Color32, title: &str, desc: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 60.0).max(120.0));
            ui.label(RichText::new(title).color(TEXT));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(11.5).color(DIM));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = switch(ui, on, accent).changed();
        });
    });
    changed
}

/// Rounded selectable pill. Returns the click response.
pub fn chip(ui: &mut Ui, selected: bool, accent: Color32, text: &str, dot: Option<Color32>) -> Response {
    let font = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, if selected { WHITE } else { TEXT });
    let pad = vec2(14.0, 7.0);
    let dot_w = if dot.is_some() { 12.0 } else { 0.0 };
    let size = vec2(galley.size().x + pad.x * 2.0 + dot_w, galley.size().y + pad.y * 2.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let (fill, stroke) = if selected {
        (accent.gamma_multiply(0.14), Stroke::new(1.0, accent.gamma_multiply(0.8)))
    } else if resp.hovered() {
        (RAISED, Stroke::new(1.0, FAINT))
    } else {
        (BG, Stroke::new(1.0, LINE))
    };
    p.rect(rect, 99.0, fill, stroke, StrokeKind::Inside);
    let mut x = rect.left() + pad.x;
    if let Some(c) = dot {
        p.circle_filled(pos2(x + 3.0, rect.center().y), 3.5, c);
        x += dot_w;
    }
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    resp
}

/// Segmented control; returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)], accent: Color32) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(Color32::from_rgb(12, 12, 13))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(9)
        .inner_margin(3.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                // Inside right-to-left layouts, add in reverse so the order reads left to right.
                let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
                let ordered: Vec<&(T, &str)> = if rtl {
                    options.iter().rev().collect()
                } else {
                    options.iter().collect()
                };
                for (v, label) in ordered {
                    let sel = *value == *v;
                    let text = RichText::new(*label).size(12.5).color(if sel { Color32::BLACK } else { DIM });
                    let text = if sel { text.family(semibold()) } else { text };
                    let b = egui::Button::new(text)
                        .fill(if sel { accent } else { Color32::TRANSPARENT })
                        .corner_radius(7)
                        .min_size(vec2(0.0, 24.0));
                    if ui.add(b).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Vertical fader for the tone macros. Drag, scroll, double-click to zero.
pub fn fader(ui: &mut Ui, value: &mut f32, range: f32, label: &str, hint: &str, accent: Color32) -> Response {
    let size = vec2(78.0, 196.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let track = Rect::from_center_size(pos2(rect.center().x, rect.top() + 26.0 + 63.0), vec2(6.0, 126.0));
    let to_y = |v: f32| egui::remap_clamp(v, -range..=range, track.bottom()..=track.top());
    let before = *value;
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        *value -= resp.drag_delta().y * (2.0 * range / track.height()) * if fine { 0.2 } else { 1.0 };
    }
    if resp.hovered() {
        let s = ui.input(|i| i.smooth_scroll_delta.y);
        if s != 0.0 {
            *value += s.signum() * 0.5 * (s.abs() / 50.0).clamp(0.2, 1.0);
        }
    }
    if resp.double_clicked() {
        *value = 0.0;
    }
    *value = (value.clamp(-range, range) * 10.0).round() / 10.0;
    if *value != before {
        resp.mark_changed();
    }
    let p = ui.painter();
    let active = resp.dragged() || resp.hovered();
    let vt = if value.abs() < 0.05 {
        "0.0".to_string()
    } else {
        format!("{:+.1}", value)
    };
    p.text(
        pos2(rect.center().x, rect.top() + 9.0),
        egui::Align2::CENTER_CENTER,
        vt,
        FontId::new(15.0, semibold()),
        if value.abs() < 0.05 { DIM } else { WHITE },
    );
    // Track, zero tick, fill from zero to value.
    p.rect_filled(track, 3.0, Color32::from_rgb(22, 22, 25));
    for k in [-range, -range / 2.0, range / 2.0, range] {
        let y = to_y(k);
        p.line_segment([pos2(track.left() - 9.0, y), pos2(track.left() - 5.0, y)], Stroke::new(1.0, FAINT));
    }
    let zy = to_y(0.0);
    p.line_segment(
        [pos2(track.left() - 12.0, zy), pos2(track.right() + 12.0, zy)],
        Stroke::new(1.0, FAINT),
    );
    let vy = to_y(*value);
    let fill = Rect::from_x_y_ranges(track.x_range(), vy.min(zy)..=vy.max(zy));
    p.rect_filled(fill, 3.0, accent.gamma_multiply(if active { 1.0 } else { 0.85 }));
    let knob = Rect::from_center_size(pos2(track.center().x, vy), vec2(30.0, 12.0));
    p.rect_filled(knob.expand(if active { 1.5 } else { 0.0 }), 6.0, WHITE);
    p.line_segment(
        [pos2(knob.left() + 9.0, vy), pos2(knob.right() - 9.0, vy)],
        Stroke::new(1.5, Color32::from_gray(160)),
    );
    p.text(
        pos2(rect.center().x, rect.bottom() - 30.0),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::new(13.0, semibold()),
        TEXT,
    );
    p.text(
        pos2(rect.center().x, rect.bottom() - 13.0),
        egui::Align2::CENTER_CENTER,
        hint,
        FontId::proportional(10.0),
        DIM,
    );
    resp
}

/// Thin horizontal slider with a value label. `reset` is the double-click
/// value; `bipolar` fills from it (for +/- values) instead of from the left.
pub fn hslider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    accent: Color32,
    reset: Option<f32>,
    bipolar: bool,
    fmt: impl Fn(f32) -> String,
) -> Response {
    let center = reset;
    let w = ui.available_width();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click_and_drag());
    let track = Rect::from_min_max(
        pos2(rect.left() + 6.0, rect.center().y - 2.0),
        pos2(rect.right() - 64.0, rect.center().y + 2.0),
    );
    let (lo, hi) = (*range.start(), *range.end());
    let before = *value;
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        *value = egui::remap_clamp(p.x, track.x_range(), lo..=hi);
    }
    if resp.double_clicked()
        && let Some(c) = center
    {
        *value = c;
    }
    if resp.hovered() {
        let s = ui.input(|i| i.smooth_scroll_delta.y);
        if s != 0.0 {
            *value = (*value + s.signum() * (hi - lo) / 100.0).clamp(lo, hi);
        }
    }
    *value = (*value * 100.0).round() / 100.0;
    if *value != before {
        resp.mark_changed();
    }
    let p = ui.painter();
    p.rect_filled(track, 2.0, Color32::from_rgb(26, 26, 30));
    let x = egui::remap_clamp(*value, lo..=hi, track.x_range());
    let from = center
        .filter(|_| bipolar)
        .map(|c| egui::remap_clamp(c, lo..=hi, track.x_range()))
        .unwrap_or(track.left());
    p.rect_filled(Rect::from_x_y_ranges(x.min(from)..=x.max(from), track.y_range()), 2.0, accent);
    if let Some(c) = center.filter(|_| bipolar) {
        let cx = egui::remap_clamp(c, lo..=hi, track.x_range());
        p.line_segment(
            [pos2(cx, track.top() - 5.0), pos2(cx, track.bottom() + 5.0)],
            Stroke::new(1.0, FAINT),
        );
    }
    p.circle_filled(
        pos2(x, track.center().y),
        if resp.hovered() || resp.dragged() { 8.0 } else { 7.0 },
        WHITE,
    );
    p.text(
        pos2(rect.right(), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        fmt(*value),
        FontId::monospace(12.5),
        TEXT,
    );
    resp
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A filled area between a polyline and a baseline, fading from `top` to `bottom` alpha.
pub fn fill_to(painter: &egui::Painter, pts: &[Pos2], base_y: f32, near: Color32, far: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    for p in pts {
        mesh.colored_vertex(*p, near);
        mesh.colored_vertex(pos2(p.x, base_y), far);
    }
    for i in 0..pts.len() as u32 - 1 {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}
