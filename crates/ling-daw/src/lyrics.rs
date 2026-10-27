//! Lyrics for sung instruments: the phrase grammar and converters from several scripts to IPA.
//!
//! A phrase assigns lyrics to notes, in time order:
//!
//! ```text
//! [so-fa]-[rei][to][mi-do]
//! ```
//!
//! * `[...]` is one note; `-` inside the brackets splits it into several syllables that share the note.
//! * `-` outside the brackets, between two notes, is a **sustain / legato**: the vowel carries over and the pitch
//!   slurs into the next note instead of the next note being sung from a fresh start.
//! * `[-]` (or `[]`) is a note that only sustains the previous vowel.
//! * Without brackets, notes are separated by spaces (`so-fa rei to`), `-` still splits syllables.
//!
//! Each syllable can be IPA, plain Latin letters read phonetically (solfège, romaji, ...), pinyin (tone marks or
//! numbers), Hanzi, Hangul, hiragana / katakana or Thai script. Everything is turned into IPA when the phrase
//! is applied; a note stores `~` (tied) + its IPA syllables joined by `-`.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    Auto,
    Ipa,
    Latin,
    Pinyin,
    Korean,
    Japanese,
    Thai,
}

impl Lang {
    pub fn parse(s: &str) -> Lang {
        match s.trim().to_lowercase().as_str() {
            "ipa" => Lang::Ipa,
            "latin" | "phonetic" | "romaji" | "solfege" => Lang::Latin,
            "pinyin" | "chinese" | "zh" | "mandarin" => Lang::Pinyin,
            "korean" | "ko" | "hangul" => Lang::Korean,
            "japanese" | "ja" | "kana" => Lang::Japanese,
            "thai" | "th" => Lang::Thai,
            _ => Lang::Auto,
        }
    }
}

// ── phrase grammar ───────────────────────────────────────────────────────────

/// Split a phrase into per-note units: (tied, syllables as typed). `None` syllables = a sustain-only note.
pub fn split_phrase(text: &str) -> Vec<(bool, Vec<String>)> {
    let mut out: Vec<(bool, Vec<String>)> = vec![];
    let mut tie = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let syllables = |s: &str| -> Vec<String> { s.split('-').map(|x| x.trim()).filter(|x| !x.is_empty()).map(|x| x.to_string()).collect() };
    while i < chars.len() {
        let c = chars[i];
        if c == '[' {
            let mut j = i + 1;
            let mut body = String::new();
            while j < chars.len() && chars[j] != ']' {
                body.push(chars[j]);
                j += 1;
            }
            out.push((tie, syllables(&body)));
            tie = false;
            i = j + 1;
        } else if c == '-' || c == '~' || c == '_' {
            // between notes: sustain into the next one (only once a note exists)
            if !out.is_empty() {
                tie = true;
            }
            i += 1;
        } else if c.is_whitespace() || c == ']' {
            i += 1;
        } else {
            // bare token up to whitespace / bracket
            let mut tok = String::new();
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '[' && chars[i] != ']' {
                tok.push(chars[i]);
                i += 1;
            }
            let lead = tok.starts_with('-');
            let trail = tok.ends_with('-');
            let body = tok.trim_matches('-');
            if lead && !out.is_empty() {
                tie = true;
            }
            if !body.is_empty() {
                out.push((tie, syllables(body)));
                tie = false;
            }
            if trail {
                tie = true;
            }
        }
    }
    out
}

/// Phrase -> one stored lyric per note: `~` for a tied note, then IPA syllables joined by `-`.
pub fn parse_phrase(text: &str, lang: Lang) -> Vec<String> {
    split_phrase(text)
        .into_iter()
        .map(|(tied, syl)| {
            // an English word on its own (no hand hyphens): as the dictionary says it, all its syllables on this note
            let dict = if syl.len() == 1 && lang == Lang::Auto && detect(&syl[0]) == Lang::Latin { crate::cmudict::lookup(&syl[0]) } else { None };
            let ipa: Vec<String> = match dict {
                Some(v) => v,
                None => syl.iter().map(|s| to_ipa(s, lang)).filter(|s| !s.is_empty()).collect(),
            };
            let mut s = String::new();
            if tied || ipa.is_empty() {
                s.push('~');
            }
            s.push_str(&ipa.join("-"));
            s
        })
        .collect()
}

// ── script detection ─────────────────────────────────────────────────────────

fn script_of(c: char) -> Lang {
    match c as u32 {
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7A3 => Lang::Korean,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Lang::Japanese,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF => Lang::Pinyin,
        0x0E00..=0x0E7F => Lang::Thai,
        0x0250..=0x02AF | 0x1D00..=0x1DBF | 0x02B0..=0x02FF | 0x0361 | 0x035C => Lang::Ipa,
        _ => Lang::Latin,
    }
}

pub fn detect(text: &str) -> Lang {
    let mut found = Lang::Latin;
    for c in text.chars() {
        match script_of(c) {
            Lang::Latin => {}
            Lang::Ipa => {
                if found == Lang::Latin {
                    found = Lang::Ipa
                }
            }
            other => return other,
        }
    }
    if found == Lang::Latin && is_pinyin_with_tone(text) {
        return Lang::Pinyin;
    }
    found
}

fn is_pinyin_with_tone(s: &str) -> bool {
    let t = s.trim();
    let n = t.chars().count();
    n >= 2 && t.chars().last().map(|c| ('1'..='5').contains(&c)).unwrap_or(false) && t.chars().take(n - 1).all(|c| c.is_ascii_alphabetic() || c == 'ü')
}

/// One syllable group (no brackets, no `-`) to IPA; a group may be a whole word (Hangul, Thai, kana ...) giving several syllables joined by `-`.
pub fn to_ipa(text: &str, lang: Lang) -> String {
    let lang = if lang == Lang::Auto { detect(text) } else { lang };
    match lang {
        Lang::Ipa => text.trim().to_string(),
        Lang::Latin | Lang::Auto => latin_to_ipa(text),
        Lang::Pinyin => pinyin_to_ipa(text),
        Lang::Korean => hangul_to_ipa(text),
        Lang::Japanese => kana_to_ipa(text),
        Lang::Thai => thai_to_ipa(text),
    }
}

// ── Latin letters read phonetically (solfège, romaji-ish, ...) ───────────────────────

fn is_vowel_letter(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')
}

pub fn latin_to_ipa(text: &str) -> String {
    let s: Vec<char> = text.to_lowercase().chars().filter(|c| c.is_alphabetic() || *c == 'ː' || *c == ':').collect();
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let nx = s.get(i + 1).copied().unwrap_or(' ');
        let nn = s.get(i + 2).copied().unwrap_or(' ');
        let two = |a: char, b: char| c == a && nx == b;
        if c == nx && !is_vowel_letter(c) {
            // doubled consonant: one (the sustain / closure is handled by the singer)
            i += 1;
            continue;
        }
        if c == 't' && nx == 's' && nn == 'h' {
            out.push_str("tʃ");
            i += 3;
        } else if two('s', 'h') {
            out.push('ʃ');
            i += 2;
        } else if two('c', 'h') {
            out.push_str("tʃ");
            i += 2;
        } else if two('z', 'h') {
            out.push('ʒ');
            i += 2;
        } else if two('t', 'h') {
            out.push('θ');
            i += 2;
        } else if two('n', 'g') {
            out.push('ŋ');
            i += 2;
        } else if two('n', 'y') && is_vowel_letter(nn) {
            out.push('ɲ');
            i += 2;
        } else if two('k', 'h') {
            out.push_str("kʰ");
            i += 2;
        } else if two('d', 'j') {
            out.push_str("dʒ");
            i += 2;
        } else if two('t', 's') {
            out.push_str("ts");
            i += 2;
        } else if two('d', 'z') {
            out.push_str("dz");
            i += 2;
        } else if two('q', 'u') {
            out.push_str("kw");
            i += 2;
        } else {
            match c {
                'a' => out.push('a'),
                'e' => out.push('e'),
                'i' => out.push('i'),
                'o' => out.push('o'),
                'u' => out.push('u'),
                'ü' => out.push('y'),
                'é' | 'è' => out.push('ɛ'),
                'ä' => out.push('æ'),
                'ö' => out.push('ø'),
                'y' => out.push(if is_vowel_letter(nx) { 'j' } else { 'i' }),
                'w' => out.push('w'),
                'c' => out.push(if matches!(nx, 'e' | 'i' | 'y') { 's' } else { 'k' }),
                'g' => out.push('g'),
                'j' => out.push_str("dʒ"),
                'q' => out.push('k'),
                'r' => out.push('ɾ'),
                'x' => out.push_str("ks"),
                ':' | 'ː' => out.push('ː'),
                'ß' => out.push('s'),
                'b' | 'd' | 'f' | 'h' | 'k' | 'l' | 'm' | 'n' | 'p' | 's' | 't' | 'v' | 'z' => out.push(c),
                _ => {}
            }
            i += 1;
        }
    }
    out
}

// ── Korean ───────────────────────────────────────────────────────────────────

const K_INITIAL: [&str; 19] = ["k", "k͈", "n", "t", "t͈", "ɾ", "m", "p", "p͈", "s", "s͈", "", "tɕ", "t͈ɕ", "tɕʰ", "kʰ", "tʰ", "pʰ", "h"];
const K_VOWEL: [&str; 21] = ["a", "ɛ", "ja", "jɛ", "ʌ", "e", "jʌ", "je", "o", "wa", "wɛ", "we", "jo", "u", "wʌ", "we", "wi", "ju", "ɯ", "ɰi", "i"];
const K_FINAL: [&str; 28] = ["", "k̚", "k̚", "k̚", "n", "n", "n", "t̚", "l", "k̚", "m", "p̚", "t̚", "t̚", "p̚", "l", "m", "p̚", "p̚", "t̚", "t̚", "ŋ", "t̚", "t̚", "k̚", "t̚", "p̚", "t̚"];

pub fn hangul_to_ipa(text: &str) -> String {
    // (initial, vowel, final) per syllable block
    let mut blocks: Vec<(usize, usize, usize)> = vec![];
    for c in text.chars() {
        let u = c as u32;
        if (0xAC00..=0xD7A3).contains(&u) {
            let n = u - 0xAC00;
            blocks.push(((n / 588) as usize, ((n % 588) / 28) as usize, (n % 28) as usize));
        }
    }
    let mut out: Vec<String> = vec![];
    let voiced = |ini: usize| -> Option<&'static str> {
        match ini {
            0 => Some("g"),
            3 => Some("d"),
            7 => Some("b"),
            12 => Some("dʑ"),
            _ => None,
        }
    };
    for (n, &(ini, vow, fin)) in blocks.iter().enumerate() {
        let mut onset = K_INITIAL[ini].to_string();
        // a plain stop / affricate between vowels is voiced
        if n > 0 {
            let (_, _, pf) = blocks[n - 1];
            if pf == 0 {
                if let Some(v) = voiced(ini) {
                    onset = v.to_string();
                }
            }
        }
        // liaison: a final consonant moves to an empty onset that follows
        let mut coda = K_FINAL[fin].to_string();
        if n > 0 {
            let (_, _, pf) = blocks[n - 1];
            if pf != 0 && ini == 11 {
                onset = match K_FINAL[pf] {
                    "k̚" => "g",
                    "t̚" => "d",
                    "p̚" => "b",
                    "l" => "ɾ",
                    other => other,
                }
                .to_string();
            }
        }
        if n + 1 < blocks.len() && fin != 0 && blocks[n + 1].0 == 11 {
            coda.clear();
        }
        out.push(format!("{onset}{}{coda}", K_VOWEL[vow]));
    }
    out.join("-")
}

// ── Japanese kana ────────────────────────────────────────────────────────────

fn kana_base(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' => "a", 'い' => "i", 'う' => "ɯ", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "kɯ", 'け' => "ke", 'こ' => "ko",
        'さ' => "sa", 'し' => "ɕi", 'す' => "sɯ", 'せ' => "se", 'そ' => "so",
        'た' => "ta", 'ち' => "tɕi", 'つ' => "tsɯ", 'て' => "te", 'と' => "to",
        'な' => "na", 'に' => "ni", 'ぬ' => "nɯ", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "çi", 'ふ' => "ɸɯ", 'へ' => "he", 'ほ' => "ho",
        'ま' => "ma", 'み' => "mi", 'む' => "mɯ", 'め' => "me", 'も' => "mo",
        'や' => "ja", 'ゆ' => "jɯ", 'よ' => "jo",
        'ら' => "ɾa", 'り' => "ɾi", 'る' => "ɾɯ", 'れ' => "ɾe", 'ろ' => "ɾo",
        'わ' => "wa", 'を' => "o", 'ん' => "ɴ",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gɯ", 'げ' => "ge", 'ご' => "go",
        'ざ' => "dza", 'じ' => "dʑi", 'ず' => "dzɯ", 'ぜ' => "dze", 'ぞ' => "dzo",
        'だ' => "da", 'ぢ' => "dʑi", 'づ' => "dzɯ", 'で' => "de", 'ど' => "do",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bɯ", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pɯ", 'ぺ' => "pe", 'ぽ' => "po",
        'ぁ' => "a", 'ぃ' => "i", 'ぅ' => "ɯ", 'ぇ' => "e", 'ぉ' => "o",
        'ゔ' => "bɯ",
        _ => return None,
    })
}

pub fn kana_to_ipa(text: &str) -> String {
    // katakana -> hiragana
    let chars: Vec<char> = text
        .chars()
        .map(|c| {
            let u = c as u32;
            if (0x30A1..=0x30F6).contains(&u) {
                char::from_u32(u - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let nx = chars.get(i + 1).copied();
        if c == 'っ' {
            // geminate: close the previous syllable with a glottal stop
            if let Some(l) = out.last_mut() {
                l.push('ʔ');
            }
            i += 1;
            continue;
        }
        if c == 'ー' || c == '〜' {
            if let Some(l) = out.last_mut() {
                l.push('ː');
            }
            i += 1;
            continue;
        }
        if let Some(b) = kana_base(c) {
            // small ya / yu / yo after an i-row kana: a palatalised syllable
            if let Some(sm) = nx.filter(|n| matches!(n, 'ゃ' | 'ゅ' | 'ょ')) {
                let v = match sm {
                    'ゃ' => "a",
                    'ゅ' => "ɯ",
                    _ => "o",
                };
                let cons = b.trim_end_matches(|ch| matches!(ch, 'i' | 'ɯ' | 'e'));
                let cons = if cons == "ɕ" || cons == "tɕ" || cons == "dʑ" { cons.to_string() } else { format!("{cons}j") };
                out.push(format!("{cons}{v}"));
                i += 2;
                continue;
            }
            out.push(b.to_string());
        }
        i += 1;
    }
    out.join("-")
}

// ── Mandarin (pinyin, Hanzi) ───────────────────────────────────────────────────

fn strip_tone(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let r = match c {
            'ā' | 'á' | 'ǎ' | 'à' => 'a',
            'ē' | 'é' | 'ě' | 'è' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' => 'v',
            '1'..='5' | '\'' => continue,
            c => c.to_ascii_lowercase(),
        };
        out.push(r);
    }
    out
}

fn pinyin_syllable(s: &str) -> String {
    let p = strip_tone(s);
    if p.is_empty() {
        return String::new();
    }
    // y / w spellings
    let (init, fin): (String, String) = {
        let t = p.as_str();
        let table: [(&str, &str); 12] = [
            ("yu", "v"), ("yi", "i"), ("yan", "ian"), ("yuan", "van"), ("yue", "ve"), ("yun", "vn"), ("wu", "u"), ("wen", "un"), ("weng", "ueng"), ("wei", "ui"), ("you", "iu"), ("yong", "iong"),
        ];
        if let Some((_, f)) = table.iter().find(|(a, _)| *a == t) {
            (String::new(), f.to_string())
        } else if let Some(rest) = t.strip_prefix('y') {
            // ya ye yao yang ying yin ...
            (String::new(), format!("i{rest}"))
        } else if let Some(rest) = t.strip_prefix('w') {
            (String::new(), format!("u{rest}"))
        } else {
            let inits = ["zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r", "z", "c", "s"];
            match inits.iter().find(|i| t.starts_with(**i)) {
                Some(i) => (i.to_string(), t[i.len()..].to_string()),
                None => (String::new(), t.to_string()),
            }
        }
    };
    let init_ipa = match init.as_str() {
        "b" => "p", "p" => "pʰ", "m" => "m", "f" => "f", "d" => "t", "t" => "tʰ", "n" => "n", "l" => "l", "g" => "k", "k" => "kʰ", "h" => "x",
        "j" => "tɕ", "q" => "tɕʰ", "x" => "ɕ", "zh" => "ʈʂ", "ch" => "ʈʂʰ", "sh" => "ʂ", "r" => "ɻ", "z" => "ts", "c" => "tsʰ", "s" => "s", _ => "",
    };
    let mut fin = fin;
    // u after j q x (and y) is ü
    if matches!(init.as_str(), "j" | "q" | "x") {
        if let Some(rest) = fin.strip_prefix('u') {
            fin = format!("v{rest}");
        }
    }
    let fin_ipa: String = match fin.as_str() {
        "a" => "a".into(), "o" => if matches!(init.as_str(), "b" | "p" | "m" | "f") { "wo".into() } else { "o".into() }, "e" => "ɤ".into(),
        "ai" => "ai".into(), "ei" => "ei".into(), "ao" => "au".into(), "ou" => "ou".into(), "an" => "an".into(), "en" => "ən".into(),
        "ang" => "aŋ".into(), "eng" => "əŋ".into(), "ong" => "ʊŋ".into(), "er" => "əɻ".into(),
        "i" => if matches!(init.as_str(), "z" | "c" | "s" | "zh" | "ch" | "sh" | "r") { "ɨ".into() } else { "i".into() },
        "ia" => "ja".into(), "ie" => "je".into(), "iao" => "jau".into(), "iu" => "jou".into(), "ian" => "jɛn".into(), "in" => "in".into(),
        "iang" => "jaŋ".into(), "ing" => "iŋ".into(), "iong" => "jʊŋ".into(),
        "u" => "u".into(), "ua" => "wa".into(), "uo" => "wo".into(), "uai" => "wai".into(), "ui" => "wei".into(), "uan" => "wan".into(),
        "un" => "wən".into(), "uang" => "waŋ".into(), "ueng" => "wəŋ".into(),
        "v" => "y".into(), "ve" => "ɥe".into(), "van" => "ɥɛn".into(), "vn" => "yn".into(),
        other => latin_to_ipa(other),
    };
    format!("{init_ipa}{fin_ipa}")
}

pub fn pinyin_to_ipa(text: &str) -> String {
    use pinyin::ToPinyin;
    let mut syl: Vec<String> = vec![];
    // Hanzi first (each character is one syllable), then anything typed as pinyin
    let mut latin = String::new();
    let flush = |latin: &mut String, syl: &mut Vec<String>| {
        if !latin.is_empty() {
            for part in latin.split(|c: char| c.is_whitespace() || c == '\'') {
                if !part.is_empty() {
                    syl.push(pinyin_syllable(part));
                }
            }
            latin.clear();
        }
    };
    for c in text.chars() {
        if script_of(c) == Lang::Pinyin {
            flush(&mut latin, &mut syl);
            if let Some(p) = c.to_pinyin() {
                syl.push(pinyin_syllable(p.with_tone_num_end()));
            }
        } else {
            latin.push(c);
        }
    }
    flush(&mut latin, &mut syl);
    syl.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

// ── Thai ─────────────────────────────────────────────────────────────────────

fn thai_initial(c: char) -> Option<&'static str> {
    Some(match c {
        'ก' => "k", 'ข' | 'ฃ' | 'ค' | 'ฅ' | 'ฆ' => "kʰ", 'ง' => "ŋ", 'จ' => "tɕ", 'ฉ' | 'ช' | 'ฌ' => "tɕʰ", 'ซ' | 'ศ' | 'ษ' | 'ส' => "s",
        'ญ' | 'ย' => "j", 'ฎ' | 'ด' => "d", 'ฏ' | 'ต' => "t", 'ฐ' | 'ฑ' | 'ฒ' | 'ถ' | 'ท' | 'ธ' => "tʰ", 'ณ' | 'น' => "n", 'บ' => "b", 'ป' => "p",
        'ผ' | 'พ' | 'ภ' => "pʰ", 'ฝ' | 'ฟ' => "f", 'ม' => "m", 'ร' => "r", 'ล' | 'ฬ' => "l", 'ว' => "w", 'ห' | 'ฮ' => "h", 'อ' => "ʔ",
        _ => return None,
    })
}

fn thai_final(c: char) -> Option<&'static str> {
    Some(match c {
        'ก' | 'ข' | 'ค' | 'ฆ' => "k̚", 'ง' => "ŋ", 'จ' | 'ช' | 'ซ' | 'ด' | 'ต' | 'ถ' | 'ท' | 'ธ' | 'ศ' | 'ษ' | 'ส' | 'ฎ' | 'ฏ' | 'ฐ' | 'ฑ' | 'ฒ' | 'ฉ' => "t̚",
        'ญ' | 'น' | 'ณ' | 'ร' | 'ล' | 'ฬ' => "n", 'บ' | 'ป' | 'พ' | 'ฟ' | 'ภ' | 'ผ' | 'ฝ' => "p̚", 'ม' => "m", 'ย' => "j", 'ว' => "w",
        _ => return None,
    })
}

fn is_thai_tone(c: char) -> bool {
    matches!(c, '\u{0E48}' | '\u{0E49}' | '\u{0E4A}' | '\u{0E4B}' | '\u{0E4C}' | '\u{0E47}' | '\u{0E4D}')
}

/// One Thai syllable (consonant + vowel marks) to IPA; a simplified reading of the writing system.
fn thai_syllable(s: &[char]) -> String {
    let mut v: Vec<char> = s.iter().copied().filter(|c| !is_thai_tone(*c) || *c == '\u{0E47}').collect();
    // a consonant followed by the silencing mark is dropped (already removed above except 0E4C handled by filter)
    if v.is_empty() {
        return String::new();
    }
    let mut lead: Option<char> = None;
    if matches!(v[0], 'เ' | 'แ' | 'โ' | 'ไ' | 'ใ') {
        lead = Some(v.remove(0));
    }
    if v.is_empty() {
        return String::new();
    }
    // initial (with ห leading a low consonant, and a second consonant for clusters kr kl kw ...)
    let mut init = String::new();
    let mut k = 0;
    if let Some(i) = thai_initial(v[0]) {
        init.push_str(i);
        k = 1;
        if v.len() > 1 && v[0] == 'ห' && thai_initial(v[1]).is_some() && !matches!(v[1], 'อ') {
            init = thai_initial(v[1]).unwrap().to_string();
            k = 2;
        } else if v.len() > 1 && matches!(v[1], 'ร' | 'ล' | 'ว') && matches!(v[0], 'ก' | 'ข' | 'ค' | 'ต' | 'ป' | 'พ') && (v.len() > 2 || lead.is_some()) {
            // consonant cluster (kr, kl, kw, tr, pr, ...)
            init.push_str(thai_initial(v[1]).unwrap());
            k = 2;
        }
    }
    let rest = &v[k..];
    let has = |c: char| rest.contains(&c);
    let mut final_c: Option<&'static str> = None;
    // trailing consonant that is not part of the vowel
    let vowel_chars = ['ะ', 'า', 'ิ', 'ี', 'ึ', 'ื', 'ุ', 'ู', 'ั', 'ำ', '็'];
    let mut core: Vec<char> = vec![];
    for (idx, &c) in rest.iter().enumerate() {
        if vowel_chars.contains(&c) || (c == 'อ' && idx == 0) || (c == 'ย' && idx == 0 && lead.is_some()) || (c == 'ว' && idx == 0 && !has('ั') && false) {
            core.push(c);
        } else if c == 'ว' && core.contains(&'ั') {
            core.push(c);
        } else if let Some(f) = thai_final(c) {
            final_c = Some(f);
        }
    }
    let cs: String = core.iter().collect();
    let vowel: &str = match (lead, cs.as_str()) {
        (Some('เ'), "ีย") => "iːa",
        (Some('เ'), "ือ") => "ɯːa",
        (Some('เ'), "อ") => "ɤː",
        (Some('เ'), "า") => "aw",
        (Some('เ'), "ะ") | (Some('เ'), "็") => "e",
        (Some('เ'), "") => "eː",
        (Some('เ'), "ิ") => "ɤː",
        (Some('แ'), "ะ") | (Some('แ'), "็") => "ɛ",
        (Some('แ'), _) => "ɛː",
        (Some('โ'), "ะ") => "o",
        (Some('โ'), _) => "oː",
        (Some('ไ'), _) | (Some('ใ'), _) => "aj",
        (None, "ะ") => "a",
        (None, "า") => "aː",
        (None, "ิ") => "i",
        (None, "ี") => "iː",
        (None, "ึ") => "ɯ",
        (None, "ื") => "ɯː",
        (None, "ุ") => "u",
        (None, "ู") => "uː",
        (None, "ำ") => "am",
        (None, "ัว") => "uːa",
        (None, "ั") | (None, "็") => "a",
        (None, "อ") => "ɔː",
        (None, "") => {
            if final_c.is_some() {
                "o"
            } else {
                "a"
            }
        }
        _ => "a",
    };
    // glide finals already in the vowel
    let fin = match (vowel, final_c) {
        ("aj", _) | ("aw", _) => "",
        (_, Some(f)) => f,
        _ => "",
    };
    format!("{init}{vowel}{fin}")
}

pub fn thai_to_ipa(text: &str) -> String {
    // split into syllables at spaces / hyphens; each piece is converted as one syllable
    text.split(|c: char| c.is_whitespace() || c == '-')
        .filter(|p| !p.is_empty())
        .map(|p| thai_syllable(&p.chars().collect::<Vec<_>>()))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_from_the_request_splits_into_notes() {
        let p = split_phrase("[so-fa]-[rei][to][mi-do]");
        let flat: Vec<(bool, String)> = p.iter().map(|(t, s)| (*t, s.join("+"))).collect();
        assert_eq!(flat, vec![(false, "so+fa".into()), (true, "rei".into()), (false, "to".into()), (false, "mi+do".into())]);
    }

    #[test]
    fn bare_tokens_hyphens_and_sustain_notes() {
        let p = split_phrase("la-la-la - [-] ti");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].1, vec!["la", "la", "la"]);
        assert!(p[1].0 && p[1].1.is_empty(), "[-] after a sustain is a tied note with no syllable");
        assert_eq!(p[2].1, vec!["ti"]);
    }

    #[test]
    fn stored_form_marks_ties_and_joins_syllables() {
        let v = parse_phrase("[so-fa]-[rei][to][mi-do]", Lang::Latin);
        assert_eq!(v, vec!["so-fa", "~ɾei", "to", "mi-do"]);
    }

    #[test]
    fn ipa_passes_through() {
        assert_eq!(to_ipa("ʃɑː", Lang::Auto), "ʃɑː");
    }

    #[test]
    fn hangul_reads_as_ipa_with_voicing_and_liaison() {
        assert_eq!(hangul_to_ipa("사랑"), "sa-ɾaŋ");
        assert_eq!(hangul_to_ipa("안녕"), "an-njʌŋ");
        // 한국어: liaison moves ㄱ to the next syllable
        assert_eq!(hangul_to_ipa("한국어"), "han-ku-gʌ");
        assert_eq!(to_ipa("바다", Lang::Auto), "pa-da");
    }

    #[test]
    fn kana_reads_as_ipa() {
        assert_eq!(kana_to_ipa("さくら"), "sa-kɯ-ɾa");
        assert_eq!(kana_to_ipa("ありがとう"), "a-ɾi-ga-to-ɯ");
        assert_eq!(kana_to_ipa("きょう"), "kjo-ɯ");
        assert_eq!(kana_to_ipa("サクラ"), "sa-kɯ-ɾa");
        assert_eq!(kana_to_ipa("がっこう"), "gaʔ-ko-ɯ");
        assert_eq!(kana_to_ipa("らーめん"), "ɾaː-me-ɴ");
    }

    #[test]
    fn pinyin_and_hanzi_read_as_ipa() {
        assert_eq!(pinyin_to_ipa("ni3 hao3"), "ni-xau");
        assert_eq!(pinyin_to_ipa("nǐ hǎo"), "ni-xau");
        assert_eq!(pinyin_to_ipa("zhong1"), "ʈʂʊŋ");
        assert_eq!(pinyin_to_ipa("xue2"), "ɕɥe");
        assert_eq!(pinyin_to_ipa("你好"), "ni-xau");
        assert_eq!(to_ipa("ma1", Lang::Auto), "ma");
    }

    #[test]
    fn thai_reads_as_ipa() {
        assert_eq!(thai_to_ipa("สะ วัด ดี"), "sa-wat̚-diː");
        assert_eq!(thai_to_ipa("กา"), "kaː");
        assert_eq!(thai_to_ipa("ไป"), "paj");
        assert_eq!(thai_to_ipa("นก"), "nok̚");
        assert_eq!(thai_to_ipa("เกา"), "kaw");
        assert_eq!(thai_to_ipa("มี"), "miː");
    }

    #[test]
    fn latin_is_read_phonetically() {
        assert_eq!(latin_to_ipa("sha"), "ʃa");
        assert_eq!(latin_to_ipa("chi"), "tʃi");
        assert_eq!(latin_to_ipa("do"), "do");
        assert_eq!(latin_to_ipa("ring"), "ɾiŋ");
    }
}
