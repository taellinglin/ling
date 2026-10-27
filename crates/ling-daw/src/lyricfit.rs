//! Fitting lyrics to notes: paste a song's words, get them spread over a clip's notes the way a singer would phrase them.
//!
//! The text is cleaned (section labels, chords and stage directions go), split into syllables (English by rule, or by the hyphens a
//! model / the user put in; Hangul, kana and Hanzi by character) and turned into IPA. The notes are grouped by onset, so a chord is ONE
//! thing that can carry one sustained syllable; short notes after a syllable become sustains; the lyric lines are matched to the phrases
//! the rests in the music make. Three ways to spread the syllables:
//!
//! * `Fit`: one syllable per note group when they match; fewer syllables than groups means melismas (the syllable lands on a strong beat
//!   or a long note near its even share and the following notes sustain it); more syllables than groups means the longest notes carry two.
//! * `Even`: the same, but purely by count, ignoring the beat.
//! * `Strict`: one syllable per group in order, nothing more.
//!
//! `looped` repeats the lyrics (lines across phrases, syllables across notes) when the notes outlast them.
//!
//! `shape` (option string mode "shape") first reshapes a sung line to the words ([`shape`]): the lines are laid over the melody where it
//! breathes, then each line gets one note per syllable (long notes split, surplus short notes joined to the note before), the last word
//! of every line is held and a breath follows before the next line. The fit after it is then one syllable per note.
use crate::lyrics::{self, Lang};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Fit,
    Even,
    Strict,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Chord {
    /// every note of a chord sings the syllable
    All,
    /// the top note sings it, the others hold the default vowel
    Top,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    pub mode: Mode,
    pub looped: bool,
    /// notes shorter than this many beats, right after another note, sustain it instead of taking a syllable (0 = off)
    pub short: f64,
    pub chord: Chord,
    /// a rest this long (beats) between notes starts a new phrase
    pub gap: f64,
    pub lang: Lang,
    /// reshape the notes to the lyrics before fitting (see [`shape`])
    pub shape: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { mode: Mode::Fit, looped: false, short: 0.3, chord: Chord::All, gap: 1.0, lang: Lang::Auto, shape: false }
    }
}

impl Opts {
    /// `fit:<mode>:<loop 0|1>:<short>:<chord>:<gap>:<lang>` as stored in a clip's `lyric_lang`; None when it is not a fit setting
    pub fn parse(s: &str) -> Option<Opts> {
        let p: Vec<&str> = s.split(':').collect();
        if p.first() != Some(&"fit") {
            return None;
        }
        let mut o = Opts::default();
        if let Some(m) = p.get(1) {
            o.shape = *m == "shape";
            o.mode = match *m {
                "even" => Mode::Even,
                "strict" => Mode::Strict,
                _ => Mode::Fit,
            };
        }
        if let Some(l) = p.get(2) {
            o.looped = *l == "1";
        }
        if let Some(x) = p.get(3).and_then(|x| x.parse::<f64>().ok()) {
            o.short = x.clamp(0.0, 4.0);
        }
        if let Some(c) = p.get(4) {
            o.chord = if *c == "top" { Chord::Top } else { Chord::All };
        }
        if let Some(g) = p.get(5).and_then(|x| x.parse::<f64>().ok()) {
            o.gap = g.clamp(0.25, 16.0);
        }
        if let Some(l) = p.get(6) {
            o.lang = Lang::parse(l);
        }
        Some(o)
    }
}

pub struct NoteIn {
    pub start: f64,
    pub len: f64,
    pub key: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub syllables: usize,
    pub lines: usize,
    pub groups: usize,
    pub phrases: usize,
    pub chords: usize,
    pub bearing: usize,
}

// ── cleaning ──────────────────────────────────────────────────────────────────

fn is_chord_token(t: &str) -> bool {
    let mut c = t.trim_matches(|c: char| c == '(' || c == ')');
    if let Some((a, _)) = c.split_once('/') {
        c = a;
    }
    let mut ch = c.chars();
    match ch.next() {
        Some('A'..='G') => {}
        _ => return false,
    }
    let mut rest: &str = ch.as_str();
    if rest.starts_with('#') || rest.starts_with('b') {
        rest = &rest[1..];
    }
    loop {
        if rest.is_empty() {
            return true;
        }
        let mut found = false;
        for suf in ["maj", "min", "dim", "aug", "sus", "add", "m", "M"] {
            if let Some(r) = rest.strip_prefix(suf) {
                rest = r;
                found = true;
                break;
            }
        }
        if !found {
            if let Some(r) = rest.strip_prefix(|c: char| c.is_ascii_digit() || c == '#' || c == '+' || c == '-') {
                rest = r;
                found = true;
            }
        }
        if !found {
            return false;
        }
    }
}

fn is_direction(inner: &str) -> bool {
    let l = inner.trim().to_lowercase();
    let digits = l.trim_start_matches('x').trim_end_matches('x');
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        || ["chorus", "verse", "intro", "outro", "bridge", "repeat", "refrain", "hook", "solo", "instrumental", "interlude", "pre-chorus", "x2", "x3", "x4"].iter().any(|w| l.contains(w))
}

/// the lyric lines of pasted text: section labels `[Chorus]`, chord lines, timestamps and `(x2)` go; lines are split at newlines and at a
/// spaced `/` or `|`; punctuation goes, apostrophes and hyphens stay
pub fn clean(raw: &str) -> Vec<String> {
    let text = raw.replace('\r', "").replace(" / ", "\n").replace(" | ", "\n").replace('\u{2028}', "\n");
    let mut out = vec![];
    for line in text.lines() {
        // drop [..] and directive-like (..)
        let mut s = String::new();
        let mut depth_sq = 0;
        let mut paren = String::new();
        let mut in_paren = false;
        for c in line.chars() {
            if c == '[' {
                depth_sq += 1;
                continue;
            }
            if c == ']' {
                depth_sq = (depth_sq - 1).max(0);
                continue;
            }
            if depth_sq > 0 {
                continue;
            }
            if c == '(' {
                in_paren = true;
                paren.clear();
                continue;
            }
            if c == ')' && in_paren {
                in_paren = false;
                if !is_direction(&paren) {
                    s.push(' ');
                    s.push_str(&paren);
                    s.push(' ');
                }
                continue;
            }
            if in_paren {
                paren.push(c);
            } else {
                s.push(c);
            }
        }
        if in_paren {
            s.push_str(&paren);
        }
        let toks: Vec<&str> = s.split_whitespace().collect();
        if toks.len() >= 2 && toks.iter().all(|t| is_chord_token(t)) {
            continue;
        }
        let kept: String = s
            .chars()
            .map(|c| if c.is_alphabetic() || c == '\'' || c == '’' || c == '-' || c == '~' || c == '_' || c.is_whitespace() { c } else { ' ' })
            .collect();
        let words: Vec<&str> = kept.split_whitespace().collect();
        if !words.is_empty() {
            out.push(words.join(" "));
        }
    }
    out
}

// ── syllables ─────────────────────────────────────────────────────────────────

fn is_vowel_at(w: &[char], i: usize) -> bool {
    match w[i] {
        'a' | 'e' | 'i' | 'o' | 'u' => true,
        'y' => i > 0 && !matches!(w[i - 1], 'a' | 'e' | 'i' | 'o' | 'u') || (i > 0 && matches!(w[i - 1], 'a' | 'e' | 'o' | 'u') && (i + 1 == w.len() || !is_vowel_letter(w[i + 1]))),
        'w' => i > 0 && matches!(w[i - 1], 'a' | 'e' | 'o') && (i + 1 == w.len() || !is_vowel_letter(w[i + 1])),
        _ => false,
    }
}

fn is_vowel_letter(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

const GOOD_ONSETS: [&str; 22] = ["bl", "br", "cl", "cr", "dr", "fl", "fr", "gl", "gr", "pl", "pr", "tr", "tw", "th", "sh", "ch", "ph", "wh", "kn", "wr", "thr", "str"];

/// An English word split into its syllables by rule: vowel groups are the nuclei, a final silent e does not count, a consonant between
/// vowels goes with the next syllable, a cluster is split after its first consonant unless its end is a natural onset (pl, tr, th ...).
pub fn syllabify_en(word: &str) -> Vec<String> {
    let w: Vec<char> = word.to_lowercase().chars().filter(|c| c.is_alphabetic()).collect();
    // the pieces keep the letters' own case ("I", "Wonder-ful")
    let orig: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    let src: &[char] = if orig.len() == w.len() { &orig } else { &w };
    if w.len() <= 2 {
        return if w.is_empty() { vec![] } else { vec![src.iter().collect()] };
    }
    let n = w.len();
    let mut vowel: Vec<bool> = (0..n).map(|i| is_vowel_at(&w, i)).collect();
    // vowels in a word with no other vowel: the "y" of "my", the "e" of "me"
    let groups_of = |v: &[bool]| -> usize { (0..n).filter(|i| v[*i] && (*i == 0 || !v[*i - 1])).count() };
    let g = groups_of(&vowel);
    if g >= 2 {
        // silent final e (not "-le" after a consonant: ta-ble)
        if w[n - 1] == 'e' && !is_vowel_letter(w[n - 2]) && !(w[n - 2] == 'l' && n >= 3 && !is_vowel_letter(w[n - 3])) {
            vowel[n - 1] = false;
        } else if n >= 3 && w[n - 2] == 'e' && w[n - 1] == 'd' && !matches!(w[n - 3], 't' | 'd') && !is_vowel_letter(w[n - 3]) && !(w[n - 3] == 'r' && n >= 4 && !is_vowel_letter(w[n - 4])) {
            vowel[n - 2] = false;
        } else if n >= 3 && w[n - 2] == 'e' && w[n - 1] == 's' && !matches!(w[n - 3], 's' | 'x' | 'z' | 'c' | 'g' | 'h' | 'j') && !is_vowel_letter(w[n - 3]) {
            vowel[n - 2] = false;
        }
    }
    // nuclei: maximal runs of vowel letters
    let mut nuclei: Vec<(usize, usize)> = vec![];
    let mut i = 0;
    while i < n {
        if vowel[i] {
            let s = i;
            while i < n && vowel[i] {
                i += 1;
            }
            nuclei.push((s, i));
        } else {
            i += 1;
        }
    }
    if nuclei.len() <= 1 {
        return vec![src.iter().collect()];
    }
    let mut cuts: Vec<usize> = vec![];
    for k in 0..nuclei.len() - 1 {
        let (a, b) = (nuclei[k].1, nuclei[k + 1].0);
        let cl = b - a;
        let cut = if cl == 0 {
            a
        } else if cl == 1 {
            a
        } else {
            let cluster: String = w[a..b].iter().collect();
            // the longest natural onset at the end of the cluster (the whole cluster may be one: ta-ble)
            let mut best = 0;
            for len in (2..=3.min(cl)).rev() {
                let suf = &cluster[cluster.len() - len..];
                if GOOD_ONSETS.contains(&suf) {
                    best = len;
                    break;
                }
            }
            if best > 0 {
                b - best
            } else if cl == 2 {
                a + 1
            } else {
                b - 1
            }
        };
        cuts.push(cut);
    }
    let mut out = vec![];
    let mut s = 0;
    for c in cuts {
        out.push(src[s..c].iter().collect::<String>());
        s = c;
    }
    out.push(src[s..].iter().collect::<String>());
    out.retain(|x: &String| !x.is_empty());
    out
}

/// English-ish spelling of one syllable -> a spelling `latin_to_ipa` reads right ("make" -> "meik", "light" -> "lait")
pub fn english_phonetic(syl: &str) -> String {
    const EXCEPT: [(&str, &str); 14] = [("love", "lav"), ("give", "giv"), ("have", "hav"), ("live", "liv"), ("come", "kam"), ("some", "sam"), ("done", "dan"), ("one", "wan"), ("gone", "gon"), ("none", "nan"), ("move", "muv"), ("dove", "dav"), ("bove", "bav"), ("the", "da")];
    let s = syl.to_lowercase();
    if let Some((_, r)) = EXCEPT.iter().find(|(k, _)| *k == s) {
        return r.to_string();
    }
    let c: Vec<char> = s.chars().collect();
    // magic e: one vowel, one consonant, a final e: the vowel says its name
    if c.len() >= 3 && c[c.len() - 1] == 'e' && !is_vowel_letter(c[c.len() - 2]) {
        let core = &c[..c.len() - 1];
        if let Some(p) = core.iter().rposition(|x| is_vowel_letter(*x)) {
            let single = p == 0 || !is_vowel_letter(core[p - 1]);
            if single && core[p + 1..].len() == 1 {
                let rep = match core[p] {
                    'a' => "ei",
                    'i' => "ai",
                    'o' => "ou",
                    'u' => "ju",
                    'e' => "i",
                    _ => "",
                };
                if !rep.is_empty() {
                    let mut o: String = core[..p].iter().collect();
                    o.push_str(rep);
                    o.extend(core[p + 1..].iter());
                    return phonetic_edges(&o);
                }
            }
        }
    }
    let mut s = s;
    for (a, b) in [("ee", "i"), ("ea", "i"), ("oo", "u"), ("ou", "au"), ("ai", "ei"), ("ay", "ei"), ("oa", "ou"), ("oy", "oi"), ("ie", "i")] {
        s = s.replace(a, b);
    }
    for (a, b) in [("tion", "shon"), ("sion", "shon"), ("eigh", "ei"), ("igh", "ai")] {
        s = s.replace(a, b);
    }
    phonetic_edges(&s)
}

fn phonetic_edges(s: &str) -> String {
    let mut s = s.replace("ph", "f").replace("ck", "k").replace("gh", "");
    if s.starts_with("kn") || s.starts_with("wr") {
        s = s[1..].to_string();
    }
    if s.starts_with("wh") {
        s = s.replacen("wh", "w", 1);
    }
    s
}

/// A word's letters cut into `n` written syllables (when the dictionary's count differs from the spelling rules): the cuts go at
/// consonant-to-vowel boundaries nearest to even positions, so every piece is a readable chunk of the word ("ev-ery" -> "ev-er-y").
fn split_letters(word: &str, n: usize) -> Vec<String> {
    let c: Vec<char> = word.chars().collect();
    if n <= 1 || c.len() < n {
        let mut v = vec![word.to_string()];
        v.resize(n.max(1), String::new());
        return v;
    }
    let vowel = |ch: char| "aeiouyAEIOUY".contains(ch);
    // where a syllable may start: a consonant before a vowel, or a vowel after a consonant
    let cands: Vec<usize> = (1..c.len()).filter(|i| vowel(c[*i]) && !vowel(c[*i - 1])).map(|i| if i >= 2 && !vowel(c[i - 1]) { i - 1 } else { i }).collect();
    let mut cuts: Vec<usize> = vec![];
    for k in 1..n {
        let want = k * c.len() / n;
        let lo = cuts.last().copied().unwrap_or(0) + 1;
        let best = cands.iter().copied().filter(|x| *x >= lo && *x < c.len()).min_by_key(|x| (*x as i64 - want as i64).abs()).unwrap_or(want.max(lo).min(c.len() - 1));
        cuts.push(best.max(lo).min(c.len() - (n - k)));
    }
    let mut out = vec![];
    let mut s = 0;
    for x in cuts.into_iter().chain(std::iter::once(c.len())) {
        out.push(c[s..x].iter().collect());
        s = x;
    }
    out
}

/// a lyric word is a sustain marker
fn is_sustain_token(t: &str) -> bool {
    matches!(t, "~" | "_" | "-" | "--" | "—" | "–")
}

fn script_class(c: char) -> u8 {
    match c as u32 {
        0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F => 1, // Hangul
        0x3040..=0x30FF | 0x31F0..=0x31FF => 2,                    // kana
        0x4E00..=0x9FFF | 0x3400..=0x4DBF => 3,                    // Han
        0x0E00..=0x0E7F => 4,                                      // Thai
        0x0250..=0x02AF | 0x1D00..=0x1DBF => 5,                    // IPA
        _ => 0,
    }
}

/// One syllable as it is written and as it is sung (IPA); an empty `ipa` is a sustain.
#[derive(Clone, Debug, PartialEq)]
pub struct Syl {
    pub orth: String,
    pub ipa: String,
}

/// A line of lyrics as syllables (Latin words by rule or by the hyphens already in them).
pub fn syllabify_line(line: &str, lang: Lang) -> Vec<Syl> {
    let mut out = vec![];
    for tok in line.split_whitespace() {
        if is_sustain_token(tok) {
            out.push(Syl { orth: "~".into(), ipa: String::new() });
            continue;
        }
        let first = tok.chars().find(|c| c.is_alphabetic());
        let class = first.map(script_class).unwrap_or(0);
        let forced_ipa = lang == Lang::Ipa || (lang == Lang::Auto && tok.chars().any(|c| script_class(c) == 5));
        if forced_ipa {
            for p in tok.split('-').filter(|p| !p.is_empty()) {
                out.push(Syl { orth: p.to_string(), ipa: p.to_string() });
            }
            continue;
        }
        match class {
            1 => {
                for c in tok.chars().filter(|c| script_class(*c) == 1) {
                    let s = c.to_string();
                    let ipa = lyrics::hangul_to_ipa(&s);
                    if !ipa.is_empty() {
                        out.push(Syl { orth: s, ipa });
                    }
                }
            }
            2 => {
                let chars: Vec<char> = tok.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    let c = chars[i];
                    if c == 'ー' {
                        out.push(Syl { orth: "ー".into(), ipa: String::new() });
                        i += 1;
                        continue;
                    }
                    if c == 'っ' || c == 'ッ' {
                        i += 1;
                        continue;
                    }
                    let mut g = c.to_string();
                    while i + 1 < chars.len() && "ゃゅょャュョぁぃぅぇぉァィゥェォ".contains(chars[i + 1]) {
                        g.push(chars[i + 1]);
                        i += 1;
                    }
                    let ipa = lyrics::kana_to_ipa(&g);
                    if !ipa.is_empty() {
                        out.push(Syl { orth: g, ipa });
                    }
                    i += 1;
                }
            }
            3 => {
                for c in tok.chars().filter(|c| script_class(*c) == 3) {
                    let s = c.to_string();
                    let ipa = lyrics::to_ipa(&s, Lang::Pinyin);
                    if !ipa.is_empty() {
                        out.push(Syl { orth: s, ipa });
                    }
                }
            }
            4 => {
                let ipa = lyrics::thai_to_ipa(tok);
                if !ipa.is_empty() {
                    out.push(Syl { orth: tok.to_string(), ipa });
                }
            }
            _ => {
                // a real English word: as the dictionary says it (cmudict.rs), split into its sung syllables; a word hyphenated by
                // hand, or Latin read letter by letter (solfege, romaji), keeps the spelling rules below
                let plain: String = tok.chars().filter(|c| c.is_alphabetic() || *c == '\'' || *c == '’').collect();
                if lang != Lang::Latin && !tok.contains('-') {
                    if let Some(ipas) = crate::cmudict::lookup(&plain) {
                        let letters: String = plain.chars().filter(|c| c.is_alphabetic()).collect();
                        // the spelling of each syllable when the rules split the word the same way, else the word on the first
                        let mut orth = syllabify_en(&letters);
                        if orth.len() != ipas.len() {
                            orth = split_letters(&letters, ipas.len());
                        }
                        for (ipa, o) in ipas.into_iter().zip(orth) {
                            out.push(Syl { orth: o, ipa });
                        }
                        continue;
                    }
                }
                let word: String = tok.chars().filter(|c| c.is_alphabetic() || *c == '-').collect();
                let parts: Vec<String> = if word.contains('-') { word.split('-').filter(|p| !p.is_empty()).map(|p| p.to_string()).collect() } else { syllabify_en(&word) };
                for p in parts {
                    let ph = if lang == Lang::Latin && p.chars().count() <= 3 { p.to_lowercase() } else { english_phonetic(&p) };
                    let ipa = lyrics::latin_to_ipa(&ph);
                    if !ipa.is_empty() {
                        out.push(Syl { orth: p, ipa });
                    }
                }
            }
        }
    }
    out
}

/// the cleaned lyrics with every Latin word hyphenated into syllables (what the fit will sing, in letters), lines joined with ` / `
pub fn prepare(raw: &str) -> String {
    clean(raw)
        .iter()
        .map(|l| {
            l.split_whitespace()
                .map(|t| {
                    let c = t.chars().find(|c| c.is_alphabetic());
                    if t.contains('-') || is_sustain_token(t) || c.map(script_class).unwrap_or(0) != 0 || c.is_none() {
                        t.to_string()
                    } else {
                        let s = syllabify_en(t);
                        if s.is_empty() {
                            t.to_string()
                        } else {
                            s.join("-")
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase()
}

/// Take a model's hyphenated version of the lyrics: every word whose letters still match the original is kept as the model split it
/// (1 to 8 syllables), every other word is split by rule. Lines are matched by position when the counts agree, else by word order.
pub fn accept_split(raw: &str, reply: &str) -> String {
    let orig = clean(raw);
    let rep = clean(reply.trim().trim_start_matches("```").trim_end_matches("```"));
    let flat = |ls: &[String]| -> Vec<String> { ls.iter().flat_map(|l| l.split_whitespace().map(|w| w.to_string())).collect() };
    let rule = |w: &str| -> String {
        let c = w.chars().find(|c| c.is_alphabetic());
        if w.contains('-') || is_sustain_token(w) || c.map(script_class).unwrap_or(0) != 0 || c.is_none() {
            w.to_string()
        } else {
            syllabify_en(w).join("-")
        }
    };
    let pick = |w: &str, r: Option<&String>| -> String {
        if let Some(r) = r {
            let parts = r.split('-').filter(|p| !p.is_empty()).count();
            if letters(r) == letters(w) && (1..=8).contains(&parts) && !r.starts_with('-') && !r.ends_with('-') && script_class(w.chars().find(|c| c.is_alphabetic()).unwrap_or('a')) == 0 {
                return r.trim_matches('-').to_string();
            }
        }
        rule(w)
    };
    let mut lines = vec![];
    let same_lines = orig.len() == rep.len();
    let rep_flat = flat(&rep);
    let mut at = 0usize;
    for (li, l) in orig.iter().enumerate() {
        let ws: Vec<&str> = l.split_whitespace().collect();
        let mut o = vec![];
        for (wi, w) in ws.iter().enumerate() {
            let r = if same_lines { rep[li].split_whitespace().nth(wi).map(|x| x.to_string()) } else { rep_flat.get(at + wi).cloned() };
            o.push(pick(w, r.as_ref()));
        }
        at += ws.len();
        lines.push(o.join(" "));
    }
    lines.join(" / ")
}

pub fn qwen_system() -> String {
    "You prepare song lyrics for a singing synthesizer. Remove section labels like [Verse] and [Chorus], chord symbols and anything that is not sung. Keep one line of lyrics per line. Split every word into the syllables it is sung with, joined by hyphens, as in won-der-ful, ba-by, hel-lo, fire, love, to-geth-er. One-syllable words stay as they are. Do not change, add or drop any word. Answer with the lyrics only.".to_string()
}

pub fn qwen_example() -> String {
    "<|user|>\n[Verse 1]\nAm       F\nWonderful tonight, my baby\nI'll hold you in the morning light (x2)\n<|assistant|>\nWon-der-ful to-night my ba-by\nI'll hold you in the morn-ing light\n".to_string()
}

pub fn qwen_prompt(raw: &str) -> String {
    format!("{}<|user|>\n{}\n", qwen_example(), clean_keep_labels(raw))
}

/// what the model is shown: the pasted text with line breaks, trimmed (it is told to remove the labels itself)
fn clean_keep_labels(raw: &str) -> String {
    let text = raw.replace('\r', "").replace(" / ", "\n");
    let s: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).take(80).collect();
    s.join("\n")
}

// ── fitting ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Group {
    start: f64,
    len: f64,
    end: f64,
    idx: Vec<usize>,
    top: usize,
}

fn groups(notes: &[NoteIn]) -> Vec<Group> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start).then(notes[*a].key.cmp(&notes[*b].key)));
    let mut gs: Vec<Group> = vec![];
    for i in order {
        let n = &notes[i];
        match gs.last_mut() {
            Some(g) if n.start - g.start < 0.06 => {
                g.idx.push(i);
                g.len = g.len.max(n.len);
                g.end = g.start + g.len;
                if n.key >= notes[g.top].key {
                    g.top = i;
                }
            }
            _ => gs.push(Group { start: n.start, len: n.len, end: n.start + n.len, idx: vec![i], top: i }),
        }
    }
    gs
}

fn break_score(g: &Group, prev: Option<&Group>) -> f64 {
    let on = |x: f64, m: f64| (x / m - (x / m).round()).abs() < 0.02;
    let mut sc = 0.0;
    if on(g.start, 4.0) {
        sc += 2.5;
    } else if on(g.start, 1.0) {
        sc += 1.5;
    } else if on(g.start, 0.5) {
        sc += 0.6;
    }
    if let Some(p) = prev {
        let gap = g.start - p.end;
        if gap >= 0.5 {
            sc += 3.0;
        } else if gap >= 0.2 {
            sc += 1.0;
        }
        if p.len >= 1.0 {
            sc += 0.8;
        }
    }
    if g.len >= 0.75 {
        sc += 0.8;
    }
    if g.len < 0.2 {
        sc -= 2.0;
    }
    sc
}

type Slot = Option<Vec<String>>;

/// Spread `units` (IPA syllables, "" = sustain) over the groups `gs[idxs[..]]`.
fn assign(gs: &[Group], idxs: &[usize], units: &[String], mode: Mode, looped: bool) -> Vec<Slot> {
    let n = idxs.len();
    let s = units.len();
    if n == 0 {
        return vec![];
    }
    if s == 0 {
        return vec![None; n];
    }
    let strict = |units: &[String]| -> Vec<Slot> { (0..n).map(|i| units.get(i).map(|u| vec![u.clone()])).collect() };
    if looped && s < n {
        return (0..n).map(|i| Some(vec![units[i % s].clone()])).collect();
    }
    match mode {
        Mode::Strict => strict(units),
        Mode::Even => {
            if s <= n {
                let mut out: Vec<Slot> = vec![Some(vec![]); n];
                for k in 0..s {
                    out[k * n / s] = Some(vec![units[k].clone()]);
                }
                out
            } else {
                (0..n).map(|i| Some(units[i * s / n..(i + 1) * s / n].to_vec())).collect()
            }
        }
        Mode::Fit => {
            if s == n {
                strict(units)
            } else if s < n {
                let score = |i: usize| -> f64 {
                    let g = &gs[idxs[i]];
                    break_score(g, if i > 0 { Some(&gs[idxs[i - 1]]) } else { None })
                };
                // the last syllable of the line (its last word) is sung on the phrase's final note, held there: the last long note of
                // the last three if the final one is a short pickup
                let anchor = {
                    let last = n - 1;
                    if gs[idxs[last]].len >= 0.5 || n < 3 {
                        last
                    } else {
                        (n.saturating_sub(3)..n).rev().find(|i| gs[idxs[*i]].len >= 0.75).unwrap_or(last)
                    }
                };
                // (a line of one or two syllables - a single word - is simply spread over its notes)
                let anchor = if s >= 3 { anchor.max(s - 1) } else { n - 1 };
                let mut starts = vec![0usize];
                for k in 1..s {
                    if s >= 3 && k == s - 1 {
                        starts.push(anchor.max(starts[k - 1] + 1).min(n - 1));
                        continue;
                    }
                    let span = if s >= 3 { anchor as f64 } else { n as f64 };
                    let parts = if s >= 3 { (s - 1) as f64 } else { s as f64 };
                    let ideal = k as f64 * span / parts;
                    let w = (span / parts * 0.5).max(1.0);
                    let lo = (starts[k - 1] + 1).max((ideal - w).ceil() as usize);
                    let limit = if s >= 3 { anchor - (s - 1 - k) } else { n - (s - k) };
                    let lo = lo.min(limit);
                    let hi = limit.min((ideal + w).floor() as usize).max(lo);
                    let mut best = lo;
                    let mut bs = f64::MIN;
                    for i in lo..=hi {
                        let v = score(i) - 0.3 * (i as f64 - ideal).abs();
                        if v > bs + 1e-9 {
                            bs = v;
                            best = i;
                        }
                    }
                    starts.push(best);
                }
                let mut out: Vec<Slot> = vec![Some(vec![]); n];
                for (k, st) in starts.iter().enumerate() {
                    out[*st] = Some(vec![units[k].clone()]);
                }
                out
            } else {
                // more syllables than notes: one per note, and the longest notes take a second (never more than two). The second goes to
                // notes of a beat or more first, then to half-beat notes, then to any; only a line that still does not fit gets a third on
                // its longest notes, since dropping words is worse
                let mut count = vec![1usize; n];
                let mut left = s - n;
                for (min_len, cap) in [(0.9f64, 2usize), (0.5, 2), (0.0, 2), (0.0, 3), (0.0, 4), (0.0, usize::MAX)] {
                    while left > 0 {
                        let mut best: Option<usize> = None;
                        let mut bv = f64::MIN;
                        for i in 0..n {
                            if gs[idxs[i]].len < min_len || count[i] >= cap {
                                continue;
                            }
                            let v = gs[idxs[i]].len / count[i] as f64;
                            if v > bv + 1e-9 {
                                bv = v;
                                best = Some(i);
                            }
                        }
                        match best {
                            Some(i) => {
                                count[i] += 1;
                                left -= 1;
                            }
                            None => break,
                        }
                    }
                }
                let mut at = 0;
                count
                    .iter()
                    .map(|c| {
                        let v = units[at..at + c].to_vec();
                        at += c;
                        Some(v)
                    })
                    .collect()
            }
        }
    }
}

/// Cut the singable notes into one stretch per lyric line (stretch k = positions `r[k].0 .. r[k].1`) so that lines end where the music
/// breathes: a rest, a bar line or a long note. Each line wants room for its syllables (one per note, a second only on long notes), a
/// stretch that is about as long as its line, and a start on a strong break; the best cut over the whole song is found by dynamic
/// programming, so the last word of a line is the last thing in its phrase and no word is split across lines.
fn split_lines(gs: &[Group], singable: &[usize], counts: &[usize]) -> Option<Vec<(usize, usize)>> {
    let n = singable.len();
    let l = counts.len();
    if l == 0 || n < l {
        return None;
    }
    // how well does a line start at position i? (the first position is free)
    let bs: Vec<f64> = (0..n).map(|i| if i == 0 { 0.0 } else { break_score(&gs[singable[i]], Some(&gs[singable[i - 1]])) }).collect();
    // prefix count of long notes (they can take a second syllable)
    let mut longs = vec![0usize; n + 1];
    for i in 0..n {
        longs[i + 1] = longs[i] + (gs[singable[i]].len >= 0.9) as usize;
    }
    let inf = f64::INFINITY;
    let mut dp = vec![vec![inf; n + 1]; l + 1];
    let mut back = vec![vec![0usize; n + 1]; l + 1];
    dp[0][0] = 0.0;
    for k in 1..=l {
        let syl = counts[k - 1] as f64;
        let hi_i = if k == l { n } else { n - (l - k) };
        let lo_i = if k == l { n } else { k };
        for i in lo_i..=hi_i {
            for j in (k - 1)..i {
                if !dp[k - 1][j].is_finite() {
                    continue;
                }
                let groups = (i - j) as f64;
                let cap = groups + (longs[i] - longs[j]) as f64;
                let mut c = 0.0;
                if syl > cap {
                    c += 25.0 * (syl - cap);
                } else if syl > groups {
                    c += 0.4 * (syl - groups) / groups;
                } else {
                    c += 0.6 * (groups - syl) / groups;
                }
                if k > 1 {
                    c -= 1.2 * bs[j].clamp(0.0, 6.0);
                }
                // a line that ends on a long note is a line that ends
                if gs[singable[i - 1]].len >= 0.75 {
                    c -= 0.5;
                }
                let v = dp[k - 1][j] + c;
                if v < dp[k][i] {
                    dp[k][i] = v;
                    back[k][i] = j;
                }
            }
        }
    }
    if !dp[l][n].is_finite() {
        return None;
    }
    let mut out = vec![(0usize, 0usize); l];
    let mut i = n;
    for k in (1..=l).rev() {
        let j = back[k][i];
        out[k - 1] = (j, i);
        i = j;
    }
    Some(out)
}

/// a note split in two on the sixteenth grid (the first half rounded), when it is at least `min` beats long
fn split_longest(line: &mut Vec<NoteIn>, min: f64) -> bool {
    let Some(i) = (0..line.len()).filter(|i| line[*i].len >= min - 1e-9).max_by(|a, b| line[*a].len.total_cmp(&line[*b].len)) else { return false };
    let n = NoteIn { start: line[i].start, len: line[i].len, key: line[i].key };
    let half = ((n.len / 2.0 / 0.25).round() * 0.25).max(0.25);
    line[i].len = half;
    line.insert(i + 1, NoteIn { start: n.start + half, len: n.len - half, key: n.key });
    true
}

/// The sung line reshaped to the lyrics (a monophonic line: the top note of every chord). The lines of the lyrics are laid over the
/// notes where the music breathes (the same cut the fitter uses); each line then gets exactly one note per syllable - the longest notes
/// split (never below a sixteenth), surplus notes joined to the note before them (shortest first, so the melody keeps its long notes) -
/// and its last note, the line's last word, is held (at least a beat and a half when there is room, at most four) with a breath before
/// the next line. With `looped` the lyrics repeat while the melody has room for another round. Fitting the result is one syllable per
/// note; only a line with more syllables than sixteenths left gets two on its longest notes.
pub fn shape(notes: &[NoteIn], raw: &str, o: &Opts) -> Vec<NoteIn> {
    let base: Vec<usize> = clean(raw).iter().map(|l| syllabify_line(l, o.lang).iter().filter(|s| !s.ipa.is_empty()).count()).filter(|c| *c > 0).collect();
    let gs = groups(notes);
    let mut line: Vec<NoteIn> = gs.iter().map(|g| NoteIn { start: g.start, len: g.len, key: notes[g.top].key }).collect();
    for i in 0..line.len().saturating_sub(1) {
        let next = line[i + 1].start;
        if line[i].start + line[i].len > next {
            line[i].len = (next - line[i].start).max(0.05);
        }
    }
    if base.is_empty() || line.is_empty() {
        return line;
    }
    let mut counts = base.clone();
    if o.looped {
        let per: usize = base.iter().sum();
        let mut rounds = 1;
        while (rounds + 1) * per <= line.len() && rounds < 16 {
            counts.extend(&base);
            rounds += 1;
        }
    }
    let need: usize = counts.iter().sum();
    while line.len() < need && split_longest(&mut line, 0.5) {}
    let gl = groups(&line);
    let singable: Vec<usize> = (0..gl.len()).collect();
    let ranges = if counts.len() >= 2 { split_lines(&gl, &singable, &counts) } else { None }.unwrap_or_else(|| {
        if counts.len() == 1 {
            vec![(0, line.len())]
        } else {
            // not enough notes for the lines even after splitting: share them out evenly
            let n = line.len();
            let l = counts.len();
            (0..l).map(|k| (k * n / l, ((k + 1) * n / l).max(k * n / l + 1).min(n))).collect()
        }
    });
    let mut out: Vec<NoteIn> = vec![];
    for (k, (a, b)) in ranges.iter().enumerate() {
        if a >= b {
            continue;
        }
        let mut seg: Vec<NoteIn> = line[*a..*b].iter().map(|n| NoteIn { start: n.start, len: n.len, key: n.key }).collect();
        let s = counts[k];
        while seg.len() < s && split_longest(&mut seg, 0.5) {}
        while seg.len() > s && seg.len() > 1 {
            // the shortest note (not the line's first or last) joins the one before it
            let cand = if seg.len() > 2 { 1..seg.len() - 1 } else { 1..seg.len() };
            let Some(i) = cand.min_by(|x, y| seg[*x].len.total_cmp(&seg[*y].len)) else { break };
            let end = seg[i].start + seg[i].len;
            seg.remove(i);
            let p = &mut seg[i - 1];
            p.len = (end - p.start).max(p.len);
        }
        // the last word is held, then a breath before the next line
        let next = ranges.get(k + 1).filter(|(a2, b2)| a2 < b2).map(|(a2, _)| line[*a2].start);
        if let Some(last) = seg.last_mut() {
            let hold = last.len.max(1.5).min(4.0).max(last.len.min(4.0));
            last.len = match next {
                Some(ns) => {
                    let room = ns - last.start;
                    let breath = if room >= 2.0 { 0.5 } else { 0.25 };
                    (room - breath).min(hold).max(0.25)
                }
                None => hold,
            };
        }
        out.extend(seg);
    }
    out
}

/// what a note stores for a set of syllables: `~` alone sustains, otherwise the IPA syllables joined by `-`
fn compose(units: &[String]) -> String {
    let parts: Vec<&str> = units.iter().map(|u| u.as_str()).filter(|u| !u.is_empty()).collect();
    if parts.is_empty() {
        return "~".to_string();
    }
    let lead = units.first().map(|u| u.is_empty()).unwrap_or(false);
    format!("{}{}", if lead { "~" } else { "" }, parts.join("-"))
}

/// Fit the lyrics to the notes: one stored lyric per note (same order as `notes`) and what happened.
pub fn fit(notes: &[NoteIn], raw: &str, o: &Opts) -> (Vec<String>, Info) {
    let lines: Vec<Vec<String>> = clean(raw).iter().map(|l| syllabify_line(l, o.lang).into_iter().map(|s| s.ipa).collect::<Vec<_>>()).filter(|l: &Vec<String>| !l.is_empty()).collect();
    fit_lines(notes, lines, o)
}

/// The same fit as `fit`, but giving what is sung as written: per note the syllables joined by `-`, a leading space where a word starts
/// (`" ly-ric"`), `~` for a sustain, empty for a rest. The spread is identical to `fit`'s because the units are in the same places.
pub fn fit_text(notes: &[NoteIn], raw: &str, o: &Opts) -> Vec<String> {
    let lines: Vec<Vec<String>> = clean(raw)
        .iter()
        .map(|l| {
            let mut v: Vec<String> = vec![];
            for tok in l.split_whitespace() {
                for (k, s) in syllabify_line(tok, o.lang).into_iter().enumerate() {
                    // the first sung unit of a line carries a line break marker
                    let brk = if v.iter().all(|u| u.is_empty()) && !s.ipa.is_empty() { "\n" } else { "" };
                    v.push(if s.ipa.is_empty() { String::new() } else if k == 0 { format!("{brk} {}", s.orth) } else { format!("{brk}{}", s.orth) });
                }
            }
            v
        })
        .filter(|l: &Vec<String>| !l.is_empty())
        .collect();
    fit_lines(notes, lines, o).0
}

fn fit_lines(notes: &[NoteIn], lines: Vec<Vec<String>>, o: &Opts) -> (Vec<String>, Info) {
    let flat: Vec<String> = lines.iter().flatten().cloned().collect();
    let gs = groups(notes);
    let mut lyric = vec![String::new(); notes.len()];
    let mut info = Info { syllables: flat.iter().filter(|u| !u.is_empty()).count(), lines: lines.len(), groups: gs.len(), chords: gs.iter().filter(|g| g.idx.len() > 1).count(), ..Info::default() };
    if gs.is_empty() || flat.is_empty() {
        return (lyric, info);
    }
    // short notes right after a note sustain it
    let mut auto_tied = vec![false; gs.len()];
    if o.short > 0.0 {
        for i in 1..gs.len() {
            if gs[i].len < o.short && gs[i].start - gs[i - 1].end < 0.5 {
                auto_tied[i] = true;
            }
        }
    }
    let singable: Vec<usize> = (0..gs.len()).filter(|i| !auto_tied[*i]).collect();
    info.bearing = singable.len();
    // phrases: a rest of `gap` beats starts a new one
    let mut phrases: Vec<Vec<usize>> = vec![];
    for (pos, gi) in singable.iter().enumerate() {
        let new = pos == 0 || gs[*gi].start - gs[singable[pos - 1]].end >= o.gap;
        if new {
            phrases.push(vec![pos]);
        } else if let Some(p) = phrases.last_mut() {
            p.push(pos);
        }
    }
    info.phrases = phrases.len();
    let by_phrase = o.looped && lines.len() >= 2 && phrases.len() >= 2 && lines.len() < phrases.len();
    let counts: Vec<usize> = lines.iter().map(|l| l.iter().filter(|u| !u.is_empty()).count().max(1)).collect();
    let cut = if lines.len() >= 2 && !by_phrase { split_lines(&gs, &singable, &counts) } else { None };
    let mut slots: Vec<Slot> = vec![None; singable.len()];
    if let Some(ranges) = cut {
        for (k, (a, b)) in ranges.iter().enumerate() {
            let idxs: Vec<usize> = (*a..*b).map(|pos| singable[pos]).collect();
            let part = assign(&gs, &idxs, &lines[k], o.mode, o.looped);
            for (m, pos) in (*a..*b).enumerate() {
                slots[pos] = part[m].clone();
            }
        }
    } else if by_phrase {
        for (p, positions) in phrases.iter().enumerate() {
            let line = if o.looped { &lines[p % lines.len()] } else { &lines[p] };
            let idxs: Vec<usize> = positions.iter().map(|pos| singable[*pos]).collect();
            let part = assign(&gs, &idxs, line, o.mode, o.looped);
            for (k, pos) in positions.iter().enumerate() {
                slots[*pos] = part[k].clone();
            }
        }
    } else {
        slots = assign(&gs, &singable, &flat, o.mode, o.looped);
    }
    let give = |lyric: &mut Vec<String>, g: &Group, text: &str| match o.chord {
        Chord::All => {
            for i in &g.idx {
                lyric[*i] = text.to_string();
            }
        }
        Chord::Top => {
            for i in &g.idx {
                lyric[*i] = if *i == g.top { text.to_string() } else { String::new() };
            }
        }
    };
    for (pos, gi) in singable.iter().enumerate() {
        match &slots[pos] {
            Some(units) => give(&mut lyric, &gs[*gi], &compose(units)),
            None => give(&mut lyric, &gs[*gi], ""),
        }
    }
    for gi in 0..gs.len() {
        if auto_tied[gi] {
            give(&mut lyric, &gs[gi], "~");
        }
    }
    (lyric, info)
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(spec: &[(f64, f64, u8)]) -> Vec<NoteIn> {
        spec.iter().map(|(s, l, k)| NoteIn { start: *s, len: *l, key: *k }).collect()
    }

    fn syl(w: &str) -> Vec<String> {
        syllabify_en(w)
    }

    #[test]
    fn a_line_shaped_to_the_lyrics_sings_one_syllable_per_note_and_holds_its_last_word() {
        // a melody of 4 bars: busy sixteenths in bar 1, two long notes, a rest, then four quarter notes
        let mut spec: Vec<(f64, f64, u8)> = (0..16).map(|i| (i as f64 * 0.25, 0.25, 60 + (i % 5) as u8)).collect();
        spec.extend([(4.0, 2.0, 67), (6.0, 2.0, 65), (9.0, 1.0, 64), (10.0, 1.0, 62), (11.0, 1.0, 60), (12.0, 1.0, 62)]);
        let n = notes(&spec);
        let lyr = "walking in the city lights tonight\nhold me now";
        let mut o = Opts::parse("fit:shape:0:0.3:top:1.0:auto").unwrap();
        assert!(o.shape);
        let shaped = shape(&n, lyr, &o);
        // 9 + 3 syllables: 12 notes, nothing shorter than a sixteenth
        assert_eq!(shaped.len(), 12, "{:?}", shaped.iter().map(|x| (x.start, x.len)).collect::<Vec<_>>());
        assert!(shaped.iter().all(|x| x.len >= 0.25 - 1e-9));
        o.short = 0.0;
        let (ly, _) = fit(&shaped, lyr, &o);
        assert!(ly.iter().all(|l| !l.is_empty() && !l.contains('-') && l != "~"), "one syllable on every note: {ly:?}");
        // the last word of the first line is held and followed by a breath
        let end1 = &shaped[8];
        assert!(end1.len >= 1.0, "{:?}", (end1.start, end1.len));
        assert!(shaped[9].start - (end1.start + end1.len) >= 0.25 - 1e-9);
        // too few notes: long ones are split so every syllable gets its own
        let few = notes(&[(0.0, 4.0, 60), (4.0, 4.0, 62)]);
        let s2 = shape(&few, "one two three four five six", &o);
        assert_eq!(s2.len(), 6);
    }

    #[test]
    fn a_word_is_cut_into_as_many_written_pieces_as_it_has_syllables() {
        for (w, n) in [("every", 3), ("orange", 2), ("fire", 2), ("a", 1), ("rhythm", 2), ("beautiful", 3)] {
            let v = split_letters(w, n);
            assert_eq!(v.len(), n, "{w}");
            assert_eq!(v.concat(), w);
            assert!(v.iter().all(|p| !p.is_empty()), "{w}: {v:?}");
        }
    }

    #[test]
    fn english_words_split_into_the_syllables_they_are_sung_with() {
        assert_eq!(syl("hello"), ["hel", "lo"]);
        assert_eq!(syl("wonderful"), ["won", "der", "ful"]);
        assert_eq!(syl("beautiful"), ["beau", "ti", "ful"]);
        assert_eq!(syl("baby"), ["ba", "by"]);
        assert_eq!(syl("happy"), ["hap", "py"]);
        assert_eq!(syl("water"), ["wa", "ter"]);
        assert_eq!(syl("table"), ["ta", "ble"]);
        assert_eq!(syl("monster"), ["mons", "ter"]);
        assert_eq!(syl("hundred"), ["hun", "dred"]);
        assert_eq!(syl("angry"), ["an", "gry"]);
        for one in ["love", "time", "make", "light", "dream", "fire", "you", "the", "walked", "my", "me", "night", "heart", "cry"] {
            assert_eq!(syl(one).len(), 1, "{one}: {:?}", syl(one));
        }
        assert_eq!(syl("wanted").len(), 2);
        assert_eq!(syl("yesterday").len(), 3);
        assert_eq!(syl("morning").len(), 2);
        assert!(syl("").is_empty());
        assert_eq!(syl("Wonderful"), ["Won", "der", "ful"], "case is kept");
        assert_eq!(syl("I"), ["I"]);
    }

    #[test]
    fn spelling_becomes_something_the_singer_can_say() {
        assert_eq!(english_phonetic("make"), "meik");
        assert_eq!(english_phonetic("time"), "taim");
        assert_eq!(english_phonetic("light"), "lait");
        assert_eq!(english_phonetic("love"), "lav");
        assert_eq!(english_phonetic("dream"), "drim");
        assert_eq!(english_phonetic("knight"), "nait");
    }

    #[test]
    fn pasted_text_loses_labels_chords_and_directions() {
        let raw = "[Verse 1]\nAm       F\nWonderful tonight, my baby!\n(x2)\n[Chorus]\nI'll hold you (ooh) in the light\nC G Am";
        let l = clean(raw);
        assert_eq!(l, ["Wonderful tonight my baby", "I'll hold you ooh in the light"], "{l:?}");
        assert_eq!(clean("one / two | three"), ["one", "two", "three"]);
        assert!(clean("").is_empty());
        assert!(clean("[Intro]\n[Chorus]").is_empty());
        // a line that is just "A" or "Am" is a word, not a chord line
        assert_eq!(clean("A"), ["A"]);
    }

    #[test]
    fn other_scripts_split_per_character() {
        let k = syllabify_line("사랑해", Lang::Auto);
        assert_eq!(k.len(), 3);
        let j = syllabify_line("さくら", Lang::Auto);
        assert_eq!(j.len(), 3);
        let jj = syllabify_line("きょう", Lang::Auto);
        assert_eq!(jj.len(), 2, "kyo is one mora: {jj:?}");
        let z = syllabify_line("你好", Lang::Auto);
        assert_eq!(z.len(), 2);
        let s = syllabify_line("hello ~ world", Lang::Auto);
        assert_eq!(s.iter().filter(|x| x.ipa.is_empty()).count(), 1, "the ~ is a sustain");
        assert_eq!(s.len(), 4);
        // hyphens are trusted as syllable breaks
        assert_eq!(syllabify_line("fan-tas-tic", Lang::Auto).len(), 3);
    }

    #[test]
    fn matching_counts_give_one_syllable_per_note() {
        let n = notes(&[(0.0, 1.0, 60), (1.0, 1.0, 62), (2.0, 1.0, 64), (3.0, 1.0, 65)]);
        let (l, info) = fit(&n, "do re mi fa", &Opts::default());
        assert!(l.iter().all(|x| !x.is_empty() && !x.starts_with('~')), "{l:?}");
        assert_eq!(info.syllables, 4);
        assert_eq!(info.groups, 4);
    }

    #[test]
    fn extra_notes_sustain_the_syllable_before_them_and_land_on_strong_beats() {
        // 2 syllables over 8 eighth-note-ish notes: the second syllable should start at the downbeat of bar 2, not at a random note
        let n = notes(&[(0.0, 0.5, 60), (0.5, 0.5, 62), (1.0, 0.5, 64), (1.5, 0.5, 65), (2.0, 0.5, 67), (2.5, 0.5, 65), (3.0, 0.5, 64), (3.5, 0.5, 62)]);
        let (l, _) = fit(&n, "hel lo", &Opts { short: 0.0, ..Opts::default() });
        assert!(!l[0].starts_with('~'));
        let second = (1..l.len()).find(|i| !l[*i].starts_with('~')).expect("the second syllable is placed");
        assert!([3, 4, 5].contains(&second), "the second syllable lands near the middle: at note {second}: {l:?}");
        assert!(l.iter().enumerate().all(|(i, x)| i == 0 || i == second || x == "~"));
    }

    #[test]
    fn more_syllables_than_notes_pack_the_longest_notes() {
        let n = notes(&[(0.0, 0.5, 60), (1.0, 3.0, 64), (4.0, 0.5, 62)]);
        // 4 syllables on 3 notes: one each, and the 3-beat note takes the one extra (two syllables at most)
        let (l, _) = fit(&n, "a b c d", &Opts { short: 0.0, ..Opts::default() });
        assert_eq!(l[1].matches('-').count(), 1, "{l:?}");
        assert!(!l[0].contains('-') && !l[2].contains('-'));
        // 5 do not fit that way: a third syllable is the last resort, no word is dropped
        let (l5, _) = fit(&n, "a b c d e", &Opts { short: 0.0, ..Opts::default() });
        let sung: usize = l5.iter().map(|x| x.split('-').filter(|u| !u.is_empty() && *u != "~").count()).sum();
        assert_eq!(sung, 5, "{l5:?}");
    }

    #[test]
    fn short_notes_never_take_two_syllables_while_long_ones_can() {
        // four quick notes, two long ones, eight syllables
        let n = notes(&[(0.0, 0.25, 60), (0.25, 0.25, 62), (0.5, 0.25, 64), (0.75, 0.25, 65), (1.0, 2.0, 67), (3.0, 2.0, 65)]);
        let (l, _) = fit(&n, "one two three four five six eight nine", &Opts { short: 0.0, ..Opts::default() });
        for (i, x) in l.iter().enumerate() {
            assert!(x.matches('-').count() <= 1, "note {i} has more than two syllables: {l:?}");
            if i < 4 {
                assert_eq!(x.matches('-').count(), 0, "a short note took two syllables: {l:?}");
            }
        }
        assert_eq!(l[4].matches('-').count() + l[5].matches('-').count(), 2, "{l:?}");
    }

    #[test]
    fn a_line_ends_with_its_last_word_at_the_end_of_its_phrase() {
        // two phrases (a 3-beat rest between): the first has 6 notes, the second 4. Line 1 has 4 syllables, line 2 has 3.
        let mut spec = vec![];
        for k in 0..6 {
            spec.push((k as f64 * 0.5, 0.5, 60 + k as u8));
        }
        spec.push((5.5, 0.5, 60));
        let _ = spec.pop();
        spec.push((2.5, 1.5, 67));
        let first_end = spec.len() - 1;
        for k in 0..4 {
            spec.push((9.0 + k as f64 * 0.5, if k == 3 { 2.0 } else { 0.5 }, 62 + k as u8));
        }
        let n = notes(&spec);
        let o = Opts { short: 0.0, ..Opts::default() };
        let (l, info) = fit(&n, "love is all around\ncome home tonight", &o);
        assert_eq!(info.phrases, 2);
        // the first line's last syllable ("round") is sung on the first phrase's last note, and the second line starts the second phrase
        assert!(!l[first_end].is_empty() && !l[first_end].starts_with('~'), "last note of phrase 1: {l:?}");
        assert!(!l[first_end + 1].is_empty() && !l[first_end + 1].starts_with('~'), "phrase 2 starts a new line: {l:?}");
        let last = l.len() - 1;
        assert!(!l[last].starts_with('~'), "the last word lands on the last note: {l:?}");
    }

    #[test]
    fn lines_follow_the_music_when_there_are_no_rests() {
        // sixteen steady notes, with a long note every four, and four lines of different lengths
        let spec: Vec<(f64, f64, u8)> = (0..16).map(|i| (i as f64, if i % 4 == 3 { 1.0 } else { 0.5 }, 60 + (i % 5) as u8)).collect();
        let n = notes(&spec);
        let text = "walking down the road\nthe sun is going down and I feel fine\nhome\none more time";
        let (l, _) = fit(&n, text, &Opts { short: 0.0, ..Opts::default() });
        assert!(l.iter().all(|x| x.matches('-').count() <= 1), "{l:?}");
        assert!(!l[0].starts_with('~'));
        assert!(!l[15].starts_with('~') || l[14] != "~", "the song ends on a word: {l:?}");
    }

    #[test]
    fn a_chord_carries_one_sustained_syllable_on_every_note() {
        // two chords of three notes and a long single note
        let n = notes(&[(0.0, 4.0, 60), (0.0, 4.0, 64), (0.0, 4.0, 67), (4.0, 4.0, 62), (4.0, 4.0, 65), (4.0, 4.0, 69), (8.0, 2.0, 60)]);
        let (l, info) = fit(&n, "a-ma-zing", &Opts::default());
        assert_eq!(info.groups, 3);
        assert_eq!(info.chords, 2);
        assert_eq!(l[0], l[1]);
        assert_eq!(l[1], l[2]);
        assert_eq!(l[3], l[4]);
        assert_ne!(l[0], l[3]);
        // top-note mode: only the top note sings, the rest take the default vowel
        let (t, _) = fit(&n, "a-ma-zing", &Opts { chord: Chord::Top, ..Opts::default() });
        assert!(!t[2].is_empty() && t[0].is_empty() && t[1].is_empty(), "{t:?}");
        // one syllable over a chord and the notes that follow: it is held
        let (h, _) = fit(&n, "ah", &Opts::default());
        assert!(!h[0].starts_with('~') && h[3] == "~" && h[4] == "~" && h[6] == "~", "{h:?}");
    }

    #[test]
    fn short_notes_after_a_note_become_sustains() {
        let n = notes(&[(0.0, 1.0, 60), (1.0, 0.15, 62), (1.15, 0.15, 64), (2.0, 1.0, 65)]);
        let (l, info) = fit(&n, "la la", &Opts::default());
        assert_eq!(l[1], "~");
        assert_eq!(l[2], "~");
        assert!(!l[3].starts_with('~'));
        assert_eq!(info.bearing, 2);
        let (l, _) = fit(&n, "a b c d", &Opts { short: 0.0, ..Opts::default() });
        assert!(l.iter().all(|x| !x.starts_with('~')));
    }

    #[test]
    fn lyrics_loop_when_the_notes_outlast_them() {
        let n: Vec<NoteIn> = (0..8).map(|i| NoteIn { start: i as f64, len: 1.0, key: 60 + i as u8 }).collect();
        let plain = fit(&n, "la li lu", &Opts { short: 0.0, ..Opts::default() }).0;
        let looped = fit(&n, "la li lu", &Opts { looped: true, short: 0.0, ..Opts::default() }).0;
        // without loop the lyrics are stretched with sustains, with loop they come round again
        assert!(plain.iter().filter(|x| x.as_str() == "~").count() >= 4);
        assert!(looped.iter().all(|x| !x.is_empty() && x != "~"));
        assert_eq!(looped[0], looped[3]);
        assert_eq!(looped[1], looped[4]);
        assert_ne!(looped[0], looped[1]);
    }

    #[test]
    fn lines_follow_the_phrases_the_rests_make_and_loop_across_them() {
        // three phrases of three notes, separated by 2-beat rests
        let mut spec = vec![];
        for p in 0..3 {
            for k in 0..3 {
                spec.push((p as f64 * 6.0 + k as f64, 0.9, 60 + k as u8));
            }
        }
        let n = notes(&spec);
        let o = Opts { short: 0.0, ..Opts::default() };
        // three lines, three phrases: line 2 starts at the second phrase
        let (l, info) = fit(&n, "one two three\nfour five six\nseven eight nine", &o);
        assert_eq!(info.phrases, 3);
        assert_eq!(l.len(), 9);
        assert!(l.iter().all(|x| !x.is_empty() && x != "~"));
        // two lines over three phrases: without loop the global fit runs; with loop line 1 comes back for phrase 3
        let (lp, _) = fit(&n, "one two three\nfour five six", &Opts { looped: true, ..o.clone() });
        assert_eq!(lp[0], lp[6]);
        assert_eq!(lp[1], lp[7]);
        assert_ne!(lp[0], lp[3]);
    }

    #[test]
    fn modes_differ_and_nothing_panics_on_odd_input() {
        let n: Vec<NoteIn> = (0..12).map(|i| NoteIn { start: i as f64 * 0.5, len: 0.5, key: 60 }).collect();
        let a = fit(&n, "one two three four", &Opts { mode: Mode::Even, short: 0.0, ..Opts::default() }).0;
        let b = fit(&n, "one two three four", &Opts { mode: Mode::Strict, short: 0.0, ..Opts::default() }).0;
        assert_eq!(a.iter().filter(|x| x.as_str() != "~").count(), 4);
        assert_eq!(b.iter().filter(|x| !x.is_empty()).count(), 4, "strict: one each for the first four notes only");
        for text in ["", "   ", "[Chorus]", "~ ~ ~", "-", "日本語 한국어 hello", "a", "(((", "ɑ ɛ ɔ", "😀 😀"] {
            for notes_n in [0usize, 1, 5] {
                let nn: Vec<NoteIn> = (0..notes_n).map(|i| NoteIn { start: i as f64, len: 0.0, key: 60 }).collect();
                for mode in [Mode::Fit, Mode::Even, Mode::Strict] {
                    for looped in [false, true] {
                        let (l, _) = fit(&nn, text, &Opts { mode, looped, ..Opts::default() });
                        assert_eq!(l.len(), notes_n);
                    }
                }
            }
        }
    }

    #[test]
    fn options_round_trip_through_the_clip_setting() {
        let o = Opts::parse("fit:even:1:0.25:top:2:korean").unwrap();
        assert_eq!((o.mode, o.looped, o.chord, o.lang), (Mode::Even, true, Chord::Top, Lang::Korean));
        assert_eq!(o.short, 0.25);
        assert_eq!(o.gap, 2.0);
        assert!(Opts::parse("auto").is_none());
        assert_eq!(Opts::parse("fit").unwrap(), Opts::default());
        assert_eq!(Opts::parse("fit:bogus:x:y:z:w:q").unwrap().mode, Mode::Fit);
    }

    #[test]
    fn a_models_hyphenation_is_kept_only_when_it_matches_the_words() {
        let raw = "Wonderful tonight, my baby\nI hold you";
        let good = "Won-der-ful to-night my ba-by\nI hold you";
        assert_eq!(accept_split(raw, good), "Won-der-ful to-night my ba-by / I hold you");
        // a word the model changed is split by rule instead
        let bad = "Won-der-ful to-day my ba-by\nI hold you";
        assert_eq!(accept_split(raw, bad), "Won-der-ful to-night my ba-by / I hold you");
        // prose, nothing, or the wrong number of lines: the rule-based split
        assert_eq!(accept_split(raw, "Sure! Here you go."), prepare(raw));
        assert_eq!(accept_split(raw, ""), prepare(raw));
        // too many hyphens
        assert_eq!(accept_split("hello", "h-e-l-l-o-w-o-r-l-d"), "hel-lo");
        assert_eq!(prepare("[x]\nhello world"), "hel-lo world");
    }
}
