//! equinox: a system-wide equalizer for Windows, driving Equalizer APO.
//! Shared by the app (`equinox.exe`) and the command line (`eqx.exe`).

pub mod analyzer;
pub mod apo;
pub mod apo_setup;
pub mod autoeq;
pub mod devices;
pub mod dsp;
pub mod engine;
pub mod eqtext;
pub mod icon;
pub mod model;
pub mod paths;
pub mod presets;
pub mod sys;
