//! Keyboard shortcut management and hotkey registry.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
    };

    pub fn ctrl() -> Self {
        Self {
            ctrl: true,
            ..Self::NONE
        }
    }

    pub fn ctrl_shift() -> Self {
        Self {
            ctrl: true,
            shift: true,
            ..Self::NONE
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub key: String,
    pub modifiers: Modifiers,
}

impl KeyCombo {
    pub fn new(key: impl Into<String>, modifiers: Modifiers) -> Self {
        Self {
            key: key.into().to_uppercase(),
            modifiers,
        }
    }

    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if self.modifiers.alt {
            parts.push("Alt");
        }
        if self.modifiers.shift {
            parts.push("Shift");
        }
        if self.modifiers.meta {
            parts.push("Meta");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

#[derive(Debug, Clone)]
pub struct ShortcutBinding {
    pub id: String,
    pub combo: KeyCombo,
    pub description: String,
    pub global: bool,
}

#[derive(Debug, Default)]
pub struct ShortcutRegistry {
    bindings: HashMap<String, ShortcutBinding>,
    combo_index: HashMap<KeyCombo, String>,
}

impl ShortcutRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        id: impl Into<String>,
        combo: KeyCombo,
        description: impl Into<String>,
        global: bool,
    ) {
        let id_str = id.into();
        let binding = ShortcutBinding {
            id: id_str.clone(),
            combo: combo.clone(),
            description: description.into(),
            global,
        };
        self.combo_index.insert(combo, id_str.clone());
        self.bindings.insert(id_str, binding);
    }

    pub fn match_combo(&self, combo: &KeyCombo) -> Option<&ShortcutBinding> {
        self.combo_index
            .get(combo)
            .and_then(|id| self.bindings.get(id))
    }

    pub fn get_display(&self, id: &str) -> Option<String> {
        self.bindings.get(id).map(|b| b.combo.display())
    }

    pub fn bindings(&self) -> &HashMap<String, ShortcutBinding> {
        &self.bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combo_display() {
        let combo = KeyCombo::new("Z", Modifiers::ctrl_shift());
        assert_eq!(combo.display(), "Ctrl+Shift+Z");
    }

    #[test]
    fn test_registry_lookup() {
        let mut reg = ShortcutRegistry::new();
        let combo = KeyCombo::new("S", Modifiers::ctrl());
        reg.register("file.save", combo.clone(), "Save current file", true);

        let matched = reg.match_combo(&combo);
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().id, "file.save");
        assert_eq!(reg.get_display("file.save").unwrap(), "Ctrl+S");
    }
}
