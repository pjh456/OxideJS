#![doc = "OxideJS - Compiler-independent compiled module cache"]

use std::sync::Arc;

use dashmap::DashMap;
use oxide_bytecode::CompiledModule;

/// Shared compiled-module cache keyed by a caller-provided safe module hash.
///
/// The cache intentionally does not know how to parse or compile JavaScript.
/// Compiler-aware callers compute the key and provide the compile callback.
pub struct CodeForge {
    map: DashMap<u64, Arc<CompiledModule>>,
}

impl CodeForge {
    pub fn new() -> Self {
        Self {
            map: DashMap::with_shard_amount(16),
        }
    }

    pub fn get(&self, hash: u64) -> Option<Arc<CompiledModule>> {
        self.map.get(&hash).map(|entry| Arc::clone(&entry))
    }

    pub fn insert(&self, hash: u64, module: CompiledModule) -> Arc<CompiledModule> {
        let module = Arc::new(module);
        Arc::clone(self.map.entry(hash).or_insert(module).value())
    }

    pub fn get_or_insert_with<F>(&self, hash: u64, compile: F) -> Result<Arc<CompiledModule>, String>
    where
        F: FnOnce() -> Result<CompiledModule, String>,
    {
        if let Some(module) = self.get(hash) {
            return Ok(module);
        }

        Ok(self.insert(hash, compile()?))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Default for CodeForge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_returns_same_arc_for_same_hash() {
        let forge = CodeForge::new();
        let first = forge.insert(1, CompiledModule::new());
        let second = forge.get_or_insert_with(1, || Ok(CompiledModule::new())).expect("cache hit");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn cache_misses_for_different_hashes() {
        let forge = CodeForge::new();
        let first = forge.insert(1, CompiledModule::new());
        let second = forge.insert(2, CompiledModule::new());

        assert!(!Arc::ptr_eq(&first, &second));
    }
}
