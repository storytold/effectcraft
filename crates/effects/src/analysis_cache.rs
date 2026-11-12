//! Bounded parsed-JSON caches. Keep the content key with the owned source text so repeated
//! renders do not rehash long analyses. Compare contents on lookup: evaluated parameter
//! strings can be cloned, edited in place, or allocated again at an old address.

use std::sync::{Arc, Mutex, OnceLock};

struct Entry<T> {
    json: Box<str>,
    key: u64,
    value: Arc<T>,
}

pub(crate) struct AnalysisCache<T> {
    entries: OnceLock<Mutex<Vec<Entry<T>>>>,
}

pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl<T> AnalysisCache<T> {
    pub(crate) const fn new() -> Self {
        Self { entries: OnceLock::new() }
    }

    pub(crate) fn get(&self, json: &str, parse: impl FnOnce(&str) -> Option<T>) -> Option<(u64, Arc<T>)> {
        let entries = self.entries.get_or_init(Default::default);
        {
            let m = entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = m.iter().rev().find(|e| e.json.as_ref() == json) {
                return Some((e.key, e.value.clone()));
            }
        }
        let value = Arc::new(parse(json)?);
        let key = fnv(json.as_bytes());
        let mut m = entries.lock().unwrap_or_else(|e| e.into_inner());
        // Another render may have filled the entry while this one parsed it.
        if let Some(e) = m.iter().rev().find(|e| e.json.as_ref() == json) {
            return Some((e.key, e.value.clone()));
        }
        // Preserve the existing nine-entry bound and clear-on-overflow policy.
        if m.len() > 8 {
            m.clear();
        }
        m.push(Entry { json: json.into(), key, value: value.clone() });
        Some((key, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn reuses_content_and_invalidates_same_allocation_edits() {
        let cache = AnalysisCache::new();
        let calls = Cell::new(0);
        let parse = |s: &str| {
            calls.set(calls.get() + 1);
            Some(s.to_owned())
        };
        let mut json = String::from("analysis-1");
        let (key, a) = cache.get(&json, parse).unwrap();
        let copy = json.clone();
        assert_ne!(json.as_ptr(), copy.as_ptr());
        let (same_key, same) = cache.get(&copy, parse).unwrap();
        assert_eq!(key, fnv(json.as_bytes()));
        assert_eq!(key, same_key);
        assert!(Arc::ptr_eq(&a, &same));
        assert_eq!(calls.get(), 1);
        let ptr = json.as_ptr();
        json.replace_range(9..10, "2");
        assert_eq!(ptr, json.as_ptr());
        let (changed_key, changed) = cache.get(&json, parse).unwrap();
        assert_ne!(key, changed_key);
        assert_eq!(&*changed, "analysis-2");
        assert_eq!(&*a, "analysis-1");
        assert_eq!(calls.get(), 2);
        assert!(cache.get("invalid", |_| None).is_none());
        assert!(Arc::ptr_eq(&cache.get(&json, parse).unwrap().1, &changed));
    }

    #[test]
    fn source_text_retention_is_bounded() {
        let cache = AnalysisCache::new();
        for i in 0..30 {
            cache.get(&i.to_string(), |s| Some(s.to_owned())).unwrap();
            assert!(cache.entries.get().unwrap().lock().unwrap().len() <= 9);
        }
    }
}
