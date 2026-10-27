//! English words as they are pronounced: the CMU Pronouncing Dictionary (assets/lexicon/cmudict.txt, 123k words, BSD-style
//! license - its notice is assets/lexicon/CMUDICT_LICENSE) read into IPA syllables. Spelling rules guess "with" as /wiθ/ and "me" as
//! /me/; the dictionary knows /wɪð/ and /miː/.
//!
//! * ARPAbet to IPA in the conventions espeak's en-us voice (and the phoneme recognizer the Ling Singer is trained with) uses:
//!   long iː uː ɑː ɔː, stressed ER ɜː and unstressed ɚ, stressed AH ʌ and unstressed ə, the diphthongs eɪ aɪ aʊ oʊ ɔɪ, ɹ.
//! * Syllables: one per vowel; the consonants between two vowels give the next syllable the longest legal English onset
//!   ("ex-tra", "a-pple" -> "æ-pəl", "con-struct").
//! * Lookup ignores case and surrounding punctuation, keeps apostrophes ("don't", "i'm"); the first pronunciation is used.
use std::collections::HashMap;
use std::sync::OnceLock;

fn dict() -> &'static HashMap<String, Vec<String>> {
    static D: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    D.get_or_init(|| {
        let mut m = HashMap::new();
        let paths = [std::env::var("LING_CMUDICT").unwrap_or_default(), "assets/lexicon/cmudict.txt".into(), "../LingStationX/assets/lexicon/cmudict.txt".into()];
        let Some(text) = paths.iter().filter(|p| !p.is_empty()).find_map(|p| std::fs::read_to_string(p).ok()) else { return m };
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let (Some(w), Some(n)) = (it.next(), it.next()) else { continue };
            // the NLTK layout: WORD <variant> PHONES...; the plain one: WORD PHONES... (variants as WORD(2))
            let (word, phones): (String, Vec<String>) = if n.chars().all(|c| c.is_ascii_digit()) {
                if n != "1" {
                    continue;
                }
                (w.to_lowercase(), it.map(str::to_string).collect())
            } else {
                if w.contains('(') {
                    continue;
                }
                (w.to_lowercase(), std::iter::once(n.to_string()).chain(it.map(str::to_string)).collect())
            };
            m.entry(word).or_insert(phones);
        }
        m
    })
}

/// is the dictionary there (assets/lexicon/cmudict.txt found)?
pub fn available() -> bool {
    !dict().is_empty()
}

fn is_vowel(p: &str) -> bool {
    p.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
}

fn ipa(p: &str) -> &'static str {
    let (base, stress) = match p.chars().last() {
        Some(c) if c.is_ascii_digit() => (&p[..p.len() - 1], c),
        _ => (p, ' '),
    };
    match base {
        "AA" => "ɑː",
        "AE" => "æ",
        "AH" => if stress == '0' { "ə" } else { "ʌ" },
        "AO" => "ɔː",
        "AW" => "aʊ",
        "AY" => "aɪ",
        "EH" => "ɛ",
        "ER" => if stress == '0' { "ɚ" } else { "ɜː" },
        "EY" => "eɪ",
        "IH" => "ɪ",
        "IY" => "iː",
        "OW" => "oʊ",
        "OY" => "ɔɪ",
        "UH" => "ʊ",
        "UW" => "uː",
        "B" => "b",
        "CH" => "tʃ",
        "D" => "d",
        "DH" => "ð",
        "F" => "f",
        "G" => "ɡ",
        "HH" => "h",
        "JH" => "dʒ",
        "K" => "k",
        "L" => "l",
        "M" => "m",
        "N" => "n",
        "NG" => "ŋ",
        "P" => "p",
        "R" => "ɹ",
        "S" => "s",
        "SH" => "ʃ",
        "T" => "t",
        "TH" => "θ",
        "V" => "v",
        "W" => "w",
        "Y" => "j",
        "Z" => "z",
        "ZH" => "ʒ",
        _ => "",
    }
}

/// a legal English onset (the consonants a syllable may start with)
fn legal_onset(c: &[&str]) -> bool {
    match c.len() {
        0 | 1 => !matches!(c.first(), Some(&"NG")),
        2 => {
            let (a, b) = (c[0], c[1]);
            (a == "S" && matches!(b, "P" | "T" | "K" | "M" | "N" | "L" | "W" | "F"))
                || (matches!(a, "P" | "B" | "K" | "G" | "F") && matches!(b, "L" | "R"))
                || (matches!(a, "T" | "D" | "TH" | "SH") && b == "R")
                || (matches!(a, "T" | "D" | "K" | "G" | "S" | "TH") && b == "W")
                || (matches!(a, "P" | "B" | "K" | "F" | "V" | "M" | "HH") && b == "Y")
        }
        3 => c[0] == "S" && matches!(c[1], "P" | "T" | "K") && matches!(c[2], "L" | "R" | "W" | "Y"),
        _ => false,
    }
}

/// ARPAbet phones -> IPA syllables (maximal legal onsets)
pub fn syllables(phones: &[String]) -> Vec<String> {
    let p: Vec<&str> = phones.iter().map(|s| s.as_str()).collect();
    let vowels: Vec<usize> = (0..p.len()).filter(|i| is_vowel(p[*i])).collect();
    if vowels.is_empty() {
        return vec![p.iter().map(|x| ipa(x)).collect()];
    }
    // where each syllable starts
    let mut starts = vec![0usize];
    for w in vowels.windows(2) {
        let (a, b) = (w[0], w[1]);
        let cons = &p[a + 1..b];
        let mut k = cons.len();
        // the longest tail of the cluster that is a legal onset
        for take in (0..=cons.len()).rev() {
            if legal_onset(&cons[cons.len() - take..]) {
                k = cons.len() - take;
                break;
            }
        }
        starts.push(a + 1 + k);
    }
    starts.push(p.len());
    starts.windows(2).map(|w| p[w[0]..w[1]].iter().map(|x| ipa(x)).collect::<String>()).filter(|s| !s.is_empty()).collect()
}

/// a word's IPA syllables, if the dictionary knows it
pub fn lookup(word: &str) -> Option<Vec<String>> {
    let w: String = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase().replace('’', "'");
    if w.is_empty() {
        return None;
    }
    let d = dict();
    let phones = d.get(&w).or_else(|| d.get(w.trim_end_matches('\'')))?;
    Some(syllables(phones))
}

/// a word's stress per syllable (1 primary, 2 secondary, 0 none), if the dictionary knows it
pub fn stress(word: &str) -> Option<Vec<u8>> {
    let w: String = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase().replace('’', "'");
    if w.is_empty() {
        return None;
    }
    let d = dict();
    let phones = d.get(&w).or_else(|| d.get(w.trim_end_matches('\'')))?;
    Some(phones.iter().filter_map(|p| p.chars().last().and_then(|c| c.to_digit(10)).map(|x| x as u8)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn arpabet_becomes_ipa_syllables_with_english_onsets() {
        assert_eq!(syllables(&s(&["HH", "AH0", "L", "OW1"])), vec!["hə", "loʊ"]);
        assert_eq!(syllables(&s(&["T", "AH0", "N", "AY1", "T"])), vec!["tə", "naɪt"]);
        assert_eq!(syllables(&s(&["EH1", "K", "S", "T", "R", "AH0"])), vec!["ɛk", "stɹə"]);
        assert_eq!(syllables(&s(&["W", "ER1", "L", "D"])), vec!["wɜːld"]);
        assert_eq!(syllables(&s(&["S", "IH1", "NG", "IH0", "NG"])), vec!["sɪŋ", "ɪŋ"]);
        // the dictionary itself, when it is there
        if available() {
            assert_eq!(lookup("With").unwrap(), vec!["wɪð"]);
            assert_eq!(lookup("tonight,").unwrap(), vec!["tə", "naɪt"]);
            assert_eq!(lookup("don't").unwrap(), vec!["doʊnt"]);
            assert!(lookup("zxqvjj").is_none());
        }
    }
}
