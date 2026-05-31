//! Lexicon storage and lookup.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageCode {
    En, Zh, Ja, Ko, Th, Ru, Hi, Ar, Es, Fr, De, Pt, Vi, El, He, Tr, Pl,
}

impl LanguageCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en", Self::Zh => "zh", Self::Ja => "ja", Self::Ko => "ko",
            Self::Th => "th", Self::Ru => "ru", Self::Hi => "hi", Self::Ar => "ar",
            Self::Es => "es", Self::Fr => "fr", Self::De => "de", Self::Pt => "pt",
            Self::Vi => "vi", Self::El => "el", Self::He => "he", Self::Tr => "tr",
            Self::Pl => "pl",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Self::En), "zh" => Some(Self::Zh), "ja" => Some(Self::Ja),
            "ko" => Some(Self::Ko), "th" => Some(Self::Th), "ru" => Some(Self::Ru),
            "hi" => Some(Self::Hi), "ar" => Some(Self::Ar), "es" => Some(Self::Es),
            "fr" => Some(Self::Fr), "de" => Some(Self::De), "pt" => Some(Self::Pt),
            "vi" => Some(Self::Vi), "el" => Some(Self::El), "he" => Some(Self::He),
            "tr" => Some(Self::Tr), "pl" => Some(Self::Pl), _ => None,
        }
    }
}

/// One entry: canonical English key → native-script surface form.
#[derive(Debug, Clone)]
pub struct LexiconEntry {
    pub canonical: String,
    pub native:    String,
}

/// A complete lexicon for one language.
#[derive(Debug, Clone)]
pub struct Lexicon {
    pub code:     LanguageCode,
    /// keyword/builtin → native word
    pub keywords: HashMap<String, String>,
    /// native word → canonical English key (reverse index)
    pub reverse:  HashMap<String, String>,
}

impl Lexicon {
    pub fn new(code: LanguageCode) -> Self {
        Self { code, keywords: HashMap::new(), reverse: HashMap::new() }
    }

    pub fn insert(&mut self, canonical: impl Into<String>, native: impl Into<String>) {
        let c = canonical.into();
        let n = native.into();
        self.reverse.insert(n.clone(), c.clone());
        self.keywords.insert(c, n);
    }

    pub fn to_native(&self, canonical: &str) -> Option<&str> {
        self.keywords.get(canonical).map(|s| s.as_str())
    }

    pub fn to_canonical(&self, native: &str) -> Option<&str> {
        self.reverse.get(native).map(|s| s.as_str())
    }
}
