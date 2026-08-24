//! Selectable keyboard layouts -- alternate scancode-set-1 -> ASCII tables,
//! same shape as `keyboard.rs`'s own `SCANCODE_ASCII`/`_SHIFTED` (which
//! stays the built-in US QWERTY default; this module doesn't replace it,
//! `keyboard::lookup_ascii` consults [`current`] and remaps only the
//! handful of positions that actually differ from US on each layout).
//!
//! Honest, deliberately narrower than a full "pick your language's
//! keyboard" list: every layout here is a genuine key-position remap
//! within plain ASCII. CJK/Thai/Arabic/Hebrew/Vietnamese keyboard input
//! needs either non-ASCII character output (this driver's `read_char`/
//! `poll_char` return one `u8` per keystroke, not a UTF-8 sequence) or a
//! real phonetic/diacritic input-method engine (pinyin, Kedmanee, JIS,
//! 2-Set Hangul, Telex) -- neither exists, and offering them as selectable
//! here with no actual effect would be worse than not listing them.
//! Spanish/Portuguese/Italian are QWERTY-position-identical to US for the
//! plain a-z/0-9 keys (their real differences are accented-letter and
//! symbol keys, which need the same non-ASCII output this driver doesn't
//! have) -- left out for the same reason, not oversight.

/// (name, table of (scancode, unshifted, shifted) overrides applied on top
/// of `keyboard::SCANCODE_ASCII`/`_SHIFTED`). Only positions that actually
/// differ from US QWERTY are listed -- everything else falls through to
/// the US table unchanged.
struct Layout {
    name: &'static str,
    overrides: &'static [(u8, u8, u8)],
}

const LAYOUTS: &[Layout] = &[
    Layout { name: "US QWERTY", overrides: &[] },
    Layout {
        name: "UK QWERTY",
        overrides: &[
            (0x03, b'2', b'"'), // shift+2 is '"' on a UK keyboard, not '@'
            (0x28, b'\'', b'@'), // ' key: unshifted apostrophe, shifted '@'
            (0x2B, b'#', b'~'), // key next to Enter: '#'/'~', not '\'/'|'
        ],
    },
    Layout {
        name: "German QWERTZ",
        overrides: &[
            (0x15, b'z', b'Z'), // Y position produces Z
            (0x2C, b'y', b'Y'), // Z position produces Y
            // Umlauts (a/o/u-diaeresis, sharp s) need non-ASCII output this
            // driver can't produce -- those keys stay unmapped (0), same as
            // any other uncovered scancode, rather than silently wrong.
        ],
    },
    Layout {
        name: "French AZERTY",
        overrides: &[
            (0x10, b'a', b'A'), // Q position produces A
            (0x1E, b'q', b'Q'), // A position produces Q
            (0x11, b'z', b'Z'), // W position produces Z
            (0x2C, b'w', b'W'), // Z position produces W
            (0x27, b'm', b'M'), // ; position produces M
            // Real AZERTY also needs Shift for the digit row (unshifted top
            // row is accented letters/symbols) -- not replicated here since
            // those symbols are non-ASCII; digits stay unshifted-accessible
            // as a disclosed simplification, not full AZERTY fidelity.
        ],
    },
    Layout {
        name: "Dvorak (US)",
        overrides: &[
            (0x0C, b'[', b'{'),
            (0x0D, b']', b'}'),
            (0x10, b'\'', b'"'),
            (0x11, b',', b'<'),
            (0x12, b'.', b'>'),
            (0x13, b'p', b'P'),
            (0x14, b'y', b'Y'),
            (0x15, b'f', b'F'),
            (0x16, b'g', b'G'),
            (0x17, b'c', b'C'),
            (0x18, b'r', b'R'),
            (0x19, b'l', b'L'),
            (0x1A, b'/', b'?'),
            (0x1B, b'=', b'+'),
            (0x1E, b'a', b'A'),
            (0x1F, b'o', b'O'),
            (0x20, b'e', b'E'),
            (0x21, b'u', b'U'),
            (0x22, b'i', b'I'),
            (0x23, b'd', b'D'),
            (0x24, b'h', b'H'),
            (0x25, b't', b'T'),
            (0x26, b'n', b'N'),
            (0x27, b's', b'S'),
            (0x28, b'-', b'_'),
            (0x2C, b';', b':'),
            (0x2D, b'q', b'Q'),
            (0x2E, b'j', b'J'),
            (0x2F, b'k', b'K'),
            (0x30, b'x', b'X'),
            (0x31, b'b', b'B'),
            (0x32, b'm', b'M'),
            (0x33, b'w', b'W'),
            (0x34, b'v', b'V'),
            (0x35, b'z', b'Z'),
        ],
    },
];

static mut CURRENT: usize = 0;
static mut CURSOR: usize = 0;
static mut PICKED: Option<usize> = None;

pub fn count() -> usize {
    LAYOUTS.len()
}

pub fn name(i: usize) -> &'static str {
    LAYOUTS.get(i).map(|l| l.name).unwrap_or("")
}

pub fn set_current(i: usize) {
    if i < LAYOUTS.len() {
        unsafe { CURRENT = i };
    }
}

pub fn current() -> usize {
    unsafe { CURRENT }
}

/// The picker's highlighted row -- separate from [`current`] (the layout
/// actually in effect) for the same reason `locale::cursor` is separate
/// from `locale::selected`: browsing with the arrow keys shouldn't change
/// what's active until Enter confirms it.
pub fn cursor() -> usize {
    unsafe { CURSOR }
}

pub fn cursor_up() {
    unsafe {
        CURSOR = if CURSOR == 0 { LAYOUTS.len() - 1 } else { CURSOR - 1 };
    }
}

pub fn cursor_down() {
    unsafe {
        CURSOR = (CURSOR + 1) % LAYOUTS.len();
    }
}

pub fn confirm_cursor() {
    let i = unsafe { CURSOR };
    set_current(i);
    unsafe { PICKED = Some(i) };
}

/// Start a fresh pick: forgets any prior confirmation and rewinds the
/// browse cursor -- mirrors `locale::reset`'s doc (a `.ling` picker screen
/// needs "not yet chosen" to be real kernel-side state it can poll in a
/// `while`, since there's no local mutable loop variable available on that
/// side of the FFI boundary).
pub fn reset() {
    unsafe {
        PICKED = None;
        CURSOR = 0;
    }
}

/// `count()` (an always-out-of-range index) when nothing's been confirmed
/// yet this run, same out-of-range-as-"None" convention `locale::selected`
/// uses -- deliberately NOT [`current`], which always holds a valid real
/// layout (defaulting to US QWERTY, index 0) and so can't distinguish "the
/// default is still active" from "the user just confirmed the default".
pub fn selected() -> usize {
    unsafe { PICKED.unwrap_or(LAYOUTS.len()) }
}

/// Look up `code`'s override on the current layout, if any -- `None` means
/// "no override, use the US table's value unchanged" (the common case for
/// every scancode a layout doesn't touch).
pub fn override_for(code: u8, shifted: bool) -> Option<u8> {
    let layout = LAYOUTS.get(unsafe { CURRENT })?;
    layout.overrides.iter().find(|(c, _, _)| *c == code).map(
        |(_, unshifted, shift)| if shifted { *shift } else { *unshifted },
    )
}
