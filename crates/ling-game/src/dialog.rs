//! Cinematic dialog boxes — Ocarina/Majora-style typed-out text with color-coded
//! highlighting, paging and auto-advance. This module is **pure logic** (no
//! rendering): it parses inline markup, reveals characters over time, and hands
//! the host the currently-visible coloured segments to draw with whatever font.
//!
//! Markup: `{n}…{/}` = character/name, `{p}…{/}` = place, `{i}…{/}` = item,
//! plain text otherwise; `\n` newline; `||` is a page break.

/// Colour role for a run of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role { Text = 0, Name = 1, Place = 2, Item = 3 }

impl Role {
    pub fn index(self) -> usize { self as usize }
    fn from_tag(c: char) -> Option<Role> {
        match c { 'n' => Some(Role::Name), 'p' => Some(Role::Place), 'i' => Some(Role::Item), _ => None }
    }
}

/// One styled character (newlines carried as `'\n'`).
#[derive(Clone, Debug)]
struct Glyph { ch: char, role: Role }

/// A visible run of same-role text for the host to draw in one colour.
#[derive(Clone, Debug)]
pub struct Run { pub text: String, pub role: Role, pub newline_before: bool }

/// A cinematic dialog box.
#[derive(Clone, Debug)]
pub struct Dialog {
    pages: Vec<Vec<Glyph>>, // each page = styled glyphs
    page: usize,
    pub cps: f32,           // characters revealed per second
    pub revealed: f32,      // fractional chars shown on the current page
    pub hold: f32,          // seconds to wait, fully-typed, before auto-advance
    wait_t: f32,
    pub auto_advance: bool,
    pub closed: bool,
}

impl Dialog {
    /// Parse `raw` markup into a paged, styled dialog.
    pub fn new(raw: &str, cps: f32) -> Self {
        let mut pages: Vec<Vec<Glyph>> = vec![Vec::new()];
        let mut role = Role::Text;
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            // page break "||"
            if chars[i] == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
                pages.push(Vec::new()); i += 2; continue;
            }
            // tag "{x}" / "{/}"
            if chars[i] == '{' && i + 2 < chars.len() && chars[i + 2] == '}' {
                let t = chars[i + 1];
                if t == '/' { role = Role::Text; i += 3; continue; }
                if let Some(r) = Role::from_tag(t) { role = r; i += 3; continue; }
            }
            pages.last_mut().unwrap().push(Glyph { ch: chars[i], role });
            i += 1;
        }
        Self { pages, page: 0, cps: cps.max(1.0), revealed: 0.0, hold: 1.4, wait_t: 0.0, auto_advance: true, closed: false }
    }

    fn page_len(&self) -> usize { self.pages.get(self.page).map(|p| p.len()).unwrap_or(0) }

    /// Advance the typewriter + auto-advance timers by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        if self.closed { return; }
        let len = self.page_len();
        if (self.revealed as usize) < len {
            self.revealed += self.cps * dt;
        } else {
            self.wait_t += dt;
            if self.auto_advance && self.wait_t >= self.hold { self.advance(); }
        }
    }

    pub fn is_typing(&self) -> bool { (self.revealed as usize) < self.page_len() }
    pub fn is_last_page(&self) -> bool { self.page + 1 >= self.pages.len() }
    pub fn is_closed(&self) -> bool { self.closed }

    /// On a key press: if still typing, reveal the whole page; otherwise go to the
    /// next page (or close on the last page).
    pub fn advance(&mut self) {
        if self.closed { return; }
        if self.is_typing() {
            self.revealed = self.page_len() as f32;
        } else if self.is_last_page() {
            self.closed = true;
        } else {
            self.page += 1;
            self.revealed = 0.0;
            self.wait_t = 0.0;
        }
    }

    pub fn close(&mut self) { self.closed = true; }

    /// The currently-revealed text as coloured runs (split on role changes and
    /// newlines) — the host renders each run in its role colour.
    pub fn visible_runs(&self) -> Vec<Run> {
        let mut out: Vec<Run> = Vec::new();
        let page = match self.pages.get(self.page) { Some(p) => p, None => return out };
        let shown = (self.revealed as usize).min(page.len());
        let mut nl = false;
        for g in &page[..shown] {
            if g.ch == '\n' { nl = true; continue; }
            let push_new = match out.last() {
                Some(r) => r.role != g.role || nl,
                None => true,
            };
            if push_new {
                out.push(Run { text: String::new(), role: g.role, newline_before: nl });
                nl = false;
            }
            out.last_mut().unwrap().ch_push(g.ch);
        }
        out
    }
}

impl Run { fn ch_push(&mut self, c: char) { self.text.push(c); } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_markup_and_pages() {
        let d = Dialog::new("Hi {n}Link{/}, take the {i}Sword{/}.||Bye, {p}Hyrule{/}!", 1000.0);
        assert_eq!(d.pages.len(), 2);
    }
    #[test]
    fn typewriter_reveals_over_time() {
        let mut d = Dialog::new("Hello world", 10.0);
        assert!(d.is_typing());
        d.update(0.3); // ~3 chars
        let shown: usize = d.visible_runs().iter().map(|r| r.text.chars().count()).sum();
        assert!(shown >= 2 && shown <= 5, "shown={shown}");
        d.advance(); // reveal all
        assert!(!d.is_typing());
    }
    #[test]
    fn advance_pages_then_closes() {
        let mut d = Dialog::new("a||b", 1000.0);
        d.update(0.5);  // reveal page 0
        d.advance();    // -> page 1
        d.update(0.5);  // reveal page 1
        d.advance();    // last page, fully typed -> close
        assert!(d.is_closed());
    }
    #[test]
    fn runs_carry_roles() {
        let mut d = Dialog::new("go {p}home{/}", 1000.0);
        d.advance();
        let runs = d.visible_runs();
        assert!(runs.iter().any(|r| r.role == Role::Place && r.text.contains("home")));
    }
}
