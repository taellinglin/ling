//! Asset registry for the web runtime (and a thin shim natively).
//!
//! The browser cannot read files, so game assets are *streamed*: Ling code (or a
//! builtin such as `font_load`) calls [`request`], the JS host drains
//! [`take_requests`], fetches the bytes however it likes (cache, priority,
//! distance-based eviction) and hands them back through [`put`].  Ling polls
//! `asset_ready(path)`.  Keys are matched case-insensitively on the full path and,
//! failing that, on the file name alone, so a game script may keep native paths.
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static REG: RefCell<HashMap<String, Vec<u8>>> = RefCell::new(HashMap::new());
    static BASE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

fn norm(p: &str) -> String { p.replace('\\', "/").to_lowercase() }
fn base(p: &str) -> String { norm(p).rsplit('/').next().unwrap_or("").to_string() }

pub fn put(path: &str, bytes: Vec<u8>) {
    let k = norm(path);
    BASE.with(|b| b.borrow_mut().insert(base(path), k.clone()));
    REG.with(|r| r.borrow_mut().insert(k, bytes));
    PENDING.with(|p| p.borrow_mut().retain(|x| norm(x) != norm(path)));
}

fn key_for(path: &str) -> Option<String> {
    let k = norm(path);
    if REG.with(|r| r.borrow().contains_key(&k)) { return Some(k); }
    BASE.with(|b| b.borrow().get(&base(path)).cloned())
        .filter(|k| REG.with(|r| r.borrow().contains_key(k)))
}

pub fn has(path: &str) -> bool { key_for(path).is_some() }
pub fn get(path: &str) -> Option<Vec<u8>> {
    key_for(path).and_then(|k| REG.with(|r| r.borrow().get(&k).cloned()))
}
pub fn remove(path: &str) {
    if let Some(k) = key_for(path) { REG.with(|r| r.borrow_mut().remove(&k)); }
}
pub fn request(path: &str) {
    if has(path) { return; }
    PENDING.with(|p| { let mut p = p.borrow_mut(); if !p.iter().any(|x| norm(x) == norm(path)) { p.push(path.to_string()); } });
}
/// Newline-separated paths Ling has asked for since the last call.
pub fn take_requests() -> String {
    PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())).join("\n")
}
pub fn bytes_total() -> usize { REG.with(|r| r.borrow().values().map(|v| v.len()).sum()) }
pub fn count() -> usize { REG.with(|r| r.borrow().len()) }
