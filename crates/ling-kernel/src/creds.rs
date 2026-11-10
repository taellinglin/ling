//! A shared credential store: a login or API key saved once, usable by the apps the user allows.
//!
//! * Sealed at rest: the file is `home/<user>/Private/credentials`, a private path, so `vfs` seals it
//!   with the account's key (and refuses to write while the keyring is locked).
//! * Consent is per app and the kernel decides who is asking: the caller is the running app's name
//!   from the app table (`lingapp::active_app_name`), or "desktop" when no app is on screen. An app
//!   cannot claim another name. A credential is readable only by its owner (the app that saved it)
//!   and the apps the user allowed in Settings > Sign-in (the desktop's "Saved logins" list).
//! * Only the desktop can grant or revoke, list every entry or see who has access.
//! * Secrets never appear in a listing, only service names.

use alloc::string::String;
use alloc::vec::Vec;

struct Entry {
    service: String,
    secret: String,
    owner: String,
    allowed: Vec<String>,
}

static mut STORE: Vec<Entry> = Vec::new();
static mut LOADED: bool = false;

const DESKTOP: &str = "desktop";
const MAX_ENTRIES: usize = 64;
const MAX_SECRET: usize = 4096;

fn store() -> &'static mut Vec<Entry> {
    unsafe { &mut *(&raw mut STORE) }
}

fn caller() -> String {
    // the kernel's own answer ("desktop" when no app has the screen): an app cannot name itself
    crate::lingapp::active_app_name()
}

fn path() -> Option<String> {
    crate::keyring::user().map(|u| alloc::format!("home/{u}/Private/credentials"))
}

fn valid_service(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'_')
}

fn hex(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        o.push_str(&alloc::format!("{b:02x}"));
    }
    o
}

fn unhex(s: &str) -> Option<String> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut v = Vec::new();
    for i in (0..s.len()).step_by(2) {
        v.push(u8::from_str_radix(&s[i..i + 2], 16).ok()?);
    }
    String::from_utf8(v).ok()
}

fn ensure_loaded() {
    unsafe {
        if LOADED {
            return;
        }
    }
    let Some(p) = path() else { return };
    unsafe { LOADED = true };
    let Some(bytes) = crate::vfs::read(&p) else { return };
    let Ok(text) = core::str::from_utf8(&bytes) else { return };
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 4 {
            continue;
        }
        let (Some(secret), true) = (unhex(f[1]), valid_service(f[0])) else { continue };
        store().push(Entry {
            service: String::from(f[0]),
            secret,
            owner: String::from(f[2]),
            allowed: f[3].split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
        });
    }
}

fn save() -> bool {
    let Some(p) = path() else { return false };
    let mut out = String::new();
    for e in store().iter() {
        out.push_str(&alloc::format!("{}\t{}\t{}\t{}\n", e.service, hex(&e.secret), e.owner, e.allowed.join(",")));
    }
    let user = crate::keyring::user().unwrap_or_default();
    let _ = crate::vfs::mkdir(&alloc::format!("home/{user}/Private"));
    crate::vfs::write(&p, out.as_bytes())
}

fn can_read(e: &Entry, who: &str) -> bool {
    e.owner == who || e.allowed.iter().any(|a| a == who)
}

/// Save (or replace) a credential. The saver owns a new entry; an existing one can be replaced by its
/// owner or the desktop only.
pub fn set(service: &str, secret: &str) -> bool {
    ensure_loaded();
    let who = caller();
    if !valid_service(service) || secret.is_empty() || secret.len() > MAX_SECRET || path().is_none() {
        return false;
    }
    if let Some(e) = store().iter_mut().find(|e| e.service == service) {
        if e.owner != who && who != DESKTOP {
            return false;
        }
        e.secret = String::from(secret);
    } else {
        if store().len() >= MAX_ENTRIES {
            return false;
        }
        store().push(Entry { service: String::from(service), secret: String::from(secret), owner: who.clone(), allowed: alloc::vec![who] });
    }
    save()
}

/// The secret, if the caller owns it or was allowed.
pub fn get(service: &str) -> Option<String> {
    ensure_loaded();
    let who = caller();
    store().iter().find(|e| e.service == service && can_read(e, &who)).map(|e| e.secret.clone())
}

/// Service names the caller can read (the desktop sees them all).
pub fn list() -> Vec<String> {
    ensure_loaded();
    let who = caller();
    store().iter().filter(|e| who == DESKTOP || can_read(e, &who)).map(|e| e.service.clone()).collect()
}

/// "owner|app,app,..." for the desktop's Saved logins list; "" for anyone else.
pub fn info(service: &str) -> String {
    ensure_loaded();
    if caller() != DESKTOP {
        return String::new();
    }
    store()
        .iter()
        .find(|e| e.service == service)
        .map(|e| alloc::format!("{}|{}", e.owner, e.allowed.join(",")))
        .unwrap_or_default()
}

/// Let `app` read `service` (or not): the desktop only. The owner always keeps access.
pub fn allow(service: &str, app: &str, on: bool) -> bool {
    ensure_loaded();
    if caller() != DESKTOP || app.is_empty() || app.contains(',') || app.contains('\t') {
        return false;
    }
    let Some(e) = store().iter_mut().find(|e| e.service == service) else { return false };
    if e.owner == app {
        return true;
    }
    e.allowed.retain(|a| a != app);
    if on {
        e.allowed.push(String::from(app));
    }
    save()
}

/// Delete a credential: its owner or the desktop.
pub fn forget(service: &str) -> bool {
    ensure_loaded();
    let who = caller();
    let before = store().len();
    store().retain(|e| !(e.service == service && (e.owner == who || who == DESKTOP)));
    store().len() != before && save()
}
