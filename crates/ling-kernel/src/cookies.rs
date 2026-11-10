//! Horizon's cookie jar, so a login survives page loads and reboots.
//!
//! Privacy rules, because Horizon was cookie-less by design:
//! * First-party only. Horizon names the page's site (`set_site`) before each top-level navigation;
//!   a request is sent cookies, and a response may set them, only when its host belongs to that same
//!   site. A stylesheet, script or image from another site neither receives nor stores cookies.
//! * `Secure` cookies are only sent over https and only accepted from https; `__Secure-` and
//!   `__Host-` prefixes are enforced; a `Domain` that is a bare top-level domain is refused.
//! * Kept on the card sealed with the account's key (`home/<user>/Programs/Horizon/` is a private
//!   path: `vfs::write` seals it, and refuses to write at all while the keyring is locked, so
//!   nothing readable is ever left behind). Session cookies (no expiry) stay in memory.
//! * `clear()` forgets everything; `set_enabled(false)` stops sending and storing.

use alloc::string::String;
use alloc::vec::Vec;

struct Cookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    /// Unix seconds; 0 = a session cookie (not kept across reboots).
    expires: u64,
}

static mut JAR: Vec<Cookie> = Vec::new();
static mut SITE: String = String::new();
static mut ENABLED: bool = true;
static mut LOADED: bool = false;

const MAX_COOKIES: usize = 200;
const MAX_PER_SITE: usize = 50;
const MAX_LEN: usize = 4096;

fn jar() -> &'static mut Vec<Cookie> {
    unsafe { &mut *(&raw mut JAR) }
}

fn path_of_jar() -> Option<String> {
    crate::keyring::user().map(|u| alloc::format!("home/{u}/Programs/Horizon/cookies"))
}

/// Two-label public suffixes that a plain "last two labels" guess would get wrong.
const SECOND_LEVEL: [&str; 14] = [
    "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.jp", "ne.jp", "or.jp", "co.nz", "com.br", "co.kr", "com.cn",
];

/// The registrable domain of `host` (a guess without the full public-suffix list): "a.b.example.com"
/// -> "example.com", "x.example.co.uk" -> "example.co.uk". IP addresses and single labels stand for themselves.
pub fn site_of(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<core::net::Ipv4Addr>().is_ok() || !host.contains('.') {
        return host;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    if n >= 3 {
        let last2 = alloc::format!("{}.{}", labels[n - 2], labels[n - 1]);
        if SECOND_LEVEL.contains(&last2.as_str()) {
            return alloc::format!("{}.{}", labels[n - 3], last2);
        }
    }
    alloc::format!("{}.{}", labels[n - 2], labels[n - 1])
}

/// Name the site of the page being opened (its URL's host). Cookies flow only for this site.
pub fn set_site(host: &str) {
    unsafe { *(&raw mut SITE) = site_of(host) };
}

fn same_site(host: &str) -> bool {
    let site = unsafe { &*(&raw const SITE) };
    !site.is_empty() && site_of(host) == *site
}

pub fn set_enabled(on: bool) {
    unsafe { ENABLED = on };
}

pub fn enabled() -> bool {
    unsafe { ENABLED }
}

pub fn count() -> usize {
    ensure_loaded();
    jar().len()
}

pub fn clear() {
    jar().clear();
    unsafe { LOADED = true };
    save();
}

fn now() -> u64 {
    crate::clock::unix_now()
}

fn domain_matches(c: &Cookie, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if c.host_only {
        return host == c.domain;
    }
    host == c.domain || (host.len() > c.domain.len() && host.ends_with(&c.domain) && host.as_bytes()[host.len() - c.domain.len() - 1] == b'.')
}

fn path_matches(cookie_path: &str, req_path: &str) -> bool {
    if req_path == cookie_path {
        return true;
    }
    if !req_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || req_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

fn expired(c: &Cookie, t: u64) -> bool {
    c.expires != 0 && t != 0 && c.expires <= t
}

/// The `Cookie:` header value for a request, if any cookie applies.
pub fn header_for(host: &str, path: &str, https: bool) -> Option<String> {
    if !enabled() || !same_site(host) {
        return None;
    }
    ensure_loaded();
    let t = now();
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let mut parts: Vec<(usize, String)> = Vec::new();
    for c in jar().iter() {
        if expired(c, t) || (c.secure && !https) || !domain_matches(c, host) || !path_matches(&c.path, path) {
            continue;
        }
        parts.push((c.path.len(), alloc::format!("{}={}", c.name, c.value)));
    }
    if parts.is_empty() {
        return None;
    }
    // longer paths first, as browsers send them
    parts.sort_by(|a, b| b.0.cmp(&a.0));
    Some(parts.into_iter().map(|p| p.1).collect::<Vec<_>>().join("; "))
}

fn month(s: &str) -> Option<u64> {
    let m = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let l = s.to_ascii_lowercase();
    m.iter().position(|x| l.starts_with(x)).map(|i| i as u64 + 1)
}

/// "Wed, 21 Oct 2026 07:28:00 GMT" (also "21-Oct-26") -> unix seconds.
fn parse_date(s: &str) -> Option<u64> {
    let (mut day, mut mon, mut year, mut hms) = (None, None, None, None);
    for tok in s.split(|c: char| c == ' ' || c == ',' || c == '-').filter(|t| !t.is_empty()) {
        if tok.contains(':') && hms.is_none() {
            let v: Vec<u64> = tok.split(':').filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 2 {
                hms = Some((v[0], v[1], *v.get(2).unwrap_or(&0)));
            }
        } else if mon.is_none() && month(tok).is_some() && tok.chars().all(|c| c.is_ascii_alphabetic()) && tok.len() >= 3 {
            mon = month(tok);
        } else if let Ok(n) = tok.parse::<u64>() {
            if day.is_none() && n >= 1 && n <= 31 && tok.len() <= 2 {
                day = Some(n);
            } else if year.is_none() {
                year = Some(if tok.len() <= 2 { if n < 70 { 2000 + n } else { 1900 + n } } else { n });
            }
        }
    }
    let (d, m, y, (hh, mm, ss)) = (day?, mon?, year?, hms?);
    if y < 1970 {
        return Some(1);
    }
    // days from civil (Howard Hinnant)
    let (yy, mp) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy / 400;
    let yoe = yy - era * 400;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn clean(s: &str) -> bool {
    !s.bytes().any(|b| b < 0x20 || b == 0x7f || b == b';')
}

/// Take the `Set-Cookie` lines of a response's header block (`host` / `path` / `https` describe the
/// request it answers).
pub fn store_from_response(host: &str, req_path: &str, https: bool, headers: &str) {
    if !enabled() || !same_site(host) {
        return;
    }
    ensure_loaded();
    let t = now();
    let mut changed = false;
    for line in headers.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        if !k.trim().eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let v = v.trim();
        if v.len() > MAX_LEN {
            continue;
        }
        let mut segs = v.split(';');
        let Some(nv) = segs.next() else { continue };
        let Some((name, value)) = nv.split_once('=') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || !clean(name) || !clean(value) {
            continue;
        }
        let mut c = Cookie {
            name: String::from(name),
            value: String::from(value),
            domain: host.to_ascii_lowercase(),
            host_only: true,
            path: {
                // the default path: the request's directory
                let p = req_path.split(['?', '#']).next().unwrap_or("/");
                match p.rfind('/') {
                    Some(0) | None => String::from("/"),
                    Some(i) => String::from(&p[..i]),
                }
            },
            secure: false,
            expires: 0,
        };
        let mut max_age: Option<i64> = None;
        let mut bad = false;
        for a in segs {
            let a = a.trim();
            let (an, av) = a.split_once('=').map(|(x, y)| (x.trim(), y.trim())).unwrap_or((a, ""));
            match an.to_ascii_lowercase().as_str() {
                "domain" => {
                    let d = av.trim_start_matches('.').to_ascii_lowercase();
                    let on_host = host.eq_ignore_ascii_case(&d) || (host.len() > d.len() && host.to_ascii_lowercase().ends_with(&d) && host.as_bytes()[host.len() - d.len() - 1] == b'.');
                    // a bare TLD ("com") or a domain the host is not under is refused
                    if d.is_empty() || !d.contains('.') || !on_host || SECOND_LEVEL.contains(&d.as_str()) || !same_site(&d) {
                        bad = true;
                    } else {
                        c.domain = d;
                        c.host_only = false;
                    }
                },
                "path" => {
                    if av.starts_with('/') {
                        c.path = String::from(av);
                    }
                },
                "secure" => c.secure = true,
                "max-age" => max_age = av.parse::<i64>().ok(),
                "expires" => {
                    if let Some(e) = parse_date(av) {
                        c.expires = e;
                    }
                },
                _ => {},
            }
        }
        if bad || (c.secure && !https) {
            continue;
        }
        if name.starts_with("__Secure-") && !c.secure {
            continue;
        }
        if name.starts_with("__Host-") && (!c.secure || !c.host_only || c.path != "/") {
            continue;
        }
        // Max-Age beats Expires; both need a clock to mean anything
        if let Some(m) = max_age {
            c.expires = if m <= 0 { 1 } else if t == 0 { 0 } else { t + m as u64 };
        } else if t == 0 {
            c.expires = 0;
        }
        let gone = c.expires != 0 && t != 0 && c.expires <= t;
        // replace the cookie of the same name, domain and path
        let j = jar();
        let before = j.len();
        j.retain(|o| !(o.name == c.name && o.domain == c.domain && o.path == c.path));
        let replaced = j.len() != before;
        if gone {
            changed |= replaced;
            continue;
        }
        if j.len() >= MAX_COOKIES || j.iter().filter(|o| site_of(&o.domain) == site_of(&c.domain)).count() >= MAX_PER_SITE {
            // make room: drop the oldest of this site (the front)
            if let Some(i) = j.iter().position(|o| site_of(&o.domain) == site_of(&c.domain)) {
                j.remove(i);
            } else if !j.is_empty() {
                j.remove(0);
            }
        }
        changed |= c.expires != 0 || replaced;
        j.push(c);
    }
    if changed {
        save();
    }
}

fn ensure_loaded() {
    unsafe {
        if LOADED {
            return;
        }
    }
    let Some(p) = path_of_jar() else { return };
    unsafe { LOADED = true };
    let Some(bytes) = crate::vfs::read(&p) else { return };
    let Ok(text) = core::str::from_utf8(&bytes) else { return };
    let t = now();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 {
            continue;
        }
        let c = Cookie {
            name: String::from(f[0]),
            value: String::from(f[1]),
            domain: String::from(f[2]),
            host_only: f[3] == "1",
            path: String::from(f[4]),
            secure: f[5] == "1",
            expires: f[6].parse().unwrap_or(0),
        };
        if c.expires != 0 && !expired(&c, t) && jar().len() < MAX_COOKIES {
            jar().push(c);
        }
    }
}

/// Write the persistent cookies to the card (sealed). Silently skipped while the keyring is locked.
fn save() {
    let Some(p) = path_of_jar() else { return };
    let mut out = String::new();
    for c in jar().iter().filter(|c| c.expires != 0) {
        out.push_str(&alloc::format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", c.name, c.value, c.domain, c.host_only as u8, c.path, c.secure as u8, c.expires));
    }
    let _ = crate::vfs::mkdir(&alloc::format!("home/{}/Programs", crate::keyring::user().unwrap_or_default()));
    let _ = crate::vfs::mkdir(&alloc::format!("home/{}/Programs/Horizon", crate::keyring::user().unwrap_or_default()));
    if !crate::vfs::write(&p, out.as_bytes()) {
        crate::console_write(b"cookies: could not save the cookie jar (is the keyring unlocked?)\n");
    }
}
