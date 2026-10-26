//! The AutoEq database: ~9k headphone measurements turned into EQ by
//! jaakkopasanen/AutoEq. The index and each profile are fetched once and cached.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};

use crate::model::Correction;

const RAW: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/";
const INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub source: String,
    pub rig: Option<String>,
    /// Path under results/, still URL-encoded as in INDEX.md.
    pub path: String,
    norm: String,
    compact: String,
}

impl Entry {
    pub fn source_label(&self) -> String {
        match &self.rig {
            Some(r) => format!("{} · {r}", self.source),
            None => self.source.clone(),
        }
    }

    /// Stable id stored in settings (e.g. dismissed suggestions).
    pub fn key(&self) -> String {
        format!("{}|{}", self.name, self.source)
    }
}

/// Measurement sources, roughly in the order AutoEq itself recommends.
fn source_rank(source: &str) -> i32 {
    match source {
        "oratory1990" => 6,
        "crinacle" => 5,
        "Rtings" => 4,
        "Innerfidelity" => 3,
        "Headphone.com Legacy" | "HypetheSonics" => 2,
        _ => 1,
    }
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_string()
}

pub fn parse_index(md: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for line in md.lines() {
        // - [Name](./source/rig/Name) by Source on Rig
        let Some(rest) = line.strip_prefix("- [") else { continue };
        let Some(close) = rest.find("](") else { continue };
        let name = rest[..close].to_string();
        let rest = &rest[close + 2..];
        let Some(end) = rest.find(") by ") else { continue };
        let path = rest[..end].trim_start_matches("./").to_string();
        let by = &rest[end + 5..];
        let (source, rig) = match by.split_once(" on ") {
            Some((s, r)) => (s.trim().to_string(), Some(r.trim().to_string())),
            None => (by.trim().to_string(), None),
        };
        let norm = normalize(&name);
        let compact = norm.replace(' ', "");
        out.push(Entry {
            name,
            source,
            rig,
            path,
            norm,
            compact,
        });
    }
    out
}

fn index_cache() -> PathBuf {
    crate::paths::cache_dir().join("autoeq-index.md")
}

fn get(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(20))
        .call()
        .with_context(|| format!("GET {url}"))?;
    Ok(resp.into_string()?)
}

/// The index, from cache when fresh (or when offline), otherwise downloaded.
pub fn load_index(refresh: bool) -> Result<Vec<Entry>> {
    let cache = index_cache();
    let age = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    if !refresh
        && age.is_some_and(|a| a < INDEX_MAX_AGE)
        && let Ok(md) = std::fs::read_to_string(&cache)
    {
        return Ok(parse_index(&md));
    }
    match get(&format!("{RAW}INDEX.md")) {
        Ok(md) => {
            let _ = std::fs::create_dir_all(cache.parent().unwrap());
            let _ = std::fs::write(&cache, &md);
            Ok(parse_index(&md))
        }
        Err(e) => match std::fs::read_to_string(&cache) {
            Ok(md) => Ok(parse_index(&md)),
            Err(_) => Err(e.context("couldn't download the AutoEq index")),
        },
    }
}

pub fn index_age() -> Option<Duration> {
    std::fs::metadata(index_cache())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
}

/// Fuzzy search: every query word must match (spaces and dashes optional,
/// so "hd600" finds "HD 600"); ranks exact and prefix hits and better sources first.
pub fn search<'a>(entries: &'a [Entry], query: &str, limit: usize) -> Vec<&'a Entry> {
    let q = normalize(query);
    if q.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = q.split(' ').collect();
    let qcompact = q.replace(' ', "");
    let mut scored: Vec<(i32, &Entry)> = entries
        .iter()
        .filter_map(|e| {
            let mut score = 0;
            for w in &words {
                if e.norm.split(' ').any(|t| t == *w) {
                    score += 30;
                } else if e.norm.split(' ').any(|t| t.starts_with(w)) {
                    score += 18;
                } else if e.compact.contains(w) {
                    score += 8;
                } else {
                    return None;
                }
            }
            let model = e.norm.split_once(' ').map(|(_, m)| m.replace(' ', "")).unwrap_or_default();
            if e.compact == qcompact || model == qcompact {
                score += 120;
            } else if e.compact.starts_with(&qcompact) {
                score += 40;
            } else if e.compact.contains(&qcompact) {
                score += 20;
            }
            score -= (e.compact.len() as i32 - qcompact.len() as i32).max(0) / 3;
            score += source_rank(&e.source) * 3;
            Some((score, e))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

/// Guess the headphone model from a device's name (Bluetooth and USB headsets
/// usually report it, e.g. "WH-1000XM4"). The model words must appear as whole
/// consecutive words; a lone model word without a digit also needs the brand.
pub fn suggest<'a>(entries: &'a [Entry], device_text: &str) -> Option<&'a Entry> {
    const GENERIC: [&str; 22] = [
        "headphones",
        "headphone",
        "headset",
        "speakers",
        "speaker",
        "stereo",
        "hands",
        "free",
        "audio",
        "realtek",
        "nvidia",
        "high",
        "definition",
        "device",
        "output",
        "digital",
        "usb",
        "r",
        "amd",
        "intel",
        "virtual",
        "earphone",
    ];
    let norm = normalize(device_text);
    let words: Vec<&str> = norm.split(' ').filter(|w| !GENERIC.contains(w)).collect();
    if words.is_empty() {
        return None;
    }
    // Every run of consecutive device words, glued: "major iv" -> "majoriv".
    let mut runs = std::collections::HashSet::new();
    for i in 0..words.len() {
        for j in i + 1..=words.len().min(i + 6) {
            runs.insert(words[i..j].concat());
        }
    }
    let mut best: Option<(usize, i32, &Entry)> = None;
    for e in entries {
        let mut it = e.norm.split(' ');
        let brand = it.next().unwrap_or("");
        let model_words: Vec<&str> = it.collect();
        let model = model_words.concat();
        let has_brand = words.contains(&brand);
        let distinctive = model.chars().any(|c| c.is_ascii_digit()) || model_words.len() >= 2;
        let hit = if model.len() >= 3 && runs.contains(&model) && (distinctive || has_brand) {
            Some(model.len() + if has_brand { brand.len() } else { 0 })
        } else if has_brand && runs.contains(&e.compact) {
            Some(e.compact.len())
        } else {
            None
        };
        if let Some(len) = hit {
            let rank = source_rank(&e.source);
            if best.is_none_or(|(bl, br, _)| len > bl || (len == bl && rank > br)) {
                best = Some((len, rank, e));
            }
        }
    }
    best.map(|b| b.2)
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode(s: &str) -> String {
    let mut out = String::new();
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.~()".contains(&c) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

fn profile_cache(e: &Entry) -> PathBuf {
    let safe: String = pct_decode(&e.path)
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
        .collect();
    crate::paths::cache_dir().join("autoeq").join(format!("{safe}.txt"))
}

/// Download (or read cached) ParametricEQ.txt for an entry.
pub fn fetch(e: &Entry) -> Result<Correction> {
    let cache = profile_cache(e);
    let text = match std::fs::read_to_string(&cache) {
        Ok(t) => t,
        Err(_) => {
            let folder = pct_decode(e.path.rsplit('/').next().unwrap_or(&e.name));
            let url = format!("{RAW}{}/{}", e.path, pct_encode(&format!("{folder} ParametricEQ.txt")));
            let t = get(&url)?;
            let _ = std::fs::create_dir_all(cache.parent().unwrap());
            let _ = std::fs::write(&cache, &t);
            t
        }
    };
    let parsed = crate::eqtext::parse(&text);
    if parsed.bands.is_empty() {
        bail!("no filters in the AutoEq profile for {}", e.name);
    }
    Ok(Correction {
        name: e.name.clone(),
        source: format!("AutoEq · {}", e.source_label()),
        filters: parsed.bands,
        graphic: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Index\n\n- [Sennheiser HD 600](./oratory1990/over-ear/Sennheiser%20HD%20600) by oratory1990\n- [Sennheiser HD 600](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20600) by crinacle on GRAS 43AG-7\n- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990\n- [Sony WH-1000XM4](./oratory1990/over-ear/Sony%20WH-1000XM4) by oratory1990\n- [Sony WH-1000XM3](./oratory1990/over-ear/Sony%20WH-1000XM3) by oratory1990\n";

    #[test]
    fn parses() {
        let e = parse_index(MD);
        assert_eq!(e.len(), 5);
        assert_eq!(e[1].rig.as_deref(), Some("GRAS 43AG-7"));
        assert_eq!(e[0].path, "oratory1990/over-ear/Sennheiser%20HD%20600");
    }

    #[test]
    fn searches_compact() {
        let e = parse_index(MD);
        let r = search(&e, "hd600", 10);
        assert_eq!(r[0].name, "Sennheiser HD 600");
        assert_eq!(r[0].source, "oratory1990");
        assert!(r.iter().all(|x| x.name.contains("600")));
        assert_eq!(search(&e, "xm4", 5)[0].name, "Sony WH-1000XM4");
    }

    #[test]
    fn suggests_from_device_name() {
        let e = parse_index(MD);
        assert_eq!(suggest(&e, "Headphones (WH-1000XM4)").unwrap().name, "Sony WH-1000XM4");
        assert!(suggest(&e, "Speakers (Realtek(R) Audio)").is_none());
        let more = parse_index(
            "- [Kitsound Mini](./x/y/Kitsound%20Mini) by x
- [ARC Oculus](./x/y/ARC%20Oculus) by x
- [Marshall Major IV](./x/y/Marshall%20Major%20IV) by crinacle
",
        );
        assert!(suggest(&more, "Main 1/2 (Minifuse 1)").is_none());
        assert!(suggest(&more, "Headphones (Oculus Virtual Audio Device)").is_none());
        assert_eq!(suggest(&more, "Headphones (MAJOR IV)").unwrap().name, "Marshall Major IV");
    }

    #[test]
    fn url_encoding() {
        assert_eq!(pct_decode("Sennheiser%20HD%20600"), "Sennheiser HD 600");
        assert_eq!(pct_encode("A B (x).txt"), "A%20B%20(x).txt");
    }
}
