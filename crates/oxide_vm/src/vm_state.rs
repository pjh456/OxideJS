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
use oxide_types::object::{Cell as UpvalueCell, JsObject, JsString};
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
    /// upvalue cell（`Box<Cell>`）追踪表。`RefCell` 使 `&self` 的分配入口也能
    /// 登记新 box。cell 独立堆分配、地址稳定，不参与对象搬移（sweep 只重写
    /// `cell.value` 中的对象引用），只在 full_reset 统一释放。
    pub(crate) session_cell_ptrs: RefCell<Vec<*mut UpvalueCell>>,
    pub(crate) session_bytes_allocated: usize,
    /// 执行期字符串 GC 的触发水位：本次收集后的存活字节 + 阈值增量。
    /// 仅当账目超过水位才在指令边界触发回收——活串超阈值时不会每指令重复
    /// 触发无死串可回收的白跑，且保证触发点恒在无 builtin 局部活值的边界。
    pub(crate) string_gc_watermark: usize,
    pub(crate) forwarding: HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
}

impl GcState {
    pub(crate) fn track_epoch_object(&mut self, ptr: *mut JsObject) {
        self.epoch_object_ptrs.push(ptr);
    }

    /// 分配一个 upvalue cell：独立堆分配并返回裸指针，脱离 session arena 生命周期。
    ///
    /// cell 指针登记进 `session_cell_ptrs`，在 `full_reset` 统一释放。对象 sweep
    /// 搬移不触碰 cell 结构体（地址稳定），只重写 `cell.value` 中的对象引用。
    /// `&self` 使调用方可与 `cell_stack` 等其它字段的借用并存（分字段借用）。
    pub(crate) fn alloc_cell(&self, value: JsValue, initialized: bool) -> *mut UpvalueCell {
        let ptr = Box::into_raw(Box::new(UpvalueCell::new(value, initialized)));
        self.session_cell_ptrs.borrow_mut().push(ptr);
        ptr
    }

    /// 释放全部 session 堆 upvalue cell box。仅在完全隔离重置（`full_reset`）时调用，
    /// 此时没有存活的 cell_stack / 函数对象 upvalues 会引用它们。
    pub(crate) fn free_cells(&mut self) {
        for ptr in self.session_cell_ptrs.borrow_mut().drain(..) {
            // SAFETY: 每个指针来自 alloc_cell 的 Box::into_raw(Box::new(Cell))，
            // 且只在这里恰好释放一次。
            unsafe {
                drop(Box::from_raw(ptr));
            }
        }
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

/// for-of 迭代器栈条目：迭代器对象与其最近一次 `next()` 结果对象配对存放。
///
/// `last_result` 只属于本迭代器（嵌套 for-of / 数组解构 / spread 各自持有自己的
/// 结果），`FOR_OF_CLOSE`/`FOR_AWAIT_OF_CLOSE` 据此判定本迭代器是否自然 done，
/// 避免共享单槽被其它迭代器覆盖导致漏调/多调 `return()`。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ForOfEntry {
    /// 迭代器对象（同步迭代器 / 异步迭代器 / AsyncFromSyncIterator 包装）。
    pub(crate) iterator: JsValue,
    /// 本迭代器最近一次 DONE 返回的结果对象（CLOSE 判 done 用）。
    pub(crate) last_result: JsValue,
    /// 是否为 for-await-of 的异步迭代器：异步逃出关闭须 await return() 的
    /// promise（独立异步机制），同步逃出路径（break/continue/return 计数关闭）
    /// 不得同步调用其 return()。
    pub(crate) is_async: bool,
}

/// for-in / for-of 的活跃迭代器状态。
pub(crate) struct IterState {
    pub(crate) for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) for_of_iters: Vec<ForOfEntry>,
}

impl IterState {
    pub(crate) fn reset(&mut self) {
        self.for_in_iters.clear();
        self.for_of_iters.clear();
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

    /// 压入新迭代器条目：`last_result` 初始为 undefined（尚未执行任何 next()）。
    /// `is_async` 标记 for-await-of 的异步迭代器（异步逃出关闭走独立机制）。
    pub(crate) fn push_for_of(&mut self, iterator: JsValue, is_async: bool) {
        self.for_of_iters.push(ForOfEntry {
            iterator,
            last_result: JsValue::undefined(),
            is_async,
        });
    }

    pub(crate) fn last_for_of(&self) -> Option<JsValue> {
        self.for_of_iters.last().map(|e| e.iterator)
    }

    pub(crate) fn pop_for_of(&mut self) -> Option<ForOfEntry> {
        self.for_of_iters.pop()
    }

    /// 栈顶条目最近一次 DONE 结果；栈空时返回 undefined（防御）。
    pub(crate) fn last_result(&self) -> JsValue {
        self.for_of_iters.last().map(|e| e.last_result).unwrap_or(JsValue::undefined())
    }

    /// 写入栈顶条目的 DONE 结果（本迭代器自身的结果，不跨迭代器覆盖）。
    pub(crate) fn set_last_result(&mut self, val: JsValue) {
        if let Some(entry) = self.for_of_iters.last_mut() {
            entry.last_result = val;
        }
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
