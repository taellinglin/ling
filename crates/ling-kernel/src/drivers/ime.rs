//! Typing in any script on a keyboard that sends ASCII.
//!
//! The keyboard drivers (PS/2 `keyboard.rs`, USB `xhci.rs`, the serial line) all deliver US-layout
//! ASCII. This layer sits between them and whoever takes text (the Ling app runtime's `text_poll`,
//! the desktop's windows) and turns that ASCII into Unicode in the current input mode:
//!
//!   Latin      passes through (the default)
//!   Accents    US-International dead keys: ' ` ^ ~ " then a letter (' e -> é, ~ n -> ñ, " u -> ü)
//!   Русский    the standard ЙЦУКЕН layout on the same keys
//!   Ελληνικά   the Greek layout; ; then a vowel gives the tonos (; a -> ά)
//!   한국어      2-set (dubeolsik) Hangul, jamo composed into syllables as they are typed
//!   ひらがな    romaji to hiragana (ka -> か, kka -> っか, n' or nn -> ん, - -> ー)
//!   カタカナ    the same, to katakana
//!   U+         a code point in hex, committed with Space or Enter (U+ 4E2D -> 中)
//!
//! Ctrl+Space cycles the modes. What is still being composed (a dead key, a Hangul syllable that
//! may take another jamo, romaji that is not a kana yet, hex digits) is held back as the pre-edit
//! and shown by the desktop's indicator; only finished text goes to the program, so programs that
//! edit their text with Backspace never see characters replaced under them.
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Latin,
    Accents,
    Russian,
    Greek,
    Hangul,
    Hiragana,
    Katakana,
    Hex,
}

const MODES: [Mode; 8] = [Mode::Latin, Mode::Accents, Mode::Russian, Mode::Greek, Mode::Hangul, Mode::Hiragana, Mode::Katakana, Mode::Hex];

struct State {
    mode: Mode,
    /// pending dead key / romaji / hex digits
    pend: String,
    /// Hangul syllable being built: initial, medial, final (compatibility jamo)
    l: Option<char>,
    v: Option<char>,
    t: Option<char>,
}

static mut ST: State = State { mode: Mode::Latin, pend: String::new(), l: None, v: None, t: None };

fn st() -> &'static mut State {
    unsafe { &mut *(&raw mut ST) }
}

pub fn mode() -> Mode {
    st().mode
}

/// Anything other than plain Latin (the desktop shows its indicator then).
pub fn active() -> bool {
    st().mode != Mode::Latin
}

/// The mode's short name for the indicator.
pub fn label() -> &'static str {
    match st().mode {
        Mode::Latin => "EN",
        Mode::Accents => "Áa",
        Mode::Russian => "Ру",
        Mode::Greek => "Ελ",
        Mode::Hangul => "한",
        Mode::Hiragana => "あ",
        Mode::Katakana => "ア",
        Mode::Hex => "U+",
    }
}

/// The mode's full name (Settings, the indicator's tooltip-like line).
pub fn name() -> &'static str {
    match st().mode {
        Mode::Latin => "English (US)",
        Mode::Accents => "US International (accents)",
        Mode::Russian => "Русский",
        Mode::Greek => "Ελληνικά",
        Mode::Hangul => "한국어 (두벌식)",
        Mode::Hiragana => "日本語 ひらがな",
        Mode::Katakana => "日本語 カタカナ",
        Mode::Hex => "Unicode (hex code point)",
    }
}

/// What is being composed, for the indicator ("" when nothing is).
pub fn preedit() -> String {
    let s = st();
    let mut out = String::new();
    if s.mode == Mode::Hex && !s.pend.is_empty() {
        out.push_str("U+");
    }
    out.push_str(&s.pend);
    if let Some(c) = hangul_current(s) {
        out.push(c);
    }
    out
}

/// Ctrl+Space: the next mode. Whatever was being composed is committed into `out` first.
pub fn cycle(out: &mut String) {
    flush(out);
    let s = st();
    let i = MODES.iter().position(|&m| m == s.mode).unwrap_or(0);
    s.mode = MODES[(i + 1) % MODES.len()];
}

pub fn set_mode(m: Mode, out: &mut String) {
    flush(out);
    st().mode = m;
}

/// Commit what is being composed (before Enter, Tab, a click, a mode change).
pub fn flush(out: &mut String) {
    let s = st();
    match s.mode {
        Mode::Hangul => hangul_commit(s, out),
        Mode::Hiragana | Mode::Katakana => {
            if s.pend == "n" {
                out.push(kana(s.mode, 'ん'));
            } else {
                out.push_str(&s.pend);
            }
        },
        Mode::Hex => {
            if let Some(c) = hex_char(&s.pend) {
                out.push(c);
            } else if !s.pend.is_empty() {
                out.push_str("U+");
                out.push_str(&s.pend);
            }
        },
        _ => out.push_str(&s.pend),
    }
    s.pend.clear();
    s.l = None;
    s.v = None;
    s.t = None;
}

/// Backspace: true when it only took a step back in the composition (the program must not see it).
pub fn backspace() -> bool {
    let s = st();
    if s.t.is_some() {
        s.t = split_final(s.t.unwrap()).map(|(a, _)| a);
        return true;
    }
    if let Some(v) = s.v {
        s.v = split_vowel(v);
        return true;
    }
    if s.l.is_some() {
        s.l = None;
        return true;
    }
    if !s.pend.is_empty() {
        s.pend.pop();
        return true;
    }
    false
}

/// One printable ASCII character typed; whatever is finished is appended to `out`.
pub fn feed(c: char, out: &mut String) {
    let s = st();
    match s.mode {
        Mode::Latin => out.push(c),
        Mode::Accents => feed_accents(s, c, out),
        Mode::Russian => out.push(russian(c)),
        Mode::Greek => feed_greek(s, c, out),
        Mode::Hangul => feed_hangul(s, c, out),
        Mode::Hiragana | Mode::Katakana => feed_kana(s, c, out),
        Mode::Hex => feed_hex(s, c, out),
    }
}

/// The length of `buf[..len]` without its last UTF-8 character (Backspace removes a whole one).
pub fn utf8_pop(buf: &[u8], len: usize) -> usize {
    let mut n = len.min(buf.len());
    while n > 0 && buf[n - 1] & 0xC0 == 0x80 {
        n -= 1;
    }
    n.saturating_sub(1)
}

/// The text `bytes` (printable ASCII as the keyboard sends it) through the current mode.
pub fn feed_str(bytes: &[u8], out: &mut String) {
    for &b in bytes {
        feed(b as char, out);
    }
}

// ── dead keys ──

fn dead_accent(dead: char, c: char) -> Option<char> {
    let t: &[(char, char)] = match dead {
        '\'' => &[('a', 'á'), ('e', 'é'), ('i', 'í'), ('o', 'ó'), ('u', 'ú'), ('y', 'ý'), ('c', 'ç'), ('A', 'Á'), ('E', 'É'), ('I', 'Í'), ('O', 'Ó'), ('U', 'Ú'), ('Y', 'Ý'), ('C', 'Ç')],
        '`' => &[('a', 'à'), ('e', 'è'), ('i', 'ì'), ('o', 'ò'), ('u', 'ù'), ('A', 'À'), ('E', 'È'), ('I', 'Ì'), ('O', 'Ò'), ('U', 'Ù')],
        '^' => &[('a', 'â'), ('e', 'ê'), ('i', 'î'), ('o', 'ô'), ('u', 'û'), ('A', 'Â'), ('E', 'Ê'), ('I', 'Î'), ('O', 'Ô'), ('U', 'Û')],
        '~' => &[('a', 'ã'), ('o', 'õ'), ('n', 'ñ'), ('A', 'Ã'), ('O', 'Õ'), ('N', 'Ñ')],
        '"' => &[('a', 'ä'), ('e', 'ë'), ('i', 'ï'), ('o', 'ö'), ('u', 'ü'), ('y', 'ÿ'), ('s', 'ß'), ('A', 'Ä'), ('E', 'Ë'), ('I', 'Ï'), ('O', 'Ö'), ('U', 'Ü')],
        _ => &[],
    };
    t.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

fn feed_accents(s: &mut State, c: char, out: &mut String) {
    if let Some(dead) = s.pend.chars().next() {
        s.pend.clear();
        if let Some(a) = dead_accent(dead, c) {
            out.push(a);
        } else if c == ' ' || c == dead {
            out.push(dead); // the accent mark itself
        } else {
            out.push(dead);
            feed_accents(s, c, out);
        }
        return;
    }
    if "'`^~\"".contains(c) {
        s.pend.push(c);
    } else {
        out.push(c);
    }
}

// ── Cyrillic and Greek layouts ──

fn russian(c: char) -> char {
    const LOW: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,./`";
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю.ё";
    const UP: &str = "QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?~";
    const RU_UP: &str = "ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,Ё";
    if let Some(i) = LOW.chars().position(|x| x == c) {
        return RU.chars().nth(i).unwrap_or(c);
    }
    if let Some(i) = UP.chars().position(|x| x == c) {
        return RU_UP.chars().nth(i).unwrap_or(c);
    }
    c
}

fn greek(c: char) -> char {
    const LAT: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const GR: &str = "αβψδεφγηιξκλμνοπ;ρστθωςχυζΑΒΨΔΕΦΓΗΙΞΚΛΜΝΟΠ:ΡΣΤΘΩΣΧΥΖ";
    match LAT.chars().position(|x| x == c) {
        Some(i) => GR.chars().nth(i).unwrap_or(c),
        None => c,
    }
}

fn greek_tonos(c: char) -> Option<char> {
    let t = [('α', 'ά'), ('ε', 'έ'), ('η', 'ή'), ('ι', 'ί'), ('ο', 'ό'), ('υ', 'ύ'), ('ω', 'ώ'), ('Α', 'Ά'), ('Ε', 'Έ'), ('Η', 'Ή'), ('Ι', 'Ί'), ('Ο', 'Ό'), ('Υ', 'Ύ'), ('Ω', 'Ώ')];
    t.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

fn greek_dialytika(c: char) -> Option<char> {
    let t = [('ι', 'ϊ'), ('υ', 'ϋ'), ('Ι', 'Ϊ'), ('Υ', 'Ϋ')];
    t.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

fn feed_greek(s: &mut State, c: char, out: &mut String) {
    if let Some(dead) = s.pend.chars().next() {
        s.pend.clear();
        let g = greek(c);
        let acc = if dead == ';' { greek_tonos(g) } else { greek_dialytika(g) };
        match acc {
            Some(a) => out.push(a),
            None => {
                out.push(if dead == ';' { '΄' } else { '¨' });
                if c != ' ' {
                    feed_greek(s, c, out);
                }
            },
        }
        return;
    }
    if c == ';' || c == ':' {
        s.pend.push(c);
    } else {
        out.push(greek(c));
    }
}

// ── Hangul (2-set) ──

const CHO: &str = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
const JUNG: &str = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ";
const JONG: &str = "ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ";

fn idx(set: &str, c: char) -> Option<u32> {
    set.chars().position(|x| x == c).map(|i| i as u32)
}

/// The jamo a key gives in 2-set Hangul (None: not a jamo key).
fn jamo_of(c: char) -> Option<char> {
    const KEYS: &str = "qwertyuiopasdfghjklzxcvbnmQWERTOP";
    const JAMO: &str = "ㅂㅈㄷㄱㅅㅛㅕㅑㅐㅔㅁㄴㅇㄹㅎㅗㅓㅏㅣㅋㅌㅊㅍㅠㅜㅡㅃㅉㄸㄲㅆㅒㅖ";
    if let Some(i) = KEYS.chars().position(|x| x == c) {
        return JAMO.chars().nth(i);
    }
    // other shifted letters type the plain jamo
    if c.is_ascii_uppercase() {
        return jamo_of(c.to_ascii_lowercase());
    }
    None
}

fn is_vowel(j: char) -> bool {
    idx(JUNG, j).is_some()
}

fn combine_vowel(a: char, b: char) -> Option<char> {
    match (a, b) {
        ('ㅗ', 'ㅏ') => Some('ㅘ'),
        ('ㅗ', 'ㅐ') => Some('ㅙ'),
        ('ㅗ', 'ㅣ') => Some('ㅚ'),
        ('ㅜ', 'ㅓ') => Some('ㅝ'),
        ('ㅜ', 'ㅔ') => Some('ㅞ'),
        ('ㅜ', 'ㅣ') => Some('ㅟ'),
        ('ㅡ', 'ㅣ') => Some('ㅢ'),
        _ => None,
    }
}

fn split_vowel(v: char) -> Option<char> {
    match v {
        'ㅘ' | 'ㅙ' | 'ㅚ' => Some('ㅗ'),
        'ㅝ' | 'ㅞ' | 'ㅟ' => Some('ㅜ'),
        'ㅢ' => Some('ㅡ'),
        _ => None,
    }
}

fn combine_final(a: char, b: char) -> Option<char> {
    match (a, b) {
        ('ㄱ', 'ㅅ') => Some('ㄳ'),
        ('ㄴ', 'ㅈ') => Some('ㄵ'),
        ('ㄴ', 'ㅎ') => Some('ㄶ'),
        ('ㄹ', 'ㄱ') => Some('ㄺ'),
        ('ㄹ', 'ㅁ') => Some('ㄻ'),
        ('ㄹ', 'ㅂ') => Some('ㄼ'),
        ('ㄹ', 'ㅅ') => Some('ㄽ'),
        ('ㄹ', 'ㅌ') => Some('ㄾ'),
        ('ㄹ', 'ㅍ') => Some('ㄿ'),
        ('ㄹ', 'ㅎ') => Some('ㅀ'),
        ('ㅂ', 'ㅅ') => Some('ㅄ'),
        _ => None,
    }
}

/// A compound final as (kept, moved to the next syllable); a simple final as (None, itself).
fn split_final(t: char) -> Option<(char, char)> {
    match t {
        'ㄳ' => Some(('ㄱ', 'ㅅ')),
        'ㄵ' => Some(('ㄴ', 'ㅈ')),
        'ㄶ' => Some(('ㄴ', 'ㅎ')),
        'ㄺ' => Some(('ㄹ', 'ㄱ')),
        'ㄻ' => Some(('ㄹ', 'ㅁ')),
        'ㄼ' => Some(('ㄹ', 'ㅂ')),
        'ㄽ' => Some(('ㄹ', 'ㅅ')),
        'ㄾ' => Some(('ㄹ', 'ㅌ')),
        'ㄿ' => Some(('ㄹ', 'ㅍ')),
        'ㅀ' => Some(('ㄹ', 'ㅎ')),
        'ㅄ' => Some(('ㅂ', 'ㅅ')),
        _ => None,
    }
}

fn syllable(l: char, v: char, t: Option<char>) -> Option<char> {
    let li = idx(CHO, l)?;
    let vi = idx(JUNG, v)?;
    let ti = match t {
        Some(t) => idx(JONG, t)? + 1,
        None => 0,
    };
    char::from_u32(0xAC00 + (li * 21 + vi) * 28 + ti)
}

fn hangul_current(s: &State) -> Option<char> {
    match (s.l, s.v) {
        (Some(l), Some(v)) => syllable(l, v, s.t),
        (Some(l), None) => Some(l),
        (None, Some(v)) => Some(v),
        _ => None,
    }
}

fn hangul_commit(s: &mut State, out: &mut String) {
    if let Some(c) = hangul_current(s) {
        out.push(c);
    }
    s.l = None;
    s.v = None;
    s.t = None;
}

fn feed_hangul(s: &mut State, c: char, out: &mut String) {
    let Some(j) = jamo_of(c) else {
        hangul_commit(s, out);
        out.push(c);
        return;
    };
    if is_vowel(j) {
        match (s.l, s.v, s.t) {
            (_, None, _) if s.l.is_some() => s.v = Some(j),
            (Some(_), Some(v), None) => match combine_vowel(v, j) {
                Some(cv) => s.v = Some(cv),
                None => {
                    hangul_commit(s, out);
                    s.v = Some(j);
                },
            },
            (Some(_), Some(_), Some(t)) => {
                // the final moves to the new syllable (all of it, or its second half)
                let (keep, moved) = match split_final(t) {
                    Some((a, b)) => (Some(a), b),
                    None => (None, t),
                };
                s.t = keep;
                hangul_commit(s, out);
                s.l = Some(moved);
                s.v = Some(j);
            },
            (None, Some(v), _) => match combine_vowel(v, j) {
                Some(cv) => s.v = Some(cv),
                None => {
                    hangul_commit(s, out);
                    s.v = Some(j);
                },
            },
            _ => s.v = Some(j),
        }
        return;
    }
    // a consonant
    match (s.l, s.v, s.t) {
        (Some(_), Some(_), None) if idx(JONG, j).is_some() => s.t = Some(j),
        (Some(_), Some(_), Some(t)) if combine_final(t, j).is_some() => s.t = combine_final(t, j),
        (None, None, None) => s.l = Some(j),
        _ => {
            hangul_commit(s, out);
            s.l = Some(j);
        },
    }
}

// ── kana ──

const ROMAJI: &[(&str, char)] = &[
    ("a", 'あ'), ("i", 'い'), ("u", 'う'), ("e", 'え'), ("o", 'お'),
    ("ka", 'か'), ("ki", 'き'), ("ku", 'く'), ("ke", 'け'), ("ko", 'こ'),
    ("ga", 'が'), ("gi", 'ぎ'), ("gu", 'ぐ'), ("ge", 'げ'), ("go", 'ご'),
    ("sa", 'さ'), ("si", 'し'), ("shi", 'し'), ("su", 'す'), ("se", 'せ'), ("so", 'そ'),
    ("za", 'ざ'), ("zi", 'じ'), ("ji", 'じ'), ("zu", 'ず'), ("ze", 'ぜ'), ("zo", 'ぞ'),
    ("ta", 'た'), ("ti", 'ち'), ("chi", 'ち'), ("tu", 'つ'), ("tsu", 'つ'), ("te", 'て'), ("to", 'と'),
    ("da", 'だ'), ("di", 'ぢ'), ("du", 'づ'), ("de", 'で'), ("do", 'ど'),
    ("na", 'な'), ("ni", 'に'), ("nu", 'ぬ'), ("ne", 'ね'), ("no", 'の'),
    ("ha", 'は'), ("hi", 'ひ'), ("hu", 'ふ'), ("fu", 'ふ'), ("he", 'へ'), ("ho", 'ほ'),
    ("ba", 'ば'), ("bi", 'び'), ("bu", 'ぶ'), ("be", 'べ'), ("bo", 'ぼ'),
    ("pa", 'ぱ'), ("pi", 'ぴ'), ("pu", 'ぷ'), ("pe", 'ぺ'), ("po", 'ぽ'),
    ("ma", 'ま'), ("mi", 'み'), ("mu", 'む'), ("me", 'め'), ("mo", 'も'),
    ("ya", 'や'), ("yu", 'ゆ'), ("yo", 'よ'),
    ("ra", 'ら'), ("ri", 'り'), ("ru", 'る'), ("re", 'れ'), ("ro", 'ろ'),
    ("la", 'ら'), ("li", 'り'), ("lu", 'る'), ("le", 'れ'), ("lo", 'ろ'),
    ("wa", 'わ'), ("wo", 'を'), ("nn", 'ん'), ("n'", 'ん'),
    ("xa", 'ぁ'), ("xi", 'ぃ'), ("xu", 'ぅ'), ("xe", 'ぇ'), ("xo", 'ぉ'),
    ("xya", 'ゃ'), ("xyu", 'ゅ'), ("xyo", 'ょ'), ("xtu", 'っ'), ("xtsu", 'っ'), ("xwa", 'ゎ'),
    ("-", 'ー'), (",", '、'), (".", '。'), ("[", '「'), ("]", '」'), ("~", '〜'),
];

/// Two-kana romaji (kya -> きゃ, sha -> しゃ, fa -> ふぁ ...).
fn romaji_pair(s: &str) -> Option<(char, char)> {
    let base = match s.get(..s.len().saturating_sub(1))? {
        "ky" => 'き', "gy" => 'ぎ', "sy" | "sh" => 'し', "zy" | "jy" | "j" => 'じ', "ty" | "ch" | "cy" => 'ち',
        "dy" => 'ぢ', "ny" => 'に', "hy" => 'ひ', "by" => 'び', "py" => 'ぴ', "my" => 'み', "ry" | "ly" => 'り',
        "f" => 'ふ', "ts" => 'つ', "v" => 'ゔ',
        _ => return None,
    };
    let last = s.chars().last()?;
    let small = match (base, last) {
        ('ふ' | 'つ' | 'ゔ', 'a') => 'ぁ',
        ('ふ' | 'ゔ', 'i') => 'ぃ',
        ('ふ' | 'つ' | 'ゔ', 'e') => 'ぇ',
        ('ふ' | 'つ' | 'ゔ', 'o') => 'ぉ',
        ('ゔ', 'u') => return None,
        (_, 'a') => 'ゃ',
        (_, 'u') => 'ゅ',
        (_, 'o') => 'ょ',
        (_, 'e') if base != 'ふ' => 'ぇ',
        _ => return None,
    };
    Some((base, small))
}

fn kana(mode: Mode, c: char) -> char {
    if mode == Mode::Katakana && ('ぁ'..='ゖ').contains(&c) {
        char::from_u32(c as u32 + 0x60).unwrap_or(c)
    } else {
        c
    }
}

fn romaji_prefix(p: &str) -> bool {
    if ROMAJI.iter().any(|(r, _)| r.starts_with(p)) {
        return true;
    }
    // the stems of the two-kana forms
    ["ky", "gy", "sy", "sh", "zy", "jy", "j", "ty", "ch", "cy", "c", "dy", "ny", "hy", "by", "py", "my", "ry", "ly", "f", "ts", "v"].iter().any(|r| r.starts_with(p))
}

fn feed_kana(s: &mut State, c: char, out: &mut String) {
    let m = s.mode;
    let lc = c.to_ascii_lowercase();
    if !(lc.is_ascii_alphabetic() || "-,.[]~'".contains(lc)) {
        flush(out);
        st().mode = m;
        out.push(c);
        return;
    }
    s.pend.push(lc);
    loop {
        let p = s.pend.clone();
        if p.is_empty() {
            return;
        }
        if let Some((_, k)) = ROMAJI.iter().find(|(r, _)| *r == p) {
            out.push(kana(m, *k));
            s.pend.clear();
            return;
        }
        if let Some((a, b)) = romaji_pair(&p) {
            out.push(kana(m, a));
            out.push(kana(m, b));
            s.pend.clear();
            return;
        }
        if romaji_prefix(&p) {
            return; // wait for more
        }
        let cs: Vec<char> = p.chars().collect();
        // a doubled consonant: っ, the second one starts the next kana
        if cs.len() >= 2 && cs[0] == cs[1] && cs[0] != 'n' && !"aiueo".contains(cs[0]) {
            out.push(kana(m, 'っ'));
            s.pend = cs[1..].iter().collect();
            continue;
        }
        // n before a consonant: ん
        if cs[0] == 'n' {
            out.push(kana(m, 'ん'));
            s.pend = cs[1..].iter().collect();
            continue;
        }
        // nothing to make of it: the first letter as typed
        out.push(cs[0]);
        s.pend = cs[1..].iter().collect();
    }
}

// ── hex code points ──

fn hex_char(p: &str) -> Option<char> {
    if p.is_empty() {
        return None;
    }
    u32::from_str_radix(p, 16).ok().and_then(char::from_u32)
}

fn feed_hex(s: &mut State, c: char, out: &mut String) {
    if c.is_ascii_hexdigit() && s.pend.len() < 6 {
        s.pend.push(c.to_ascii_uppercase());
        return;
    }
    if c == ' ' && !s.pend.is_empty() {
        let p = core::mem::take(&mut s.pend);
        match hex_char(&p) {
            Some(ch) => out.push(ch),
            None => {
                out.push_str("U+");
                out.push_str(&p);
            },
        }
        return;
    }
    let mut tmp = String::new();
    flush(&mut tmp);
    st().mode = Mode::Hex;
    out.push_str(&tmp);
    out.push(c);
}
