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

/// 以调用方提供的安全模块哈希为键的共享编译模块缓存。
///
/// 缓存自身不负责解析或编译 JavaScript：由编译器感知的调用方计算键并提供
/// 编译回调。
///
/// 以 LRU 淘汰策略约束内存：至多保留 `capacity` 个模块，因此对无界不同源码
/// 反复编译的 eval 循环不会让缓存无限增长。
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
