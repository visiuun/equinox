//! The equinox window. State lives in `state.json`; this UI edits it, pushes
//! it to Equalizer APO as you drag, and picks up changes made by `eqx` or the
//! tray agent.

mod automation;
mod devices_ui;
mod eq;
mod graph;
mod settings;
pub mod theme;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText, Sense, Stroke, pos2, vec2};
use equinox::analyzer::Analyzer;
use equinox::apo::{self, Apo};
use equinox::autoeq::{self, Entry};
use equinox::devices::{self, AudioDevice};
use equinox::engine;
use equinox::model::{Correction, Mode, State, Target};
use equinox::sys;

use theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Eq,
    Automation,
    Settings,
}

/// AutoEq browser state; the index loads in the background at startup.
#[derive(Default)]
pub struct AutoEqUi {
    pub open: bool,
    pub query: String,
    pub index: Option<Arc<Vec<Entry>>>,
    pub index_rx: Option<Receiver<Result<Vec<Entry>, String>>>,
    pub fetch_rx: Option<Receiver<(Target, Result<Correction, String>)>>,
    pub fetching: Option<String>,
    pub error: Option<String>,
    pub focus: bool,
    /// Device id -> recognised headphone model.
    pub suggestions: HashMap<String, Entry>,
}

pub struct App {
    pub st: State,
    applied: State,
    committed: State,
    undo: Vec<State>,
    redo: Vec<State>,
    last_change: Instant,
    last_apply: Instant,
    save_due: Option<Instant>,
    file_mtime: Option<SystemTime>,
    last_poll: Instant,
    last_devices: Instant,
    pub devices: Vec<AudioDevice>,
    pub apo: Option<Apo>,
    pub hooked: bool,
    pub foreign: Vec<String>,
    pub rule_ctx: engine::Context,
    pub active_rules: Vec<u64>,
    pub target: Target,
    pub page: Page,
    pub sel_band: Option<usize>,
    pub drag_band: Option<usize>,
    pub ctx_band: Option<usize>,
    pub toast: Option<(String, Instant, bool)>,
    pub aeq: AutoEqUi,
    analyzer: Option<Analyzer>,
    pub spectrum: Vec<f32>,
    spec_time: Instant,
    pub freqs: Vec<f32>,
    pub rename: Option<String>,
    pub autostart: bool,
    pub agent: bool,
    pub accent: Color32,
    pub confirm_reset: bool,
    pub procs: Vec<String>,
    /// Graph dB range, frozen while dragging.
    pub span: f32,
    pub curve_cache: Option<Arc<graph::Curves>>,
    pub picker: devices_ui::Picker,
}

pub fn run(page: Option<&str>) {
    let Some(_guard) = sys::single_instance(sys::APP_MUTEX) else {
        sys::focus_app_window();
        return;
    };
    let icon = egui::IconData {
        rgba: equinox::icon::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(sys::WINDOW_TITLE)
            .with_icon(icon)
            .with_inner_size([1240.0, 820.0])
            .with_min_inner_size([980.0, 680.0])
            .with_decorations(false)
            .with_transparent(false),
        ..Default::default()
    };
    let open_devices = page == Some("devices");
    let page = match page {
        Some("automation") => Page::Automation,
        Some("settings") => Page::Settings,
        _ => Page::Eq,
    };
    let _ = eframe::run_native(
        sys::WINDOW_TITLE,
        opts,
        Box::new(move |cc| {
            let mut app = App::new(&cc.egui_ctx);
            app.page = page;
            if open_devices {
                app.open_picker(None);
            }
            Ok(Box::new(app))
        }),
    );
}

impl App {
    fn new(ctx: &egui::Context) -> App {
        let st = State::load();
        let first_run = !st.settings.setup_done;
        let accent = Color32::from_rgb(st.settings.accent[0], st.settings.accent[1], st.settings.accent[2]);
        theme::install(ctx);
        theme::style(ctx, accent);
        let devices = devices::list();
        let target = initial_target(&st, &devices);
        let mut app = App {
            applied: st.clone(),
            committed: st.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_change: Instant::now(),
            last_apply: Instant::now(),
            save_due: None,
            file_mtime: mtime(),
            last_poll: Instant::now(),
            last_devices: Instant::now(),
            rule_ctx: engine::Context::capture(&st, &devices),
            active_rules: Vec::new(),
            devices,
            apo: None,
            hooked: false,
            foreign: Vec::new(),
            target,
            page: Page::Eq,
            sel_band: None,
            drag_band: None,
            ctx_band: None,
            toast: None,
            aeq: AutoEqUi::default(),
            analyzer: None,
            spectrum: Vec::new(),
            spec_time: Instant::now(),
            freqs: equinox::dsp::log_freqs(360, equinox::dsp::F_MIN, equinox::dsp::F_MAX),
            rename: None,
            autostart: sys::autostart_enabled(),
            agent: sys::agent_running(),
            accent,
            confirm_reset: false,
            procs: Vec::new(),
            span: 12.0,
            curve_cache: None,
            picker: Default::default(),
            st,
        };
        app.refresh_apo();
        app.push();
        app.load_index(ctx, false);
        if first_run {
            // Starting with Windows is the default; Settings turns it off.
            match sys::set_autostart(true) {
                Ok(()) => {
                    app.autostart = true;
                    app.notify("equinox will start with Windows (change in Settings)", false);
                }
                Err(e) => app.notify(format!("couldn't set up start with Windows: {e}"), true),
            }
            app.st.settings.setup_done = true;
            app.applied = app.st.clone();
            app.committed = app.st.clone();
            let _ = app.st.save();
            app.file_mtime = mtime();
        }
        if app.st.settings.agent {
            sys::start_agent();
        }
        app
    }

    pub fn refresh_apo(&mut self) {
        self.apo = apo::detect();
        self.hooked = self.apo.as_ref().is_some_and(|a| a.hooked());
        self.foreign = self.apo.as_ref().map(|a| a.foreign_lines()).unwrap_or_default();
    }

    /// Render and write the Equalizer APO config now.
    pub fn push(&mut self) {
        let rep = engine::apply(&self.st, &self.devices, &self.rule_ctx);
        self.active_rules = rep.active_rules;
        if let Some(e) = rep.error
            && self.apo.is_some()
        {
            self.notify(e, true);
        }
        self.applied = self.st.clone();
        self.last_apply = Instant::now();
    }

    pub fn notify(&mut self, msg: impl Into<String>, err: bool) {
        self.toast = Some((msg.into(), Instant::now(), err));
    }

    pub fn set_accent(&mut self, ctx: &egui::Context, rgb: [u8; 3]) {
        self.st.settings.accent = rgb;
        self.accent = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        theme::style(ctx, self.accent);
    }

    pub fn load_index(&mut self, ctx: &egui::Context, refresh: bool) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(autoeq::load_index(refresh).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.aeq.index_rx = Some(rx);
    }

    /// Download a correction off-thread and apply it to `target` when it lands.
    pub fn fetch_correction(&mut self, ctx: &egui::Context, entry: Entry, target: Target) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.aeq.fetching = Some(entry.name.clone());
        self.aeq.error = None;
        std::thread::spawn(move || {
            let _ = tx.send((target, autoeq::fetch(&entry).map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
        self.aeq.fetch_rx = Some(rx);
    }

    fn compute_suggestions(&mut self) {
        let Some(idx) = &self.aeq.index else { return };
        self.aeq.suggestions = self
            .devices
            .iter()
            .filter(|d| d.form.is_headphone() || d.form == devices::Form::Other)
            .filter_map(|d| autoeq::suggest(idx, &d.label()).map(|e| (d.id.clone(), e.clone())))
            .collect();
    }

    fn poll_jobs(&mut self) {
        if let Some(rx) = &self.aeq.index_rx
            && let Ok(r) = rx.try_recv()
        {
            self.aeq.index_rx = None;
            match r {
                Ok(v) => {
                    self.aeq.index = Some(Arc::new(v));
                    self.compute_suggestions();
                }
                Err(e) => self.aeq.error = Some(e),
            }
        }
        if let Some(rx) = &self.aeq.fetch_rx
            && let Ok((target, r)) = rx.try_recv()
        {
            self.aeq.fetch_rx = None;
            self.aeq.fetching = None;
            match r {
                Ok(c) => {
                    let name = c.name.clone();
                    self.st.chain_mut(&target).correction = Some(c);
                    self.st.bypass = false;
                    self.aeq.open = false;
                    self.notify(format!("{name} correction applied"), false);
                }
                Err(e) => {
                    self.aeq.error = Some(e.clone());
                    self.notify(e, true);
                }
            }
        }
    }

    /// Debounced apply/save, undo checkpoints, external changes, device changes.
    fn tick(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let pointer_down = ctx.input(|i| i.pointer.any_down());

        if self.st != self.applied {
            self.last_change = now;
            self.save_due = Some(now + Duration::from_millis(400));
            if now.duration_since(self.last_apply) >= Duration::from_millis(45) {
                self.push();
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        if let Some(due) = self.save_due
            && now >= due
            && !pointer_down
        {
            self.save_due = None;
            if let Err(e) = self.st.save() {
                self.notify(format!("couldn't save settings: {e}"), true);
            }
            self.file_mtime = mtime();
        }
        // One undo step per settled edit (a whole drag is one step).
        if self.st != self.committed && !pointer_down && now.duration_since(self.last_change) > Duration::from_millis(350) {
            self.undo.push(std::mem::replace(&mut self.committed, self.st.clone()));
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }

        // Changes from eqx / the tray agent.
        if now.duration_since(self.last_poll) > Duration::from_millis(500) {
            self.last_poll = now;
            let m = mtime();
            if m != self.file_mtime && self.save_due.is_none() && !pointer_down {
                self.file_mtime = m;
                let fresh = State::load();
                if fresh != self.st {
                    self.st = fresh;
                    self.committed = self.st.clone();
                    self.applied = self.st.clone();
                    self.sel_band = None;
                    let accent = self.st.settings.accent;
                    self.set_accent(ctx, accent);
                }
            }
        }

        // Devices, rule context and agent status every 2 s.
        if now.duration_since(self.last_devices) > Duration::from_secs(2) {
            self.last_devices = now;
            let fresh = devices::list();
            let changed = fresh.len() != self.devices.len()
                || fresh
                    .iter()
                    .zip(&self.devices)
                    .any(|(a, b)| a.id != b.id || a.active != b.active || a.default != b.default || a.apo != b.apo);
            self.devices = fresh;
            if changed {
                self.compute_suggestions();
            }
            self.rule_ctx = engine::Context::capture(&self.st, &self.devices);
            self.refresh_apo();
            self.agent = sys::agent_running();
            // Writes only if the rendered text differs (rules, plugged devices).
            self.push();
        }

        if let Some((_, t, _)) = &self.toast
            && t.elapsed() > Duration::from_secs(4)
        {
            self.toast = None;
        }

        // Analyzer runs only while visible and enabled.
        let want = self.st.settings.analyzer && self.page == Page::Eq && ctx.input(|i| i.focused || i.viewport().minimized != Some(true));
        if want && self.analyzer.is_none() {
            self.analyzer = Some(Analyzer::start());
        } else if !want && self.analyzer.is_some() {
            self.analyzer = None;
            self.spectrum.clear();
        }
        if let Some(a) = &mut self.analyzer {
            let dt = self.spec_time.elapsed().as_secs_f32().min(0.1);
            self.spec_time = Instant::now();
            let sf = &graph::spectrum_freqs();
            match a.spectrum(sf) {
                Some(s) => {
                    if self.spectrum.len() != s.len() {
                        self.spectrum = vec![-120.0; s.len()];
                    }
                    for (o, n) in self.spectrum.iter_mut().zip(s) {
                        // Fast attack, slow release.
                        *o = if n > *o {
                            *o + (n - *o) * (1.0 - (-dt * 30.0).exp())
                        } else {
                            (*o - dt * 28.0).max(n)
                        };
                    }
                }
                None => {
                    for o in self.spectrum.iter_mut() {
                        *o = (*o - dt * 40.0).max(-120.0);
                    }
                }
            }
            // 30 fps while something is visible; idle polling once it has decayed to silence.
            let silent = self.spectrum.iter().all(|&v| v <= -100.0);
            ctx.request_repaint_after(Duration::from_millis(if silent { 250 } else { 33 }));
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    pub fn undo(&mut self) {
        if self.st != self.committed {
            self.undo.push(std::mem::replace(&mut self.committed, self.st.clone()));
        }
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.st, prev.clone()));
            self.committed = prev;
            self.sel_band = None;
            self.notify("Undone", false);
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.st, next.clone()));
            self.committed = next;
            self.sel_band = None;
            self.notify("Redone", false);
        }
    }

    pub fn device(&self, id: &str) -> Option<&AudioDevice> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn target_name(&self, t: &Target) -> String {
        match t {
            Target::Global if self.st.mode == Mode::Global => "All devices".into(),
            Target::Global => "Default".into(),
            Target::Device(id) => self
                .device(id)
                .map(|d| d.label())
                .or_else(|| self.st.device(id).map(|d| d.name.clone()))
                .unwrap_or_else(|| "Unknown device".into()),
        }
    }

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let bar = ui
            .available_rect_before_wrap()
            .with_max_y(ui.available_rect_before_wrap().top() + 52.0);
        // Drag area behind everything; buttons placed after take priority.
        let drag = ui.interact(bar, ui.scope_id().with("titlebar"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(18.0, 0.0))), |ui| {
            ui.horizontal_centered(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                graph::mini_mark(ui.painter(), r, self.accent);
                ui.label(RichText::new("equinox").size(17.0).family(semibold()).color(WHITE));
                ui.add_space(22.0);
                for (p, label) in [
                    (Page::Eq, "Equalizer"),
                    (Page::Automation, "Automation"),
                    (Page::Settings, "Settings"),
                ] {
                    let sel = self.page == p;
                    let t = RichText::new(label).size(13.5).color(if sel { WHITE } else { DIM });
                    let resp = ui.add(egui::Button::new(if sel { t.family(semibold()) } else { t }).frame(false));
                    if sel {
                        let r = resp.rect;
                        ui.painter().line_segment(
                            [pos2(r.left() + 4.0, r.bottom() + 6.0), pos2(r.right() - 4.0, r.bottom() + 6.0)],
                            Stroke::new(2.0, self.accent),
                        );
                    }
                    if resp.clicked() {
                        self.page = p;
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (glyph, action) in [("✕", 0), ("☐", 1), ("—", 2)] {
                        let b = egui::Button::new(RichText::new(glyph).size(13.0).color(DIM))
                            .frame(false)
                            .min_size(vec2(36.0, 30.0));
                        let r = ui.add(b);
                        if r.hovered() && action == 0 {
                            ui.painter()
                                .rect_filled(r.rect, 6.0, Color32::from_rgb(120, 24, 24).gamma_multiply(0.6));
                        }
                        if r.clicked() {
                            match action {
                                0 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                1 => {
                                    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                                }
                                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                            }
                        }
                    }
                    ui.add_space(14.0);
                    self.power_button(ui);
                });
            });
        });
        ui.advance_cursor_after_rect(bar);
    }

    /// The big on/bypass pill. Bypass is level-matched by default for honest A/B.
    fn power_button(&mut self, ui: &mut egui::Ui) {
        let on = !self.st.bypass;
        let text = if on { "EQ ON" } else { "BYPASSED" };
        let galley = ui
            .painter()
            .layout_no_wrap(text.into(), egui::FontId::new(12.5, semibold()), Color32::BLACK);
        let size = vec2(galley.size().x + 44.0, 30.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let t = ui.ctx().animate_bool_responsive(resp.id, on);
        let fill = lerp_color(Color32::from_rgb(24, 24, 27), self.accent, t);
        let p = ui.painter();
        if on {
            p.rect_filled(rect.expand(3.0), 18.0, self.accent.gamma_multiply(0.12));
        }
        p.rect_filled(rect, 15.0, fill);
        p.circle_filled(
            pos2(rect.left() + 16.0, rect.center().y),
            4.0,
            if on { Color32::BLACK } else { DIM },
        );
        p.text(
            pos2(rect.left() + 28.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::new(12.5, semibold()),
            if on { Color32::BLACK } else { TEXT },
        );
        let resp = resp.on_hover_text("Bypass the EQ (Ctrl+B). Level-matched, so the comparison is fair.");
        if resp.clicked() {
            self.st.bypass = !self.st.bypass;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.add_space(14.0);
            let (dot, text) = match (&self.apo, self.hooked) {
                (None, _) => (BAD, "Equalizer APO not installed".to_string()),
                (Some(a), false) => (WARN, format!("Equalizer APO {} · not loading equinox", a.version)),
                (Some(a), true) => {
                    let n = self.devices.iter().filter(|d| d.active && d.apo).count();
                    (
                        if n > 0 { GOOD } else { WARN },
                        format!("Equalizer APO {} · {n} device{} active", a.version, if n == 1 { "" } else { "s" }),
                    )
                }
            };
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, dot);
            ui.label(RichText::new(text).size(11.5).color(DIM));
            if !self.active_rules.is_empty() {
                let names: Vec<String> = self
                    .st
                    .rules
                    .iter()
                    .filter(|r| self.active_rules.contains(&r.id))
                    .map(|r| r.name.clone())
                    .collect();
                ui.label(RichText::new(format!("·  ⚡ {}", names.join(", "))).size(11.5).color(self.accent));
            }
            if let Some((msg, _, err)) = &self.toast {
                ui.add_space(12.0);
                ui.label(RichText::new(msg).size(11.5).color(if *err { BAD } else { TEXT }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new("Ctrl+Z undo  ·  Ctrl+B bypass  ·  eqx --help")
                        .size(11.0)
                        .color(FAINT),
                );
            });
        });
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (undo, redo, bypass, del) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                cmd && i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::Delete),
            )
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if bypass {
            self.st.bypass = !self.st.bypass;
        }
        if del && self.page == Page::Eq {
            self.delete_selected_band();
        }
    }

    /// Edge strips for resizing the borderless window.
    fn resize_edges(&self, ui: &mut egui::Ui) {
        use egui::{CursorIcon as C, ResizeDirection as D};
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let r = ctx.content_rect();
        let e = 5.0;
        let zones = [
            (
                egui::Rect::from_min_max(r.left_top(), pos2(r.left() + e * 2.0, r.top() + e * 2.0)),
                D::NorthWest,
                C::ResizeNorthWest,
            ),
            (
                egui::Rect::from_min_max(pos2(r.right() - e * 2.0, r.top()), pos2(r.right(), r.top() + e * 2.0)),
                D::NorthEast,
                C::ResizeNorthEast,
            ),
            (
                egui::Rect::from_min_max(pos2(r.left(), r.bottom() - e * 2.0), pos2(r.left() + e * 2.0, r.bottom())),
                D::SouthWest,
                C::ResizeSouthWest,
            ),
            (
                egui::Rect::from_min_max(pos2(r.right() - e * 2.0, r.bottom() - e * 2.0), r.right_bottom()),
                D::SouthEast,
                C::ResizeSouthEast,
            ),
            (
                egui::Rect::from_min_max(r.left_top(), pos2(r.left() + e, r.bottom())),
                D::West,
                C::ResizeWest,
            ),
            (
                egui::Rect::from_min_max(pos2(r.right() - e, r.top()), r.right_bottom()),
                D::East,
                C::ResizeEast,
            ),
            (
                egui::Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + e)),
                D::North,
                C::ResizeNorth,
            ),
            (
                egui::Rect::from_min_max(pos2(r.left(), r.bottom() - e), r.right_bottom()),
                D::South,
                C::ResizeSouth,
            ),
        ];
        for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
            let resp = ui.interact(rect, egui::Id::unique(("resize", i)), Sense::drag());
            if resp.hovered() || resp.dragged() {
                ctx.set_cursor_icon(cursor);
            }
            if resp.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_jobs();
        self.poll_picker();
        self.tick(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("title")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(BG))
            .show_separator_line(false)
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::bottom("status")
            .exact_size(30.0)
            .frame(egui::Frame::new().fill(BG).stroke(Stroke::new(1.0, LINE)))
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new()
                .fill(BG)
                .inner_margin(egui::Margin {
                    left: 18,
                    right: 18,
                    top: 4,
                    bottom: 14,
                })
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    match self.page {
                        Page::Eq => self.eq_page(ui),
                        Page::Automation => self.automation_page(ui),
                        Page::Settings => self.settings_page(ui),
                    }
                });
        });
        self.autoeq_modal(&ctx);
        self.picker_modal(&ctx);
        self.resize_edges(ui);
        ui.painter()
            .rect_stroke(ctx.content_rect(), 0.0, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.st != self.applied {
            self.push();
        }
        let _ = self.st.save();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Lay out `body` in a column of at most `width`, centred horizontally.
pub fn centered(ui: &mut egui::Ui, width: f32, body: impl FnOnce(&mut egui::Ui)) {
    let w = width.min(ui.available_width());
    let pad = ((ui.available_width() - w) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(w);
            body(ui);
        });
    });
}

fn mtime() -> Option<SystemTime> {
    std::fs::metadata(equinox::paths::state_file()).and_then(|m| m.modified()).ok()
}

/// Open on the device that's playing when in per-device mode.
fn initial_target(st: &State, devices: &[AudioDevice]) -> Target {
    match (st.mode, devices.iter().find(|d| d.default)) {
        (Mode::PerDevice, Some(d)) => Target::Device(d.id.clone()),
        _ => Target::Global,
    }
}
