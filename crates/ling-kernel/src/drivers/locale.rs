//! Locale/timezone table: real Earth regions grounded in actual UTC offsets,
//! the Moon grounded in the real synodic month (its actual day/night cycle
//! length, tidally locked so it differs from its ~27.3-day sidereal
//! rotation), and a couple of invented celestial/mythological locales named
//! after real figures from Chinese folk religion (月宫/Moon Palace — Chang'e
//! 嫦娥's home; 龙宫/Dragon Palace — the undersea court of the Dragon Kings,
//! from Journey to the West and wider folklore) rather than made up whole
//! cloth, per the request this was built for.
//!
//! Honest limits: there is no calendar/date math here beyond `arch::rtc`'s
//! Unix timestamp — a fictional planet's "timezone" is necessarily symbolic
//! (an offset applied to Earth-UTC, not a real independent calendar), and is
//! disclosed as such via `Kind::Celestial` rather than presented as if it
//! were a real astronomical authority. The Moon's day-cycle fraction is a
//! real computation (`moon_day_progress`), not decoration.

use crate::arch::rtc;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// A real Earth timezone — `utc_offset_min` is a genuine UTC offset.
    Earth,
    /// A locale with no real independent clock of its own — `utc_offset_min`
    /// is a symbolic offset from Earth-UTC for flavor/sorting, not a claim
    /// about how time actually works there.
    Celestial,
}

pub struct Locale {
    /// Stable short id, e.g. "zh-CN" / "moon" / "ling-country".
    pub id: &'static str,
    /// Display name in the locale's own script (UTF-8) — rendered via
    /// `drivers::font_unicode` when non-ASCII, `font8x8` otherwise.
    pub native_name: &'static str,
    /// Latin/English name — always renderable, used in text-mode contexts
    /// that can't show the native script (see this module's doc + the
    /// installer's text-mode picker).
    pub latin_name: &'static str,
    pub utc_offset_min: i32,
    pub kind: Kind,
    /// Whether this locale's UI text should render through the Daemon
    /// glyph reskin (see `font_unicode`'s doc) instead of plain Latin.
    pub uses_daemon_script: bool,
    pub flag_id: u32,
}

pub static LOCALES: &[Locale] = &[
    Locale {
        id: "en-US",
        native_name: "English (US)",
        latin_name: "English (US)",
        utc_offset_min: -5 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 0,
    },
    Locale {
        id: "en-GB",
        native_name: "English (UK)",
        latin_name: "English (UK)",
        utc_offset_min: 0,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 1,
    },
    Locale {
        id: "zh-CN",
        native_name: "简体中文",
        latin_name: "Chinese (Simplified)",
        utc_offset_min: 8 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 2,
    },
    Locale {
        id: "ko-KR",
        native_name: "한국어",
        latin_name: "Korean",
        utc_offset_min: 9 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 3,
    },
    Locale {
        id: "th-TH",
        native_name: "ภาษาไทย",
        latin_name: "Thai",
        utc_offset_min: 7 * 60,
        kind: Kind::Earth,
        uses_daemon_script: false,
        flag_id: 4,
    },
    // -- Fictional / celestial locales -----------------------------------
    Locale {
        id: "ling-country",
        native_name: "Ling Country",
        latin_name: "Ling Country",
        // 8 is a lucky number in Chinese culture (sounds like "prosper") --
        // an offset no real timezone uses, deliberately: this locale isn't
        // claiming to be a real place.
        utc_offset_min: 8 * 60 + 8,
        kind: Kind::Celestial,
        uses_daemon_script: true,
        flag_id: 5,
    },
    Locale {
        id: "new-thailand",
        native_name: "New Thailand",
        latin_name: "New Thailand",
        utc_offset_min: 7 * 60 + 30,
        kind: Kind::Celestial,
        uses_daemon_script: false,
        flag_id: 6,
    },
    Locale {
        id: "moon-palace",
        native_name: "月宫",
        latin_name: "Moon Palace (Chang'e, 嫦娥)",
        // Symbolic sort key only -- see `moon_day_progress` for the real
        // computation this locale's clock actually runs on.
        utc_offset_min: 0,
        kind: Kind::Celestial,
        uses_daemon_script: false,
        flag_id: 7,
    },
    Locale {
        id: "dragon-palace",
        native_name: "龙宫",
        latin_name: "Dragon Palace (water world, ~3x Earth radius)",
        // Invented rotation period for this water world: 34h, expressed as
        // an offset purely for display ordering, not a real clock model.
        utc_offset_min: 34 * 60,
        kind: Kind::Celestial,
        uses_daemon_script: false,
        flag_id: 8,
    },
];

pub fn count() -> usize {
    LOCALES.len()
}

pub fn get(i: usize) -> Option<&'static Locale> {
    LOCALES.get(i)
}

static mut SELECTED: Option<usize> = None;

/// Record the user's locale choice — kernel-side state for the same reason
/// `wm_liquid`'s spring simulation is (see that module's doc): `.ling`'s
/// AOT kernel path has no mutable-reassignment construct, so a selection
/// made on one frame of an infinite event loop can't be remembered by a
/// `bind` on the next frame. The picker screen polls [`selected`] each
/// frame instead.
pub fn select(i: usize) {
    unsafe { SELECTED = Some(i) };
}

pub fn selected() -> Option<usize> {
    unsafe { SELECTED }
}

/// How far through its real day/night cycle the Moon currently is, as a
/// permille (0..1000) of a full cycle. Grounded in the actual synodic month
/// (29.530589 days — the Moon's tidally-locked day length, distinct from
/// its ~27.3-day sidereal rotation) applied to the real RTC-derived Unix
/// timestamp, not an invented number.
pub fn moon_day_progress_permille() -> u32 {
    const SYNODIC_MONTH_SECS: i64 = 2_551_443; // 29.530589 days
    // A fixed reference new-moon epoch (2000-01-06 18:14 UTC, a commonly
    // cited J2000-era reference new moon) so the phase is at least
    // approximately right rather than arbitrary from an epoch-zero start.
    const REFERENCE_NEW_MOON_UNIX: i64 = 947_182_440;
    let ts = rtc::unix_timestamp();
    let elapsed = (ts - REFERENCE_NEW_MOON_UNIX).rem_euclid(SYNODIC_MONTH_SECS);
    (elapsed * 1000 / SYNODIC_MONTH_SECS) as u32
}
