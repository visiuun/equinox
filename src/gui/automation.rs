//! Automation page: rules (device / time / app -> profile / bypass / strength) and hotkeys.

use eframe::egui::{self, Align, Layout, RichText, Sense, Ui, vec2};
use equinox::devices::AudioDevice;
use equinox::engine::parse_hhmm;
use equinox::model::{Action, Mode, Rule, Target, Trigger};
use equinox::sys;

use super::App;
use super::theme::*;

impl App {
    pub fn automation_page(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().id_salt("auto").auto_shrink([false, false]).show(ui, |ui| super::centered(ui, 900.0, |ui| {
            ui.add_space(10.0);
            heading(ui, "Automation");
            ui.label(
                RichText::new("Rules change your sound on their own: when an output becomes the default, at certain hours, or while an app runs. When several match, the lower one wins.")
                    .color(DIM),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                if self.agent {
                    ui.painter().circle_filled(r.center(), 3.5, GOOD);
                    ui.label(RichText::new("Background agent running in the tray, so rules and hotkeys work with this window closed.").size(12.5).color(DIM));
                } else {
                    ui.painter().circle_filled(r.center(), 3.5, WARN);
                    ui.label(RichText::new("Rules only run while this window is open.").size(12.5).color(DIM));
                    if ghost(ui, "Start background agent").clicked() {
                        self.st.settings.agent = true;
                        sys::start_agent();
                    }
                }
            });
            ui.add_space(12.0);

            caption(ui, "Add a rule");
            ui.horizontal_wrapped(|ui| {
                let current = self.st.chain(&self.target).profile.or(self.st.profiles.first().map(|p| p.id)).unwrap_or(0);
                let find = |s: &equinox::model::State, n: &str| s.find_profile(n).unwrap_or(current);
                let mut add = None;
                if ghost(ui, "🎧  When an output is selected").clicked() {
                    let dev = self.devices.iter().find(|d| d.form.is_headphone() && d.active).or(self.devices.iter().find(|d| d.default));
                    add = Some((
                        "Headphones".to_string(),
                        Trigger::DefaultDevice { device: dev.map(|d| d.id.clone()).unwrap_or_default() },
                        Action::UseProfile { target: Target::Global, profile: current },
                    ));
                }
                if ghost(ui, "🌙  Night mode").clicked() {
                    add = Some((
                        "Night mode".into(),
                        Trigger::TimeRange { start: "22:00".into(), end: "07:00".into() },
                        Action::UseProfile { target: Target::Global, profile: find(&self.st, "late night") },
                    ));
                }
                if ghost(ui, "🎮  While a game runs").clicked() {
                    add = Some((
                        "Gaming".into(),
                        Trigger::AppRunning { exe: "cs2.exe".into() },
                        Action::UseProfile { target: Target::Global, profile: find(&self.st, "footsteps") },
                    ));
                }
                if ghost(ui, "⏻  Bypass while an app runs").clicked() {
                    add = Some(("Bypass for app".into(), Trigger::AppRunning { exe: "obs64.exe".into() }, Action::Bypass { on: true }));
                }
                if let Some((name, trigger, action)) = add {
                    let id = self.st.new_id();
                    self.st.rules.push(Rule { id, name, enabled: true, trigger, action });
                }
            });
            ui.add_space(12.0);

            if self.st.rules.is_empty() {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("No rules yet. Start from one of the templates above.").color(DIM));
                });
            }
            let mut rules = self.st.rules.clone();
            let mut remove = None;
            let mut swap = None;
            let n = rules.len();
            for (i, r) in rules.iter_mut().enumerate() {
                match self.rule_card(ui, r, i, n) {
                    RuleOp::None => {}
                    RuleOp::Remove => remove = Some(i),
                    RuleOp::Up => swap = Some((i, i - 1)),
                    RuleOp::Down => swap = Some((i, i + 1)),
                }
                ui.add_space(8.0);
            }
            if let Some(i) = remove {
                rules.remove(i);
            }
            if let Some((a, b)) = swap {
                rules.swap(a, b);
            }
            if rules != self.st.rules {
                self.st.rules = rules;
            }

            ui.add_space(16.0);
            caption(ui, "Global hotkeys");
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut on = self.st.settings.hotkeys;
                if switch_row(ui, &mut on, self.accent, "Enable hotkeys", "Handled by the background agent, so they work in any app") {
                    self.st.settings.hotkeys = on;
                }
                ui.add_space(6.0);
                for (k, d) in [("Ctrl + Alt + E", "Toggle EQ / bypass"), ("Ctrl + Alt + .", "Next profile"), ("Ctrl + Alt + ,", "Previous profile")] {
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(vec2(140.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
                            ui.label(RichText::new(k).monospace().color(WHITE));
                        });
                        ui.label(RichText::new(d).color(DIM));
                    });
                }
            });
        }));
    }

    fn rule_card(&mut self, ui: &mut Ui, r: &mut Rule, i: usize, n: usize) -> RuleOp {
        let mut op = RuleOp::None;
        let active = self.active_rules.contains(&r.id);
        let devices: Vec<AudioDevice> = self.devices.clone();
        let profiles: Vec<(u64, String)> = self.st.profiles.iter().map(|p| (p.id, p.name.clone())).collect();
        let per_device = self.st.mode == Mode::PerDevice;
        let accent = self.accent;
        let frame = if active {
            card().stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.5)))
        } else {
            card()
        };
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                switch(ui, &mut r.enabled, accent);
                ui.add(
                    egui::TextEdit::singleline(&mut r.name)
                        .frame(egui::Frame::NONE)
                        .desired_width(260.0)
                        .font(egui::FontId::new(14.5, semibold())),
                );
                if active {
                    ui.label(RichText::new("● active now").size(11.5).color(accent));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if flat(ui, RichText::new("✕").color(DIM)).on_hover_text("Delete rule").clicked() {
                        op = RuleOp::Remove;
                    }
                    if i + 1 < n
                        && flat(ui, RichText::new("↓").color(DIM))
                            .on_hover_text("Lower priority… (later rules win)")
                            .clicked()
                    {
                        op = RuleOp::Down;
                    }
                    if i > 0 && flat(ui, RichText::new("↑").color(DIM)).clicked() {
                        op = RuleOp::Up;
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(46.0, 24.0), Layout::left_to_right(Align::Center), |ui| caption(ui, "When"));
                let kind = match r.trigger {
                    Trigger::DefaultDevice { .. } => 0,
                    Trigger::TimeRange { .. } => 1,
                    Trigger::AppRunning { .. } => 2,
                };
                let mut k = kind;
                egui::ComboBox::from_id_salt(("trig", r.id))
                    .width(170.0)
                    .selected_text(["Output device is", "Time is between", "App is running"][k])
                    .show_ui(ui, |ui| {
                        for (v, l) in ["Output device is", "Time is between", "App is running"].iter().enumerate() {
                            ui.selectable_value(&mut k, v, *l);
                        }
                    });
                if k != kind {
                    r.trigger = match k {
                        0 => Trigger::DefaultDevice {
                            device: devices.iter().find(|d| d.default).map(|d| d.id.clone()).unwrap_or_default(),
                        },
                        1 => Trigger::TimeRange {
                            start: "22:00".into(),
                            end: "07:00".into(),
                        },
                        _ => Trigger::AppRunning { exe: String::new() },
                    };
                }
                match &mut r.trigger {
                    Trigger::DefaultDevice { device } => {
                        let label = devices
                            .iter()
                            .find(|d| &d.id == device)
                            .map(|d| d.label())
                            .or_else(|| self.st.device(device).map(|d| d.name.clone()))
                            .unwrap_or("Choose…".into());
                        egui::ComboBox::from_id_salt(("dev", r.id))
                            .width(300.0)
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                for d in &devices {
                                    ui.selectable_value(
                                        device,
                                        d.id.clone(),
                                        format!("{}{}", d.label(), if d.active { "" } else { "  (unplugged)" }),
                                    );
                                }
                            });
                    }
                    Trigger::TimeRange { start, end } => {
                        for (j, s) in [start, end].into_iter().enumerate() {
                            if j == 1 {
                                ui.label(RichText::new("and").color(DIM));
                            }
                            let ok = parse_hhmm(s).is_some();
                            ui.add(
                                egui::TextEdit::singleline(s)
                                    .desired_width(56.0)
                                    .text_color(if ok { WHITE } else { BAD })
                                    .hint_text("HH:MM"),
                            );
                        }
                    }
                    Trigger::AppRunning { exe } => {
                        ui.add(egui::TextEdit::singleline(exe).desired_width(170.0).hint_text("game.exe"));
                        egui::ComboBox::from_id_salt(("procs", r.id))
                            .width(120.0)
                            .selected_text(RichText::new("Running…").color(DIM))
                            .show_ui(ui, |ui| {
                                if self.procs.is_empty() {
                                    let mut p: Vec<String> = sys::processes().into_iter().filter(|p| p.ends_with(".exe")).collect();
                                    p.sort();
                                    self.procs = p;
                                }
                                for p in &self.procs {
                                    if ui.selectable_label(false, p).clicked() {
                                        *exe = p.clone();
                                    }
                                }
                            });
                    }
                }
            });
            if !ui.ctx().any_popup_open() {
                self.procs.clear();
            }
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(46.0, 24.0), Layout::left_to_right(Align::Center), |ui| caption(ui, "Then"));
                let kind = match r.action {
                    Action::UseProfile { .. } => 0,
                    Action::Bypass { .. } => 1,
                    Action::Strength { .. } => 2,
                };
                let mut k = kind;
                let names = ["Use profile", "Bypass / force EQ", "Correction strength"];
                egui::ComboBox::from_id_salt(("act", r.id))
                    .width(170.0)
                    .selected_text(names[k])
                    .show_ui(ui, |ui| {
                        for (v, l) in names.iter().enumerate() {
                            ui.selectable_value(&mut k, v, *l);
                        }
                    });
                if k != kind {
                    r.action = match k {
                        0 => Action::UseProfile {
                            target: Target::Global,
                            profile: profiles.first().map(|p| p.0).unwrap_or(0),
                        },
                        1 => Action::Bypass { on: true },
                        _ => Action::Strength {
                            target: Target::Global,
                            value: 0.5,
                        },
                    };
                }
                let target_combo = |ui: &mut Ui, t: &mut Target, salt: (&str, u64)| {
                    let label = |t: &Target| match t {
                        Target::Global if per_device => "Default".to_string(),
                        Target::Global => "All devices".to_string(),
                        Target::Device(id) => devices
                            .iter()
                            .find(|d| &d.id == id)
                            .map(|d| d.label())
                            .unwrap_or_else(|| id.clone()),
                    };
                    ui.label(RichText::new("on").color(DIM));
                    egui::ComboBox::from_id_salt(salt)
                        .width(200.0)
                        .selected_text(label(t))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(t, Target::Global, label(&Target::Global));
                            if per_device {
                                for d in devices.iter().filter(|d| d.active) {
                                    ui.selectable_value(t, Target::Device(d.id.clone()), d.label());
                                }
                            }
                        });
                };
                match &mut r.action {
                    Action::UseProfile { target, profile } => {
                        let name = profiles
                            .iter()
                            .find(|p| p.0 == *profile)
                            .map(|p| p.1.clone())
                            .unwrap_or("Choose…".into());
                        egui::ComboBox::from_id_salt(("prof", r.id))
                            .width(170.0)
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                for (id, n) in &profiles {
                                    ui.selectable_value(profile, *id, n);
                                }
                            });
                        target_combo(ui, target, ("tgt", r.id));
                    }
                    Action::Bypass { on } => {
                        segmented(ui, on, &[(true, "Bypass EQ"), (false, "Force EQ on")], accent);
                    }
                    Action::Strength { target, value } => {
                        ui.allocate_ui_with_layout(vec2(220.0, 26.0), Layout::left_to_right(Align::Center), |ui| {
                            let mut v = *value * 100.0;
                            if hslider(ui, &mut v, 0.0..=150.0, VIOLET, Some(100.0), false, |v| format!("{v:.0} %")).changed() {
                                *value = v / 100.0;
                            }
                        });
                        target_combo(ui, target, ("tgt", r.id));
                    }
                }
            });
        });
        op
    }
}

enum RuleOp {
    None,
    Remove,
    Up,
    Down,
}
