//! ling-music — a 2030-ready music toolkit for Ling.
//!
//! Goes well beyond playback: decode WAV/FLAC/OGG/MP3/AAC ([`decode`]), extract
//! tempo & musical key ([`analysis`]), detect sung pitch ([`pitch`]), synthesize
//! "any GM instrument" from a `.ling` patch ([`synth`] + [`patch`]), and power
//! rhythm games ([`rhythm`]) and karaoke ([`karaoke`]) — all driven live by the
//! [`MusicEngine`].

pub mod note;
pub mod pitch;
pub mod analysis;
pub mod synth;
pub mod patch;
pub mod rhythm;
pub mod karaoke;
pub mod decode;
pub mod midi;
pub mod engine;

pub use decode::{load, DecodedAudio};
pub use synth::{Patch, Synth, Wave};
pub use rhythm::{Beatmap, Grade, Scorer, HitNote};
pub use karaoke::{Lyrics, LyricLine, pitch_score};
pub use engine::MusicEngine;
pub use midi::{MidiSong, MidiNote};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
