//! ling-music — a 2030-ready music toolkit for Ling.
//!
//! Goes well beyond playback: decode WAV/FLAC/OGG/MP3/AAC ([`decode`]), extract
//! tempo & musical key ([`analysis`]), detect sung pitch ([`pitch`]), synthesize
//! "any GM instrument" from a `.ling` patch ([`synth`] + [`patch`]), and power
//! rhythm games ([`rhythm`]) and karaoke ([`karaoke`]) — all driven live by the
//! [`MusicEngine`].

pub mod analysis;
pub mod decode;
pub mod engine;
pub mod karaoke;
pub mod midi;
pub mod note;
pub mod patch;
pub mod pitch;
pub mod rhythm;
pub mod synth;

pub use decode::{load, DecodedAudio};
pub use engine::MusicEngine;
pub use karaoke::{pitch_score, LyricLine, Lyrics};
pub use midi::{MidiNote, MidiSong};
pub use rhythm::{Beatmap, Grade, HitNote, Scorer};
pub use synth::{Patch, Synth, Wave};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
