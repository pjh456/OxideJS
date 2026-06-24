use std::hash::{Hash, Hasher};
use std::sync::RwLock;

use dashmap::DashMap;
use oxide_types::object::JsString;
use rustc_hash::FxHasher;

use crate::{kernel_debug, kernel_trace};

/// Full 64-bit content hash. Replaces the old 16-bit `hash16` so the interner's
/// hash→candidate map has negligible collision risk.
fn hash64(s: &str) -> u64 {
    let mut h = FxHasher::default();
    s.hash(&mut h);
    h.finish()
}

/// One interned key. `data` is a leaked `&'static str` — permanent keys are never
/// freed (append-only by design), so the leak is the storage model, not a bug.
#[derive(Clone, Copy)]
struct PermEntry {
    data: &'static str,
    hash: u64,
}

/// Append-only, never-move, lock-free-read key interner shared by all VMs.
///
/// Replaces the old ref-counted `StringForge` (and its buggy `maybe_sweep`
/// renumber path). Keys (property names, method names) are interned once and
/// addressed by a stable `u32` id that the shape/IC system keys on. Runtime
/// string *values* are no longer interned here — they are heap `JsString`
/// pointers (see `oxide_vm::Vm::new_string`).
///
/// Concurrency:
/// - `hash_map` (`DashMap`) gives sharded, lock-free reads of the hash→candidates
///   mapping on the hot intern path.
/// - `entries` sits behind a short `RwLock`; reads copy out a `&'static str`
///   (Copy) so the borrow outlives the guard.
pub struct PermInterner {
    hash_map: DashMap<u64, Vec<u32>>,
    entries: RwLock<Vec<PermEntry>>,
    /// Lazily materialized permanent `JsString` values, indexed by entry id.
    /// Used when a permanent key must also exist as a JS string *value*
    /// (e.g. method names exposed as property values at builtin init).
    permanent_strings: RwLock<Vec<Option<Box<JsString>>>>,
}

impl PermInterner {
    pub fn new() -> Self {
        Self {
            hash_map: DashMap::new(),
            entries: RwLock::new(Vec::new()),
            permanent_strings: RwLock::new(Vec::new()),
        }
    }

    /// Intern a key. Returns its stable id and full 64-bit hash. Each unique
    /// string is stored exactly once (single allocation, no double `to_string`).
    pub fn intern(&self, s: &str) -> (u32, u64) {
        let hash = hash64(s);

        // Fast path: lock-free candidate read, short entries read-lock.
        if let Some(candidates) = self.hash_map.get(&hash) {
            let entries = self.entries.read().unwrap();
            for &id in candidates.iter() {
                if entries[id as usize].data == s {
                    kernel_trace!("PermInterner intern hit id={}", id);
                    return (id, hash);
                }
            }
        }

        // Slow path: append under write lock, re-checking for a racing insert.
        let mut entries = self.entries.write().unwrap();
        if let Some(candidates) = self.hash_map.get(&hash) {
            for &id in candidates.iter() {
                if entries[id as usize].data == s {
                    return (id, hash);
                }
            }
        }
        let id = entries.len() as u32;
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        entries.push(PermEntry { data: leaked, hash });
        drop(entries);
        self.hash_map.entry(hash).or_default().push(id);
        kernel_debug!("PermInterner intern new id={} len={}", id, s.len());
        (id, hash)
    }

    /// Resolve a key id to its text with zero clone. The returned `&'static str`
    /// is valid for the program lifetime (keys are never freed).
    pub fn lookup(&self, id: u32) -> Option<&'static str> {
        let entries = self.entries.read().unwrap();
        entries.get(id as usize).map(|e| e.data)
    }

    /// Full 64-bit hash for a key id.
    pub fn get_hash(&self, id: u32) -> Option<u64> {
        let entries = self.entries.read().unwrap();
        entries.get(id as usize).map(|e| e.hash)
    }

    /// Total number of unique keys interned.
    pub fn entry_count(&self) -> u32 {
        self.entries.read().unwrap().len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.entry_count() == 0
    }

    /// Materialize (once) and return a stable pointer to a permanent `JsString`
    /// for the given key id. The `JsString` lives for the program lifetime.
    pub fn string_ptr(&self, id: u32) -> *const JsString {
        {
            let perm = self.permanent_strings.read().unwrap();
            if let Some(Some(boxed)) = perm.get(id as usize) {
                return boxed.as_ref() as *const JsString;
            }
        }
        let text = self.lookup(id).unwrap_or("");
        let mut perm = self.permanent_strings.write().unwrap();
        if perm.len() <= id as usize {
            perm.resize_with(id as usize + 1, || None);
        }
        if perm[id as usize].is_none() {
            perm[id as usize] = Some(Box::new(JsString::new(text.to_string())));
        }
        perm[id as usize].as_ref().unwrap().as_ref() as *const JsString
    }
}

impl Default for PermInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_dedup() {
        let interner = PermInterner::new();
        let (i1, h1) = interner.intern("abc");
        let (i2, h2) = interner.intern("abc");
        assert_eq!(i1, i2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn intern_different() {
        let interner = PermInterner::new();
        let (i1, _) = interner.intern("x");
        let (i2, _) = interner.intern("y");
        assert_ne!(i1, i2);
    }

    #[test]
    fn lookup_zero_clone() {
        let interner = PermInterner::new();
        let (id, _) = interner.intern("hello");
        assert_eq!(interner.lookup(id), Some("hello"));
    }

    #[test]
    fn lookup_not_found() {
        let interner = PermInterner::new();
        assert_eq!(interner.lookup(99999), None);
    }

    #[test]
    fn entry_count_monotonic() {
        let interner = PermInterner::new();
        assert_eq!(interner.entry_count(), 0);
        interner.intern("a");
        interner.intern("b");
        interner.intern("a");
        assert_eq!(interner.entry_count(), 2);
    }

    #[test]
    fn many_unique_no_collision() {
        let interner = PermInterner::new();
        for i in 0..10_000 {
            let s = format!("key{i}");
            let (id, _) = interner.intern(&s);
            assert_eq!(interner.lookup(id), Some(&*Box::leak(s.into_boxed_str())));
        }
        assert_eq!(interner.entry_count(), 10_000);
    }

    #[test]
    fn string_ptr_roundtrip() {
        let interner = PermInterner::new();
        let (id, _) = interner.intern("perm");
        let ptr = interner.string_ptr(id);
        assert_eq!(unsafe { (*ptr).as_str() }, "perm");
        // Second call returns the same stable pointer (materialized once).
        assert_eq!(interner.string_ptr(id), ptr);
    }
}
