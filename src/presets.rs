//! Starter profiles. They're ordinary profiles once created: rename, edit or delete them.

use crate::model::{Band, FilterKind::*, Tone};

fn tone(bass: f32, warmth: f32, clarity: f32, air: f32) -> Tone {
    Tone {
        bass,
        warmth,
        clarity,
        air,
    }
}

pub fn builtin() -> Vec<(&'static str, Tone, Vec<Band>)> {
    vec![
        ("Flat", Tone::default(), vec![]),
        ("Bass Boost", tone(6.0, 1.0, 0.0, 0.0), vec![]),
        ("Warm", tone(2.5, 2.0, -1.0, -1.5), vec![]),
        ("Bright", tone(0.0, -1.0, 1.5, 3.5), vec![]),
        ("V-Shape", tone(4.5, -1.0, -1.5, 3.5), vec![]),
        ("Vocal", tone(-2.0, -1.0, 3.0, 1.0), vec![Band::new(HighPass, 70.0, 0.0, 0.707)]),
        (
            "Late Night",
            tone(-3.0, 0.0, 1.0, -2.0),
            vec![Band::new(Peak, 3500.0, -1.5, 1.2), Band::new(HighPass, 45.0, 0.0, 0.6)],
        ),
        (
            "Footsteps",
            tone(-4.0, -2.0, 2.0, 1.0),
            vec![Band::new(Peak, 2500.0, 3.0, 1.0), Band::new(Peak, 5000.0, 2.0, 1.4)],
        ),
        (
            "Treble Tame",
            Tone::default(),
            vec![Band::new(Peak, 6500.0, -3.0, 2.0), Band::new(HighShelf, 9000.0, -2.0, 0.7)],
        ),
        (
            "Laptop Speakers",
            tone(0.0, -1.5, 2.0, 1.5),
            vec![
                Band::new(HighPass, 140.0, 0.0, 0.707),
                Band::new(Peak, 180.0, 3.0, 1.5),
                Band::new(Peak, 450.0, -2.0, 1.4),
            ],
        ),
        ("Loudness", tone(5.0, 0.0, -1.0, 3.0), vec![]),
    ]
}
