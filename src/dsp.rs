//! Filter maths. Equalizer APO runs RBJ-cookbook biquads, so the curves drawn
//! here are what you actually hear (computed at 48 kHz).

use crate::model::{Band, FilterKind};

pub const FS: f64 = 48_000.0;
pub const F_MIN: f32 = 20.0;
pub const F_MAX: f32 = 20_000.0;

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    pub fn design(band: &Band) -> Biquad {
        let f = (band.freq as f64).clamp(1.0, FS * 0.499);
        let q = (band.q as f64).max(0.01);
        let a = 10f64.powf(band.gain as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * f / FS;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match band.kind {
            FilterKind::Peak => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cs + sa),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - sa),
                (a + 1.0) + (a - 1.0) * cs + sa,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - sa,
            ),
            FilterKind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cs + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - sa),
                (a + 1.0) - (a - 1.0) * cs + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - sa,
            ),
            FilterKind::LowPass => ((1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            FilterKind::HighPass => ((1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            FilterKind::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        };
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Magnitude in dB at `freq` Hz.
    pub fn db(&self, freq: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / FS;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let num = nr * nr + ni * ni;
        let den = (dr * dr + di * di).max(1e-30);
        10.0 * (num.max(1e-30) / den).log10()
    }
}

/// GraphicEQ value at `freq`: linear in log-frequency, flat beyond the ends.
pub fn graphic_db(points: &[(f32, f32)], freq: f32) -> f32 {
    match points {
        [] => 0.0,
        [only] => only.1,
        _ => {
            if freq <= points[0].0 {
                return points[0].1;
            }
            let i = points.partition_point(|p| p.0 < freq);
            if i >= points.len() {
                return points[points.len() - 1].1;
            }
            let (f0, g0) = points[i - 1];
            let (f1, g1) = points[i];
            if f1 <= f0 {
                return g1;
            }
            let t = (freq.ln() - f0.ln()) / (f1.ln() - f0.ln());
            g0 + (g1 - g0) * t
        }
    }
}

/// `n` log-spaced frequencies between `lo` and `hi`.
pub fn log_freqs(n: usize, lo: f32, hi: f32) -> Vec<f32> {
    let (l, h) = (lo.ln(), hi.ln());
    (0..n).map(|i| (l + (h - l) * i as f32 / (n - 1).max(1) as f32).exp()).collect()
}

/// Summed response of a filter set (plus optional GraphicEQ) at each frequency.
pub fn response(bands: &[Band], graphic: &[(f32, f32)], freqs: &[f32]) -> Vec<f32> {
    let bq: Vec<Biquad> = bands.iter().map(Biquad::design).collect();
    freqs
        .iter()
        .map(|&f| bq.iter().map(|b| b.db(f as f64)).sum::<f64>() as f32 + graphic_db(graphic, f))
        .collect()
}

/// Highest boost anywhere in the audible band, checked densely and at every band centre.
pub fn peak_db(bands: &[Band], graphic: &[(f32, f32)]) -> f32 {
    let mut freqs = log_freqs(1024, F_MIN, F_MAX);
    freqs.extend(bands.iter().map(|b| b.freq.clamp(F_MIN, F_MAX)));
    freqs.extend(graphic.iter().map(|p| p.0.clamp(F_MIN, F_MAX)));
    response(bands, graphic, &freqs).into_iter().fold(f32::MIN, f32::max).max(-60.0)
}

/// Preamp that keeps the loudest frequency at or below 0 dBFS, rounded down to 0.1 dB.
pub fn auto_preamp(bands: &[Band], graphic: &[(f32, f32)], headroom: f32) -> f32 {
    preamp_for_peak(peak_db(bands, graphic), headroom)
}

/// Same, from an already computed peak.
pub fn preamp_for_peak(peak: f32, headroom: f32) -> f32 {
    let p = -(peak.max(0.0) + headroom.max(0.0));
    (p * 10.0).floor() / 10.0
}

/// Bandwidth in octaves to Q.
pub fn bw_to_q(oct: f32) -> f32 {
    let p = 2f32.powf(oct);
    p.sqrt() / (p - 1.0)
}

/// Pretty frequency: 63 Hz, 1.25 kHz, 16 kHz.
pub fn fmt_freq(f: f32) -> String {
    if f >= 10_000.0 {
        format!("{:.1} kHz", f / 1000.0).replace(".0 kHz", " kHz")
    } else if f >= 1000.0 {
        let s = format!("{:.2}", f / 1000.0);
        format!("{} kHz", s.trim_end_matches('0').trim_end_matches('.'))
    } else {
        format!("{:.0} Hz", f)
    }
}

pub fn fmt_db(g: f32) -> String {
    if g.abs() < 0.05 { "0.0 dB".into() } else { format!("{:+.1} dB", g) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_hits_its_gain_at_centre() {
        let b = Band::new(FilterKind::Peak, 1000.0, 6.0, 1.0);
        let r = Biquad::design(&b).db(1000.0);
        assert!((r - 6.0).abs() < 0.01, "{r}");
        assert!(Biquad::design(&b).db(20.0).abs() < 0.1);
    }

    #[test]
    fn shelves_reach_gain() {
        let ls = Biquad::design(&Band::new(FilterKind::LowShelf, 105.0, 6.5, 0.7));
        assert!((ls.db(20.0) - 6.5).abs() < 0.3);
        assert!(ls.db(5000.0).abs() < 0.1);
        let hs = Biquad::design(&Band::new(FilterKind::HighShelf, 10_000.0, -3.0, 0.7));
        assert!((hs.db(20_000.0) + 3.0).abs() < 0.5);
    }

    #[test]
    fn auto_preamp_cancels_peak() {
        let bands = vec![Band::new(FilterKind::Peak, 100.0, 5.0, 1.0)];
        let p = auto_preamp(&bands, &[], 0.0);
        assert!((-5.1..=-4.9).contains(&p), "{p}");
        assert_eq!(auto_preamp(&[Band::new(FilterKind::Peak, 100.0, -5.0, 1.0)], &[], 0.0), 0.0);
    }

    #[test]
    fn graphic_interpolates() {
        let pts = [(100.0, 0.0), (1000.0, 10.0)];
        assert!((graphic_db(&pts, 316.2) - 5.0).abs() < 0.05);
        assert_eq!(graphic_db(&pts, 10.0), 0.0);
        assert_eq!(graphic_db(&pts, 20_000.0), 10.0);
    }

    #[test]
    fn freq_format() {
        assert_eq!(fmt_freq(63.0), "63 Hz");
        assert_eq!(fmt_freq(1250.0), "1.25 kHz");
        assert_eq!(fmt_freq(2000.0), "2 kHz");
        assert_eq!(fmt_freq(16000.0), "16 kHz");
    }
}
