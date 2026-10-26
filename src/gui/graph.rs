//! The frequency response graph: live spectrum behind, correction / profile /
//! total curves, and draggable band nodes.

use std::sync::{Arc, OnceLock};

use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use equinox::dsp::{self, Biquad, F_MAX, F_MIN, fmt_db, fmt_freq};
use equinox::engine::{self, Resolved};
use equinox::model::{Band, Chain, FilterKind, Profile};

use super::App;
use super::theme::*;

pub fn spectrum_freqs() -> &'static [f32] {
    static F: OnceLock<Vec<f32>> = OnceLock::new();
    F.get_or_init(|| dsp::log_freqs(200, F_MIN, F_MAX))
}

/// Tiny curve logo for the title bar.
pub fn mini_mark(p: &Painter, r: Rect, accent: Color32) {
    p.rect(r, 6.0, Color32::from_rgb(10, 10, 12), Stroke::new(1.0, LINE), StrokeKind::Inside);
    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let x = 0.16 + 0.68 * t;
            let y = 0.56 - 0.24 * (-((x - 0.36) / 0.12).powi(2)).exp() + 0.16 * (-((x - 0.7) / 0.11).powi(2)).exp();
            pos2(r.left() + x * r.width(), r.top() + y * r.height())
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(1.8, accent)));
}

const GRID_F: [(f32, &str); 10] = [
    (20.0, "20"),
    (50.0, "50"),
    (100.0, "100"),
    (200.0, "200"),
    (500.0, "500"),
    (1e3, "1k"),
    (2e3, "2k"),
    (5e3, "5k"),
    (1e4, "10k"),
    (2e4, "20k"),
];
const MINOR_F: [f32; 18] = [
    30.0, 40.0, 60.0, 70.0, 80.0, 90.0, 300.0, 400.0, 600.0, 700.0, 800.0, 900.0, 3e3, 4e3, 6e3, 7e3, 8e3, 9e3,
];

struct Map {
    plot: Rect,
    span: f32,
}

impl Map {
    fn x(&self, f: f32) -> f32 {
        let t = (f.max(1.0) / F_MIN).ln() / (F_MAX / F_MIN).ln();
        self.plot.left() + t * self.plot.width()
    }
    fn f(&self, x: f32) -> f32 {
        let t = (x - self.plot.left()) / self.plot.width();
        F_MIN * (F_MAX / F_MIN).powf(t)
    }
    fn y(&self, db: f32) -> f32 {
        self.plot.center().y - db / self.span * (self.plot.height() / 2.0)
    }
    fn db(&self, y: f32) -> f32 {
        (self.plot.center().y - y) / (self.plot.height() / 2.0) * self.span
    }
}

/// Everything the graph and output card derive from the selected chain.
/// Rebuilt only when the chain, its profile or the headroom change.
pub struct Curves {
    key: (Chain, Option<Profile>, f32),
    pub r: Resolved,
    pub corr: Vec<f32>,
    pub prof: Vec<f32>,
    pub total: Vec<f32>,
    /// Response of each profile band on its own (index = band index).
    pub bands: Vec<Vec<f32>>,
}

impl App {
    pub fn curves(&mut self) -> Arc<Curves> {
        let chain = self.st.chain(&self.target).clone();
        let profile = chain.profile.and_then(|id| self.st.profile(id)).cloned();
        let key = (chain, profile, self.st.settings.headroom);
        if let Some(c) = &self.curve_cache
            && c.key == key
        {
            return c.clone();
        }
        let r = engine::resolve(&self.st, &key.0);
        let corr = dsp::response(&r.correction, &r.graphic, &self.freqs);
        let prof = dsp::response(&r.profile, &[], &self.freqs);
        let total = corr.iter().zip(&prof).map(|(a, b)| a + b).collect();
        let bands = key
            .1
            .as_ref()
            .map(|p| {
                p.bands
                    .iter()
                    .map(|b| dsp::response(std::slice::from_ref(b), &[], &self.freqs))
                    .collect()
            })
            .unwrap_or_default();
        let c = Arc::new(Curves {
            key,
            r,
            corr,
            prof,
            total,
            bands,
        });
        self.curve_cache = Some(c.clone());
        c
    }

    pub fn graph(&mut self, ui: &mut Ui, height: f32) {
        let (resp, painter) = ui.allocate_painter(vec2(ui.available_width(), height), Sense::click_and_drag());
        let outer = resp.rect;
        painter.rect(outer, 14.0, CARD, Stroke::new(1.0, LINE), StrokeKind::Inside);
        let plot = Rect::from_min_max(outer.min + vec2(46.0, 40.0), outer.max - vec2(18.0, 28.0));

        let cv = self.curves();
        let (r, corr, prof, total) = (&cv.r, &cv.corr, &cv.prof, &cv.total);
        let pid = self.st.chain(&self.target).profile.filter(|id| self.st.profile(*id).is_some());
        let bands: Vec<Band> = pid.and_then(|id| self.st.profile(id)).map(|p| p.bands.clone()).unwrap_or_default();
        let has_corr = !r.correction.is_empty() || !r.graphic.is_empty();

        // Auto range, frozen while dragging so nodes don't jump under the cursor.
        if self.drag_band.is_none() {
            let m = total
                .iter()
                .chain(corr)
                .chain(prof)
                .chain(bands.iter().map(|b| &b.gain))
                .fold(0f32, |m, v| m.max(v.abs()));
            self.span = if m <= 10.5 {
                12.0
            } else if m <= 16.5 {
                18.0
            } else {
                24.0
            };
        }
        let span = ui.ctx().animate_value_with_time(resp.id.with("span"), self.span, 0.2);
        let map = Map { plot, span };
        let accent = self.accent;

        let grid = Stroke::new(1.0, Color32::from_rgb(20, 20, 23));
        for f in MINOR_F {
            let x = map.x(f);
            painter.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(1.0, Color32::from_rgb(13, 13, 15)),
            );
        }
        for (f, label) in GRID_F {
            let x = map.x(f);
            painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], grid);
            painter.text(
                pos2(x, plot.bottom() + 14.0),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(10.5),
                DIM,
            );
        }
        let step = 6.0;
        let mut db = -span.floor();
        while db <= span + 0.01 {
            if db.rem_euclid(step) < 0.01 {
                let y = map.y(db);
                if y >= plot.top() - 1.0 && y <= plot.bottom() + 1.0 {
                    let zero = db.abs() < 0.01;
                    painter.line_segment(
                        [pos2(plot.left(), y), pos2(plot.right(), y)],
                        if zero {
                            Stroke::new(1.0, Color32::from_rgb(52, 52, 58))
                        } else {
                            grid
                        },
                    );
                    painter.text(
                        pos2(plot.left() - 10.0, y),
                        Align2::RIGHT_CENTER,
                        if zero { "0".into() } else { format!("{db:+.0}") },
                        FontId::proportional(10.5),
                        if zero { TEXT } else { DIM },
                    );
                }
            }
            db += 1.0;
        }

        if !self.spectrum.is_empty() {
            let sf = spectrum_freqs();
            let pts: Vec<Pos2> = sf
                .iter()
                .zip(&self.spectrum)
                .map(|(&f, &v)| {
                    let t = ((v + 90.0) / 80.0).clamp(0.0, 1.0);
                    pos2(map.x(f), egui::lerp(plot.bottom()..=plot.top(), t))
                })
                .collect();
            if pts.iter().any(|p| p.y < plot.bottom() - 1.0) {
                fill_to(
                    &painter,
                    &pts,
                    plot.bottom(),
                    Color32::from_rgba_unmultiplied(120, 120, 135, 46),
                    Color32::from_rgba_unmultiplied(60, 60, 70, 4),
                );
                painter.add(egui::Shape::line(
                    pts,
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(150, 150, 165, 60)),
                ));
            }
        }

        let to_pts = |v: &[f32]| -> Vec<Pos2> {
            self.freqs
                .iter()
                .zip(v)
                .map(|(&f, &d)| pos2(map.x(f), map.y(d).clamp(plot.top() - 6.0, plot.bottom() + 6.0)))
                .collect()
        };
        let zero_y = map.y(0.0);
        let clip = painter.with_clip_rect(plot.expand2(vec2(2.0, 6.0)));

        // Individual band shapes (subtle), selected band highlighted.
        if self.st.settings.band_curves || self.sel_band.is_some() {
            for (i, b) in bands.iter().enumerate() {
                if !b.enabled || (!self.st.settings.band_curves && self.sel_band != Some(i)) {
                    continue;
                }
                let Some(resp_b) = cv.bands.get(i) else { continue };
                let pts = to_pts(resp_b);
                let c = band_color(i);
                let sel = self.sel_band == Some(i);
                fill_to(
                    &clip,
                    &pts,
                    zero_y,
                    c.gamma_multiply(if sel { 0.22 } else { 0.07 }),
                    c.gamma_multiply(0.0),
                );
                clip.add(egui::Shape::line(
                    pts,
                    Stroke::new(1.0, c.gamma_multiply(if sel { 0.9 } else { 0.35 })),
                ));
            }
        }

        if has_corr {
            clip.add(egui::Shape::line(to_pts(corr), Stroke::new(1.6, VIOLET.gamma_multiply(0.85))));
            if !r.profile.is_empty() {
                clip.add(egui::Shape::line(
                    to_pts(prof),
                    Stroke::new(1.2, Color32::from_rgba_unmultiplied(235, 235, 240, 90)),
                ));
            }
        }
        let tp = to_pts(total);
        fill_to(&clip, &tp, zero_y, accent.gamma_multiply(0.22), accent.gamma_multiply(0.0));
        let dim = if self.st.bypass { 0.35 } else { 1.0 };
        clip.add(egui::Shape::line(tp.clone(), Stroke::new(6.0, accent.gamma_multiply(0.12 * dim))));
        clip.add(egui::Shape::line(tp, Stroke::new(2.4, accent.gamma_multiply(dim))));

        let node_pos: Vec<Pos2> = bands
            .iter()
            .map(|b| {
                let y = if b.kind.has_gain() {
                    b.gain
                } else {
                    Biquad::design(b).db(b.freq as f64) as f32
                };
                pos2(map.x(b.freq), map.y(y).clamp(plot.top(), plot.bottom()))
            })
            .collect();
        let pointer = resp.hover_pos();
        let nearest = |p: Pos2| -> Option<usize> {
            node_pos
                .iter()
                .enumerate()
                .map(|(i, n)| (i, n.distance(p)))
                .filter(|(_, d)| *d < 14.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let hovered = pointer.and_then(nearest);

        if resp.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            self.drag_band = nearest(origin);
            if self.drag_band.is_some() {
                self.sel_band = self.drag_band;
            }
        }
        if resp.dragged()
            && let (Some(i), Some(pid)) = (self.drag_band, pid)
        {
            let d = resp.drag_delta();
            let k = if ui.input(|i| i.modifiers.shift) { 0.2 } else { 1.0 };
            if let Some(b) = self.st.profile_mut(pid).and_then(|p| p.bands.get_mut(i)) {
                b.freq = map.f(map.x(b.freq) + d.x * k);
                if b.kind.has_gain() {
                    b.gain = map.db(map.y(b.gain) + d.y * k);
                    b.gain = (b.gain * 10.0).round() / 10.0;
                }
                b.freq = round_freq(b.freq);
                b.clamp();
            }
        }
        if resp.drag_stopped() {
            self.drag_band = None;
        }
        if resp.clicked() {
            self.sel_band = hovered;
        }
        if resp.double_clicked()
            && hovered.is_none()
            && let Some(p) = pointer
            && plot.contains(p)
        {
            let pid = self.st.ensure_profile(&self.target);
            let mut b = Band::new(FilterKind::Peak, round_freq(map.f(p.x)), (map.db(p.y) * 2.0).round() / 2.0, 1.41);
            b.clamp();
            if let Some(prof) = self.st.profile_mut(pid) {
                prof.bands.push(b);
                self.sel_band = Some(prof.bands.len() - 1);
            }
            self.st.settings.advanced = true;
        }
        if resp.secondary_clicked() {
            self.ctx_band = hovered;
            if hovered.is_some() {
                self.sel_band = hovered;
            }
        }
        // Scroll: Q of the hovered (or selected) band.
        if resp.hovered()
            && let Some(pid) = pid
            && let Some(i) = hovered.or(self.sel_band)
        {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            if s != 0.0
                && let Some(b) = self.st.profile_mut(pid).and_then(|p| p.bands.get_mut(i))
            {
                b.q = ((b.q * (s * 0.0025).exp()) * 100.0).round() / 100.0;
                b.clamp();
            }
        }
        // Arrow keys nudge the selected band.
        if let (Some(i), Some(pid)) = (self.sel_band, pid)
            && !ui.ctx().egui_wants_keyboard_input()
            && self.page == super::Page::Eq
        {
            let (l, r_, u, d, fine) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.modifiers.shift,
                )
            });
            if (l || r_ || u || d)
                && let Some(b) = self.st.profile_mut(pid).and_then(|p| p.bands.get_mut(i))
            {
                let oct = if fine { 1.0 / 48.0 } else { 1.0 / 12.0 };
                let gstep = if fine { 0.1 } else { 0.5 };
                if l {
                    b.freq /= 2f32.powf(oct);
                }
                if r_ {
                    b.freq *= 2f32.powf(oct);
                }
                if u {
                    b.gain += gstep;
                }
                if d {
                    b.gain -= gstep;
                }
                b.freq = round_freq(b.freq);
                b.gain = (b.gain * 10.0).round() / 10.0;
                b.clamp();
            }
        }
        let ctx_band = self.ctx_band;
        resp.context_menu(|ui| self.graph_menu(ui, ctx_band, pointer.map(|p| (map.f(p.x), map.db(p.y)))));

        // Draw nodes on top (re-read in case the interaction moved them).
        let bands_now: Vec<Band> = pid.and_then(|id| self.st.profile(id)).map(|p| p.bands.clone()).unwrap_or_default();
        for (i, b) in bands_now.iter().enumerate() {
            let y = if b.kind.has_gain() {
                b.gain
            } else {
                Biquad::design(b).db(b.freq as f64) as f32
            };
            let p = pos2(map.x(b.freq), map.y(y).clamp(plot.top(), plot.bottom()));
            let c = band_color(i);
            let sel = self.sel_band == Some(i);
            let hot = hovered == Some(i) || self.drag_band == Some(i);
            if sel {
                painter.circle_stroke(p, 13.0, Stroke::new(1.5, c.gamma_multiply(0.7)));
            }
            let rad = if hot { 9.5 } else { 8.5 };
            if b.enabled {
                painter.circle(p, rad, c, Stroke::new(2.0, Color32::BLACK));
            } else {
                painter.circle(p, rad, Color32::BLACK, Stroke::new(1.5, c.gamma_multiply(0.6)));
            }
            painter.text(
                p,
                Align2::CENTER_CENTER,
                format!("{}", i + 1),
                FontId::new(10.0, semibold()),
                if b.enabled { Color32::BLACK } else { c },
            );
        }

        if let Some(i) = hovered.or(self.drag_band)
            && let Some(b) = bands_now.get(i)
        {
            let y = if b.kind.has_gain() { b.gain } else { 0.0 };
            let anchor = pos2(map.x(b.freq), map.y(y).clamp(plot.top(), plot.bottom()));
            let text = if b.kind.has_gain() {
                format!("{}  ·  {}\n{}  ·  Q {:.2}", b.kind.label(), fmt_freq(b.freq), fmt_db(b.gain), b.q)
            } else {
                format!("{}  ·  {}\nQ {:.2}", b.kind.label(), fmt_freq(b.freq), b.q)
            };
            tooltip(&painter, anchor, &text, outer);
        } else if let Some(p) = pointer
            && plot.contains(p)
            && self.drag_band.is_none()
        {
            painter.line_segment(
                [pos2(p.x, plot.top()), pos2(p.x, plot.bottom())],
                Stroke::new(1.0, Color32::from_rgb(40, 40, 46)),
            );
            let f = map.f(p.x);
            let all = r.all();
            let v = dsp::response(&all, &r.graphic, &[f])[0];
            painter.circle_filled(pos2(p.x, map.y(v)), 3.5, accent);
            painter.text(
                pos2(outer.right() - 18.0, outer.top() + 20.0),
                Align2::RIGHT_CENTER,
                format!("{}   {}", fmt_freq(f), fmt_db(v)),
                FontId::monospace(12.0),
                TEXT,
            );
        } else {
            let label = if self.st.bypass {
                "bypassed".to_string()
            } else {
                format!("preamp {}", fmt_db(r.preamp))
            };
            painter.text(
                pos2(outer.right() - 18.0, outer.top() + 20.0),
                Align2::RIGHT_CENTER,
                label,
                FontId::monospace(12.0),
                DIM,
            );
        }

        let mut x = outer.left() + 18.0;
        let ly = outer.top() + 20.0;
        let mut legend = |c: Color32, t: &str| {
            painter.circle_filled(pos2(x + 4.0, ly), 4.0, c);
            let g = painter.text(pos2(x + 13.0, ly), Align2::LEFT_CENTER, t, FontId::proportional(11.5), DIM);
            x = g.right() + 16.0;
        };
        legend(accent, "Response");
        if has_corr {
            legend(VIOLET, "Headphone correction");
            if !r.profile.is_empty() {
                legend(Color32::from_gray(200), "Your sound");
            }
        }
        if !self.spectrum.is_empty() {
            legend(Color32::from_gray(110), "Playing now");
        }

        if bands_now.is_empty() && r.profile.is_empty() && !has_corr {
            painter.text(
                pos2(plot.center().x, plot.center().y - 28.0),
                Align2::CENTER_CENTER,
                "Flat. Double-click to add a band, or pick a preset below",
                FontId::proportional(13.0),
                DIM,
            );
        }
        if resp.hovered() && hovered.is_some() {
            ui.ctx().set_cursor_icon(if self.drag_band.is_some() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
    }

    fn graph_menu(&mut self, ui: &mut Ui, band: Option<usize>, at: Option<(f32, f32)>) {
        ui.set_min_width(170.0);
        let pid = self.st.chain(&self.target).profile.filter(|id| self.st.profile(*id).is_some());
        match (band, pid) {
            (Some(i), Some(pid)) => {
                ui.label(RichText::new(format!("Band {}", i + 1)).color(DIM).size(11.5));
                for k in FilterKind::ALL {
                    let cur = self.st.profile(pid).and_then(|p| p.bands.get(i)).map(|b| b.kind);
                    if ui.selectable_label(cur == Some(k), k.label()).clicked() {
                        if let Some(b) = self.st.profile_mut(pid).and_then(|p| p.bands.get_mut(i)) {
                            b.kind = k;
                            if !k.has_gain() {
                                b.q = b.q.min(10.0);
                            }
                        }
                        ui.close();
                    }
                }
                ui.separator();
                let enabled = self.st.profile(pid).and_then(|p| p.bands.get(i)).is_some_and(|b| b.enabled);
                if ui.button(if enabled { "Disable" } else { "Enable" }).clicked() {
                    if let Some(b) = self.st.profile_mut(pid).and_then(|p| p.bands.get_mut(i)) {
                        b.enabled = !b.enabled;
                    }
                    ui.close();
                }
                if ui.button("Reset gain").clicked() {
                    if let Some(b) = self.st.profile_mut(pid).and_then(|p| p.bands.get_mut(i)) {
                        b.gain = 0.0;
                    }
                    ui.close();
                }
                if ui.button(RichText::new("Delete").color(BAD)).clicked() {
                    self.sel_band = Some(i);
                    self.delete_selected_band();
                    ui.close();
                }
            }
            _ => {
                if let Some((f, g)) = at {
                    for (k, label) in [
                        (FilterKind::Peak, "Add peak here"),
                        (FilterKind::LowShelf, "Add low shelf here"),
                        (FilterKind::HighShelf, "Add high shelf here"),
                    ] {
                        if ui.button(label).clicked() {
                            let pid = self.st.ensure_profile(&self.target);
                            let mut b = Band::new(
                                k,
                                round_freq(f),
                                (g * 2.0).round() / 2.0,
                                if k == FilterKind::Peak { 1.41 } else { 0.71 },
                            );
                            b.clamp();
                            if let Some(p) = self.st.profile_mut(pid) {
                                p.bands.push(b);
                                self.sel_band = Some(p.bands.len() - 1);
                            }
                            self.st.settings.advanced = true;
                            ui.close();
                        }
                    }
                    if ui.button("Add high pass here").clicked() {
                        let pid = self.st.ensure_profile(&self.target);
                        if let Some(p) = self.st.profile_mut(pid) {
                            p.bands.push(Band::new(FilterKind::HighPass, round_freq(f), 0.0, 0.707));
                            self.sel_band = Some(p.bands.len() - 1);
                        }
                        self.st.settings.advanced = true;
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Flatten profile").clicked() {
                    if let Some(p) = pid.and_then(|id| self.st.profile_mut(id)) {
                        p.bands.clear();
                        p.tone = Default::default();
                    }
                    self.sel_band = None;
                    ui.close();
                }
            }
        }
    }
}

/// Snap to sensible precision: 1 Hz below 1 kHz, 10 Hz above.
fn round_freq(f: f32) -> f32 {
    if f >= 1000.0 {
        (f / 10.0).round() * 10.0
    } else if f >= 100.0 {
        f.round()
    } else {
        (f * 10.0).round() / 10.0
    }
}

fn tooltip(p: &Painter, anchor: Pos2, text: &str, bounds: Rect) {
    let galley = p.layout(text.to_string(), FontId::proportional(12.0), TEXT, 260.0);
    let size = galley.size() + vec2(20.0, 14.0);
    let mut r = Rect::from_min_size(anchor + vec2(16.0, -size.y - 10.0), size);
    if r.right() > bounds.right() - 8.0 {
        r = r.translate(vec2(-size.x - 32.0, 0.0));
    }
    if r.top() < bounds.top() + 8.0 {
        r = r.translate(vec2(0.0, size.y + 20.0));
    }
    p.rect(r, 8.0, Color32::from_rgb(18, 18, 21), Stroke::new(1.0, FAINT), StrokeKind::Inside);
    p.galley(r.min + vec2(10.0, 7.0), galley, TEXT);
}
