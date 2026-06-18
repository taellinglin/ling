//! Heuristic language detection based on Unicode script ranges.

/// Detect the dominant human language in a Ling source file.
/// Returns the ISO 639-1 code (e.g. `"zh"`, `"ja"`, `"th"`, `"en"`).
pub fn detect_language(source: &str) -> &'static str {
    let mut counts = [0usize; 9]; // en, zh, ja, ko, th, ru, ar, hi, other
    for ch in source.chars() {
        let cp = ch as u32;
        match cp {
            0x4E00..=0x9FFF | 0x3400..=0x4DBF => counts[1] += 1, // CJK unified
            0x3040..=0x309F | 0x30A0..=0x30FF => counts[2] += 1, // Hiragana/Katakana
            0xAC00..=0xD7AF | 0x1100..=0x11FF => counts[3] += 1, // Hangul
            0x0E00..=0x0E7F => counts[4] += 1,                   // Thai
            0x0400..=0x04FF => counts[5] += 1,                   // Cyrillic
            0x0600..=0x06FF => counts[6] += 1,                   // Arabic
            0x0900..=0x097F => counts[7] += 1,                   // Devanagari
            0x0041..=0x005A | 0x0061..=0x007A => counts[0] += 1, // A-Z a-z (English)
            // Whitespace, digits and punctuation are script-neutral: skip them so
            // they don't outweigh a handful of CJK/Thai keyword characters.
            _ => {},
        }
    }
    let langs = ["en", "zh", "ja", "ko", "th", "ru", "ar", "hi", "en"];
    let max = counts
        .iter()
        .enumerate()
        .max_by_key(|&(_, v)| v)
        .map(|(i, _)| i)
        .unwrap_or(0);
    langs[max]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_thai() {
        assert_eq!(detect_language("ฟังก์ชัน หลัก"), "th");
    }
    #[test]
    fn detects_chinese() {
        assert_eq!(detect_language("令 x = 若"), "zh");
    }
    #[test]
    fn detects_english() {
        assert_eq!(detect_language("fn main() {}"), "en");
    }
}
