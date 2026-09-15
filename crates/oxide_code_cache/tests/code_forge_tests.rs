//! CodeForge LRU 缓存单测：命中共享、LRU 逐出与容量上限。
//!
//! 只走 pub API（new/insert/get/get_or_insert_with/len）。

use std::num::NonZeroUsize;
use std::sync::Arc;

use oxide_bytecode::CompiledModule;
use oxide_code_cache::CodeForge;

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

    // 触碰键 1，使键 2 成为最久未使用条目。
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
    m1.bytecode = Arc::from(vec![1, 2, 3]);
    forge.insert(42, m1);
    let _ = forge
        .get_or_insert_with(42, || {
            let mut m = CompiledModule::new();
            m.bytecode = Arc::from(vec![4, 5, 6]);
            Ok(m)
        })
        .unwrap();
}

#[test]
fn cache_hit_returns_cached_not_recompiled() {
    let forge = forge(16);
    let mut m = CompiledModule::new();
    m.bytecode = Arc::from(vec![1, 2, 3]);
    let cached = forge.insert(99, m);
    let result = forge
        .get_or_insert_with(99, || {
            let mut m = CompiledModule::new();
            m.bytecode = Arc::from(vec![1, 2, 3]);
            Ok(m)
        })
        .unwrap();
    assert!(Arc::ptr_eq(&cached, &result));
}
