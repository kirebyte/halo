//! Translations for user-facing text (CLI output, D-Bus error messages, the window).
//!
//! Keys are either the English sentence itself or a dotted id for things that have no natural
//! English source (`effect.rainbow`, `color.red`, `usage`). Tables live in `locales/*.json`, flat
//! `"key": "text"` objects embedded at build time. The language comes from `LC_ALL`,
//! `LC_MESSAGES` or `LANG` (first two letters); anything unknown falls back to English. Daemon
//! logs are not translated: they go to the journal, in English.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

const EN: &str = include_str!("../../../locales/en.json");
const ES: &str = include_str!("../../../locales/es.json");
const FR: &str = include_str!("../../../locales/fr.json");
const JA: &str = include_str!("../../../locales/ja.json");

/// Languages shipped, with their tables.
pub const LANGUAGES: &[(&str, &str)] = &[("en", EN), ("es", ES), ("fr", FR), ("ja", JA)];

type Table = HashMap<String, String>;

fn parse(json: &str) -> Table {
    serde_json::from_str(json).expect("locales/*.json is a flat object of strings")
}

fn english() -> &'static Table {
    static EN_TABLE: OnceLock<Table> = OnceLock::new();
    EN_TABLE.get_or_init(|| parse(EN))
}

fn current() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let lang = language();
        LANGUAGES.iter().find(|(l, _)| *l == lang).map_or_else(HashMap::new, |(_, json)| parse(json))
    })
}

/// The two-letter language in use: `en` unless the environment asks for another shipped one.
pub fn language() -> &'static str {
    static LANG: OnceLock<&'static str> = OnceLock::new();
    LANG.get_or_init(|| {
        let wanted = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
            .map(|v| v.chars().take(2).collect::<String>().to_lowercase());
        LANGUAGES.iter().map(|(l, _)| *l).find(|l| wanted.as_deref() == Some(l)).unwrap_or("en")
    })
}

/// The text for `key` in the current language; English, or the key itself, when missing.
pub fn tr(key: &str) -> String {
    current().get(key).or_else(|| english().get(key)).cloned().unwrap_or_else(|| key.to_string())
}

/// Fills `{name}` placeholders of a translated template.
pub fn format(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// `tr!("Some text")` or `tr!("Hello {name}", name = who)`.
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::i18n::tr($key)
    };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::format(&$crate::i18n::tr($key), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_parse_and_agree() {
        let tables: Vec<(&str, Table)> = LANGUAGES.iter().map(|(l, j)| (*l, parse(j))).collect();
        let mut keys: Vec<_> = tables.iter().filter(|(l, _)| *l != "en").map(|(l, t)| (*l, t.keys().collect::<std::collections::BTreeSet<_>>())).collect();
        let (first, reference) = keys.remove(0);
        for (lang, set) in keys {
            let missing: Vec<_> = reference.difference(&set).collect();
            let extra: Vec<_> = set.difference(&reference).collect();
            assert!(missing.is_empty() && extra.is_empty(), "{lang} vs {first}: missing {missing:?}, extra {extra:?}");
        }
        // Dotted ids need an English text, since the key is not a sentence.
        let en = &tables[0].1;
        for key in reference.iter().filter(|k| !k.contains(' ') && k.contains('.')) {
            assert!(en.contains_key(*key), "en.json lacks {key}");
        }
        // Placeholders must survive translation.
        for (lang, table) in &tables {
            for (key, text) in table {
                for ph in key.split('{').skip(1).filter_map(|s| s.split('}').next()) {
                    assert!(text.contains(&format!("{{{ph}}}")), "{lang}: {key:?} lost {{{ph}}}");
                }
            }
        }
    }

    #[test]
    fn placeholders() {
        assert_eq!(format("{a} and {b}", &[("a", &1), ("b", &"two")]), "1 and two");
    }
}
