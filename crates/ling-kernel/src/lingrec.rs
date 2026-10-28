//! Records (`form`) and results (`ok` / `bad`) for compiled Ling.
//!
//! The kernel's value model has numbers, strings and lists; the AOT compiler
//! routes everything else through `ling_builtin` by name, the same way the
//! host JIT hands it to its interpreter:
//!
//! * a constructor `Meta(a, b)` calls the builtin "Meta" -- a form the image
//!   declared (`__ling_form_def`, emitted by the compiler at start-up);
//! * a field read `r.height` calls "<recv>.height" with the receiver first;
//! * `ok(v)` / `bad(e)` are builtins, and `match` arms call
//!   `__ling_pat_test` / `__ling_pat_part`.
//!
//! Both kinds are lists with a tag string first (it starts with \u{1}, which
//! no Ling string literal holds): a record is `[tag "\u{1}R<Form>", names,
//! v0, v1, ..]` (`names` is a list of the field names, shared by every record
//! of the form), a result is `[tag "\u{1}Vok" | "\u{1}Vbad", payload]`. Being
//! lists they are collected, copied across channels and compared like any
//! other value.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lingrt::{is_list, is_str, list_from, list_items, mk_str, num, str_of, to_f64, UNIT};

const TAG_REC: &str = "\u{1}R";
const TAG_OK: &str = "\u{1}Vok";
const TAG_BAD: &str = "\u{1}Vbad";

/// Form name -> its field-name list (a Ling list, kept alive as a GC root).
static mut FORMS: BTreeMap<String, u64> = BTreeMap::new();
/// The two result tags, made once (roots too).
static mut OK_TAG: u64 = 0;
static mut BAD_TAG: u64 = 0;

fn forms() -> &'static mut BTreeMap<String, u64> {
    unsafe { &mut *(&raw mut FORMS) }
}

/// GC roots this module holds.
pub fn roots(mark: &mut dyn FnMut(u64)) {
    for &v in forms().values() {
        mark(v);
    }
    unsafe {
        if OK_TAG != 0 {
            mark(OK_TAG);
            mark(BAD_TAG);
        }
    }
}

fn tags() -> (u64, u64) {
    unsafe {
        if OK_TAG == 0 {
            OK_TAG = mk_str(TAG_OK);
            BAD_TAG = mk_str(TAG_BAD);
        }
        (OK_TAG, BAD_TAG)
    }
}

/// The tag of a record / result value ("" for anything else).
fn tag_of(v: u64) -> &'static str {
    if !is_list(v) {
        return "";
    }
    match list_items(v).first() {
        Some(&t) if is_str(t) => {
            let s = str_of(t);
            if s.starts_with('\u{1}') { s } else { "" }
        },
        _ => "",
    }
}

// ── Results ──────────────────────────────────────────────────────────────────

pub fn mk_ok(v: u64) -> u64 {
    list_from(alloc::vec![tags().0, v])
}

pub fn mk_bad(v: u64) -> u64 {
    list_from(alloc::vec![tags().1, v])
}

pub fn mk_bad_str(s: &str) -> u64 {
    mk_bad(mk_str(s))
}

pub fn ok_unit() -> u64 {
    mk_ok(UNIT)
}

/// `Some((true, payload))` for `ok(payload)`, `Some((false, e))` for `bad(e)`.
pub fn result_of(v: u64) -> Option<(bool, u64)> {
    match tag_of(v) {
        TAG_OK => Some((true, list_items(v).get(1).copied().unwrap_or(UNIT))),
        TAG_BAD => Some((false, list_items(v).get(1).copied().unwrap_or(UNIT))),
        _ => None,
    }
}

// ── Records ──────────────────────────────────────────────────────────────────

/// A field-name list for `mk_record` (cache it for repeated shapes).
pub fn names_list(names: &[&str]) -> u64 {
    list_from(names.iter().map(|n| mk_str(n)).collect())
}

/// A record of form `form` whose fields are named by `names` (a list made by
/// `names_list`, or a registered form's) holding `vals`.
pub fn mk_record(form: &str, names: u64, vals: &[u64]) -> u64 {
    let mut items = Vec::with_capacity(vals.len() + 2);
    items.push(mk_str(&alloc::format!("{TAG_REC}{form}")));
    items.push(names);
    items.extend_from_slice(vals);
    list_from(items)
}

/// A record of a form the program declared, fields in declaration order
/// (missing ones read as unit). `None` for an unknown form.
pub fn mk_form(form: &str, vals: &[u64]) -> Option<u64> {
    let names = *forms().get(form)?;
    let n = list_items(names).len();
    let mut v: Vec<u64> = vals.iter().copied().take(n).collect();
    v.resize(n, UNIT);
    Some(mk_record(form, names, &v))
}

pub fn is_record(v: u64) -> bool {
    tag_of(v).starts_with(TAG_REC)
}

/// A record's field (`None` if `v` is not a record or has no such field).
pub fn field(v: u64, name: &str) -> Option<u64> {
    if !is_record(v) {
        return None;
    }
    let items = list_items(v);
    let names = list_items(*items.get(1)?);
    let i = names.iter().position(|&n| str_of(n) == name)?;
    items.get(2 + i).copied()
}

/// The form name of a record ("" otherwise).
pub fn form_of(v: u64) -> &'static str {
    tag_of(v).strip_prefix(TAG_REC).unwrap_or("")
}

// ── Builtins ─────────────────────────────────────────────────────────────────

pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let arg = |i: usize| args.get(i).copied().unwrap_or(UNIT);
    match name {
        b"ok" | b"\xE5\xA5\xBD" => return Some(mk_ok(arg(0))), // ok, 好
        b"bad" | b"\xE5\x9D\x8F" => return Some(mk_bad(arg(0))), // bad, 坏
        b"__ling_form_def" => {
            let form = String::from(str_of(arg(0)));
            let names = list_from(args.iter().skip(1).copied().collect());
            forms().insert(form, names);
            return Some(UNIT);
        },
        b"__ling_pat_test" => return Some(crate::lingrt::boolean(pat_test(arg(0), str_of(arg(1)), str_of(arg(2)), to_f64(arg(3))))),
        b"__ling_pat_part" => return Some(pat_part(arg(0), str_of(arg(1)), to_f64(arg(3)))),
        b"is_ok" => return Some(crate::lingrt::boolean(matches!(result_of(arg(0)), Some((true, _))))),
        b"is_bad" | b"is_err" => return Some(crate::lingrt::boolean(matches!(result_of(arg(0)), Some((false, _))))),
        _ => {},
    }
    let name = core::str::from_utf8(name).ok()?;
    // `recv.field`: a field read (the compiler passes the receiver first)
    if let Some((_, fname)) = name.rsplit_once('.') {
        let recv = *args.first()?;
        if is_record(recv) {
            return Some(field(recv, fname).unwrap_or(UNIT));
        }
        if let Some((good, payload)) = result_of(recv) {
            // the interpreter's Result fields
            return Some(match fname {
                "is_ok" => crate::lingrt::boolean(good),
                "is_bad" | "is_err" => crate::lingrt::boolean(!good),
                "value" | "unwrap" if good => payload,
                "error" if !good => payload,
                _ => UNIT,
            });
        }
        if fname == "len" {
            return Some(num(if is_str(recv) { str_of(recv).chars().count() } else { list_items(recv).len() } as f64));
        }
        return None;
    }
    if forms().contains_key(name) {
        return mk_form(name, args);
    }
    None
}

/// `__ling_pat_test(val, "c" | "v", name, arity)`: the interpreter's
/// `match_pattern` for constructor (`ok(x)`, `bad(e)`) and variant patterns.
fn pat_test(val: u64, kind: &str, ctor: &str, _n: f64) -> bool {
    let res = result_of(val);
    match ctor {
        "ok" | "好" | "Ok" => match res {
            Some((good, _)) => good,
            // `ok(v)` also matches a plain value (anything but a `bad`)
            None => kind == "c",
        },
        "bad" | "坏" | "err" | "Bad" | "Err" => matches!(res, Some((false, _))),
        _ => is_record(val) && form_of(val) == ctor,
    }
}

fn pat_part(val: u64, kind: &str, i: f64) -> u64 {
    if let Some((_, payload)) = result_of(val) {
        return payload;
    }
    if is_record(val) {
        return list_items(val).get(2 + i.max(0.0) as usize).copied().unwrap_or(UNIT);
    }
    if kind == "c" { val } else { UNIT }
}
