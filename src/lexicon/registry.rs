// src/lexicon/registry.rs
use phf::phf_map;
use std::collections::HashMap;

pub struct LexiconRegistry {
    lexicons: HashMap<String, Lexicon>,
    default: String,
}

impl LexiconRegistry {
    pub fn global() -> &'static Self {
        static REGISTRY: once_cell::sync::OnceCell<LexiconRegistry> = once_cell::sync::OnceCell::new();
        REGISTRY.get_or_init(|| Self::load_all())
    }
    
    fn load_all() -> Self {
        let mut registry = Self {
            lexicons: HashMap::new(),
            default: "en".to_string(),
        };
        
        // Load all 16 lexicons from lexicons/*.ling
        for code in &["en", "zh", "ja", "ko", "ru", "ar", "hi", "th", "de", "fr", "es", "el", "he", "vi", "tr", "pl"] {
            if let Ok(lex) = Lexicon::load(code) {
                registry.lexicons.insert(code.to_string(), lex);
            }
        }
        
        registry
    }
    
    pub fn get(&self, code: &str) -> Option<&Lexicon> {
        self.lexicons.get(code)
    }
    
    pub fn translate(&self, text: &str, from: &str, to: &str) -> String {
        // Translate between lexicons
        text.to_string()
    }
}
