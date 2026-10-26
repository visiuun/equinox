//! eqx: the equinox command line. Everything the app can do, scriptable.
//! Changes are saved to the same state file the app and tray agent watch,
//! so a running app updates live.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use equinox::apo;
use equinox::autoeq;
use equinox::devices::{self, AudioDevice};
use equinox::dsp;
use equinox::engine::{self, Resolved};
use equinox::eqtext;
use equinox::model::{Action, Band, Correction, FilterKind, Mode, Profile, State, Target, Tone, Trigger};
use equinox::sys;

const EXAMPLES: &str = "\
EXAMPLES
  eqx status                         what's applied where
  eqx use \"bass boost\"               switch profile (current device, or everywhere)
  eqx use vocal -d headphones        per-device: give the headphones their own profile
  eqx toggle                         A/B: bypass on/off (level-matched)
  eqx autoeq apply hd 600            find a headphone in AutoEq and correct it
  eqx strength 70                    blend the correction to 70%
  eqx tone --bass 3 --air -1         simple-mode sliders
  eqx band add pk 3000 -2.5 2        add a peak filter: type, Hz, dB, Q
  eqx curve                          draw the response in the terminal
  eqx import my.txt --correction     load an Equalizer APO / AutoEq / REW / CSV file

Devices (-d) match by name, GUID or 'default'; 'all' means the global chain.
";

#[derive(Parser)]
#[command(name = "eqx", version, about = "Control equinox, the system-wide equalizer", after_help = EXAMPLES, allow_negative_numbers = true)]
struct Cli {
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the active setup (default when no command is given).
    Status,
    /// List output devices.
    Devices,
    /// List profiles.
    Profiles,
    /// Switch to a profile.
    Use {
        profile: String,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Turn the EQ on.
    On,
    /// Bypass the EQ.
    Off,
    /// Flip between on and bypassed.
    Toggle,
    /// One sound everywhere, or a chain per device.
    Mode { mode: ModeArg },
    /// Preamp in dB, or "auto" to prevent clipping automatically.
    Preamp {
        #[arg(allow_hyphen_values = true)]
        value: String,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Left/right balance in dB (positive = right).
    Balance {
        #[arg(allow_hyphen_values = true)]
        db: f32,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Simple-mode tone sliders, in dB.
    Tone {
        #[arg(long, allow_hyphen_values = true)]
        bass: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        warmth: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        clarity: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        air: Option<f32>,
        /// Reset all four to 0.
        #[arg(long)]
        reset: bool,
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Edit parametric bands of a profile.
    Band {
        #[command(subcommand)]
        cmd: BandCmd,
        #[arg(short, long, global = true)]
        profile: Option<String>,
        #[arg(short, long, global = true)]
        device: Option<String>,
    },
    /// Create, copy, rename or delete profiles.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Import an EQ file as a new profile (or as a headphone correction).
    Import {
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Load as the device's headphone correction instead of a profile.
        #[arg(long)]
        correction: bool,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Export the full chain (or one profile) as an Equalizer APO / AutoEq text file.
    Export {
        #[arg(short, long)]
        profile: Option<String>,
        #[arg(short, long)]
        device: Option<String>,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Headphone correction from the AutoEq database.
    Autoeq {
        #[command(subcommand)]
        cmd: AutoEqCmd,
    },
    /// How much of the headphone correction to apply, 0-150 %.
    Strength {
        percent: f32,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Draw the frequency response in the terminal.
    Curve {
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Automation rules.
    Rules {
        #[command(subcommand)]
        cmd: Option<RuleCmd>,
    },
    /// Equalizer APO integration.
    Apo {
        #[command(subcommand)]
        cmd: ApoCmd,
    },
    /// Start the tray agent at sign-in.
    Startup { state: OnOff },
    /// The background tray agent (rules, hotkeys).
    Agent { action: AgentArg },
    /// Open the equinox window.
    Open,
    /// Re-write the Equalizer APO config from the saved state.
    Apply,
}

#[derive(Subcommand)]
enum BandCmd {
    /// List bands.
    List,
    /// Add a band: TYPE (pk, ls, hs, lp, hp, notch, bp), Hz, dB, Q.
    Add {
        kind: String,
        freq: f32,
        #[arg(allow_hyphen_values = true)]
        gain: Option<f32>,
        q: Option<f32>,
    },
    /// Change band N (1-based).
    Set {
        index: usize,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        freq: Option<f32>,
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<f32>,
        #[arg(long)]
        q: Option<f32>,
        #[arg(long)]
        on: bool,
        #[arg(long)]
        off: bool,
    },
    /// Remove band N (1-based).
    Rm { index: usize },
    /// Remove all bands.
    Clear,
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// New flat profile.
    New {
        name: String,
    },
    /// Copy a profile.
    Copy {
        from: String,
        name: Option<String>,
    },
    Rename {
        profile: String,
        name: String,
    },
    Rm {
        profile: String,
    },
}

#[derive(Subcommand)]
enum AutoEqCmd {
    /// Search headphones.
    Search {
        query: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 15)]
        limit: usize,
    },
    /// Apply the best match (or --pick N from the search list).
    Apply {
        query: Vec<String>,
        #[arg(long)]
        pick: Option<usize>,
        #[arg(short, long)]
        device: Option<String>,
        /// 0-150 %.
        #[arg(long)]
        strength: Option<f32>,
    },
    /// Remove the headphone correction.
    Clear {
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Guess headphone models from connected device names.
    Suggest,
    /// Re-download the AutoEq index.
    Refresh,
}

#[derive(Subcommand)]
enum RuleCmd {
    List,
    Enable { rule: String },
    Disable { rule: String },
    Rm { rule: String },
}

#[derive(Subcommand)]
enum ApoCmd {
    /// Installation and hook status.
    Status,
    /// Point Equalizer APO's config.txt at equinox (backs up the old one).
    Hook,
    /// Restore the config.txt from before equinox.
    Release,
    /// Download and install Equalizer APO itself, then enable EQ on the default output.
    Install,
    /// Enable EQ on devices (installs Equalizer APO on them; one admin prompt).
    Enable { devices: Vec<String> },
    /// Remove Equalizer APO from devices.
    Disable { devices: Vec<String> },
    /// Open Equalizer APO's own Device Selector.
    Selector,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Global,
    #[value(alias = "per-device", alias = "device", alias = "perdevice")]
    PerDevice,
}

#[derive(Clone, Copy, ValueEnum)]
enum AgentArg {
    Start,
    Stop,
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eqx: {e:#}");
            ExitCode::FAILURE
        }
    }
}

struct Ctx {
    state: State,
    devices: Vec<AudioDevice>,
    json: bool,
}

impl Ctx {
    /// Which chain a command acts on. No -d: the default device in per-device
    /// mode, else global. An explicit device gets its own chain (switching to
    /// per-device mode if needed).
    fn target(&mut self, device: &Option<String>, write: bool) -> Result<Target> {
        let Some(q) = device else {
            return Ok(match (self.state.mode, self.devices.iter().find(|d| d.default)) {
                (Mode::PerDevice, Some(d)) => Target::Device(d.id.clone()),
                _ => Target::Global,
            });
        };
        if matches!(q.to_lowercase().as_str(), "all" | "global" | "*") {
            return Ok(Target::Global);
        }
        let d = devices::find(&self.devices, q)
            .ok_or_else(|| anyhow!("no output device matches '{q}' (see `eqx devices`)"))?
            .clone();
        if write && !self.state.owns_chain(&Target::Device(d.id.clone())) {
            if self.state.mode == Mode::Global {
                eprintln!("note: switched to per-device mode; other devices keep the global sound");
            }
            self.state.detach_device(&d.id, &d.label());
        }
        Ok(Target::Device(d.id))
    }

    fn target_name(&self, t: &Target) -> String {
        match t {
            Target::Global => "all devices".into(),
            Target::Device(id) => self
                .devices
                .iter()
                .find(|d| &d.id == id)
                .map(|d| d.label())
                .or_else(|| self.state.device(id).map(|d| d.name.clone()))
                .unwrap_or_else(|| id.clone()),
        }
    }

    fn profile_arg(&mut self, profile: &Option<String>, device: &Option<String>) -> Result<u64> {
        match profile {
            Some(p) => self
                .state
                .find_profile(p)
                .ok_or_else(|| anyhow!("no profile matches '{p}' (see `eqx profiles`)")),
            None => {
                let t = self.target(device, true)?;
                Ok(self.state.ensure_profile(&t))
            }
        }
    }

    /// Save, push to Equalizer APO and warn about anything that keeps it from being heard.
    fn commit(&self) -> Result<()> {
        self.state.save().context("saving state")?;
        let ctx = engine::Context::capture(&self.state, &self.devices);
        let rep = engine::apply(&self.state, &self.devices, &ctx);
        if let Some(e) = rep.error {
            eprintln!("warning: {e}");
        } else if !rep.hooked {
            eprintln!("warning: Equalizer APO isn't loading equinox yet; run `eqx apo hook`");
        }
        if let Some(d) = self.devices.iter().find(|d| d.default)
            && !d.apo
        {
            eprintln!(
                "warning: Equalizer APO isn't installed on '{}'; run `eqx apo enable default`",
                d.label()
            );
        }
        Ok(())
    }

    fn print(&self, human: impl FnOnce() -> String, json: impl FnOnce() -> serde_json::Value) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(&json()).unwrap());
        } else {
            let s = human();
            if !s.is_empty() {
                println!("{}", s.trim_end());
            }
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut c = Ctx {
        state: State::load(),
        devices: devices::list(),
        json: cli.json,
    };
    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Status => status(&c),
        Cmd::Devices => {
            c.print(
                || {
                    let mut s = String::new();
                    for d in &c.devices {
                        s += &format!(
                            "{} {:<46} {:<8} {} {}\n",
                            if d.default { "▸" } else { " " },
                            d.label(),
                            if d.active { "active" } else { "unplugged" },
                            if d.apo { "APO ✓" } else { "APO ✗" },
                            d.id
                        );
                    }
                    s
                },
                || json!(c.devices.iter().map(|d| json!({"id": d.id, "name": d.label(), "active": d.active, "default": d.default, "apo": d.apo, "headphone": d.form.is_headphone()})).collect::<Vec<_>>()),
            );
            Ok(())
        }
        Cmd::Profiles => {
            let in_use: Vec<u64> = std::iter::once(&c.state.global)
                .chain(c.state.devices.iter().map(|d| &d.chain))
                .filter_map(|ch| ch.profile)
                .collect();
            c.print(
                || {
                    c.state
                        .profiles
                        .iter()
                        .map(|p| {
                            format!(
                                "{} {:>3}  {:<22} {}\n",
                                if in_use.contains(&p.id) { "●" } else { " " },
                                p.id,
                                p.name,
                                describe(p)
                            )
                        })
                        .collect()
                },
                || serde_json::to_value(&c.state.profiles).unwrap(),
            );
            Ok(())
        }
        Cmd::Use { profile, device } => {
            let id = c
                .state
                .find_profile(&profile)
                .ok_or_else(|| anyhow!("no profile matches '{profile}' (see `eqx profiles`)"))?;
            let t = c.target(&device, true)?;
            c.state.chain_mut(&t).profile = Some(id);
            c.state.bypass = false;
            c.commit()?;
            let name = c.state.profile(id).unwrap().name.clone();
            c.print(
                || format!("{name} → {}", c.target_name(&t)),
                || json!({"profile": name, "target": c.target_name(&t)}),
            );
            Ok(())
        }
        Cmd::On => set_bypass(&mut c, Some(false)),
        Cmd::Off => set_bypass(&mut c, Some(true)),
        Cmd::Toggle => set_bypass(&mut c, None),
        Cmd::Mode { mode } => {
            c.state.mode = match mode {
                ModeArg::Global => Mode::Global,
                ModeArg::PerDevice => Mode::PerDevice,
            };
            c.commit()?;
            c.print(
                || format!("mode: {}", mode_name(c.state.mode)),
                || json!({"mode": mode_name(c.state.mode)}),
            );
            Ok(())
        }
        Cmd::Preamp { value, device } => {
            let t = c.target(&device, true)?;
            let ch = c.state.chain_mut(&t);
            if value.eq_ignore_ascii_case("auto") {
                ch.auto_preamp = true;
            } else {
                ch.auto_preamp = false;
                ch.preamp = value
                    .trim_end_matches("dB")
                    .trim()
                    .parse::<f32>()
                    .context("preamp: a number in dB, or auto")?
                    .clamp(-30.0, 12.0);
            }
            c.commit()?;
            let r = engine::resolve(&c.state, c.state.chain(&t));
            c.print(
                || format!("preamp {} on {}", dsp::fmt_db(r.preamp), c.target_name(&t)),
                || json!({"preamp": r.preamp}),
            );
            Ok(())
        }
        Cmd::Balance { db, device } => {
            let t = c.target(&device, true)?;
            c.state.chain_mut(&t).balance = db.clamp(-12.0, 12.0);
            c.commit()?;
            c.print(
                || format!("balance {} on {}", dsp::fmt_db(db), c.target_name(&t)),
                || json!({"balance": db}),
            );
            Ok(())
        }
        Cmd::Tone {
            bass,
            warmth,
            clarity,
            air,
            reset,
            profile,
            device,
        } => {
            let id = c.profile_arg(&profile, &device)?;
            let p = c.state.profile_mut(id).unwrap();
            if reset {
                p.tone = Tone::default();
            }
            for (i, v) in [bass, warmth, clarity, air].into_iter().enumerate() {
                if let Some(v) = v {
                    *p.tone.get_mut(i) = v.clamp(-12.0, 12.0);
                }
            }
            let (name, tone) = (p.name.clone(), p.tone);
            c.commit()?;
            c.print(
                || {
                    format!(
                        "{name}: bass {:+.1}  warmth {:+.1}  clarity {:+.1}  air {:+.1}",
                        tone.bass, tone.warmth, tone.clarity, tone.air
                    )
                },
                || serde_json::to_value(tone).unwrap(),
            );
            Ok(())
        }
        Cmd::Band { cmd, profile, device } => band(&mut c, cmd, &profile, &device),
        Cmd::Profile { cmd } => profile_cmd(&mut c, cmd),
        Cmd::Import {
            file,
            name,
            correction,
            device,
        } => {
            let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let parsed = eqtext::parse(&text);
            for w in &parsed.warnings {
                eprintln!("warning: {w}");
            }
            if parsed.is_empty() {
                bail!("no filters or curve found in {}", file.display());
            }
            let stem = name.unwrap_or_else(|| file.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported").to_string());
            let t = c.target(&device, true)?;
            if correction || !parsed.graphic.is_empty() {
                c.state.chain_mut(&t).correction = Some(Correction {
                    name: stem.clone(),
                    source: "Imported".into(),
                    filters: parsed.bands,
                    graphic: parsed.graphic,
                });
                c.commit()?;
                c.print(
                    || format!("correction '{stem}' → {}", c.target_name(&t)),
                    || json!({"correction": stem}),
                );
            } else {
                let id = c.state.new_id();
                let name = c.state.unique_name(&stem);
                let n = parsed.bands.len();
                c.state.profiles.push(Profile {
                    id,
                    name: name.clone(),
                    tone: Tone::default(),
                    bands: parsed.bands,
                });
                c.state.chain_mut(&t).profile = Some(id);
                c.commit()?;
                c.print(
                    || format!("imported {n} filters as '{name}' → {}", c.target_name(&t)),
                    || json!({"profile": name, "id": id, "filters": n}),
                );
            }
            Ok(())
        }
        Cmd::Export { profile, device, out } => {
            let text = match &profile {
                Some(p) => {
                    let id = c.state.find_profile(p).ok_or_else(|| anyhow!("no profile matches '{p}'"))?;
                    let bands = c.state.profile(id).unwrap().filters();
                    eqtext::export(dsp::auto_preamp(&bands, &[], 0.0), &bands, &[])
                }
                None => {
                    let t = c.target(&device, false)?;
                    let r = engine::resolve(&c.state, c.state.chain(&t));
                    eqtext::export(r.preamp, &r.all(), &r.graphic)
                }
            };
            match out {
                Some(path) => {
                    std::fs::write(&path, &text)?;
                    eprintln!("wrote {}", path.display());
                }
                None => print!("{text}"),
            }
            Ok(())
        }
        Cmd::Autoeq { cmd } => autoeq_cmd(&mut c, cmd),
        Cmd::Strength { percent, device } => {
            let t = c.target(&device, true)?;
            let ch = c.state.chain_mut(&t);
            if ch.correction.is_none() {
                bail!("{} has no headphone correction (try `eqx autoeq apply <model>`)", c.target_name(&t));
            }
            ch.strength = (percent / 100.0).clamp(0.0, 1.5);
            c.commit()?;
            c.print(
                || format!("correction strength {percent:.0}% on {}", c.target_name(&t)),
                || json!({"strength": percent / 100.0}),
            );
            Ok(())
        }
        Cmd::Curve { device } => {
            let t = c.target(&device, false)?;
            let r = engine::resolve(&c.state, c.state.chain(&t));
            c.print(
                || ascii_curve(&r),
                || {
                    let f = dsp::log_freqs(64, dsp::F_MIN, dsp::F_MAX);
                    let g = dsp::response(&r.all(), &r.graphic, &f);
                    json!({"freqs": f, "db": g, "preamp": r.preamp})
                },
            );
            Ok(())
        }
        Cmd::Rules { cmd } => rules_cmd(&mut c, cmd.unwrap_or(RuleCmd::List)),
        Cmd::Apo { cmd } => apo_cmd(&c, cmd),
        Cmd::Startup { state } => {
            sys::set_autostart(matches!(state, OnOff::On)).context("updating the Run key")?;
            if matches!(state, OnOff::On) {
                sys::start_agent();
            }
            c.print(
                || format!("start at sign-in: {}", if sys::autostart_enabled() { "on" } else { "off" }),
                || json!({"startup": sys::autostart_enabled()}),
            );
            Ok(())
        }
        Cmd::Agent { action } => {
            match action {
                AgentArg::Start => {
                    if !c.state.settings.agent {
                        c.state.settings.agent = true;
                        c.state.save()?;
                    }
                    sys::start_agent();
                    std::thread::sleep(std::time::Duration::from_millis(600));
                }
                AgentArg::Stop => sys::stop_agent(),
                AgentArg::Status => {}
            }
            let running = sys::agent_running();
            c.print(
                || format!("agent: {}", if running { "running" } else { "stopped" }),
                || json!({"running": running}),
            );
            Ok(())
        }
        Cmd::Open => {
            sys::open_app();
            Ok(())
        }
        Cmd::Apply => {
            c.commit()?;
            c.print(|| "applied".into(), || json!({"applied": true}));
            Ok(())
        }
    }
}

fn set_bypass(c: &mut Ctx, bypass: Option<bool>) -> Result<()> {
    c.state.bypass = bypass.unwrap_or(!c.state.bypass);
    c.commit()?;
    let on = !c.state.bypass;
    c.print(|| (if on { "EQ on" } else { "EQ bypassed" }).into(), || json!({"enabled": on}));
    Ok(())
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Global => "global",
        Mode::PerDevice => "per-device",
    }
}

fn describe(p: &Profile) -> String {
    let mut parts = Vec::new();
    for i in 0..4 {
        let v = p.tone.get(i);
        if v.abs() >= 0.05 {
            parts.push(format!("{} {v:+.1}", Tone::NAMES[i].to_lowercase()));
        }
    }
    let n = p.bands.iter().filter(|b| b.enabled).count();
    if n > 0 {
        parts.push(format!("{n} band{}", if n == 1 { "" } else { "s" }));
    }
    if parts.is_empty() { "flat".into() } else { parts.join(", ") }
}

fn chain_summary(state: &State, ch: &equinox::model::Chain) -> (String, Resolved) {
    let r = engine::resolve(state, ch);
    let mut s = ch
        .profile
        .and_then(|id| state.profile(id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Flat".into());
    if let Some(cor) = &ch.correction {
        s += &format!(" + {} ({:.0}%)", cor.name, ch.strength * 100.0);
    }
    (s, r)
}

fn status(c: &Ctx) -> Result<()> {
    let ctx = engine::Context::capture(&c.state, &c.devices);
    let (eff, active) = engine::effective(&c.state, &ctx);
    let apo = apo::detect();
    let default = c.devices.iter().find(|d| d.default);
    let targets: Vec<(String, Target)> = match eff.mode {
        Mode::Global => vec![("All devices".into(), Target::Global)],
        Mode::PerDevice => {
            let mut v = vec![("Default (following)".to_string(), Target::Global)];
            v.extend(
                c.devices
                    .iter()
                    .filter(|d| d.active && eff.owns_chain(&Target::Device(d.id.clone())))
                    .map(|d| (d.label(), Target::Device(d.id.clone()))),
            );
            v
        }
    };
    let rules: Vec<String> = eff
        .rules
        .iter()
        .filter(|r| active.contains(&r.id))
        .map(|r| r.name.clone())
        .collect();
    c.print(
        || {
            let mut s = format!(
                "equinox  {}  ·  {} mode\n",
                if eff.bypass { "○ bypassed" } else { "● on" },
                mode_name(eff.mode)
            );
            s += &match &apo {
                Some(a) if a.hooked() => format!("Equalizer APO {} · loading equinox ✓\n", a.version),
                Some(a) => format!("Equalizer APO {} · NOT loading equinox (run `eqx apo hook`)\n", a.version),
                None => "Equalizer APO not installed: https://sourceforge.net/projects/equalizerapo/\n".into(),
            };
            if let Some(d) = default {
                s += &format!("Output: {}{}\n", d.label(), if d.apo { "" } else { "  (EQ not enabled there: `eqx apo enable default`)" });
            }
            s.push('\n');
            for (name, t) in &targets {
                let (desc, r) = chain_summary(&eff, eff.chain(t));
                let mark = match t {
                    Target::Device(id) if default.is_some_and(|d| &d.id == id) => "▸",
                    Target::Global if eff.mode == Mode::Global => "▸",
                    _ => " ",
                };
                s += &format!("{mark} {name:<34} {desc:<34} preamp {}\n", dsp::fmt_db(r.preamp));
            }
            if !rules.is_empty() {
                s += &format!("\nActive rules: {}\n", rules.join(", "));
            }
            s
        },
        || {
            json!({
                "enabled": !eff.bypass,
                "mode": mode_name(eff.mode),
                "apo": apo.as_ref().map(|a| json!({"version": a.version, "hooked": a.hooked(), "config": a.config})),
                "default_device": default.map(|d| json!({"id": d.id, "name": d.label(), "apo": d.apo})),
                "targets": targets.iter().map(|(n, t)| {
                    let ch = eff.chain(t);
                    let (desc, r) = chain_summary(&eff, ch);
                    json!({"name": n, "summary": desc, "profile": ch.profile, "correction": ch.correction.as_ref().map(|c| &c.name), "strength": ch.strength, "preamp": r.preamp, "peak": r.peak})
                }).collect::<Vec<_>>(),
                "active_rules": rules,
            })
        },
    );
    Ok(())
}

fn band(c: &mut Ctx, cmd: BandCmd, profile: &Option<String>, device: &Option<String>) -> Result<()> {
    let id = c.profile_arg(profile, device)?;
    let p = c.state.profile_mut(id).unwrap();
    let pick = |p: &Profile, i: usize| -> Result<usize> {
        if i == 0 || i > p.bands.len() {
            bail!("{} has {} band(s); use 1..={}", p.name, p.bands.len(), p.bands.len());
        }
        Ok(i - 1)
    };
    let kind = |s: &str| FilterKind::from_name(s).ok_or_else(|| anyhow!("unknown filter type '{s}' (pk, ls, hs, lp, hp, notch, bp)"));
    match cmd {
        BandCmd::List => {}
        BandCmd::Add { kind: k, freq, gain, q } => {
            let k = kind(&k)?;
            let mut b = Band::new(
                k,
                freq,
                gain.unwrap_or(0.0),
                q.unwrap_or(if k.has_gain() && k != FilterKind::Peak { 0.707 } else { 1.0 }),
            );
            b.clamp();
            p.bands.push(b);
        }
        BandCmd::Set {
            index,
            kind: k,
            freq,
            gain,
            q,
            on,
            off,
        } => {
            let i = pick(p, index)?;
            let b = &mut p.bands[i];
            if let Some(k) = k {
                b.kind = kind(&k)?;
            }
            if let Some(v) = freq {
                b.freq = v;
            }
            if let Some(v) = gain {
                b.gain = v;
            }
            if let Some(v) = q {
                b.q = v;
            }
            if on {
                b.enabled = true;
            }
            if off {
                b.enabled = false;
            }
            b.clamp();
        }
        BandCmd::Rm { index } => {
            let i = pick(p, index)?;
            p.bands.remove(i);
        }
        BandCmd::Clear => p.bands.clear(),
    }
    let p = c.state.profile(id).unwrap().clone();
    c.commit()?;
    c.print(
        || {
            let mut s = format!("{}  ({})\n", p.name, describe(&p));
            for (i, b) in p.bands.iter().enumerate() {
                s += &format!(
                    "{:>3}  {:<10} {:>9}  {:>8}  Q {:<5.2}{}\n",
                    i + 1,
                    b.kind.label(),
                    dsp::fmt_freq(b.freq),
                    if b.kind.has_gain() { dsp::fmt_db(b.gain) } else { "-".into() },
                    b.q,
                    if b.enabled { "" } else { "  off" }
                );
            }
            s
        },
        || serde_json::to_value(&p).unwrap(),
    );
    Ok(())
}

fn profile_cmd(c: &mut Ctx, cmd: ProfileCmd) -> Result<()> {
    let find = |s: &State, q: &str| s.find_profile(q).ok_or_else(|| anyhow!("no profile matches '{q}'"));
    let msg = match cmd {
        ProfileCmd::New { name } => {
            let id = c.state.new_id();
            let name = c.state.unique_name(&name);
            c.state.profiles.push(Profile {
                id,
                name: name.clone(),
                tone: Tone::default(),
                bands: vec![],
            });
            format!("created '{name}'")
        }
        ProfileCmd::Copy { from, name } => {
            let src = c.state.profile(find(&c.state, &from)?).unwrap().clone();
            let id = c.state.new_id();
            let name = c.state.unique_name(&name.unwrap_or_else(|| format!("{} copy", src.name)));
            c.state.profiles.push(Profile {
                id,
                name: name.clone(),
                ..src
            });
            format!("copied to '{name}'")
        }
        ProfileCmd::Rename { profile, name } => {
            let id = find(&c.state, &profile)?;
            c.state.profile_mut(id).unwrap().name = name.clone();
            format!("renamed to '{name}'")
        }
        ProfileCmd::Rm { profile } => {
            let id = find(&c.state, &profile)?;
            let name = c.state.profile(id).unwrap().name.clone();
            c.state.delete_profile(id);
            format!("deleted '{name}'")
        }
    };
    c.commit()?;
    c.print(|| msg.clone(), || json!({"ok": true, "message": msg}));
    Ok(())
}

fn autoeq_cmd(c: &mut Ctx, cmd: AutoEqCmd) -> Result<()> {
    match cmd {
        AutoEqCmd::Search { query, limit } => {
            let idx = autoeq::load_index(false)?;
            let hits = autoeq::search(&idx, &query.join(" "), limit);
            c.print(
                || {
                    hits.iter()
                        .enumerate()
                        .map(|(i, e)| format!("{:>3}  {:<40} {}\n", i + 1, e.name, e.source_label()))
                        .collect()
                },
                || {
                    json!(
                        hits.iter()
                            .map(|e| json!({"name": e.name, "source": e.source, "rig": e.rig}))
                            .collect::<Vec<_>>()
                    )
                },
            );
            if hits.is_empty() && !c.json {
                eprintln!("no matches");
            }
            Ok(())
        }
        AutoEqCmd::Apply {
            query,
            pick,
            device,
            strength,
        } => {
            let idx = autoeq::load_index(false)?;
            let q = query.join(" ");
            let hits = autoeq::search(&idx, &q, 50);
            let e = match pick {
                Some(n) => hits
                    .get(n.saturating_sub(1))
                    .ok_or_else(|| anyhow!("--pick {n}: only {} results", hits.len()))?,
                None => hits.first().ok_or_else(|| anyhow!("no AutoEq profile matches '{q}'"))?,
            };
            let cor = autoeq::fetch(e)?;
            let t = c.target(&device, true)?;
            let ch = c.state.chain_mut(&t);
            ch.correction = Some(cor.clone());
            if let Some(s) = strength {
                ch.strength = (s / 100.0).clamp(0.0, 1.5);
            }
            c.commit()?;
            c.print(
                || {
                    format!(
                        "{} ({}) → {}, {} filters",
                        cor.name,
                        e.source_label(),
                        c.target_name(&t),
                        cor.filters.len()
                    )
                },
                || json!({"name": cor.name, "source": cor.source, "filters": cor.filters.len(), "target": c.target_name(&t)}),
            );
            Ok(())
        }
        AutoEqCmd::Clear { device } => {
            let t = c.target(&device, true)?;
            c.state.chain_mut(&t).correction = None;
            c.commit()?;
            c.print(|| format!("correction removed from {}", c.target_name(&t)), || json!({"ok": true}));
            Ok(())
        }
        AutoEqCmd::Suggest => {
            let idx = autoeq::load_index(false)?;
            let found: Vec<(String, String)> = c
                .devices
                .iter()
                .filter_map(|d| autoeq::suggest(&idx, &d.label()).map(|e| (d.label(), format!("{} ({})", e.name, e.source_label()))))
                .collect();
            c.print(
                || {
                    if found.is_empty() {
                        "no headphone models recognised in device names".into()
                    } else {
                        found.iter().map(|(d, e)| format!("{d}  →  {e}\n")).collect()
                    }
                },
                || json!(found.iter().map(|(d, e)| json!({"device": d, "match": e})).collect::<Vec<_>>()),
            );
            Ok(())
        }
        AutoEqCmd::Refresh => {
            let n = autoeq::load_index(true)?.len();
            c.print(|| format!("AutoEq index: {n} profiles"), || json!({"profiles": n}));
            Ok(())
        }
    }
}

fn find_rule(s: &State, q: &str) -> Result<usize> {
    let q = q.to_lowercase();
    s.rules
        .iter()
        .position(|r| r.id.to_string() == q || r.name.to_lowercase() == q)
        .or_else(|| s.rules.iter().position(|r| r.name.to_lowercase().contains(&q)))
        .ok_or_else(|| anyhow!("no rule matches '{q}'"))
}

fn rules_cmd(c: &mut Ctx, cmd: RuleCmd) -> Result<()> {
    match cmd {
        RuleCmd::List => {
            let ctx = engine::Context::capture(&c.state, &c.devices);
            let (_, active) = engine::effective(&c.state, &ctx);
            c.print(
                || {
                    if c.state.rules.is_empty() {
                        return "no rules yet (add them in the app's Automation tab)".into();
                    }
                    c.state
                        .rules
                        .iter()
                        .map(|r| {
                            format!(
                                "{} {:>3}  {:<22} {}{}\n",
                                if active.contains(&r.id) { "●" } else { " " },
                                r.id,
                                r.name,
                                rule_text(&c.state, &c.devices, r),
                                if r.enabled { "" } else { "  (disabled)" }
                            )
                        })
                        .collect()
                },
                || serde_json::to_value(&c.state.rules).unwrap(),
            );
            Ok(())
        }
        RuleCmd::Enable { rule } => set_rule(c, &rule, Some(true)),
        RuleCmd::Disable { rule } => set_rule(c, &rule, Some(false)),
        RuleCmd::Rm { rule } => set_rule(c, &rule, None),
    }
}

fn set_rule(c: &mut Ctx, rule: &str, enable: Option<bool>) -> Result<()> {
    let i = find_rule(&c.state, rule)?;
    let name = c.state.rules[i].name.clone();
    match enable {
        Some(on) => c.state.rules[i].enabled = on,
        None => {
            c.state.rules.remove(i);
        }
    }
    c.commit()?;
    let msg = match enable {
        Some(true) => format!("enabled '{name}'"),
        Some(false) => format!("disabled '{name}'"),
        None => format!("removed '{name}'"),
    };
    c.print(|| msg.clone(), || json!({"ok": true}));
    Ok(())
}

pub fn rule_text(s: &State, devices: &[AudioDevice], r: &equinox::model::Rule) -> String {
    let dev_name = |id: &str| {
        devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.label())
            .or_else(|| s.device(id).map(|d| d.name.clone()))
            .unwrap_or_else(|| id.to_string())
    };
    let when = match &r.trigger {
        Trigger::DefaultDevice { device } => format!("when output is {}", dev_name(device)),
        Trigger::TimeRange { start, end } => format!("from {start} to {end}"),
        Trigger::AppRunning { exe } => format!("while {exe} runs"),
    };
    let target = |t: &Target| match t {
        Target::Global => "all devices".to_string(),
        Target::Device(id) => dev_name(id),
    };
    let what = match &r.action {
        Action::UseProfile { target: t, profile } => {
            format!(
                "use {} on {}",
                s.profile(*profile).map(|p| p.name.as_str()).unwrap_or("?"),
                target(t)
            )
        }
        Action::Bypass { on } => (if *on { "bypass EQ" } else { "force EQ on" }).into(),
        Action::Strength { target: t, value } => format!("correction {:.0}% on {}", value * 100.0, target(t)),
    };
    format!("{when}: {what}")
}

fn apo_cmd(c: &Ctx, cmd: ApoCmd) -> Result<()> {
    if let ApoCmd::Install = cmd {
        if apo::detect().is_some() {
            println!("Equalizer APO is already installed");
            return Ok(());
        }
        eprintln!("Downloading Equalizer APO…");
        let path = equinox::apo_setup::download_equalizer_apo().map_err(|e| anyhow!(e))?;
        eprintln!("Approve the Windows prompt; installing…");
        let dev: Vec<String> = c.devices.iter().filter(|d| d.default && d.active).map(|d| d.id.clone()).collect();
        let rep = equinox::apo_setup::run_elevated(&dev, &[], Some(&path)).map_err(|e| anyhow!(e))?;
        if let Some(e) = rep.error {
            bail!(e);
        }
        if let Some(a) = apo::detect() {
            a.take_control().context("pointing Equalizer APO at equinox")?;
        }
        c.commit()?;
        println!("Equalizer APO installed and EQ enabled on the default output");
        return Ok(());
    }
    let a = apo::detect()
        .ok_or_else(|| anyhow!("Equalizer APO isn't installed. Get it from https://sourceforge.net/projects/equalizerapo/"))?;
    match cmd {
        ApoCmd::Status => {
            let foreign = a.foreign_lines();
            let on: Vec<&AudioDevice> = c.devices.iter().filter(|d| d.apo).collect();
            c.print(
                || {
                    let mut s = format!("Equalizer APO {}  ({})\n", a.version, a.install.display());
                    s += &format!("config: {}\n", a.config.display());
                    s += &format!("loading equinox: {}\n", if a.hooked() { "yes" } else { "no (`eqx apo hook`)" });
                    if !foreign.is_empty() {
                        s += &format!("other config lines also active (they stack with equinox): {}\n", foreign.join(" | "));
                    }
                    s += &format!(
                        "installed on: {}\n",
                        if on.is_empty() { "no current device (`eqx apo enable <device>`)".to_string() } else { on.iter().map(|d| d.label()).collect::<Vec<_>>().join(", ") }
                    );
                    s
                },
                || json!({"version": a.version, "hooked": a.hooked(), "config": a.config, "foreign": foreign, "devices": on.iter().map(|d| &d.id).collect::<Vec<_>>()}),
            );
        }
        ApoCmd::Hook => {
            a.take_control().context("writing config.txt")?;
            c.commit()?;
            println!("Equalizer APO now loads equinox (old config.txt backed up)");
        }
        ApoCmd::Release => {
            a.release().context("restoring config.txt")?;
            println!("restored your previous Equalizer APO config");
        }
        // Handled before Equalizer APO is required.
        ApoCmd::Install => {}
        ApoCmd::Selector => {
            if !sys::run_elevated(&a.device_selector()) {
                bail!("couldn't start {}", a.device_selector().display());
            }
        }
        ApoCmd::Enable { devices } | ApoCmd::Disable { devices } if devices.is_empty() => {
            let _ = devices;
            bail!("name the device(s), e.g. `eqx apo enable default` (see `eqx devices`)");
        }
        ApoCmd::Enable { devices: names } => apo_setup_cmd(c, &names, true)?,
        ApoCmd::Disable { devices: names } => apo_setup_cmd(c, &names, false)?,
    }
    Ok(())
}

fn apo_setup_cmd(c: &Ctx, names: &[String], enable: bool) -> Result<()> {
    let mut ids = Vec::new();
    for n in names {
        let d = devices::find(&c.devices, n).ok_or_else(|| anyhow!("no output device matches '{n}'"))?;
        ids.push(d.id.clone());
    }
    eprintln!("Approve the Windows prompt; sound restarts for a moment…");
    let (install, remove) = if enable { (ids, vec![]) } else { (vec![], ids) };
    let rep = equinox::apo_setup::run_elevated(&install, &remove, None).map_err(|e| anyhow!(e))?;
    if enable && rep.devices.iter().any(|d| d.installed) && apo::detect().is_some_and(|a| !a.hooked()) {
        apo::detect().unwrap().take_control().context("pointing Equalizer APO at equinox")?;
    }
    c.commit()?;
    c.print(
        || {
            let mut s = String::new();
            for d in &rep.devices {
                let name = c
                    .devices
                    .iter()
                    .find(|x| x.id == d.guid)
                    .map(|x| x.label())
                    .unwrap_or_else(|| d.guid.clone());
                let mark = match (d.installed, d.verified) {
                    (_, Some(true)) => "✓",
                    (false, _) if d.message == "removed" => "−",
                    (_, Some(false)) => "!",
                    _ if d.message.starts_with("couldn't") => "✕",
                    _ => "·",
                };
                let mode = d.mode.map(|m| format!(" ({})", m.label())).unwrap_or_default();
                s += &format!(
                    "{mark} {name}: {}{mode}
",
                    d.message
                );
            }
            if let Some(e) = &rep.error {
                s += &format!(
                    "error: {e}
"
                );
            }
            if rep.reboot_suggested {
                s += "Restart Windows when convenient to make sure everything is picked up.
";
            }
            s
        },
        || serde_json::to_value(&rep).unwrap(),
    );
    Ok(())
}

/// Response drawn with braille-free ASCII so it works in any console font.
fn ascii_curve(r: &Resolved) -> String {
    const W: usize = 64;
    const H: usize = 15;
    let freqs = dsp::log_freqs(W, dsp::F_MIN, dsp::F_MAX);
    let db = dsp::response(&r.all(), &r.graphic, &freqs);
    let span = db.iter().fold(6.0f32, |m, v| m.max(v.abs())).ceil();
    let row = |v: f32| (((span - v) / (2.0 * span)) * (H - 1) as f32).round().clamp(0.0, (H - 1) as f32) as usize;
    let zero = row(0.0);
    let mut grid = vec![vec![' '; W]; H];
    grid[zero].fill('·');
    for (x, &v) in db.iter().enumerate() {
        let y = row(v);
        grid[y][x] = '●';
        // Fill toward zero for a readable shape.
        let (a, b) = if y < zero { (y + 1, zero) } else { (zero + 1, y) };
        for line in &mut grid[a..b] {
            if line[x] == ' ' {
                line[x] = '│';
            }
        }
    }
    let mut s = String::new();
    for (i, line) in grid.iter().enumerate() {
        let label = if i == 0 {
            format!("{:+3.0} dB", span)
        } else if i == zero {
            "  0 dB".into()
        } else if i == H - 1 {
            format!("{:+3.0} dB", -span)
        } else {
            String::new()
        };
        s += &format!("{label:>7} {}\n", line.iter().collect::<String>());
    }
    let mut axis = vec![' '; W + 4];
    for (f, l) in [
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
    ] {
        let x = ((f as f32 / dsp::F_MIN).ln() / (dsp::F_MAX / dsp::F_MIN).ln() * (W - 1) as f32).round() as usize;
        let start = x.saturating_sub(l.len() / 2).min(W + 4 - l.len());
        for (i, ch) in l.chars().enumerate() {
            axis[start + i] = ch;
        }
    }
    s += &format!("        {}\n", axis.iter().collect::<String>().trim_end());
    s += &format!("        preamp {}   peak {}\n", dsp::fmt_db(r.preamp), dsp::fmt_db(r.peak));
    s
}
