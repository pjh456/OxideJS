//! 内存管理抽象：跨 epoch 持久存储与每次调用（agent call）内的 arena 分配。
//!
//! `P<T>` 是基于 `Arc` 的持久指针，持有者跨 `Epoch::reset()` 存活；
//! `PersistentHeap` 负责把对象"提升"到持久堆；`Epoch` 则是对
//! `bumpalo::Bump` 的封装，用于每次调用内的高频分配与 O(1) 整体回收，
//! 并通过 epoch ID 辅助悬挂指针检测。

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// Reference-counted persistent pointer.
///
/// Wraps `Arc<T>` for cross-epoch object storage. `Clone` increments the
/// reference count; `Drop` decrements it. Objects wrapped in `P<T>` survive
/// `Epoch::reset()` — they live on the global heap.
#[repr(transparent)]
pub struct P<T>(Arc<T>);

impl<T> P<T> {
    /// 在堆上分配并包装 `value`。
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// 返回内部 `Arc<T>` 的底层裸指针。
    pub fn as_ptr(&self) -> *const T {
        Arc::as_ptr(&self.0)
    }

    /// 返回可变裸指针（用于外部 GC / 遍历改写）。
    #[inline(always)]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.as_ptr() as *mut T
    }
}

impl<T> Clone for P<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for P<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for P<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P({:?})", self.0)
    }
}

impl<T: fmt::Display> fmt::Display for P<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Persistent heap for cross-epoch object storage.
///
/// Provides a minimal API: `promote(value)` moves `value` to the
/// global heap under `Arc` and returns `P<T>`. Typed storage for shapes,
/// code, IC templates, and strings lives in the OxideKernel.
pub struct PersistentHeap;

impl PersistentHeap {
    /// 创建空持久堆（无内部状态，仅提供 `promote`）。
    pub fn new() -> Self {
        Self
    }

    /// Move a value to the persistent heap and return a reference-counted pointer.
    /// The returned `P<T>` survives epoch resets — it's stored on the global heap
    /// under `Arc`.
    pub fn promote<T>(&self, value: T) -> P<T> {
        P::new(value)
    }
}

impl Default for PersistentHeap {
    fn default() -> Self {
        Self::new()
    }
}
///
/// Wraps `bumpalo::Bump` with an epoch ID counter for dangling pointer detection.
/// All Agent-call objects are allocated here; `reset()` clears them in O(1)
/// at the end of each call.
pub struct Epoch {
    bump: bumpalo::Bump,
    epoch_id: u64,
}

impl Epoch {
    /// 创建从 epoch 0 开始、空 arena 的调用级分配器。
    pub fn new() -> Self {
        Self {
            bump: bumpalo::Bump::new(),
            epoch_id: 0,
        }
    }

    /// Bump-allocate a value and return a raw pointer to it.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid until `reset()` is called on this epoch.
    /// Callers must not store it beyond that boundary, and must ensure no
    /// aliases are used after the arena is reset.
    pub fn alloc<T>(&self, value: T) -> *mut T {
        self.bump.alloc(value)
    }

    /// 直接访问底层 `bumpalo::Bump`。
    ///
    /// 供需要依赖 bumpalo API（如 `alloc_slice`）的调用方使用。
    pub fn bump(&self) -> &bumpalo::Bump {
        &self.bump
    }

    /// Bump-allocate a value using a closure for initialization.
    /// Better for compiler optimization — construct directly on arena.
    pub fn alloc_with<T, F>(&self, f: F) -> *mut T
    where
        F: FnOnce() -> T,
    {
        self.bump.alloc_with(f)
    }

    /// Return true when `ptr` lies inside one of the currently allocated bump chunks.
    ///
    /// This only compares addresses and does not dereference `ptr`.
    #[inline]
    pub fn is_epoch_ptr(&self, ptr: *const u8) -> bool {
        let addr = ptr as usize;
        if addr == 0 {
            return false;
        }
        // SAFETY: this helper performs no allocations while walking the raw chunk
        // iterator, so bumpalo's chunk list cannot change during iteration.
        unsafe {
            self.bump.iter_allocated_chunks_raw().any(|(base, len)| {
                let start = base as usize;
                let end = start.saturating_add(len);
                addr >= start && addr < end
            })
        }
    }

    /// O(1) mass deallocation. All previous allocations become invalid.
    /// Increments epoch ID to invalidate stale pointers (debug_assert guard).
    pub fn reset(&mut self) {
        self.bump.reset();
        self.epoch_id += 1;
    }

    /// Current epoch ID. Arena-allocated objects store this value;
    /// dereference checks they match the current ID (debug_assert only).
    pub fn current_id(&self) -> u64 {
        self.epoch_id
    }
}

impl Default for Epoch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::JsObject;
    use crate::shape::EMPTY_SHAPE_ID;
    use crate::value::JsValue;

    #[test]
    fn is_epoch_ptr_returns_true_for_epoch_object() {
        let epoch = Epoch::new();
        let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        let ptr = epoch.alloc(obj);

        assert!(epoch.is_epoch_ptr(ptr.cast::<u8>()));
    }

    #[test]
    fn is_epoch_ptr_returns_false_for_heap_and_stack_pointers() {
        let epoch = Epoch::new();
        let heap = PersistentHeap::new();
        let persistent = heap.promote(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let stack_value = 7i32;

        assert!(!epoch.is_epoch_ptr(persistent.as_ptr().cast::<u8>()));
        assert!(!epoch.is_epoch_ptr((&stack_value as *const i32).cast::<u8>()));
        assert!(!epoch.is_epoch_ptr(std::ptr::null()));
    }
}
