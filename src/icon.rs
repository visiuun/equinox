// Shared by build.rs (exe icon) and the app (window and tray icon); no dependencies.

/// RGBA pixels of the equinox mark at `n`x`n`: an EQ curve with one boosted
/// node on a black rounded tile. Supersampled 4x4 for clean edges at small sizes.
pub fn icon_rgba(n: u32) -> Vec<u8> {
    icon_rgba_tinted(n, [125, 249, 255])
}

/// Same mark, with the curve in `tint` (the tray dims it while bypassed).
pub fn icon_rgba_tinted(n: u32, tint: [u8; 3]) -> Vec<u8> {
    const SS: u32 = 4;
    // Thicker strokes at tray sizes so the curve still reads at 16 px.
    let width = if n <= 24 {
        0.095
    } else if n <= 48 {
        0.075
    } else {
        0.06
    };
    let curve: Vec<(f32, f32)> = (0..=96)
        .map(|i| {
            let x = 0.16 + 0.68 * i as f32 / 96.0;
            (x, curve_y(x))
        })
        .collect();
    let mut out = Vec::with_capacity((n * n * 4) as usize);
    for py in 0..n {
        for px in 0..n {
            let (mut acc, mut cov) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / n as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / n as f32;
                    if let Some(c) = sample(x, y, &curve, width, tint) {
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                        cov += 1.0;
                    }
                }
            }
            let a = cov / (SS * SS) as f32;
            let px_rgb: Vec<u8> = (0..3)
                .map(|k| {
                    if cov > 0.0 {
                        (acc[k] / cov).round().clamp(0.0, 255.0) as u8
                    } else {
                        0
                    }
                })
                .collect();
            out.extend_from_slice(&[px_rgb[0], px_rgb[1], px_rgb[2], (a * 255.0).round() as u8]);
        }
    }
    out
}

/// A bass bump and a treble dip: what an EQ curve looks like at a glance.
fn curve_y(x: f32) -> f32 {
    let bump = (-((x - 0.36) / 0.12).powi(2)).exp();
    let dip = (-((x - 0.7) / 0.11).powi(2)).exp();
    0.56 - 0.24 * bump + 0.16 * dip
}

fn sample(x: f32, y: f32, curve: &[(f32, f32)], width: f32, tint: [u8; 3]) -> Option<[f32; 3]> {
    let (m, r) = (0.03, 0.22);
    let qx = (x - 0.5).abs() - (0.5 - m - r);
    let qy = (y - 0.5).abs() - (0.5 - m - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
    if outside > 0.0 {
        return None;
    }
    let tint = [tint[0] as f32, tint[1] as f32, tint[2] as f32];
    let mut d = f32::MAX;
    for w in curve.windows(2) {
        d = d.min(seg_dist((x, y), w[0], w[1]));
    }
    let node = (0.36, curve_y(0.36));
    let nd = ((x - node.0).powi(2) + (y - node.1).powi(2)).sqrt();
    if nd < width * 1.25 {
        return Some([255.0, 255.0, 255.0]);
    }
    if d < width / 2.0 {
        return Some(tint);
    }
    // Soft glow under the curve, fading into the black tile.
    let glow = (1.0 - (d - width / 2.0) / 0.14).clamp(0.0, 1.0).powi(2) * 0.28;
    let base = 4.0;
    Some([base + tint[0] * glow, base + tint[1] * glow, base + tint[2] * glow])
}

fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}
