//! State -> Equalizer APO config. Rules are evaluated here as temporary
//! overrides, so the GUI, the CLI and the tray agent all render the same text.

use std::collections::HashSet;

use crate::apo::{self, Apo};
use crate::devices::AudioDevice;
use crate::dsp;
use crate::eqtext;
use crate::model::{Action, Band, Chain, Mode, Rule, State, Target, Trigger};

/// The live facts rules are checked against.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub default_device: Option<String>,
    pub processes: HashSet<String>,
    pub minutes: u32,
}

impl Context {
    /// Only snapshots processes when a rule needs them.
    pub fn capture(state: &State, devices: &[AudioDevice]) -> Context {
        let needs_procs = state
            .rules
            .iter()
            .any(|r| r.enabled && matches!(r.trigger, Trigger::AppRunning { .. }));
        Context {
            default_device: devices.iter().find(|d| d.default).map(|d| d.id.clone()),
            processes: if needs_procs { crate::sys::processes() } else { HashSet::new() },
            minutes: crate::sys::local_minutes(),
        }
    }
}

pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn rule_active(rule: &Rule, ctx: &Context) -> bool {
    match &rule.trigger {
        Trigger::DefaultDevice { device } => ctx.default_device.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(device)),
        Trigger::TimeRange { start, end } => match (parse_hhmm(start), parse_hhmm(end)) {
            (Some(s), Some(e)) if s <= e => (s..e).contains(&ctx.minutes),
            (Some(s), Some(e)) => ctx.minutes >= s || ctx.minutes < e,
            _ => false,
        },
        Trigger::AppRunning { exe } => {
            let exe = exe.trim().to_lowercase();
            let exe = if exe.ends_with(".exe") { exe } else { format!("{exe}.exe") };
            ctx.processes.contains(&exe)
        }
    }
}

/// State with every active rule applied in order (later rules win), plus their ids.
pub fn effective(state: &State, ctx: &Context) -> (State, Vec<u64>) {
    let mut s = state.clone();
    let mut active = Vec::new();
    for r in state.rules.iter().filter(|r| r.enabled && rule_active(r, ctx)) {
        active.push(r.id);
        match &r.action {
            Action::UseProfile { target, profile } => {
                if s.profile(*profile).is_some() {
                    s.chain_mut(target).profile = Some(*profile);
                }
            }
            Action::Bypass { on } => s.bypass = *on,
            Action::Strength { target, value } => s.chain_mut(target).strength = value.clamp(0.0, 1.5),
        }
    }
    (s, active)
}

/// A chain flattened to filters, with its preamp worked out.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub correction: Vec<Band>,
    pub graphic: Vec<(f32, f32)>,
    pub profile: Vec<Band>,
    pub preamp: f32,
    /// Highest boost of the combined curve, before preamp.
    pub peak: f32,
    pub balance: f32,
}

impl Resolved {
    pub fn all(&self) -> Vec<Band> {
        self.correction.iter().chain(&self.profile).cloned().collect()
    }

    /// Positive when the output can clip.
    pub fn clip_margin(&self) -> f32 {
        self.peak + self.preamp
    }
}

pub fn resolve(state: &State, chain: &Chain) -> Resolved {
    let (correction, graphic) = chain.correction.as_ref().map(|c| c.scaled(chain.strength)).unwrap_or_default();
    let profile = chain
        .profile
        .and_then(|id| state.profile(id))
        .map(|p| p.filters())
        .unwrap_or_default();
    let all: Vec<Band> = correction.iter().chain(&profile).cloned().collect();
    let peak = dsp::peak_db(&all, &graphic);
    let preamp = if chain.auto_preamp {
        dsp::preamp_for_peak(peak, state.settings.headroom)
    } else {
        chain.preamp
    };
    Resolved {
        correction,
        graphic,
        profile,
        preamp,
        peak,
        balance: chain.balance,
    }
}

pub fn render(state: &State, devices: &[AudioDevice]) -> String {
    let mut s = String::from("# Written by equinox. Changes here are overwritten: use the app or `eqx`.\n");
    if state.bypass {
        s.push_str("# Bypassed\n");
    }
    match state.mode {
        Mode::Global => emit(&mut s, state, &state.global),
        Mode::PerDevice => {
            // Devices with their own chain get a section each; the rest share one.
            let (own, follow): (Vec<&AudioDevice>, Vec<&AudioDevice>) =
                devices.iter().partition(|d| state.owns_chain(&Target::Device(d.id.clone())));
            for d in own {
                s.push_str(&format!("\n# {}\nDevice: {}\n", d.label(), d.id));
                emit(&mut s, state, state.chain(&Target::Device(d.id.clone())));
            }
            if !follow.is_empty() {
                let ids: Vec<&str> = follow.iter().map(|d| d.id.as_str()).collect();
                s.push_str(&format!("\n# Every other device\nDevice: {}\n", ids.join("; ")));
                emit(&mut s, state, &state.global);
            }
        }
    }
    s
}

fn emit(s: &mut String, state: &State, chain: &Chain) {
    let r = resolve(state, chain);
    if state.bypass {
        // Level-matched bypass keeps the gain change so A/B isn't won by loudness.
        if state.settings.level_matched_bypass && r.preamp.abs() >= 0.05 {
            s.push_str(&format!("Preamp: {:.1} dB\n", r.preamp));
        }
        return;
    }
    if r.preamp.abs() >= 0.05 {
        s.push_str(&format!("Preamp: {:.1} dB\n", r.preamp));
    }
    if r.balance.abs() >= 0.05 {
        let (side, cut) = if r.balance > 0.0 { ("L", r.balance) } else { ("R", -r.balance) };
        s.push_str(&format!("Channel: {side}\nPreamp: {:.1} dB\nChannel: all\n", -cut));
    }
    for b in r.correction.iter().chain(&r.profile) {
        s.push_str(&format!("Filter: {}\n", eqtext::filter_body(b)));
    }
    if !r.graphic.is_empty() {
        s.push_str(&eqtext::graphic_line(&r.graphic));
        s.push('\n');
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub apo: Option<Apo>,
    pub hooked: bool,
    pub written: bool,
    pub active_rules: Vec<u64>,
    pub error: Option<String>,
}

/// Evaluate rules, render and write the Equalizer APO file (only if it changed).
pub fn apply(state: &State, devices: &[AudioDevice], ctx: &Context) -> Report {
    let (eff, active_rules) = effective(state, ctx);
    let mut rep = Report {
        active_rules,
        ..Default::default()
    };
    let Some(a) = apo::detect() else {
        rep.error = Some("Equalizer APO is not installed".into());
        return rep;
    };
    rep.hooked = a.hooked();
    match a.write(&render(&eff, devices)) {
        Ok(w) => rep.written = w,
        Err(e) => rep.error = Some(format!("can't write {}: {e}", a.our_file().display())),
    }
    rep.apo = Some(a);
    rep
}

/// One-shot for the CLI and tray: fresh devices and context, then apply.
pub fn apply_now(state: &State) -> Report {
    let devices = crate::devices::list();
    let ctx = Context::capture(state, &devices);
    apply(state, &devices, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Band, FilterKind};

    fn dev(id: &str) -> AudioDevice {
        AudioDevice {
            id: id.into(),
            name: "Headphones".into(),
            interface: "Test".into(),
            active: true,
            form: crate::devices::Form::Headphones,
            apo: true,
            default: true,
        }
    }

    #[test]
    fn renders_global_with_preamp() {
        let mut s = State::default();
        let id = s.find_profile("bass boost").unwrap();
        s.global.profile = Some(id);
        let text = render(&s, &[]);
        assert!(text.contains("Preamp: -"), "{text}");
        assert!(text.contains("Filter: ON LSC Fc 105.0 Hz Gain 6.00 dB"), "{text}");
    }

    #[test]
    fn per_device_and_bypass() {
        let mut s = State::default();
        s.detach_device("{abc}", "Headphones");
        s.device_mut("{abc}", "").chain.profile = s.find_profile("bright");
        let text = render(&s, &[dev("{abc}")]);
        assert!(text.contains("Device: {abc}"));
        assert!(text.contains("HSC"));
        s.bypass = true;
        let text = render(&s, &[dev("{abc}")]);
        assert!(!text.contains("Filter:"));
        assert!(text.contains("Preamp:"), "level-matched bypass keeps preamp: {text}");
    }

    #[test]
    fn rules_override() {
        let mut s = State::default();
        let night = s.find_profile("late night").unwrap();
        let id = s.new_id();
        s.rules.push(Rule {
            id,
            name: "night".into(),
            enabled: true,
            trigger: Trigger::TimeRange {
                start: "22:00".into(),
                end: "07:00".into(),
            },
            action: Action::UseProfile {
                target: Target::Global,
                profile: night,
            },
        });
        let ctx = Context {
            minutes: 23 * 60,
            ..Default::default()
        };
        let (eff, active) = effective(&s, &ctx);
        assert_eq!(active, vec![id]);
        assert_eq!(eff.global.profile, Some(night));
        let ctx = Context {
            minutes: 12 * 60,
            ..Default::default()
        };
        assert!(effective(&s, &ctx).1.is_empty());
    }

    #[test]
    fn correction_strength_scales() {
        let mut s = State::default();
        s.global.profile = s.find_profile("flat");
        s.global.correction = Some(crate::model::Correction {
            name: "x".into(),
            source: String::new(),
            filters: vec![Band::new(FilterKind::Peak, 1000.0, 6.0, 1.0)],
            graphic: vec![],
        });
        s.global.strength = 0.5;
        let r = resolve(&s, &s.global);
        assert!((r.correction[0].gain - 3.0).abs() < 1e-6);
        assert!((r.preamp + 3.0).abs() < 0.11);
    }
}
