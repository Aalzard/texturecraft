//! UI text lookup. Keys are the English strings, so English needs no table and a missing
//! translation falls back to English. A language is one `Language` entry whose table lists
//! `(English, translation)` pairs sorted by the English key; `Prefs::language` picks it.

use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Language {
    /// BCP 47 code ("en", "de", "ja").
    pub code: &'static str,
    /// The language's own name, for a picker.
    pub name: &'static str,
    /// `(English, translation)`, sorted by the English key.
    pub table: &'static [(&'static str, &'static str)],
}

pub const LANGUAGES: &[Language] = &[Language { code: "en", name: "English", table: &[] }];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Switch the UI language (unknown codes fall back to English).
pub fn set_language(code: &str) {
    CURRENT.store(LANGUAGES.iter().position(|l| l.code == code).unwrap_or(0), Ordering::Relaxed);
}

/// `text` in the current language.
pub fn tr(text: &'static str) -> &'static str {
    let Some(lang) = LANGUAGES.get(CURRENT.load(Ordering::Relaxed)) else { return text };
    lang.table.binary_search_by_key(&text, |(en, _)| en).ok().and_then(|i| lang.table.get(i)).map_or(text, |(_, t)| t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_english_is_identity() {
        for l in LANGUAGES {
            assert!(l.table.windows(2).all(|w| w[0].0 < w[1].0), "{} table must be sorted by key", l.code);
        }
        set_language("xx");
        assert_eq!(tr("Layers"), "Layers");
    }
}
