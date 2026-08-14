use crate::parser::ast::{Expr, Item};

/// Entry-point binding names recognised across Ling's supported human languages.
/// Shared by the tree-walker and the MIR backends so every execution path agrees
/// on which top-level binding starts the program.
pub const ENTRY_NAMES: &[&str] = &[
    "start",
    "main",
    "启",
    "เริ่ม",
    "시작",
    "начать",
    "начало",
    "inicio",
    "comenzar",
    "début",
    "commencer",
    "démarrer",
    "anfang",
    "starten",
    "início",
    "शुरू",
    "ابدأ",
    "شروع", // Persian "start" — also used natively in Urdu
    "התחל", // Hebrew "start"
];

/// Resolve the entry binding among top-level items: a bind named after a known
/// entry keyword, otherwise the first bind whose value is a `do { }` block.
/// Mirrors the tree-walker's `find_entry`.
pub fn entry_name(items: &[Item]) -> Option<String> {
    for key in ENTRY_NAMES {
        if items
            .iter()
            .any(|i| matches!(i, Item::Bind(n, _) if n == key))
        {
            return Some((*key).to_string());
        }
    }
    items.iter().find_map(|i| match i {
        Item::Bind(n, Expr::Do(_)) => Some(n.clone()),
        _ => None,
    })
}
