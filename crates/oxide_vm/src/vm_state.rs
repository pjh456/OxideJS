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
///
/// Field-classify only. `mark`/`sweep`/`rewrite_vm_roots` live on `Vm` because
/// GC scans all sub-struct roots (regs, frames, for_in_iters, for_of_iters,
/// exception_value, etc.) — they cannot be confined to GcState.
pub(crate) struct GcState {
    pub(crate) session_epoch: bumpalo::Bump,
    pub(crate) session_gc: SessionGc,
    pub(crate) epoch_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_string_ptrs: Vec<*mut JsString>,
    pub(crate) session_bytes_allocated: usize,
    pub(crate) forwarding: HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
}

impl GcState {
    pub(crate) fn track_epoch_object(&mut self, ptr: *mut JsObject) {
        self.epoch_object_ptrs.push(ptr);
    }
}

/// `Symbol` interning state.
pub(crate) struct SymbolState {
    pub(crate) symbol_counter: u32,
    pub(crate) symbol_descriptions: Vec<String>,
    pub(crate) symbol_registry: HashMap<String, u32>,
}

impl SymbolState {
    pub(crate) fn reset(&mut self) {
        self.symbol_counter = 0;
        self.symbol_descriptions.clear();
        self.symbol_registry.clear();
    }

    pub(crate) fn intern(&mut self, description: String) -> u32 {
        self.symbol_counter = self.symbol_counter.wrapping_add(1);
        let idx = self.symbol_descriptions.len() as u32;
        self.symbol_descriptions.push(description);
        idx
    }

    pub(crate) fn register_global(&mut self, key: String, id: u32) {
        self.symbol_registry.insert(key, id);
    }

    pub(crate) fn lookup_global(&self, key: &str) -> Option<u32> {
        self.symbol_registry.get(key).copied()
    }

    pub(crate) fn description(&self, id: u32) -> Option<&str> {
        self.symbol_descriptions.get(id as usize).map(|s| s.as_str())
    }

    pub(crate) fn key_for_id(&self, id: u32) -> Option<String> {
        self.symbol_registry.iter().find(|(_, &v)| v == id).map(|(k, _)| k.clone())
    }

    pub(crate) fn registry_len(&self) -> usize {
        self.symbol_registry.len()
    }
}

/// Live iterator state for `for-in` / `for-of`.
pub(crate) struct IterState {
    pub(crate) for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) for_of_iters: Vec<JsValue>,
    pub(crate) last_for_of_result: JsValue,
}

impl IterState {
    pub(crate) fn reset(&mut self) {
        self.for_in_iters.clear();
        self.for_of_iters.clear();
        self.last_for_of_result = JsValue::undefined();
    }

    pub(crate) fn push_for_in(&mut self, iter: *mut ForInIter<'static>) {
        self.for_in_iters.push(iter);
    }

    pub(crate) fn pop_for_in(&mut self) {
        self.for_in_iters.pop();
    }

    pub(crate) fn last_for_in(&self) -> *mut ForInIter<'static> {
        self.for_in_iters.last().copied().unwrap_or(std::ptr::null_mut())
    }

    pub(crate) fn push_for_of(&mut self, val: JsValue) {
        self.for_of_iters.push(val);
    }

    pub(crate) fn last_for_of(&self) -> Option<JsValue> {
        self.for_of_iters.last().copied()
    }

    pub(crate) fn pop_for_of(&mut self) -> Option<JsValue> {
        self.for_of_iters.pop()
    }

    pub(crate) fn last_for_of_result(&self) -> JsValue {
        self.last_for_of_result
    }

    pub(crate) fn set_last_for_of_result(&mut self, val: JsValue) {
        self.last_for_of_result = val;
    }

    pub(crate) fn clear_last_for_of_result(&mut self) {
        self.last_for_of_result = JsValue::undefined();
    }
}

/// Inline-cache hit/miss and instruction counters.
pub(crate) struct ProfilingState {
    pub(crate) ic_hits: Cell<u64>,
    pub(crate) ic_misses: Cell<u64>,
    pub(crate) instruction_count: u64,
}

impl ProfilingState {
    pub(crate) fn record_ic_hit(&self) {
        self.ic_hits.set(self.ic_hits.get() + 1);
    }

    pub(crate) fn record_ic_miss(&self) {
        self.ic_misses.set(self.ic_misses.get() + 1);
    }

    pub(crate) fn set_instruction_count(&mut self, count: u64) {
        self.instruction_count = count;
    }

    pub(crate) fn ic_hit_rate(&self) -> f64 {
        let total = self.ic_hits.get() + self.ic_misses.get();
        if total == 0 {
            0.0
        } else {
            self.ic_hits.get() as f64 / total as f64
        }
    }
}
