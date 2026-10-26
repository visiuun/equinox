//! Reading and writing EQ text: Equalizer APO / AutoEq `ParametricEQ.txt`,
//! REW filter exports, `GraphicEQ:` lines and plain "Hz, dB" CSV curves.

use crate::dsp::bw_to_q;
use crate::model::{Band, FilterKind};

#[derive(Debug, Default)]
pub struct Parsed {
    pub preamp: Option<f32>,
    pub bands: Vec<Band>,
    pub graphic: Vec<(f32, f32)>,
    pub warnings: Vec<String>,
}

impl Parsed {
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty() && self.graphic.is_empty()
    }
}

pub fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut csv: Vec<(f32, f32)> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            if let Some(p) = csv_pair(line) {
                csv.push(p);
            }
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key == "preamp" {
            out.preamp = rest.split_whitespace().next().and_then(num);
        } else if key == "graphiceq" {
            for pair in rest.split(';') {
                let mut it = pair.split_whitespace().filter_map(num);
                if let (Some(f), Some(g)) = (it.next(), it.next()) {
                    out.graphic.push((f, g));
                }
            }
        } else if key.starts_with("filter") {
            match parse_filter(rest) {
                Ok(Some(b)) => out.bands.push(b),
                Ok(None) => {}
                Err(e) => out.warnings.push(format!("line {}: {e}", n + 1)),
            }
        }
    }
    if out.is_empty() && csv.len() >= 2 {
        out.graphic = csv;
    }
    out.graphic.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.graphic.dedup_by(|a, b| a.0 == b.0);
    out
}

/// "ON PK Fc 105 Hz Gain 6.5 dB Q 0.70" -> Band. `Ok(None)` for "None"/"OFF" slots.
fn parse_filter(rest: &str) -> Result<Option<Band>, String> {
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let mut i = 0;
    let mut enabled = true;
    if let Some(t) = toks.first() {
        match t.to_ascii_uppercase().as_str() {
            "ON" => i = 1,
            "OFF" => {
                enabled = false;
                i = 1;
            }
            _ => {}
        }
    }
    let Some(code) = toks.get(i) else { return Ok(None) };
    if code.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let Some(kind) = FilterKind::from_name(code) else {
        return Err(format!("unsupported filter type {code}"));
    };
    i += 1;
    // "LS 6dB" / "HS 12dB" variants: slope instead of Q.
    let mut q: Option<f32> = None;
    if let Some(t) = toks.get(i)
        && t.to_ascii_lowercase().ends_with("db")
        && !t.eq_ignore_ascii_case("db")
    {
        let slope = num(&t[..t.len() - 2]).unwrap_or(12.0);
        q = Some(if slope <= 6.0 { 0.5 } else { 0.707 });
        i += 1;
    }
    let (mut freq, mut gain) = (None, 0.0);
    while i < toks.len() {
        let k = toks[i].to_ascii_lowercase();
        let v = toks.get(i + 1).copied().and_then(num);
        match k.as_str() {
            "fc" => freq = v,
            "gain" => gain = v.unwrap_or(0.0),
            "q" => q = v,
            "bw" => {
                // "BW Oct 1.0"
                let v = if toks.get(i + 1).is_some_and(|t| t.eq_ignore_ascii_case("oct")) {
                    i += 1;
                    toks.get(i + 1).copied().and_then(num)
                } else {
                    v
                };
                q = v.map(bw_to_q);
            }
            _ => {
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let Some(freq) = freq else { return Err("missing Fc".into()) };
    let q = q.unwrap_or(match kind {
        FilterKind::Peak => 1.0,
        FilterKind::Notch | FilterKind::BandPass => 5.0,
        _ => 0.707,
    });
    let mut b = Band {
        kind,
        freq,
        gain,
        q,
        enabled,
    };
    b.clamp();
    Ok(Some(b))
}

fn num(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches(',')
        .replace(',', ".")
        .parse()
        .ok()
        .filter(|v: &f32| v.is_finite())
}

fn csv_pair(line: &str) -> Option<(f32, f32)> {
    let mut it = line.split([',', ';', '\t', ' ']).filter(|s| !s.is_empty());
    let f: f32 = it.next()?.trim().parse().ok()?;
    let g: f32 = it.next()?.trim().parse().ok()?;
    (f > 0.0 && g.is_finite()).then_some((f, g))
}

/// One Equalizer APO filter body, e.g. "ON PK Fc 105 Hz Gain 6.5 dB Q 0.700".
pub fn filter_body(b: &Band) -> String {
    let state = if b.enabled { "ON" } else { "OFF" };
    let f = if b.freq >= 1000.0 {
        format!("{:.0}", b.freq)
    } else {
        format!("{:.1}", b.freq)
    };
    if b.kind.has_gain() {
        format!("{state} {} Fc {f} Hz Gain {:.2} dB Q {:.3}", b.kind.code(), b.gain, b.q)
    } else {
        format!("{state} {} Fc {f} Hz Q {:.3}", b.kind.code(), b.q)
    }
}

pub fn graphic_line(points: &[(f32, f32)]) -> String {
    let body: Vec<String> = points.iter().map(|(f, g)| format!("{f:.1} {g:.2}")).collect();
    format!("GraphicEQ: {}", body.join("; "))
}

/// A self-contained ParametricEQ.txt that other EQ apps can read.
pub fn export(preamp: f32, bands: &[Band], graphic: &[(f32, f32)]) -> String {
    let mut s = format!("Preamp: {preamp:.1} dB\n");
    for (i, b) in bands.iter().enumerate() {
        s.push_str(&format!("Filter {}: {}\n", i + 1, filter_body(b)));
    }
    if !graphic.is_empty() {
        s.push_str(&graphic_line(graphic));
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_autoeq() {
        let p = parse(
            "Preamp: -6.3 dB\nFilter 1: ON LSC Fc 105 Hz Gain 6.5 dB Q 0.70\nFilter 2: ON PK Fc 125 Hz Gain -2.7 dB Q 0.55\nFilter 3: ON HSC Fc 10000 Hz Gain -3.1 dB Q 0.70\n",
        );
        assert_eq!(p.preamp, Some(-6.3));
        assert_eq!(p.bands.len(), 3);
        assert_eq!(p.bands[0].kind, FilterKind::LowShelf);
        assert_eq!(p.bands[1].gain, -2.7);
        assert_eq!(p.bands[2].freq, 10000.0);
    }

    #[test]
    fn parses_rew_and_variants() {
        let p = parse(
            "Filter  1: ON  PK       Fc   63.50 Hz  Gain  -5.00 dB  Q  2.000\nFilter 2: ON LS 6dB Fc 100 Hz Gain 3 dB\nFilter 3: ON PK Fc 1000 Hz Gain 2 dB BW Oct 1.0\nFilter 4: None\nFilter 5: OFF HP Fc 30 Hz",
        );
        assert_eq!(p.bands.len(), 4);
        assert_eq!(p.bands[0].freq, 63.5);
        assert!((p.bands[1].q - 0.5).abs() < 1e-6);
        assert!((p.bands[2].q - 1.414).abs() < 0.01);
        assert!(!p.bands[3].enabled);
    }

    #[test]
    fn parses_graphic_and_csv() {
        let g = parse("GraphicEQ: 20 -1.5; 1000 0; 20000 2.25");
        assert_eq!(g.graphic, vec![(20.0, -1.5), (1000.0, 0.0), (20000.0, 2.25)]);
        let c = parse("frequency,raw\n20,1.0\n1000,0.5\n");
        assert_eq!(c.graphic.len(), 2);
    }

    #[test]
    fn roundtrip() {
        let bands = vec![
            Band::new(FilterKind::Peak, 1234.0, -2.5, 1.41),
            Band::new(FilterKind::HighPass, 25.0, 0.0, 0.707),
        ];
        let p = parse(&export(-3.0, &bands, &[]));
        assert_eq!(p.preamp, Some(-3.0));
        assert_eq!(p.bands.len(), 2);
        assert_eq!(p.bands[1].kind, FilterKind::HighPass);
        assert!((p.bands[0].q - 1.41).abs() < 1e-3);
    }
}
