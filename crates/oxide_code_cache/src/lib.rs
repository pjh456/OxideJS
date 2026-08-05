//! oxide_code_cache：编译结果缓存（LRU，与编译器解耦）。
//!
//! `CodeForge` 以调用方提供的模块哈希为键缓存 `CompiledModule`（`Arc` 共享）。
//! 缓存自身不解析/编译 JS；键的计算与编译回调均由编译器侧提供。
//! LRU 上限约束内存，供 eval 循环/REPL 等重复编译场景复用产物。

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use lru::LruCache;
use oxide_bytecode::CompiledModule;

mod code_cache_log;

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
    /// 构造容量为 `capacity` 的缓存（容量为 0 会 panic）。
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            map: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// 按哈希取缓存模块；未命中返回 None（命中会提升 LRU 位置）。
    pub fn get(&self, hash: u64) -> Option<Arc<CompiledModule>> {
        let result = self.map.lock().unwrap().get(&hash).map(Arc::clone);
        if result.is_some() {
            code_cache_debug!("CodeForge hit: hash={:#x}", hash);
        }
        result
    }

    /// 插入模块并返回共享 `Arc`；超出容量时逐出最久未用项。
    pub fn insert(&self, hash: u64, module: CompiledModule) -> Arc<CompiledModule> {
        let module = Arc::new(module);
        let mut cache = self.map.lock().unwrap();
        let evicted_hash = if cache.len() == cache.cap().get() && !cache.contains(&hash) {
            cache.peek_lru().map(|(k, _)| *k)
        } else {
            None
        };
        cache.put(hash, Arc::clone(&module));
        drop(cache);
        match evicted_hash {
            Some(evicted_hash) => code_cache_debug!("CodeForge evict: hash={:#x}", evicted_hash),
            None => code_cache_debug!("CodeForge insert: hash={:#x}", hash),
        }
        module
    }

    /// 命中返回缓存模块，未命中则调用 `compile` 编译后缓存并返回。
    /// debug 构建下命中时会重编译校验 bytecode，检测结构哈希碰撞。
    pub fn get_or_insert_with<F>(&self, hash: u64, compile: F) -> Result<Arc<CompiledModule>, String>
    where
        F: Fn() -> Result<CompiledModule, String>,
    {
        {
            let mut cache = self.map.lock().unwrap();
            if let Some(module) = cache.get(&hash) {
                let module = Arc::clone(module);
                drop(cache);
                code_cache_debug!("CodeForge hit: hash={:#x}", hash);
                #[cfg(debug_assertions)]
                {
                    let fresh = compile()?;
                    debug_assert_eq!(
                        module.bytecode, fresh.bytecode,
                        "structural hash collision: cached bytecode differs from recompiled for hash {hash}",
                    );
                }
                return Ok(module);
            }
        }
        let module = Arc::new(compile()?);
        self.map.lock().unwrap().put(hash, Arc::clone(&module));
        code_cache_debug!("CodeForge miss: hash={:#x}", hash);
        Ok(module)
    }

    /// 当前缓存条目数。
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    /// 缓存是否为空。
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

    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "structural hash collision"))]
    fn cache_hit_debug_verifies_bytecode() {
        let forge = forge(16);
        let mut m1 = CompiledModule::new();
        m1.bytecode = vec![1, 2, 3];
        forge.insert(42, m1);
        let _ = forge
            .get_or_insert_with(42, || {
                let mut m = CompiledModule::new();
                m.bytecode = vec![4, 5, 6];
                Ok(m)
            })
            .unwrap();
    }

    #[test]
    fn cache_hit_returns_cached_not_recompiled() {
        let forge = forge(16);
        let mut m = CompiledModule::new();
        m.bytecode = vec![1, 2, 3];
        let cached = forge.insert(99, m);
        let result = forge
            .get_or_insert_with(99, || {
                let mut m = CompiledModule::new();
                m.bytecode = vec![1, 2, 3];
                Ok(m)
            })
            .unwrap();
        assert!(Arc::ptr_eq(&cached, &result));
    }
}
