//! ling-daw — the audio engine behind LingStationX. Everything here is driven
//! from `.ling` scripts through the `daw_*` builtins (see `api`).
pub mod ai_import;
pub mod mixai;
pub mod musicxml;
pub mod articulate;
pub mod api;
pub mod audio;
pub mod audio_in;
pub mod bundle;
pub mod export;
pub mod flp_import;
pub mod karaoke;
pub mod sheet;
pub mod keydet;
pub mod lyricfit;
pub mod lyrics;
pub mod dsp;
pub mod engine;
pub mod midi_import;
pub mod tracker;
pub mod midi_io;
pub mod model;
pub mod patch;
pub mod perf;
pub mod project;
pub mod stretch;
pub mod theory;
pub mod compose;
pub mod arrange;
pub mod structure;
pub mod harmonize;
pub mod vocal;
pub mod songgen;
pub mod guide;
pub mod soundcat;

pub use engine::{set_parallel_render, Engine, SharedEngine};
pub use model::*;

pub use api::{Daw, Val};
