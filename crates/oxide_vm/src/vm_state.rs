//! Grouped `Vm` state sub-structs.
//!
//! `Vm` owns these as fields (`gc_state`, `symbols`, `iters`, `profiling`) so
//! that workers refactoring one subsystem touch one struct instead of the
//! central `Vm` god-struct. Per the project's parallel-dev decisions:
//! Symbol/Iter/Profiling are full sinks (their logic is self-contained), while
//! `GcState` is field-classification only — GC `mark`/`sweep`/`alloc_object`
//! scan roots across *all* sub-structs and stay as `&mut Vm` methods.
//!
//! This file defines fields only; methods live with `Vm`.

use std::cell::Cell;
use std::collections::HashMap;

use rustc_hash::FxBuildHasher;

use oxide_types::object::{JsObject, JsString};
use oxide_types::value::JsValue;

use crate::session_gc::SessionGc;
use crate::vm::ForInIter;

/// Session-arena and garbage-collection bookkeeping.
pub(crate) struct GcState {
    pub(crate) session_epoch: bumpalo::Bump,
    pub(crate) session_gc: SessionGc,
    pub(crate) epoch_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_string_ptrs: Vec<*mut JsString>,
    pub(crate) session_bytes_allocated: usize,
    /// Long-lived forwarding map reused by GC sweep and promote.
    /// `mem::take`'d out for use, `clear()`'d and put back — capacity retained
    /// across collections, so no per-event `HashMap` allocation. `FxBuildHasher`
    /// because the keys are raw pointers (collision resistance is irrelevant).
    pub(crate) forwarding: HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
}

/// `Symbol` interning state.
pub(crate) struct SymbolState {
    pub(crate) symbol_counter: u32,
    pub(crate) symbol_descriptions: Vec<String>,
    pub(crate) symbol_registry: HashMap<String, u32>,
}

/// Live iterator state for `for-in` / `for-of`.
pub(crate) struct IterState {
    pub(crate) for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) for_of_iters: Vec<JsValue>,
    pub(crate) last_for_of_result: JsValue,
}

/// Inline-cache hit/miss and instruction counters.
pub(crate) struct ProfilingState {
    pub(crate) ic_hits: Cell<u64>,
    pub(crate) ic_misses: Cell<u64>,
    pub(crate) instruction_count: u64,
}
