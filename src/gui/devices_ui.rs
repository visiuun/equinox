//! "Where should equinox work?": a Windows 11-style output picker that installs
//! Equalizer APO on the chosen devices (one UAC prompt) and shows whether it
//! verifiably works, replacing Equalizer APO's Device Selector.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Ui, pos2, vec2};
use equinox::apo_setup::{self, Report};
use equinox::sys;

use super::App;
use super::theme::*;

#[derive(Default)]
pub struct Picker {
    pub open: bool,
    /// Devices the user wants EQ on.
    pub want: HashSet<String>,
    pub show_unplugged: bool,
    pub job: Option<Receiver<Result<Report, String>>>,
    /// Device labels being changed, for the progress view.
    pub busy: Vec<String>,
    pub result: Option<Result<Report, String>>,
}

impl App {
    pub fn open_picker(&mut self, preselect: Option<String>) {
        self.picker.want = self.devices.iter().filter(|d| d.apo).map(|d| d.id.clone()).collect();
        self.picker.want.extend(preselect);
        self.picker.result = None;
        self.picker.open = true;
    }

    /// Install/remove Equalizer APO on devices in an elevated helper.
    pub fn start_apo_setup(&mut self, ctx: &egui::Context, install: Vec<String>, uninstall: Vec<String>) {
        if self.picker.job.is_some() || (install.is_empty() && uninstall.is_empty()) {
            return;
        }
        self.picker.busy = install
            .iter()
            .chain(&uninstall)
            .map(|id| self.target_name(&equinox::model::Target::Device(id.clone())))
            .collect();
        self.picker.result = None;
        self.picker.open = true;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(apo_setup::run_elevated(&install, &uninstall, None));
            ctx.request_repaint();
        });
        self.picker.job = Some(rx);
    }

    /// No Equalizer APO yet: download it, then install it and enable the
    /// default output in one elevated step.
    pub fn install_everything(&mut self, ctx: &egui::Context) {
        if self.picker.job.is_some() {
            return;
        }
        let dev = self
            .devices
            .iter()
            .find(|d| d.default && d.active)
            .map(|d| (d.id.clone(), d.label()));
        self.picker.busy = std::iter::once("Equalizer APO (downloading, then installing)".to_string())
            .chain(dev.iter().map(|d| d.1.clone()))
            .collect();
        self.picker.result = None;
        self.picker.open = true;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = apo_setup::download_equalizer_apo().and_then(|p| {
                let install: Vec<String> = dev.into_iter().map(|d| d.0).collect();
                apo_setup::run_elevated(&install, &[], Some(&p))
            });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.picker.job = Some(rx);
    }

    pub fn poll_picker(&mut self) {
        let Some(rx) = &self.picker.job else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.picker.job = None;
        self.devices = equinox::devices::list();
        self.refresh_apo();
        if let Ok(rep) = &r {
            let ok = rep.devices.iter().any(|d| d.installed);
            // Installing for equinox implies Equalizer APO should load equinox.
            if ok && !self.hooked {
                self.hook_apo();
            }
            self.push();
        }
        if matches!(&r, Err(e) if e == "cancelled") {
            self.picker.busy.clear();
            self.notify("Nothing changed (the admin prompt was cancelled)", false);
            return;
        }
        self.picker.result = Some(r);
    }

    pub fn picker_modal(&mut self, ctx: &egui::Context) {
        if !self.picker.open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("device-picker"))
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, FAINT))
                    .corner_radius(16)
                    .inner_margin(22.0),
            )
            .backdrop_color(Color32::from_black_alpha(190));
        let resp = modal.show(ctx, |ui| {
            ui.set_width(600.0);
            if self.picker.job.is_some() {
                self.picker_busy(ui);
            } else if self.picker.result.is_some() {
                self.picker_result(ui);
            } else {
                self.picker_list(ui, ctx);
            }
        });
        if resp.should_close() && self.picker.job.is_none() {
            self.picker.open = false;
        }
    }

    fn picker_list(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        heading(ui, "Where should equinox work?");
        ui.label(
            RichText::new("Turn on the outputs you want to EQ. You'll get one Windows admin prompt, and sound cuts out for a second while it applies.")
                .size(12.5)
                .color(DIM),
        );
        ui.add_space(12.0);

        let active: Vec<_> = self.devices.iter().filter(|d| d.active).cloned().collect();
        let unplugged: Vec<_> = self.devices.iter().filter(|d| !d.active).cloned().collect();
        let shown: Vec<_> = if self.picker.show_unplugged {
            active.iter().chain(&unplugged).cloned().collect()
        } else {
            active.clone()
        };

        egui::Frame::new()
            .fill(Color32::from_rgb(12, 12, 14))
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(12)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, d) in shown.iter().enumerate() {
                            let on = self.picker.want.contains(&d.id);
                            let h = 64.0;
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                            let p = ui.painter();
                            if resp.hovered() {
                                p.rect_filled(rect, 0.0, Color32::from_rgb(20, 20, 23));
                            }
                            if i > 0 {
                                p.line_segment(
                                    [pos2(rect.left() + 16.0, rect.top()), pos2(rect.right() - 16.0, rect.top())],
                                    Stroke::new(1.0, LINE),
                                );
                            }
                            // Device icon tile, lit when EQ is (or will be) on.
                            let tile = egui::Rect::from_center_size(pos2(rect.left() + 34.0, rect.center().y), vec2(36.0, 36.0));
                            p.rect_filled(
                                tile,
                                9.0,
                                if on {
                                    self.accent.gamma_multiply(0.16)
                                } else {
                                    Color32::from_rgb(24, 24, 27)
                                },
                            );
                            p.text(
                                tile.center(),
                                egui::Align2::CENTER_CENTER,
                                d.form.glyph(),
                                egui::FontId::proportional(17.0),
                                if on { self.accent } else { DIM },
                            );
                            let x = rect.left() + 66.0;
                            p.text(
                                pos2(x, rect.top() + 14.0),
                                egui::Align2::LEFT_TOP,
                                &d.name,
                                egui::FontId::new(14.0, semibold()),
                                WHITE,
                            );
                            let mut sub = d.interface.clone();
                            let mut tags = Vec::new();
                            if d.default {
                                tags.push("Default device");
                            }
                            if !d.active {
                                tags.push("Unplugged");
                            }
                            if !tags.is_empty() {
                                sub = format!("{sub}  ·  {}", tags.join("  ·  "));
                            }
                            p.text(
                                pos2(x, rect.top() + 36.0),
                                egui::Align2::LEFT_TOP,
                                sub,
                                egui::FontId::proportional(12.0),
                                DIM,
                            );
                            // Current state on the right.
                            let (label, col) = match (d.apo, on) {
                                (true, true) => ("EQ active", GOOD),
                                (true, false) => ("will be removed", WARN),
                                (false, true) => ("will be enabled", self.accent),
                                (false, false) => ("", DIM),
                            };
                            p.text(
                                pos2(rect.right() - 74.0, rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                label,
                                egui::FontId::proportional(12.0),
                                col,
                            );
                            let sw = egui::Rect::from_center_size(pos2(rect.right() - 38.0, rect.center().y), vec2(36.0, 20.0));
                            let t = ui.ctx().animate_bool_responsive(resp.id.with("sw"), on);
                            p.rect_filled(sw, 10.0, lerp_color(Color32::from_rgb(38, 38, 42), self.accent, t));
                            p.circle_filled(
                                pos2(egui::lerp(sw.left() + 10.0..=sw.right() - 10.0, t), sw.center().y),
                                7.0,
                                if on { Color32::BLACK } else { Color32::from_gray(170) },
                            );
                            if resp.clicked() {
                                if on {
                                    self.picker.want.remove(&d.id);
                                } else {
                                    self.picker.want.insert(d.id.clone());
                                }
                            }
                        }
                        if shown.is_empty() {
                            ui.add_space(20.0);
                            ui.label(RichText::new("No outputs found.").color(DIM));
                        }
                    });
            });
        if !unplugged.is_empty() {
            ui.add_space(6.0);
            let t = if self.picker.show_unplugged {
                "Hide unplugged devices".to_string()
            } else {
                format!("Show {} unplugged device(s)", unplugged.len())
            };
            if flat(ui, RichText::new(t).size(12.0).color(DIM)).clicked() {
                self.picker.show_unplugged = !self.picker.show_unplugged;
            }
        }

        let install: Vec<String> = self
            .devices
            .iter()
            .filter(|d| !d.apo && self.picker.want.contains(&d.id))
            .map(|d| d.id.clone())
            .collect();
        let remove: Vec<String> = self
            .devices
            .iter()
            .filter(|d| d.apo && !self.picker.want.contains(&d.id))
            .map(|d| d.id.clone())
            .collect();
        let n = install.len() + remove.len();
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if flat(ui, RichText::new("Equalizer APO Device Selector").size(11.5).color(FAINT))
                .on_hover_text("The original tool, with troubleshooting options")
                .clicked()
                && let Some(a) = &self.apo
                && !sys::run_elevated(&a.device_selector())
            {
                self.notify("couldn't open the Device Selector", true);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = match n {
                    0 => "No changes".to_string(),
                    1 => "Apply 1 change".to_string(),
                    n => format!("Apply {n} changes"),
                };
                let apply = ui.add_enabled(
                    n > 0 && self.apo.is_some(),
                    egui::Button::new(RichText::new(label).color(Color32::BLACK).family(semibold()))
                        .fill(self.accent)
                        .corner_radius(8)
                        .min_size(vec2(0.0, 32.0)),
                );
                if apply.clicked() {
                    self.start_apo_setup(ctx, install, remove);
                }
                if ghost(ui, "Cancel").clicked() {
                    self.picker.open = false;
                }
            });
        });
        if self.apo.is_none() {
            ui.label(
                RichText::new("Install Equalizer APO first (Settings has a download link).")
                    .size(12.0)
                    .color(BAD),
            );
        }
    }

    fn picker_busy(&mut self, ui: &mut Ui) {
        heading(ui, "Setting things up…");
        ui.add_space(6.0);
        ui.label(
            RichText::new("Approve the Windows prompt. Sound restarts for a moment, then equinox checks each device really works.")
                .size(12.5)
                .color(DIM),
        );
        ui.add_space(14.0);
        for name in &self.picker.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(name).color(TEXT));
            });
        }
        ui.add_space(8.0);
    }

    fn picker_result(&mut self, ui: &mut Ui) {
        let result = self.picker.result.clone().unwrap();
        match &result {
            Err(e) => {
                heading(ui, "That didn't work");
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(BAD));
            }
            Ok(rep) => {
                let all_ok = rep.error.is_none()
                    && rep
                        .devices
                        .iter()
                        .all(|d| d.verified != Some(false) && !d.message.starts_with("couldn't"));
                heading(ui, if all_ok { "All set" } else { "Done, with a few notes" });
                ui.add_space(10.0);
                for d in &rep.devices {
                    let name = self.target_name(&equinox::model::Target::Device(d.guid.clone()));
                    let (glyph, col, text) = if !d.installed && d.message == "removed" {
                        ("−", DIM, "EQ removed".to_string())
                    } else if d.verified == Some(true) {
                        (
                            "✓",
                            GOOD,
                            format!("EQ working{}", d.mode.map(|m| format!("  ·  {}", m.label())).unwrap_or_default()),
                        )
                    } else if d.message.starts_with("couldn't") {
                        ("✕", BAD, d.message.clone())
                    } else {
                        ("!", WARN, d.message.clone())
                    };
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 10.0, col.gamma_multiply(0.18));
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            egui::FontId::new(13.0, semibold()),
                            col,
                        );
                        ui.vertical(|ui| {
                            ui.label(RichText::new(name).color(WHITE));
                            ui.label(RichText::new(text).size(12.0).color(DIM));
                        });
                    });
                    ui.add_space(4.0);
                }
                if let Some(e) = &rep.error {
                    ui.label(RichText::new(e).color(BAD));
                }
                if rep.reboot_suggested {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Restart Windows when convenient to make sure everything is picked up.")
                            .size(12.0)
                            .color(WARN),
                    );
                }
            }
        }
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary(ui, self.accent, "Done").clicked() {
                self.picker.open = false;
                self.picker.result = None;
            }
            if ghost(ui, "Back to devices").clicked() {
                self.open_picker(None);
            }
        });
    }
}
