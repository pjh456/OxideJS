#![doc = "OxideJS - Compiler-independent compiled module cache"]

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use lru::LruCache;
use oxide_bytecode::CompiledModule;

/// Shared compiled-module cache keyed by a caller-provided safe module hash.
///
/// The cache intentionally does not know how to parse or compile JavaScript.
/// Compiler-aware callers compute the key and provide the compile callback.
///
/// Bounded by an LRU eviction policy: at most `capacity` modules are retained,
/// so an eval loop that compiles unbounded distinct sources cannot grow the
/// cache without limit.
pub struct CodeForge {
    map: Mutex<LruCache<u64, Arc<CompiledModule>>>,
}

impl CodeForge {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            map: Mutex::new(LruCache::new(capacity)),
        }
    }

    pub fn get(&self, hash: u64) -> Option<Arc<CompiledModule>> {
        self.map.lock().unwrap().get(&hash).map(Arc::clone)
    }

    pub fn insert(&self, hash: u64, module: CompiledModule) -> Arc<CompiledModule> {
        let module = Arc::new(module);
        self.map.lock().unwrap().put(hash, Arc::clone(&module));
        module
    }

    pub fn get_or_insert_with<F>(&self, hash: u64, compile: F) -> Result<Arc<CompiledModule>, String>
    where
        F: FnOnce() -> Result<CompiledModule, String>,
    {
        // Two-phase: look up under the lock, then release the lock BEFORE
        // running the compile closure so a slow compilation never blocks other
        // cache users. Re-acquire the lock only to insert the freshly built
        // module.
        {
            let mut cache = self.map.lock().unwrap();
            if let Some(module) = cache.get(&hash) {
                return Ok(Arc::clone(module));
            }
        }
        let module = Arc::new(compile()?);
        self.map.lock().unwrap().put(hash, Arc::clone(&module));
        Ok(module)
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forge(capacity: usize) -> CodeForge {
        CodeForge::new(NonZeroUsize::new(capacity).unwrap())
    }

    #[test]
    fn cache_returns_same_arc_for_same_hash() {
        let forge = forge(16);
        let first = forge.insert(1, CompiledModule::new());
        let second = forge.get_or_insert_with(1, || Ok(CompiledModule::new())).expect("cache hit");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn cache_misses_for_different_hashes() {
        let forge = forge(16);
        let first = forge.insert(1, CompiledModule::new());
        let second = forge.insert(2, CompiledModule::new());

        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn lru_eviction_enforces_cap() {
        let forge = forge(2);
        forge.insert(1, CompiledModule::new());
        forge.insert(2, CompiledModule::new());
        forge.insert(3, CompiledModule::new());

        assert_eq!(forge.len(), 2);
        assert!(forge.get(1).is_none());
        assert!(forge.get(2).is_some());
        assert!(forge.get(3).is_some());
    }

    #[test]
    fn lru_eviction_evicts_least_recently_used() {
        let forge = forge(3);
        forge.insert(1, CompiledModule::new());
        forge.insert(2, CompiledModule::new());
        forge.insert(3, CompiledModule::new());

        // Touch key 1 so key 2 becomes the least-recently-used entry.
        assert!(forge.get(1).is_some());
        forge.insert(4, CompiledModule::new());

        assert!(forge.get(1).is_some());
        assert!(forge.get(2).is_none());
        assert!(forge.get(3).is_some());
        assert!(forge.get(4).is_some());
    }

    #[test]
    fn len_never_exceeds_cap_under_many_distinct_inserts() {
        let forge = forge(10);
        for hash in 0..100 {
            forge.insert(hash, CompiledModule::new());
        }
        assert_eq!(forge.len(), 10);
    }
}
