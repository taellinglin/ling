use crate::core::{LingError, LingResult};
use crate::parser;
use crate::parser::ast::{Item, Program};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolve `use` imports and `mod` blocks ahead-of-time into a single flat
/// `Program` for the MIR backend, mirroring the interpreter's `load_module` /
/// `register_item` namespacing so the JIT/AOT pipelines see exactly what
/// `ling run` sees.
///
/// Function and constructor names are prefixed with their namespace
/// (`<ns>::<name>`), matching the keys the tree-walker stores in its function
/// table. Call sites lower a `Path` to the same `::`-joined string, so name
/// resolution agrees by construction. Module-level `bind` globals are dropped:
/// MIR has no global-variable concept (an unknown identifier lowers to a
/// function constant), so only the entry file's top-level binds form `__main__`.
pub fn flatten(entry: &Program, entry_dir: &Path) -> LingResult<Program> {
    let mut out = Vec::with_capacity(entry.items.len());
    let mut loaded = HashSet::new();
    process(&entry.items, "", entry_dir, true, &mut loaded, &mut out)?;
    Ok(Program { items: out })
}

fn process(
    items: &[Item],
    ns: &str,
    base_dir: &Path,
    is_entry: bool,
    loaded: &mut HashSet<String>,
    out: &mut Vec<Item>,
) -> LingResult<()> {
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
                let child_ns = if ns.is_empty() {
                    name.clone()
                } else {
                    format!("{ns}::{name}")
                };
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
    base_dir: &Path,
    loaded: &mut HashSet<String>,
    out: &mut Vec<Item>,
) -> LingResult<()> {
    let raw = Path::new(path);
    let candidates: [PathBuf; 8] = [
        base_dir.join(format!("{path}.ling")),
        base_dir.join(format!("{path}.灵")),
        base_dir.join(format!("{path}.령")),
        base_dir.join(format!("{path}.霊")),
        base_dir.join(format!("{path}.ลิง")),
        base_dir.join(raw),
        PathBuf::from(format!("{path}.ling")),
        PathBuf::from(path),
    ];

    let resolved = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| LingError::Io(format!("use: cannot find module '{path}'")))?;

    let canonical = resolved
        .canonicalize()
        .unwrap_or_else(|_| resolved.clone())
        .to_string_lossy()
        .into_owned();

    if !loaded.insert(canonical) {
        return Ok(());
    }

    let source = std::fs::read_to_string(resolved)
        .map_err(|e| LingError::Io(format!("use: failed to read '{path}': {e}")))?;
    let program = parser::parse(&source)
        .map_err(|e| LingError::Parse(format!("use: parse error in '{path}': {e}")))?;

    let target_ns = match (parent_ns.is_empty(), alias) {
        (false, Some(a)) => format!("{parent_ns}::{a}"),
        (true, Some(a)) => a.to_string(),
        (false, None) => parent_ns.to_string(),
        (true, None) => String::new(),
    };

    let nested_dir = resolved.parent().unwrap_or(base_dir);
    process(&program.items, &target_ns, nested_dir, false, loaded, out)
}
