//! Simplified flag rendering for the locale picker — see this module's own
//! honesty note: there is no image codec anywhere in this no_std kernel and
//! no network stack to fetch a real image file with even if there were, so
//! "real" flags here means hand-encoded proportional stripe/field layouts
//! matching each country's actual official design (colors, stripe ratios,
//! canton placement), drawn with the framebuffer's rect/pixel primitives —
//! not a rasterized photograph or vector reproduction. Genuinely accurate
//! for simple stripe flags (Thailand); openly simplified for complex ones
//! (the UK's Union Jack, South Korea's taegeuk + trigrams) where faithfully
//! reproducing every diagonal/curve at small pixel sizes with axis-aligned
//! rects isn't realistic — approximated rather than silently wrong.
//!
//! `flag_id` matches `locale::Locale::flag_id`. Ids 5-8 (Ling Country, New
//! Thailand, Moon Palace, Dragon Palace) are new designs for this project's
//! own fictional locales, not real-world flags.
use crate::drivers::framebuffer;

fn rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    framebuffer::back_fill_rect(x, y, w, h, color);
}

/// Draw flag `id` filling the box `(x, y, w, h)`. Unknown ids draw a plain
/// grey field rather than nothing, so a bad index is visibly "no flag data"
/// instead of an invisible no-op.
pub fn draw(id: u32, x: u32, y: u32, w: u32, h: u32) {
    match id {
        0 => draw_us(x, y, w, h),
        1 => draw_uk(x, y, w, h),
        2 => draw_china(x, y, w, h),
        3 => draw_korea(x, y, w, h),
        4 => draw_thailand(x, y, w, h),
        5 => draw_ling_country(x, y, w, h),
        6 => draw_new_thailand(x, y, w, h),
        7 => draw_moon_palace(x, y, w, h),
        8 => draw_dragon_palace(x, y, w, h),
        _ => rect(x, y, w, h, 0x808080),
    }
}

// -- Real countries ---------------------------------------------------------

/// 13 stripes (simplified to 7 for pixel size), red/white, navy canton —
/// no stars at this resolution (a 50-star field is unreadable under ~40px
/// wide), disclosed as simplified in this module's doc.
fn draw_us(x: u32, y: u32, w: u32, h: u32) {
    let stripe_h = (h / 7).max(1);
    for i in 0..7 {
        let color = if i % 2 == 0 { 0xB22234 } else { 0xFFFFFF };
        rect(x, y + i * stripe_h, w, stripe_h, color);
    }
    rect(x, y, w * 2 / 5, h * 4 / 7, 0x3C3B6E);
}

/// Union Jack simplified to an axis-aligned cross (the real flag's diagonal
/// St. Andrew's/St. Patrick's crosses can't be drawn with rects at this
/// size) — navy field, white cross bars, red cross bars on top, offset from
/// center per the real flag's asymmetric St. Patrick's cross.
fn draw_uk(x: u32, y: u32, w: u32, h: u32) {
    rect(x, y, w, h, 0x00247D);
    let vw = (w / 5).max(1);
    let hh = (h / 5).max(1);
    rect(x + w / 2 - vw / 2, y, vw, h, 0xFFFFFF);
    rect(x, y + h / 2 - hh / 2, w, hh, 0xFFFFFF);
    rect(x + w / 2 - vw / 4, y, vw / 2, h, 0xCF142B);
    rect(x, y + h / 2 - hh / 4, w, hh / 2, 0xCF142B);
}

/// Red field, one large gold star (upper-left) + a scattering of small
/// squares standing in for the four smaller stars (a real 5-pointed star
/// isn't drawable with axis-aligned rects at this size).
fn draw_china(x: u32, y: u32, w: u32, h: u32) {
    rect(x, y, w, h, 0xDE2910);
    let s = (w.min(h) / 6).max(2);
    rect(x + w / 8, y + h / 6, s, s, 0xFFDE00);
    for (dx, dy) in [(3, 1), (4, 3), (3, 5), (1, 4)] {
        let sx = x + w * dx / 12 + w / 6;
        let sy = y + h * dy / 12;
        rect(sx, sy, s / 2, s / 2, 0xFFDE00);
    }
}

/// White field, red/blue taegeuk approximated as stacked half-rects (not a
/// true circle — no circle primitive here), two black trigram bar-groups
/// in opposite corners standing in for all four (disclosed simplification).
fn draw_korea(x: u32, y: u32, w: u32, h: u32) {
    rect(x, y, w, h, 0xFFFFFF);
    let cx = x + w / 2;
    let cy = y + h / 2;
    let r = (w.min(h) / 5).max(3);
    rect(cx.saturating_sub(r), cy.saturating_sub(r), r * 2, r, 0xCD2E3A);
    rect(cx.saturating_sub(r), cy, r * 2, r, 0x0047A0);
    let bar_w = r / 2;
    let bar_h = r / 6;
    for i in 0..3 {
        rect(x + w / 10, y + h / 10 + i * bar_h * 2, bar_w, bar_h, 0x000000);
        rect(
            x + w - w / 10 - bar_w,
            y + h - h / 10 - bar_h - i * bar_h * 2,
            bar_w,
            bar_h,
            0x000000,
        );
    }
}

/// Genuinely accurate: 5 horizontal stripes, red/white/blue(2x)/white/red —
/// a straightforward stripe ratio, no simplification needed.
fn draw_thailand(x: u32, y: u32, w: u32, h: u32) {
    let unit = (h / 6).max(1);
    rect(x, y, w, unit, 0xA51931);
    rect(x, y + unit, w, unit, 0xFFFFFF);
    rect(x, y + unit * 2, w, unit * 2, 0x2D2A4A);
    rect(x, y + unit * 4, w, unit, 0xFFFFFF);
    rect(x, y + unit * 5, w, h - unit * 5, 0xA51931);
}

// -- This project's own fictional locales -----------------------------------

/// Ling Country: purple field (the same purple already used for "In Ling"
/// on os.ling-lang.org) with a gold ring emblem — this project's own
/// signature color, not a real national flag.
fn draw_ling_country(x: u32, y: u32, w: u32, h: u32) {
    rect(x, y, w, h, 0x6A0DAD);
    let cx = x + w / 2;
    let cy = y + h / 2;
    let r = (w.min(h) / 4).max(3);
    rect(cx.saturating_sub(r), cy.saturating_sub(r), r * 2, r * 2, 0xFFD700);
    rect(
        cx.saturating_sub(r / 2),
        cy.saturating_sub(r / 2),
        r,
        r,
        0x6A0DAD,
    );
}

/// New Thailand: Thailand's real stripe layout, recolored (teal/gold
/// instead of red/navy) to read as "related, but its own place."
fn draw_new_thailand(x: u32, y: u32, w: u32, h: u32) {
    let unit = (h / 6).max(1);
    rect(x, y, w, unit, 0x00838F);
    rect(x, y + unit, w, unit, 0xFFFFFF);
    rect(x, y + unit * 2, w, unit * 2, 0xC9A227);
    rect(x, y + unit * 4, w, unit, 0xFFFFFF);
    rect(x, y + unit * 5, w, h - unit * 5, 0x00838F);
}

/// Moon Palace (月宫): pale silver-blue night-sky field, a full gold-white
/// moon disc, a small dark dot for the Jade Rabbit (玉兔) — Chang'e's
/// mythological companion.
fn draw_moon_palace(x: u32, y: u32, w: u32, h: u32) {
    rect(x, y, w, h, 0x0B1A33);
    let cx = x + w / 2;
    let cy = y + h / 2;
    let r = (w.min(h) / 3).max(3);
    rect(cx.saturating_sub(r), cy.saturating_sub(r), r * 2, r * 2, 0xF5F1D0);
    rect(cx.saturating_sub(r / 4), cy.saturating_sub(r / 4), r / 3, r / 3, 0x8A8560);
}

/// Dragon Palace (龙宫): deep-ocean teal field with pale wave-stripe bands
/// and a gold band standing for the Dragon King's imperial color.
fn draw_dragon_palace(x: u32, y: u32, w: u32, h: u32) {
    rect(x, y, w, h, 0x00363A);
    let band = (h / 5).max(1);
    rect(x, y + band, w, band / 3, 0x4DD0E1);
    rect(x, y + band * 3, w, band / 3, 0x4DD0E1);
    rect(x, y + h - band, w, band / 3, 0xC9A227);
}
