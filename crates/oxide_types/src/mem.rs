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
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    pub fn from_arena(value: &T) -> Self
    where
        T: Clone,
    {
        Self(Arc::new(value.clone()))
    }

    pub fn as_ptr(&self) -> *const T {
        Arc::as_ptr(&self.0)
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
/// Phase 3 provides a minimal API: `promote(value)` moves `value` to the
/// global heap under `Arc` and returns `P<T>`. Phase 7 (OxideKernel) expands
/// with typed storage for shapes, code, IC templates, and strings.
pub struct PersistentHeap;

impl PersistentHeap {
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
