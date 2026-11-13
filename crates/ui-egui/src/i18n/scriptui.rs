//! Display-time translations for ScriptUI windows.
//!
//! Scripts keep their strings in the script language (usually English) and a translation pack maps
//! the exact displayed strings to another language: `<settings>/Scripts/translations/<language>.json`,
//! a JSON object such as `{"OK": "确定", "Cancel": "取消"}`. The pack only changes what is drawn —
//! automation ids, control names, values and `EditText` contents stay as the script made them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use effectcraft_engine::config::ConfigStore;

/// Refuse packs above this size instead of reading an arbitrarily large settings entry.
const MAX_PACK_BYTES: usize = 1 << 20;

/// The active language's ScriptUI strings, loaded on first use and kept until the language changes.
#[derive(Default)]
pub(crate) struct Translations {
    cache: RefCell<Option<Cached>>,
}

struct Cached {
    language: &'static str,
    strings: Arc<BTreeMap<String, String>>,
}

impl Translations {
    /// The exact-match table for `language`, or an empty table when there is no pack (or it cannot
    /// be read or parsed).
    pub(crate) fn for_language(&self, language: &'static str, config: Option<&dyn ConfigStore>) -> Arc<BTreeMap<String, String>> {
        let mut cache = self.cache.borrow_mut();
        if let Some(cached) = cache.as_ref()
            && cached.language == language
        {
            return Arc::clone(&cached.strings);
        }
        let strings = load(language, config);
        *cache = Some(Cached { language, strings: Arc::clone(&strings) });
        strings
    }
}

fn load(language: &str, config: Option<&dyn ConfigStore>) -> Arc<BTreeMap<String, String>> {
    // English is the source language the scripts use; a pack for it would only be a no-op.
    if language == "en" {
        return Arc::new(BTreeMap::new());
    }
    let Some(config) = config else {
        return Arc::new(BTreeMap::new());
    };
    let name = format!("Scripts/translations/{language}.json");
    let text = match config.read_bounded(&name, MAX_PACK_BYTES) {
        Ok(Some(text)) => text,
        Ok(None) => return Arc::new(BTreeMap::new()),
        Err(error) => {
            log::warn!("could not read ScriptUI translation pack {name}: {error}");
            return Arc::new(BTreeMap::new());
        }
    };
    match serde_json::from_str::<BTreeMap<String, String>>(&text) {
        Ok(strings) => Arc::new(strings.into_iter().filter(|(source, translated)| !source.is_empty() && !translated.is_empty()).collect()),
        Err(error) => {
            log::warn!("ScriptUI translation pack {name} is not a JSON object of strings: {error}");
            Arc::new(BTreeMap::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::config::MemoryConfig;

    #[test]
    fn loads_the_pack_for_the_active_language_only() {
        let config = MemoryConfig::default();
        config.write("Scripts/translations/zh-hans.json", r#"{"OK":"确定","Cancel":"取消"}"#).unwrap();
        config.write("Scripts/translations/ja.json", r#"{"OK":"OK"}"#).unwrap();

        let translations = Translations::default();
        let zh = translations.for_language("zh-hans", Some(&config));
        assert_eq!(zh.get("OK").map(String::as_str), Some("确定"));
        let ja = translations.for_language("ja", Some(&config));
        assert_eq!(ja.get("OK").map(String::as_str), Some("OK"));
        assert_eq!(translations.for_language("en", Some(&config)).len(), 0);
    }

    #[test]
    fn missing_or_invalid_packs_are_empty() {
        let config = MemoryConfig::default();
        config.write("Scripts/translations/fr.json", "not json").unwrap();
        let translations = Translations::default();
        assert!(translations.for_language("de", Some(&config)).is_empty());
        assert!(translations.for_language("fr", Some(&config)).is_empty());
        assert!(translations.for_language("ja", None).is_empty());
    }
}
