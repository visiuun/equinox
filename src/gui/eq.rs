//! Equalizer page: device targets, smart banners, graph, simple/advanced
//! controls, headphone correction and output cards, AutoEq browser.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Ui, vec2};
use equinox::dsp::fmt_db;
use equinox::engine;
use equinox::eqtext;
use equinox::model::{Action, Band, Correction, FilterKind, Mode, Profile, Target, Tone};

use super::App;
use super::theme::*;

enum Banner {
    GetApo,
    /// Not hooked and not installed on the output: both steps in one callout.
    Setup(String, String),
    Hook,
    InstallOn(String, String),
    Rule(u64),
    Following(String),
    Suggest(String, equinox::autoeq::Entry),
}

impl App {
    pub fn eq_page(&mut self, ui: &mut Ui) {
        // Keep the selected target meaningful for the current mode.
        if self.st.mode == Mode::Global {
            self.target = Target::Global;
        }
        self.target_row(ui);
        ui.add_space(8.0);
        self.banners(ui);

        let avail = ui.available_height();
        let graph_h = (avail - 318.0).clamp(220.0, 560.0);
        self.graph(ui, graph_h);
        ui.add_space(12.0);

        let right_w = 344.0;
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            let left_w = ui.available_width() - right_w - 12.0;
            ui.allocate_ui_with_layout(vec2(left_w, h), Layout::top_down(Align::Min), |ui| {
                card().show(ui, |ui| {
                    ui.set_min_size(vec2(ui.available_width(), h - 34.0));
                    self.controls(ui);
                });
            });
            ui.allocate_ui_with_layout(vec2(right_w, h), Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rightcol")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.headphone_card(ui);
                        ui.add_space(8.0);
                        self.output_card(ui);
                    });
            });
        });
    }

    fn target_row(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut mode = self.st.mode;
            if segmented(
                ui,
                &mut mode,
                &[(Mode::Global, "One sound"), (Mode::PerDevice, "Per device")],
                self.accent,
            ) {
                self.st.mode = mode;
                self.target = match (mode, self.devices.iter().find(|d| d.default)) {
                    (Mode::PerDevice, Some(d)) => Target::Device(d.id.clone()),
                    _ => Target::Global,
                };
                self.sel_band = None;
            }
            ui.add_space(8.0);
            if self.st.mode == Mode::Global {
                let out = self
                    .devices
                    .iter()
                    .find(|d| d.default)
                    .map(|d| format!("{}  {}", d.form.glyph(), d.label()))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!("Applies to every output  ·  now playing on {out}"))
                        .color(DIM)
                        .size(12.5),
                );
                return;
            }
            egui::ScrollArea::horizontal()
                .id_salt("targets")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if chip(ui, self.target == Target::Global, self.accent, "Default", None)
                            .on_hover_text("The sound for every device that doesn't have its own")
                            .clicked()
                        {
                            self.target = Target::Global;
                            self.sel_band = None;
                        }
                        let shown: Vec<_> = self
                            .devices
                            .iter()
                            .filter(|d| {
                                d.active || self.st.owns_chain(&Target::Device(d.id.clone())) || self.target == Target::Device(d.id.clone())
                            })
                            .cloned()
                            .collect();
                        for d in &shown {
                            let t = Target::Device(d.id.clone());
                            let own = self.st.owns_chain(&t);
                            let dot = if !d.apo {
                                WARN
                            } else if own {
                                self.accent
                            } else {
                                DIM
                            };
                            let label = format!("{}{}", if d.default { "▸ " } else { "" }, d.short());
                            let mut tip = format!("{}\n{}", d.label(), if own { "Own sound" } else { "Follows Default" });
                            if !d.apo {
                                tip += "\nEqualizer APO not installed on this device";
                            }
                            if !d.active {
                                tip += "\nUnplugged";
                            }
                            let r = chip(ui, self.target == t, self.accent, &label, Some(dot)).on_hover_text(tip);
                            if r.clicked() {
                                self.target = t.clone();
                                self.sel_band = None;
                            }
                            r.context_menu(|ui| {
                                if own {
                                    if ui.button("Follow Default again").clicked() {
                                        self.st.device_mut(&d.id, &d.label()).follow_global = true;
                                        ui.close();
                                    }
                                } else if ui.button("Give it its own sound").clicked() {
                                    self.st.detach_device(&d.id, &d.label());
                                    self.target = t.clone();
                                    ui.close();
                                }
                            });
                        }
                        let others: Vec<_> = self
                            .devices
                            .iter()
                            .filter(|d| !shown.iter().any(|s| s.id == d.id))
                            .cloned()
                            .collect();
                        if !others.is_empty() {
                            egui::ComboBox::from_id_salt("more-devices")
                                .selected_text(RichText::new(format!("+{} unplugged", others.len())).color(DIM))
                                .width(130.0)
                                .show_ui(ui, |ui| {
                                    for d in others {
                                        if ui.selectable_label(false, d.label()).clicked() {
                                            self.target = Target::Device(d.id.clone());
                                        }
                                    }
                                });
                        }
                    });
                });
        });
    }

    fn banners(&mut self, ui: &mut Ui) {
        let mut list = Vec::new();
        let focus_dev = match &self.target {
            Target::Device(id) => self.device(id).cloned(),
            Target::Global => self.devices.iter().find(|d| d.default).cloned(),
        };
        let missing_on = focus_dev.as_ref().filter(|d| !d.apo && d.active).map(|d| (d.id.clone(), d.label()));
        match (&self.apo, self.hooked, missing_on) {
            (None, _, _) => list.push(Banner::GetApo),
            (Some(_), false, Some((id, dev))) => list.push(Banner::Setup(id, dev)),
            (Some(_), false, None) => list.push(Banner::Hook),
            (Some(_), true, Some((id, dev))) => list.push(Banner::InstallOn(id, dev)),
            _ => {}
        }
        // Active rules that touch what's shown.
        for r in self.st.rules.iter().filter(|r| self.active_rules.contains(&r.id)) {
            let touches = match &r.action {
                Action::UseProfile { target, .. } | Action::Strength { target, .. } => {
                    *target == self.target || (*target == Target::Global && !self.st.owns_chain(&self.target))
                }
                Action::Bypass { .. } => true,
            };
            if touches {
                list.push(Banner::Rule(r.id));
            }
        }
        if let Target::Device(id) = &self.target
            && !self.st.owns_chain(&self.target)
        {
            list.push(Banner::Following(id.clone()));
        }
        if let Some(d) = &focus_dev
            && let Some(e) = self.aeq.suggestions.get(&d.id)
        {
            let t = Target::Device(d.id.clone());
            let has = self.st.chain(&t).correction.is_some();
            let dismissed = self.st.device(&d.id).and_then(|x| x.dismissed_suggestion.clone()) == Some(e.key());
            if !has && !dismissed {
                list.push(Banner::Suggest(d.id.clone(), e.clone()));
            }
        }

        for b in list {
            match b {
                Banner::GetApo => {
                    let msg = "equinox needs Equalizer APO (free, open source) to process system audio. One click downloads it, installs it and turns the EQ on.";
                    if banner(ui, BAD, msg, &["Install"]) == Some(0) {
                        let ctx = ui.ctx().clone();
                        self.install_everything(&ctx);
                    }
                }
                Banner::Setup(id, dev) => {
                    let msg =
                        format!("equinox isn't active on {dev} yet. One click sets it up (your old Equalizer APO config is backed up).");
                    match banner(ui, WARN, &msg, &["Set up", "Other devices…"]) {
                        Some(0) => {
                            self.hook_apo();
                            let ctx = ui.ctx().clone();
                            self.start_apo_setup(&ctx, vec![id], vec![]);
                        }
                        Some(_) => self.open_picker(Some(id)),
                        None => {}
                    }
                }
                Banner::Hook => {
                    let using = if self.foreign.is_empty() {
                        String::new()
                    } else {
                        format!(" It's currently using: {}.", self.foreign.join(", "))
                    };
                    let msg = format!(
                        "Equalizer APO isn't loading equinox yet.{using} Your current config is backed up and can be restored in Settings."
                    );
                    if banner(ui, WARN, &msg, &["Use equinox"]) == Some(0) {
                        self.hook_apo();
                    }
                }
                Banner::InstallOn(id, name) => {
                    let msg = format!("EQ isn't enabled on {name} yet, so you won't hear it there.");
                    match banner(ui, WARN, &msg, &["Enable EQ here", "Choose devices…"]) {
                        Some(0) => {
                            let ctx = ui.ctx().clone();
                            self.start_apo_setup(&ctx, vec![id], vec![]);
                        }
                        Some(_) => self.open_picker(Some(id)),
                        None => {}
                    }
                }
                Banner::Rule(id) => {
                    let Some(r) = self.st.rules.iter().find(|r| r.id == id) else {
                        continue;
                    };
                    let what = match &r.action {
                        Action::UseProfile { profile, .. } => {
                            format!("using {}", self.st.profile(*profile).map(|p| p.name.as_str()).unwrap_or("?"))
                        }
                        Action::Bypass { on: true } => "EQ bypassed".into(),
                        Action::Bypass { on: false } => "EQ forced on".into(),
                        Action::Strength { value, .. } => format!("correction at {:.0}%", value * 100.0),
                    };
                    let msg = format!(
                        "⚡ Rule “{}” is active: {what}. Your edits below change the saved setup underneath it.",
                        r.name
                    );
                    if banner(ui, self.accent, &msg, &["Turn off rule"]) == Some(0)
                        && let Some(r) = self.st.rules.iter_mut().find(|r| r.id == id)
                    {
                        r.enabled = false;
                    }
                }
                Banner::Following(id) => {
                    let name = self.target_name(&Target::Device(id.clone()));
                    let msg = format!("{name} follows the Default sound, so changes here edit Default.");
                    if banner(ui, DIM, &msg, &["Give it its own sound"]) == Some(0) {
                        self.st.detach_device(&id, &name);
                    }
                }
                Banner::Suggest(dev, e) => {
                    let name = self.target_name(&Target::Device(dev.clone()));
                    let msg = format!(
                        "🎧 {name} looks like the {}. Apply its measured correction ({}) for a neutral, accurate sound?",
                        e.name,
                        e.source_label()
                    );
                    match banner(ui, VIOLET, &msg, &["Apply", "Not now"]) {
                        Some(0) => {
                            // Headphone correction belongs to that device only.
                            if self.st.mode == Mode::Global {
                                self.notify(
                                    "Switched to per-device mode so the correction only applies to these headphones",
                                    false,
                                );
                            }
                            self.st.detach_device(&dev, &name);
                            let t = Target::Device(dev.clone());
                            self.target = t.clone();
                            let ctx = ui.ctx().clone();
                            self.fetch_correction(&ctx, e, t);
                        }
                        Some(_) => self.st.device_mut(&dev, &name).dismissed_suggestion = Some(e.key()),
                        None => {}
                    }
                }
            }
            ui.add_space(6.0);
        }
    }

    pub fn hook_apo(&mut self) {
        let Some(a) = self.apo.clone() else { return };
        match a.take_control() {
            Ok(()) => {
                self.refresh_apo();
                self.push();
                self.notify("Equalizer APO now runs equinox", false);
            }
            Err(e) => self.notify(format!("couldn't update config.txt: {e}"), true),
        }
    }

    fn controls(&mut self, ui: &mut Ui) {
        let pid = self.st.chain(&self.target).profile.filter(|id| self.st.profile(*id).is_some());
        ui.horizontal(|ui| {
            self.profile_bar(ui, pid);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut adv = self.st.settings.advanced;
                if segmented(ui, &mut adv, &[(false, "Simple"), (true, "Advanced")], self.accent) {
                    self.st.settings.advanced = adv;
                }
            });
        });
        ui.add_space(10.0);
        if self.st.settings.advanced {
            self.bands_table(ui);
        } else {
            self.simple(ui);
        }
    }

    fn profile_bar(&mut self, ui: &mut Ui, pid: Option<u64>) {
        let name = pid
            .and_then(|id| self.st.profile(id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Flat".into());
        if let Some(buf) = &mut self.rename {
            let r = ui.add(
                egui::TextEdit::singleline(buf)
                    .desired_width(190.0)
                    .font(egui::FontId::new(14.0, semibold())),
            );
            r.request_focus();
            let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
            if enter || r.lost_focus() {
                let new = buf.trim().to_string();
                if !new.is_empty()
                    && let Some(p) = pid.and_then(|id| self.st.profile_mut(id))
                {
                    p.name = new;
                }
                self.rename = None;
            } else if esc {
                self.rename = None;
            }
            return;
        }
        egui::ComboBox::from_id_salt("profile")
            .selected_text(RichText::new(&name).size(14.0).family(semibold()).color(WHITE))
            .width(190.0)
            .height(420.0)
            .show_ui(ui, |ui| {
                let list: Vec<(u64, String)> = self.st.profiles.iter().map(|p| (p.id, p.name.clone())).collect();
                for (id, n) in list {
                    if ui.selectable_label(pid == Some(id), n).clicked() {
                        self.st.chain_mut(&self.target).profile = Some(id);
                        self.sel_band = None;
                    }
                }
            });
        let icon = |ui: &mut Ui, glyph: &str, tip: &str| flat(ui, RichText::new(glyph).size(15.0).color(DIM)).on_hover_text(tip).clicked();
        let text_btn =
            |ui: &mut Ui, label: &str, tip: &str| flat(ui, RichText::new(label).size(12.5).color(DIM)).on_hover_text(tip).clicked();
        if icon(ui, "+", "New flat profile") {
            let id = self.st.new_id();
            let n = self.st.unique_name("My sound");
            self.st.profiles.push(Profile {
                id,
                name: n.clone(),
                tone: Tone::default(),
                bands: vec![],
            });
            self.st.chain_mut(&self.target).profile = Some(id);
            self.rename = Some(n);
        }
        if icon(ui, "⧉", "Duplicate") {
            let src = pid.and_then(|id| self.st.profile(id)).cloned().unwrap_or(Profile {
                id: 0,
                name: "Flat".into(),
                tone: Tone::default(),
                bands: vec![],
            });
            let id = self.st.new_id();
            let n = self.st.unique_name(&format!("{} copy", src.name));
            self.st.profiles.push(Profile { id, name: n, ..src });
            self.st.chain_mut(&self.target).profile = Some(id);
        }
        if pid.is_some() && icon(ui, "✎", "Rename") {
            self.rename = Some(name.clone());
        }
        if let Some(id) = pid
            && self.st.profiles.len() > 1
            && icon(ui, "🗑", "Delete profile (Ctrl+Z brings it back)")
        {
            self.st.delete_profile(id);
            let first = self.st.profiles.first().map(|p| p.id);
            self.st.chain_mut(&self.target).profile = first;
            self.notify(format!("Deleted {name}"), false);
        }
        if text_btn(ui, "Import", "Import an EQ file (Equalizer APO, AutoEq, REW, CSV) as a profile") {
            self.import(false);
        }
        if text_btn(ui, "Export", "Export the full chain as ParametricEQ.txt for other apps") {
            self.export();
        }
    }

    fn simple(&mut self, ui: &mut Ui) {
        let pid = self.st.chain(&self.target).profile.filter(|id| self.st.profile(*id).is_some());
        let mut tone = pid.and_then(|id| self.st.profile(id)).map(|p| p.tone).unwrap_or_default();
        let before = tone;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for i in 0..4 {
                fader(ui, tone.get_mut(i), 12.0, Tone::NAMES[i], Tone::HINTS[i], self.accent)
                    .on_hover_text("Drag or scroll · Shift for fine · double-click to reset");
            }
            ui.add_space(14.0);
            let sep = ui.available_rect_before_wrap();
            ui.painter().line_segment(
                [egui::pos2(sep.left(), sep.top() + 6.0), egui::pos2(sep.left(), sep.top() + 180.0)],
                Stroke::new(1.0, LINE),
            );
            ui.add_space(18.0);
            ui.vertical(|ui| {
                caption(ui, "Presets");
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    let list: Vec<(u64, String)> = self.st.profiles.iter().map(|p| (p.id, p.name.clone())).collect();
                    for (id, n) in list {
                        if chip(ui, pid == Some(id), self.accent, &n, None).clicked() {
                            self.st.chain_mut(&self.target).profile = Some(id);
                            self.sel_band = None;
                        }
                    }
                });
                if let Some(p) = pid.and_then(|id| self.st.profile(id))
                    && p.bands.iter().any(|b| b.enabled)
                {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "+ {} parametric band(s) in Advanced",
                            p.bands.iter().filter(|b| b.enabled).count()
                        ))
                        .size(11.5)
                        .color(DIM),
                    );
                }
            });
        });
        if tone != before {
            let id = self.st.ensure_profile(&self.target);
            if let Some(p) = self.st.profile_mut(id) {
                p.tone = tone;
            }
        }
    }

    fn bands_table(&mut self, ui: &mut Ui) {
        let pid = self.st.ensure_profile(&self.target);
        let Some(mut bands) = self.st.profile(pid).map(|p| p.bands.clone()) else {
            return;
        };
        let tone_on = self.st.profile(pid).is_some_and(|p| !p.tone.is_flat());
        let mut remove = None;
        let cols = [34.0, 46.0, 120.0, 104.0, 96.0, 78.0];
        ui.horizontal(|ui| {
            let origin = ui.cursor().min;
            let gap = ui.spacing().item_spacing.x;
            let mut x = origin.x;
            for (w, h) in cols.iter().zip(["#", "On", "Type", "Frequency", "Gain", "Q"]) {
                ui.painter().text(
                    egui::pos2(x, origin.y + 13.0),
                    egui::Align2::LEFT_CENTER,
                    h.to_uppercase(),
                    egui::FontId::proportional(10.5),
                    DIM,
                );
                x += w + gap;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ghost(ui, "+  Add band").clicked() {
                    let f = suggest_free_freq(&bands);
                    bands.push(Band::new(FilterKind::Peak, f, 0.0, 1.41));
                    self.sel_band = Some(bands.len() - 1);
                }
            });
        });
        ui.add_space(2.0);
        let row_h = 30.0;
        egui::ScrollArea::vertical()
            .id_salt("bands")
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 22.0)
            .show(ui, |ui| {
                for (i, b) in bands.iter_mut().enumerate() {
                    let sel = self.sel_band == Some(i);
                    let row = ui
                        .available_rect_before_wrap()
                        .with_max_y(ui.available_rect_before_wrap().top() + row_h);
                    if sel {
                        ui.painter().rect_filled(row.expand2(vec2(4.0, 0.0)), 8.0, RAISED);
                    }
                    ui.horizontal(|ui| {
                        ui.set_min_height(row_h);
                        let c = band_color(i);
                        let (r, resp) = ui.allocate_exact_size(vec2(cols[0], 22.0), Sense::click());
                        ui.painter()
                            .circle_filled(r.center() - vec2(6.0, 0.0), 9.0, if b.enabled { c } else { Color32::BLACK });
                        ui.painter().circle_stroke(r.center() - vec2(6.0, 0.0), 9.0, Stroke::new(1.0, c));
                        ui.painter().text(
                            r.center() - vec2(6.0, 0.0),
                            egui::Align2::CENTER_CENTER,
                            format!("{}", i + 1),
                            egui::FontId::new(10.0, semibold()),
                            if b.enabled { Color32::BLACK } else { c },
                        );
                        if resp.clicked() {
                            self.sel_band = Some(i);
                        }
                        ui.allocate_ui_with_layout(vec2(cols[1], 22.0), Layout::left_to_right(Align::Center), |ui| {
                            switch(ui, &mut b.enabled, self.accent);
                        });
                        ui.allocate_ui_with_layout(vec2(cols[2], 24.0), Layout::left_to_right(Align::Center), |ui| {
                            egui::ComboBox::from_id_salt(("kind", i))
                                .selected_text(b.kind.label())
                                .width(cols[2] - 10.0)
                                .show_ui(ui, |ui| {
                                    for k in FilterKind::ALL {
                                        ui.selectable_value(&mut b.kind, k, k.label());
                                    }
                                });
                        });
                        let mut focus = false;
                        ui.allocate_ui_with_layout(vec2(cols[3], 24.0), Layout::left_to_right(Align::Center), |ui| {
                            let speed = (b.freq * 0.004).max(0.1);
                            focus |= ui
                                .add_sized(
                                    [cols[3] - 10.0, 24.0],
                                    egui::DragValue::new(&mut b.freq)
                                        .range(10.0..=22000.0)
                                        .speed(speed)
                                        .suffix(" Hz")
                                        .max_decimals(1),
                                )
                                .is_pointer_button_down_on();
                        });
                        ui.allocate_ui_with_layout(vec2(cols[4], 24.0), Layout::left_to_right(Align::Center), |ui| {
                            ui.add_enabled_ui(b.kind.has_gain(), |ui| {
                                focus |= ui
                                    .add_sized(
                                        [cols[4] - 10.0, 24.0],
                                        egui::DragValue::new(&mut b.gain)
                                            .range(-30.0..=30.0)
                                            .speed(0.05)
                                            .suffix(" dB")
                                            .fixed_decimals(1),
                                    )
                                    .is_pointer_button_down_on();
                            });
                        });
                        ui.allocate_ui_with_layout(vec2(cols[5], 24.0), Layout::left_to_right(Align::Center), |ui| {
                            focus |= ui
                                .add_sized(
                                    [cols[5] - 10.0, 24.0],
                                    egui::DragValue::new(&mut b.q).range(0.1..=30.0).speed(0.01).fixed_decimals(2),
                                )
                                .is_pointer_button_down_on();
                        });
                        if focus {
                            self.sel_band = Some(i);
                        }
                        if flat(ui, RichText::new("✕").color(DIM)).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if bands.is_empty() {
                    ui.add_space(18.0);
                    ui.label(RichText::new("No bands yet. Add one, or double-click the graph where you want it.").color(DIM));
                }
            });
        if tone_on {
            ui.label(
                RichText::new("Simple-mode tone sliders are also shaping this profile.")
                    .size(11.5)
                    .color(DIM),
            );
        }
        if let Some(i) = remove {
            bands.remove(i);
            self.sel_band = None;
        }
        if let Some(p) = self.st.profile_mut(pid)
            && p.bands != bands
        {
            p.bands = bands;
            for b in &mut p.bands {
                b.clamp();
            }
        }
    }

    pub fn delete_selected_band(&mut self) {
        let Some(i) = self.sel_band else { return };
        let Some(pid) = self.st.chain(&self.target).profile else { return };
        if let Some(p) = self.st.profile_mut(pid)
            && i < p.bands.len()
        {
            p.bands.remove(i);
        }
        self.sel_band = None;
    }

    fn headphone_card(&mut self, ui: &mut Ui) {
        card().inner_margin(14.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.set_width(ui.available_width());
            caption(ui, "Headphone correction");
            let corr = self.st.chain(&self.target).correction.clone();
            match corr {
                Some(c) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&c.name).size(15.0).family(semibold()).color(WHITE));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if flat(ui, RichText::new("Remove").size(12.0).color(DIM)).clicked() {
                                self.st.chain_mut(&self.target).correction = None;
                            }
                            if flat(ui, RichText::new("Change").size(12.0).color(DIM)).clicked() {
                                self.open_autoeq();
                            }
                        });
                    });
                    ui.label(
                        RichText::new(if c.source.is_empty() {
                            "Imported".to_string()
                        } else {
                            c.source.clone()
                        })
                        .size(11.5)
                        .color(DIM),
                    );
                    let mut s = self.st.chain(&self.target).strength * 100.0;
                    let changed = ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(vec2(62.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                            ui.label(RichText::new("Strength").color(TEXT))
                        });
                        hslider(ui, &mut s, 0.0..=150.0, VIOLET, Some(100.0), false, |v| format!("{v:.0} %"))
                            .on_hover_text("100% = measured target. Lower keeps some of the headphone's character. Double-click resets.")
                            .changed()
                    });
                    if changed.inner {
                        self.st.chain_mut(&self.target).strength = (s / 100.0).clamp(0.0, 1.5);
                    }
                }
                None => {
                    ui.label(
                        RichText::new("Make your headphones sound neutral with lab measurements from AutoEq: about 9,000 models.")
                            .color(DIM)
                            .size(12.5),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if primary(ui, self.accent, "Find my headphones").clicked() {
                            self.open_autoeq();
                        }
                        if ghost(ui, "Import…")
                            .on_hover_text("Load a correction file (ParametricEQ.txt, GraphicEQ, CSV)")
                            .clicked()
                        {
                            self.import(true);
                        }
                    });
                }
            }
        });
    }

    fn output_card(&mut self, ui: &mut Ui) {
        let cv = self.curves();
        let r = &cv.r;
        card().inner_margin(14.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.set_width(ui.available_width());
            caption(ui, "Output");
            let mut auto = self.st.chain(&self.target).auto_preamp;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Preamp").color(TEXT));
                ui.label(RichText::new(fmt_db(r.preamp)).monospace().color(WHITE));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if switch(ui, &mut auto, self.accent)
                        .on_hover_text("Lower the volume just enough that boosts can't clip")
                        .changed()
                    {
                        let ch = self.st.chain_mut(&self.target);
                        ch.auto_preamp = auto;
                        ch.preamp = r.preamp;
                    }
                    ui.label(RichText::new("Auto").size(12.0).color(DIM));
                });
            });
            if !auto {
                let mut p = self.st.chain(&self.target).preamp;
                if hslider(ui, &mut p, -24.0..=6.0, self.accent, Some(0.0), true, fmt_db).changed() {
                    self.st.chain_mut(&self.target).preamp = (p * 10.0).round() / 10.0;
                }
            }
            let margin = r.clip_margin();
            let (c, t) = if self.st.bypass {
                (DIM, "Bypassed".to_string())
            } else if margin > 0.05 {
                (BAD, format!("Can clip by {}: lower the preamp or turn on Auto", fmt_db(margin)))
            } else {
                (GOOD, format!("No clipping  ·  peak boost {}", fmt_db(r.peak.max(0.0))))
            };
            ui.horizontal(|ui| {
                let (rr, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(rr.center(), 3.5, c);
                ui.label(RichText::new(t).size(11.5).color(DIM));
            });
            ui.add_space(2.0);
            let mut bal = self.st.chain(&self.target).balance;
            let changed = ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(62.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new("Balance").color(TEXT))
                });
                hslider(ui, &mut bal, -6.0..=6.0, self.accent, Some(0.0), true, |v| {
                    if v.abs() < 0.05 {
                        "center".into()
                    } else if v > 0.0 {
                        format!("R {v:.1}")
                    } else {
                        format!("L {:.1}", -v)
                    }
                })
                .on_hover_text("Fix a channel imbalance. Double-click to center.")
                .changed()
            });
            if changed.inner {
                self.st.chain_mut(&self.target).balance = (bal * 10.0).round() / 10.0;
            }
        });
    }

    fn open_autoeq(&mut self) {
        self.aeq.open = true;
        self.aeq.focus = true;
        self.aeq.error = None;
    }

    pub fn autoeq_modal(&mut self, ctx: &egui::Context) {
        if !self.aeq.open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("autoeq"))
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, FAINT))
                    .corner_radius(16)
                    .inner_margin(20.0),
            )
            .backdrop_color(Color32::from_black_alpha(190));
        let resp = modal.show(ctx, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                heading(ui, "Find your headphones");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if flat(ui, RichText::new("✕").color(DIM)).clicked() {
                        self.aeq.open = false;
                    }
                });
            });
            ui.label(
                RichText::new(format!("Correction goes on: {}", self.target_name(&self.target)))
                    .size(12.0)
                    .color(DIM),
            );
            ui.add_space(8.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.aeq.query)
                    .hint_text("Model, e.g. HD 600, AirPods Pro, WH-1000XM5…")
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(15.0))
                    .margin(vec2(12.0, 9.0)),
            );
            if self.aeq.focus {
                r.request_focus();
                self.aeq.focus = false;
            }
            ui.add_space(6.0);
            let Some(idx) = self.aeq.index.clone() else {
                if let Some(e) = self.aeq.error.clone() {
                    ui.label(RichText::new(format!("Couldn't load the AutoEq list: {e}")).color(BAD));
                    if ghost(ui, "Retry").clicked() {
                        self.aeq.error = None;
                        self.load_index(ctx, true);
                    }
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading the AutoEq database…").color(DIM));
                    });
                }
                return;
            };
            if let Some(name) = &self.aeq.fetching {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("Downloading {name}…")).color(DIM));
                });
            }
            if let Some(e) = &self.aeq.error {
                ui.label(RichText::new(e).color(BAD).size(12.0));
            }
            let q = self.aeq.query.trim().to_string();
            let results: Vec<equinox::autoeq::Entry> = if q.is_empty() {
                // Suggest what's connected first.
                let mut v: Vec<_> = self.aeq.suggestions.values().cloned().collect();
                v.dedup_by(|a, b| a.key() == b.key());
                v
            } else {
                equinox::autoeq::search(&idx, &q, 120).into_iter().cloned().collect()
            };
            if q.is_empty() {
                caption(
                    ui,
                    if results.is_empty() {
                        "Start typing to search"
                    } else {
                        "Recognised from your devices"
                    },
                );
            } else {
                caption(
                    ui,
                    &format!("{} match{}", results.len(), if results.len() == 1 { "" } else { "es" }),
                );
            }
            let row_h = 40.0;
            let mut pick = None;
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_h, results.len(), |ui, range| {
                    for i in range {
                        let e = &results[i];
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h - 4.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 8.0, RAISED);
                        }
                        ui.painter().text(
                            rect.left_center() + vec2(12.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &e.name,
                            egui::FontId::proportional(14.0),
                            WHITE,
                        );
                        let best = i == 0 && !q.is_empty();
                        ui.painter().text(
                            rect.right_center() - vec2(12.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            format!("{}{}", if best { "best match · " } else { "" }, e.source_label()),
                            egui::FontId::proportional(11.5),
                            if best { self.accent } else { DIM },
                        );
                        if resp.clicked() {
                            pick = Some(e.clone());
                        }
                    }
                });
            if ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some(e) = results.first()
            {
                pick = Some(e.clone());
            }
            if let Some(e) = pick
                && self.aeq.fetching.is_none()
            {
                let t = self.target.clone();
                self.fetch_correction(ctx, e, t);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let age = equinox::autoeq::index_age().map(|a| a.as_secs() / 86400);
                ui.label(
                    RichText::new(format!(
                        "{} profiles from AutoEq by Jaakko Pasanen · updated {}",
                        idx.len(),
                        match age {
                            Some(0) | None => "today".to_string(),
                            Some(1) => "yesterday".to_string(),
                            Some(d) => format!("{d} days ago"),
                        }
                    ))
                    .size(11.0)
                    .color(DIM),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if flat(ui, RichText::new("Refresh").size(11.0).color(DIM)).clicked() {
                        self.aeq.index = None;
                        self.load_index(ctx, true);
                    }
                });
            });
        });
        if resp.should_close() {
            self.aeq.open = false;
        }
    }

    fn import(&mut self, as_correction: bool) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("EQ files", &["txt", "csv"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return self.notify(format!("couldn't read file: {e}"), true),
        };
        let parsed = eqtext::parse(&text);
        if parsed.is_empty() {
            return self.notify("No filters or curve found in that file", true);
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Imported")
            .replace(" ParametricEQ", "")
            .replace(" GraphicEQ", "");
        if as_correction || !parsed.graphic.is_empty() {
            self.st.chain_mut(&self.target).correction = Some(Correction {
                name: stem.clone(),
                source: "Imported file".into(),
                filters: parsed.bands,
                graphic: parsed.graphic,
            });
            self.notify(format!("Loaded {stem} as headphone correction"), false);
        } else {
            let id = self.st.new_id();
            let name = self.st.unique_name(&stem);
            let n = parsed.bands.len();
            self.st.profiles.push(Profile {
                id,
                name: name.clone(),
                tone: Tone::default(),
                bands: parsed.bands,
            });
            self.st.chain_mut(&self.target).profile = Some(id);
            self.st.settings.advanced = true;
            self.notify(format!("Imported {n} filters as {name}"), false);
        }
        if !parsed.warnings.is_empty() {
            self.notify(
                format!("Imported with {} skipped line(s): {}", parsed.warnings.len(), parsed.warnings[0]),
                true,
            );
        }
    }

    fn export(&mut self) {
        let r = engine::resolve(&self.st, self.st.chain(&self.target));
        let name = self
            .st
            .chain(&self.target)
            .profile
            .and_then(|id| self.st.profile(id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "equinox".into());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{name} ParametricEQ.txt"))
            .add_filter("Text", &["txt"])
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, eqtext::export(r.preamp, &r.all(), &r.graphic)) {
            Ok(()) => self.notify(format!("Exported to {}", path.display()), false),
            Err(e) => self.notify(format!("couldn't write: {e}"), true),
        }
    }
}

/// A callout with message and buttons. Returns the index of a clicked button.
fn banner(ui: &mut Ui, color: Color32, text: &str, buttons: &[&str]) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::new()
        .fill(color.gamma_multiply(0.07))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(6.0, 18.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.0, color);
                let btn_w = buttons.iter().map(|b| b.len() as f32 * 7.5 + 34.0).sum::<f32>() + 10.0;
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width() - btn_w, 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(TEXT)).wrap());
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for (i, b) in buttons.iter().enumerate().rev() {
                        let btn = egui::Button::new(RichText::new(*b).size(12.5).color(WHITE))
                            .fill(color.gamma_multiply(0.18))
                            .corner_radius(7)
                            .min_size(vec2(0.0, 24.0));
                        if ui.add(btn).clicked() {
                            clicked = Some(i);
                        }
                    }
                });
            });
        });
    clicked
}

/// Pick a frequency for a new band: the widest log gap between existing bands.
fn suggest_free_freq(bands: &[Band]) -> f32 {
    let mut f: Vec<f32> = bands.iter().map(|b| b.freq.ln()).collect();
    f.push(30f32.ln());
    f.push(16000f32.ln());
    f.sort_by(f32::total_cmp);
    let (mut best, mut gap) = (1000f32.ln(), 0.0);
    for w in f.windows(2) {
        if w[1] - w[0] > gap {
            gap = w[1] - w[0];
            best = (w[0] + w[1]) / 2.0;
        }
    }
    (best.exp() / 10.0).round() * 10.0
}
