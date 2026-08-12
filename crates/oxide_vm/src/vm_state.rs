//! `Vm` 的状态子结构分组。
//!
//! `Vm` 以字段持有这些子结构（`gc_state`、`symbols`、`iters`、`profiling`），
//! 使各子系统维护者只需改动一个结构而非庞大的 `Vm` 主结构。约定：
//! Symbol/Iter/Profiling 为完整子模块（逻辑自包含）；`GcState` 仅做字段分类——
//! GC 的 mark/sweep/alloc_object 需要扫描跨全部子结构的根（regs、frames、
//! 迭代器等），仍以 `&mut Vm` 方法驻留。
//!
//! 本文件只定义字段；方法随 `Vm` 存放。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use rustc_hash::FxBuildHasher;

use crate::session_gc::SessionGc;
use crate::vm::ForInIter;
use oxide_types::object::{JsObject, JsString};
use oxide_types::value::JsValue;

/// session arena 与 GC 簿记。
///
/// 仅做字段分类。mark/sweep/rewrite_vm_roots 驻留在 `Vm` 上：GC 需扫描所有
/// 子结构的根（regs、frames、for_in_iters、for_of_iters、exception_value 等），
/// 无法限制在 GcState 内。
pub(crate) struct GcState {
    pub(crate) session_epoch: bumpalo::Bump,
    pub(crate) session_gc: SessionGc,
    pub(crate) epoch_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_string_ptrs: Vec<*mut JsString>,
    /// BigInt 堆 box（`Box<i128>`）追踪表。`RefCell` 使 `&self` 的
    /// `convert_immutables` 也能登记新 box。与字符串不同，BigInt 不参与
    /// mark/sweep 回收（值量少），只在 full_reset 统一释放。
    pub(crate) session_bigint_ptrs: RefCell<Vec<*mut num_bigint::BigInt>>,
    pub(crate) session_bytes_allocated: usize,
    pub(crate) forwarding: HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
}

impl GcState {
    pub(crate) fn track_epoch_object(&mut self, ptr: *mut JsObject) {
        self.epoch_object_ptrs.push(ptr);
    }
}

/// Symbol 的 intern 状态。
pub(crate) struct SymbolState {
    pub(crate) symbol_counter: u32,
    pub(crate) symbol_descriptions: Vec<Option<String>>,
    pub(crate) symbol_registry: HashMap<String, u32>,
}

impl SymbolState {
    pub(crate) fn reset(&mut self) {
        self.symbol_counter = 0;
        self.symbol_descriptions.clear();
        self.symbol_registry.clear();
    }

    pub(crate) fn intern(&mut self, description: Option<String>) -> u32 {
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
        self.symbol_descriptions.get(id as usize).and_then(|s| s.as_deref())
    }

    pub(crate) fn key_for_id(&self, id: u32) -> Option<String> {
        self.symbol_registry.iter().find(|(_, &v)| v == id).map(|(k, _)| k.clone())
    }

    pub(crate) fn registry_len(&self) -> usize {
        self.symbol_registry.len()
    }
}

/// for-in / for-of 的活跃迭代器状态。
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

/// inline cache 命中/未命中计数与指令计数。
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
