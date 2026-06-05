//! Musical note ⇄ MIDI ⇄ frequency, and scales.
//!
//! MIDI note 69 = A4 = 440 Hz; each semitone is a factor of 2^(1/12).

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const NAMES_FLAT: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

/// MIDI note number → frequency in Hz (A4=69→440).
pub fn midi_to_hz(midi: f32) -> f32 {
    440.0 * 2.0_f32.powf((midi - 69.0) / 12.0)
}

/// Frequency in Hz → fractional MIDI note number.
pub fn hz_to_midi(hz: f32) -> f32 {
    if hz <= 0.0 { return 0.0; }
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// Parse a note name like `"C4"`, `"A#3"`, `"Db5"` → MIDI number. `C4 = 60`
/// (so A4 = 69). Returns `None` if it can't be parsed.
pub fn name_to_midi(s: &str) -> Option<i32> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.is_empty() { return None; }
    // letter
    let letter = b[0].to_ascii_uppercase();
    let mut pc: i32 = match letter {
        b'C' => 0, b'D' => 2, b'E' => 4, b'F' => 5, b'G' => 7, b'A' => 9, b'B' => 11,
        _ => return None,
    };
    let mut i = 1;
    while i < b.len() && (b[i] == b'#' || b[i] == b's') { pc += 1; i += 1; }
    while i < b.len() && b[i] == b'b' { pc -= 1; i += 1; }
    let oct: i32 = s[i..].trim().parse().ok()?;
    Some((oct + 1) * 12 + pc)
}

/// Parse a note name OR a bare number (treated as a MIDI number) → MIDI int.
pub fn parse_pitch(s: &str) -> Option<i32> {
    name_to_midi(s).or_else(|| s.trim().parse::<f32>().ok().map(|n| n.round() as i32))
}

/// Note name → Hz (e.g. `"A4"` → 440.0).
pub fn name_to_hz(s: &str) -> Option<f32> {
    name_to_midi(s).map(|m| midi_to_hz(m as f32))
}

/// MIDI number → note name with octave (sharp spelling), e.g. 60 → `"C4"`.
pub fn midi_to_name(midi: i32) -> String {
    let pc = midi.rem_euclid(12) as usize;
    let oct = midi.div_euclid(12) - 1;
    format!("{}{}", NAMES[pc], oct)
}

/// Hz → nearest note name (sharp spelling). Empty string for non-positive Hz.
pub fn hz_to_name(hz: f32) -> String {
    if hz <= 0.0 { return String::new(); }
    midi_to_name(hz_to_midi(hz).round() as i32)
}

/// Pitch-class name (no octave), sharp or flat spelling.
pub fn pitch_class_name(pc: usize, flat: bool) -> &'static str {
    if flat { NAMES_FLAT[pc % 12] } else { NAMES[pc % 12] }
}

/// Semitone offsets for a few common scales (from the root).
pub fn scale_steps(name: &str) -> &'static [i32] {
    match name.to_ascii_lowercase().as_str() {
        "minor" | "aeolian"    => &[0, 2, 3, 5, 7, 8, 10],
        "harmonic_minor"       => &[0, 2, 3, 5, 7, 8, 11],
        "pentatonic" | "pent"  => &[0, 2, 4, 7, 9],
        "minor_pentatonic"     => &[0, 3, 5, 7, 10],
        "blues"                => &[0, 3, 5, 6, 7, 10],
        "dorian"               => &[0, 2, 3, 5, 7, 9, 10],
        "chromatic"            => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        _                      => &[0, 2, 4, 5, 7, 9, 11], // major / ionian
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_freqs() {
        assert_eq!(name_to_midi("A4"), Some(69));
        assert_eq!(name_to_midi("C4"), Some(60));
        assert_eq!(name_to_midi("C#4"), Some(61));
        assert_eq!(name_to_midi("Db4"), Some(61));
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((midi_to_hz(60.0) - 261.6256).abs() < 0.01);
        assert_eq!(midi_to_name(60), "C4");
        assert_eq!(hz_to_name(440.0), "A4");
        // round trip
        for m in 24..96 { assert_eq!(hz_to_midi(midi_to_hz(m as f32)).round() as i32, m); }
    }
}
