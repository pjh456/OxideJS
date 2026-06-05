use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;

use hashbrown::hash_map::DefaultHashBuilder as FxBuildHasher;
use hashbrown::HashMap;

pub fn hash16(s: &str) -> u16 {
    let mut h = rustc_hash::FxHasher::default();
    s.hash(&mut h);
    (h.finish() >> 48) as u16
}

pub struct StringEntry {
    pub data: String,
    pub ref_count: AtomicU32,
    pub hash: u16,
}

struct StringForgeInner {
    map: HashMap<String, u32, FxBuildHasher>,
    entries: Vec<StringEntry>,
}

pub struct StringForge {
    inner: RwLock<StringForgeInner>,
}

impl StringForge {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(StringForgeInner {
                map: HashMap::with_hasher(FxBuildHasher::default()),
                entries: Vec::new(),
            }),
        }
    }

    pub fn intern(&self, s: &str) -> (u32, u16) {
        {
            let inner = self.inner.read().unwrap();
            if let Some(&idx) = inner.map.get(s) {
                let entry = &inner.entries[idx as usize];
                entry.ref_count.fetch_add(1, Ordering::Release);
                return (idx, entry.hash);
            }
        }

        let mut inner = self.inner.write().unwrap();
        if let Some(&idx) = inner.map.get(s) {
            let entry = &inner.entries[idx as usize];
            entry.ref_count.fetch_add(1, Ordering::Release);
            return (idx, entry.hash);
        }

        let h = hash16(s);
        let idx = inner.entries.len() as u32;
        inner.map.insert(s.to_string(), idx);
        inner.entries.push(StringEntry {
            data: s.to_string(),
            ref_count: AtomicU32::new(1),
            hash: h,
        });
        (idx, h)
    }

    pub fn lookup(&self, idx: u32) -> Option<String> {
        let inner = self.inner.read().unwrap();
        inner.entries.get(idx as usize).map(|e| e.data.clone())
    }

    pub fn decref(&self, idx: u32) {
        let inner = self.inner.read().unwrap();
        if let Some(entry) = inner.entries.get(idx as usize) {
            entry.ref_count.fetch_sub(1, Ordering::Release);
        }
    }

    pub fn maybe_sweep(&self, max_dead: Option<usize>) {
        let threshold = match max_dead {
            Some(t) => t,
            None => return,
        };

        let mut inner = self.inner.write().unwrap();
        let dead_count = inner
            .entries
            .iter()
            .filter(|e| e.ref_count.load(Ordering::Acquire) == 0)
            .count();

        if dead_count < threshold {
            return;
        }

        let mut new_map: HashMap<String, u32, FxBuildHasher> =
            HashMap::with_hasher(FxBuildHasher::default());
        let mut new_entries = Vec::with_capacity(inner.entries.len() - dead_count);

        for entry in inner.entries.drain(..) {
            let rc = entry.ref_count.load(Ordering::Acquire);
            if rc > 0 {
                let new_idx = new_entries.len() as u32;
                new_map.insert(entry.data.clone(), new_idx);
                new_entries.push(entry);
            }
        }

        inner.map = new_map;
        inner.entries = new_entries;
    }
}

impl Default for StringForge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash16_same() {
        assert_eq!(hash16("hello"), hash16("hello"));
    }

    #[test]
    fn test_hash16_different() {
        assert_ne!(hash16("hello"), hash16("world"));
    }

    #[test]
    fn test_intern_dedup() {
        let string_forge = StringForge::new();
        let (i1, h1) = string_forge.intern("abc");
        let (i2, h2) = string_forge.intern("abc");
        assert_eq!(i1, i2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_intern_different() {
        let string_forge = StringForge::new();
        let (i1, _) = string_forge.intern("x");
        let (i2, _) = string_forge.intern("y");
        assert_ne!(i1, i2);
    }

    #[test]
    fn test_lookup_found() {
        let string_forge = StringForge::new();
        let (idx, _) = string_forge.intern("hello");
        assert_eq!(string_forge.lookup(idx), Some("hello".to_string()));
    }

    #[test]
    fn test_lookup_not_found() {
        let string_forge = StringForge::new();
        assert_eq!(string_forge.lookup(99999), None);
    }

    #[test]
    fn test_ref_count_and_decref() {
        let string_forge = StringForge::new();
        let (idx, _) = string_forge.intern("s");
        assert_eq!(string_forge.lookup(idx), Some("s".to_string()));
        string_forge.decref(idx);
        assert_eq!(string_forge.lookup(idx), Some("s".to_string()));
    }

    #[test]
    fn test_maybe_sweep_noop_when_none() {
        let string_forge = StringForge::new();
        string_forge.intern("live");
        string_forge.maybe_sweep(None);
        assert!(string_forge.lookup(0).is_some());
    }

    #[test]
    fn test_maybe_sweep_skip_below_threshold() {
        let string_forge = StringForge::new();
        string_forge.intern("live");
        string_forge.maybe_sweep(Some(100));
        assert_eq!(string_forge.lookup(0), Some("live".to_string()));
    }
}
