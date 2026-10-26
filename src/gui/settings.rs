//! Settings page: startup, sound behaviour, display, Equalizer APO, CLI, data.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Ui, vec2};
use equinox::model::State;
use equinox::sys;

use super::App;
use super::theme::*;

impl App {
    pub fn settings_page(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).show(ui, |ui| super::centered(ui, 780.0, |ui| {
            ui.add_space(10.0);
            heading(ui, "Settings");
            ui.add_space(10.0);

            section(ui, "General", |ui| {
                let mut on = self.autostart;
                if switch_row(ui, &mut on, self.accent, "Start with Windows", "Runs the tray agent at sign-in. Equalizer APO keeps the EQ applied even without it.") {
                    match sys::set_autostart(on) {
                        Ok(()) => {
                            self.autostart = sys::autostart_enabled();
                            if on {
                                self.st.settings.agent = true;
                                sys::start_agent();
                            }
                        }
                        Err(e) => self.notify(format!("couldn't change startup: {e}"), true),
                    }
                }
                let mut agent = self.st.settings.agent;
                if switch_row(ui, &mut agent, self.accent, "Keep running in the tray", "Needed for automation rules and global hotkeys when this window is closed.") {
                    self.st.settings.agent = agent;
                    if agent {
                        sys::start_agent();
                    }
                }
                let mut hk = self.st.settings.hotkeys;
                if switch_row(ui, &mut hk, self.accent, "Global hotkeys", "Ctrl+Alt+E bypass · Ctrl+Alt+. / , next / previous profile") {
                    self.st.settings.hotkeys = hk;
                }
            });

            section(ui, "Sound", |ui| {
                let mut lm = self.st.settings.level_matched_bypass;
                if switch_row(ui, &mut lm, self.accent, "Level-matched bypass", "Bypass keeps the preamp, so A/B compares the EQ itself, not loudness.") {
                    self.st.settings.level_matched_bypass = lm;
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Extra headroom").color(TEXT));
                ui.label(RichText::new("Added on top of the auto preamp, for players or effects that push near 0 dBFS.").size(11.5).color(DIM));
                let mut h = self.st.settings.headroom;
                if hslider(ui, &mut h, 0.0..=6.0, self.accent, Some(0.0), false, |v| format!("{v:.1} dB")).changed() {
                    self.st.settings.headroom = (h * 10.0).round() / 10.0;
                }
            });

            section(ui, "Display", |ui| {
                let mut a = self.st.settings.analyzer;
                if switch_row(ui, &mut a, self.accent, "Live spectrum", "Shows what's playing behind the EQ curve (captures system output; nothing is recorded).") {
                    self.st.settings.analyzer = a;
                }
                let mut b = self.st.settings.band_curves;
                if switch_row(ui, &mut b, self.accent, "Show each band's shape", "Faint per-band curves in the graph.") {
                    self.st.settings.band_curves = b;
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Accent").color(TEXT));
                ui.horizontal(|ui| {
                    for rgb in ACCENTS {
                        let c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                        let sel = self.st.settings.accent == rgb;
                        ui.painter().circle_filled(r.center(), 10.0, c);
                        if sel {
                            ui.painter().circle_stroke(r.center(), 13.0, Stroke::new(1.5, WHITE));
                        }
                        if resp.clicked() {
                            self.set_accent(&ctx, rgb);
                        }
                    }
                });
            });

            section(ui, "Equalizer APO", |ui| match self.apo.clone() {
                None => {
                    ui.label(RichText::new("Not installed. equinox writes its filters for Equalizer APO, which does the processing inside Windows audio.").color(DIM));
                    if primary(ui, self.accent, "Install Equalizer APO").on_hover_text("Downloads the official installer, checks it and installs it").clicked() {
                        self.install_everything(&ctx);
                    }
                }
                Some(a) => {
                    kv(ui, "Version", &a.version);
                    kv(ui, "Config folder", &a.config.display().to_string());
                    kv(ui, "Loading equinox", if self.hooked { "Yes" } else { "No" });
                    let on: Vec<String> = self.devices.iter().filter(|d| d.apo).map(|d| d.label()).collect();
                    kv(ui, "Installed on", &if on.is_empty() { "No current device".to_string() } else { on.join(", ") });
                    if !self.foreign.is_empty() {
                        kv(ui, "Also active", &self.foreign.join(", "));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if !self.hooked {
                            if primary(ui, self.accent, "Use equinox").clicked() {
                                self.hook_apo();
                            }
                        } else if ghost(ui, "Restore previous config").on_hover_text("Puts back the config.txt from before equinox").clicked() {
                            match a.release() {
                                Ok(()) => {
                                    self.refresh_apo();
                                    self.notify("Restored your previous Equalizer APO config", false);
                                }
                                Err(e) => self.notify(format!("couldn't restore: {e}"), true),
                            }
                        }
                        if ghost(ui, "Choose devices…").on_hover_text("Pick which outputs get the EQ").clicked() {
                            self.open_picker(None);
                        }
                        if ghost(ui, "Open folder").clicked() {
                            sys::open_path(&a.config);
                        }
                    });
                }
            });

            section(ui, "Command line", |ui| {
                let installed = sys::install_dir();
                let eqx = if installed.join("eqx.exe").exists() { installed.join("eqx.exe") } else { equinox::paths::app_exe().with_file_name("eqx.exe") };
                ui.label(RichText::new("Everything here is scriptable with eqx, and this window updates live when it runs.").color(DIM));
                ui.add_space(4.0);
                egui::Frame::new().fill(Color32::from_rgb(10, 10, 12)).corner_radius(8).inner_margin(12.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for line in [
                        "eqx status",
                        "eqx use \"bass boost\" -d headphones",
                        "eqx toggle",
                        "eqx autoeq apply hd 600 --strength 80",
                        "eqx band add pk 3000 -2.5 2",
                        "eqx curve",
                        "eqx status --json",
                    ] {
                        ui.label(RichText::new(line).monospace().color(TEXT));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(eqx.display().to_string()).monospace().size(11.5).color(DIM));
                    if flat(ui, RichText::new("Copy path").size(11.5).color(self.accent)).clicked() {
                        ctx.copy_text(eqx.display().to_string());
                        self.notify("Copied", false);
                    }
                    if let Some(dir) = eqx.parent() {
                        if sys::on_user_path(dir) {
                            ui.label(RichText::new("· on PATH").size(11.5).color(GOOD));
                        } else if flat(ui, RichText::new("Add to PATH").size(11.5).color(self.accent))
                            .on_hover_text("Lets you type eqx in any new terminal")
                            .clicked()
                        {
                            match sys::add_to_user_path(dir) {
                                Ok(()) => self.notify("eqx is on your PATH; open a new terminal to use it", false),
                                Err(e) => self.notify(format!("couldn't update PATH: {e}"), true),
                            }
                        }
                    }
                });
            });

            section(ui, "Data", |ui| {
                ui.horizontal(|ui| {
                    if ghost(ui, "Open data folder").clicked() {
                        let _ = std::fs::create_dir_all(equinox::paths::dir());
                        sys::open_path(&equinox::paths::dir());
                    }
                    if !self.confirm_reset {
                        if ghost(ui, "Reset everything…").clicked() {
                            self.confirm_reset = true;
                        }
                    } else {
                        ui.label(RichText::new("Profiles, rules and device settings go back to defaults.").color(WARN));
                        if ui.add(egui::Button::new(RichText::new("Reset").color(WHITE)).fill(BAD.gamma_multiply(0.5))).clicked() {
                            let keep = self.st.settings.accent;
                            self.st = State::default();
                            self.st.settings.accent = keep;
                            self.confirm_reset = false;
                            self.notify("Reset to defaults (Ctrl+Z undoes it)", false);
                        }
                        if ghost(ui, "Cancel").clicked() {
                            self.confirm_reset = false;
                        }
                    }
                });
            });

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("equinox {}  ·  Created by Visiuun", env!("CARGO_PKG_VERSION"))).size(11.5).color(DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.hyperlink_to(RichText::new("AutoEq").size(11.5), "https://github.com/jaakkopasanen/AutoEq");
                    ui.label(RichText::new("Headphone data:").size(11.5).color(DIM));
                });
            });
            ui.add_space(20.0);
        }));
    }
}

fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    caption(ui, title);
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 10.0;
        body(ui);
    });
    ui.add_space(14.0);
}

fn kv(ui: &mut Ui, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(130.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
            ui.label(RichText::new(k).color(DIM));
        });
        ui.label(RichText::new(v).color(TEXT));
    });
}
