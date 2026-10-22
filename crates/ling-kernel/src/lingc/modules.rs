//! `use "path"` / `mod` resolution for the in-kernel compiler — the same
//! flattening as the host's `src/mir/modules.rs` (namespaced `ns::fn` names,
//! imported modules' top-level binds dropped, dedup by resolved path), reading
//! sources through [`read_source`] instead of `std::fs`: files baked into the
//! image (`appfs`) first, then lingfs on x86.

use super::ast::{Item, Program};
use super::parser;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A source file by path (`/`-separated, relative to the filesystem root).
pub fn read_source(path: &str) -> Option<String> {
    let p = path.trim_start_matches("./");
    // Storage first (a user's copy wins), then the image (crate::vfs does both).
    let bytes = crate::vfs::read(p)?;
    core::str::from_utf8(&bytes).ok().map(String::from)
}

/// Does `path` name a source file (without reading it)?
pub fn source_exists(path: &str) -> bool {
    let p = path.trim_start_matches("./");
    // A file, not a folder (`ling run pkg-<name>` must fall through to the
    // folder's entry).
    crate::vfs::exists(p) && !crate::vfs::is_dir(p)
}

/// Join `dir` and `rel`, collapsing `.` and `..` segments.
pub fn join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in dir.split('/').chain(rel.split('/')) {
        match seg {
            "" | "." => {},
            ".." => {
                parts.pop();
            },
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// The directory part of a path ("" for a bare file name).
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// Resolve the entry program's imports into one flat `Program`.
pub fn flatten(entry: &Program, entry_dir: &str) -> Result<Program, String> {
    let mut out = Vec::with_capacity(entry.items.len());
    let mut loaded = BTreeSet::new();
    process(&entry.items, "", entry_dir, true, &mut loaded, &mut out)?;
    Ok(Program { items: out })
}

fn process(
    items: &[Item],
    ns: &str,
    base_dir: &str,
    is_entry: bool,
    loaded: &mut BTreeSet<String>,
    out: &mut Vec<Item>,
) -> Result<(), String> {
    for item in items {
        match item {
            Item::Fn(def) => {
                let mut def = def.clone();
                if !ns.is_empty() {
                    def.name = format!("{ns}::{}", def.name);
                }
                out.push(Item::Fn(def));
            },
            // Only the entry file's top-level binds become the program body;
            // imported module globals are unrepresentable in MIR and so dropped.
            Item::Bind(name, expr) => {
                if is_entry && ns.is_empty() {
                    out.push(Item::Bind(name.clone(), expr.clone()));
                }
            },
            Item::Mod(name, body) => {
                let child_ns = if ns.is_empty() { name.clone() } else { format!("{ns}::{name}") };
                process(body, &child_ns, base_dir, is_entry, loaded, out)?;
            },
            Item::Use { path, alias } => {
                load_use(path, alias.as_deref(), ns, base_dir, loaded, out)?;
            },
            Item::Enum(..) | Item::Struct(..) | Item::TypeAlias(..) => {
                out.push(item.clone());
            },
        }
    }
    Ok(())
}

fn load_use(
    path: &str,
    alias: Option<&str>,
    parent_ns: &str,
    base_dir: &str,
    loaded: &mut BTreeSet<String>,
    out: &mut Vec<Item>,
) -> Result<(), String> {
    let candidates = [
        join(base_dir, &format!("{path}.ling")),
        join(base_dir, &format!("{path}.灵")),
        join(base_dir, &format!("{path}.령")),
        join(base_dir, &format!("{path}.霊")),
        join(base_dir, &format!("{path}.ลิง")),
        join(base_dir, path),
        join("", &format!("{path}.ling")),
        join("", path),
    ];
    let mut found: Option<(String, String)> = None;
    for c in candidates.iter() {
        if let Some(src) = read_source(c) {
            found = Some((c.clone(), src));
            break;
        }
    }
    let Some((resolved, source)) = found else {
        return Err(format!("use: cannot find module '{path}'"));
    };
    if !loaded.insert(resolved.clone()) {
        return Ok(());
    }
    let program = parser::parse(&source).map_err(|e| format!("use: parse error in '{path}': {e}"))?;
    let target_ns = match (parent_ns.is_empty(), alias) {
        (false, Some(a)) => format!("{parent_ns}::{a}"),
        (true, Some(a)) => a.to_string(),
        (false, None) => parent_ns.to_string(),
        (true, None) => String::new(),
    };
    let nested_dir = parent(&resolved).to_string();
    process(&program.items, &target_ns, &nested_dir, false, loaded, out)
}
