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

use crate::session_gc::SessionGc;
use crate::vm::ForInIter;
use oxide_builtins::iterator::BuiltinIterKind;
use oxide_types::object::{Cell as UpvalueCell, JsObject, JsString};
use oxide_types::private_key::WELL_KNOWN_SYMBOL_COUNT;
use oxide_types::value::JsValue;

/// session 堆与 GC 簿记。
///
/// 仅做字段分类。mark/sweep 驻留在 `Vm` 上：GC 需扫描所有
/// 子结构的根（regs、frames、for_in_iters、for_of_iters、exception_value 等），
/// 无法限制在 GcState 内。
pub(crate) struct GcState {
    pub(crate) session_gc: SessionGc,
    pub(crate) session_object_ptrs: Vec<*mut JsObject>,
    pub(crate) session_string_ptrs: Vec<*mut JsString>,
    /// BigInt 堆 box 追踪表。`RefCell` 使 `&self` 的分配入口（常量池
    /// `convert_immutables`）也能登记新 box。BigInt 参与 mark/sweep 回收
    /// （死 BigInt 随收集释放，存活字节由 `run_alloc_bytes` 公式按表长
    /// 单列），full_reset 为收尾兜底（表已空时 no-op）。
    pub(crate) session_bigint_ptrs: RefCell<Vec<*mut num_bigint::BigInt>>,
    /// upvalue cell（`Box<Cell>`）追踪表。`RefCell` 使 `&self` 的分配入口也能
    /// 登记新 box。cell 独立堆分配、地址稳定，不参与对象搬移（原地 sweep
    /// 不触碰 cell，也不重写 `cell.value`）；参与 mark-sweep 回收（死 cell
    /// 随收集释放），full_reset 为收尾兜底（表已空时 no-op）。
    pub(crate) session_cell_ptrs: RefCell<Vec<*mut UpvalueCell>>,
    pub(crate) session_bytes_allocated: usize,
    /// 执行期 session 堆账目的峰值高水位（`session_bytes_allocated` 的采样上界），
    /// 顶层指令边界采样，全量重置清零。留存内存观测用。
    pub(crate) session_bytes_peak: usize,
    /// 单 run 分配包络（`run_alloc_bytes`）的高水位：顶层指令边界采样，
    /// run 边界（reset/full_reset）重起算。留存内存观测锚（churn 峰值主锚）。
    pub(crate) run_alloc_peak: usize,
    /// 执行期字符串 GC 的触发水位：本次收集后的存活字节 + 阈值增量。
    /// 仅当账目超过水位才在指令边界触发回收——活串超阈值时不会每指令重复
    /// 触发无死串可回收的白跑，且保证触发点恒在无 builtin 局部活值的边界。
    pub(crate) string_gc_watermark: usize,
    /// 缓存的 GC 阈值（从 config 读一次），热路径只做 usize 比较，免 Arc 解引用。
    pub(crate) gc_threshold_cached: usize,
    /// 执行期原地 sweep 收集的触发水位：本次收集后的分配包络 + 阈值增量。
    /// 仅当 `run_alloc_bytes` 超过水位才在指令边界触发——存活包络超阈值时
    /// 不每指令重复触发无死对象可回收的白跑。
    pub(crate) gc_watermark: usize,
    /// 宿主强制收集旗标（`$262.gc()`）：native 重入中置位，由下一个顶层
    /// 指令边界（完整收集的唯一安全点：无在途 builtin 局部裸指针）消费。
    pub(crate) pending_forced_collect: bool,
    /// GC 压力模式开关（构造时读环境变量 `OXIDE_GC_PRESSURE`，存在即开）：
    /// 开启后每个顶层指令边界做一次完整收集，与强制收集共用同一安全点入口，
    /// 同时跳过两个水位触发的档（完整收集已涵盖其全部工作）。
    pub(crate) gc_pressure_mode: bool,
}

impl GcState {
    /// 分配一个 upvalue cell：独立堆分配并返回裸指针，脱离 session arena 生命周期。
    ///
    /// cell 指针登记进 `session_cell_ptrs`，在 `full_reset` 统一释放。对象
    /// sweep 原地化不搬移对象、不触碰 cell 结构体（地址稳定），cell 的
    /// 生死由 mark-sweep 自行裁定（死 cell 随收集释放）。
    /// `&self` 使调用方可与 `cell_stack` 等其它字段的借用并存（分字段借用）。
    pub(crate) fn alloc_cell(&self, value: JsValue, initialized: bool) -> *mut UpvalueCell {
        let ptr = Box::into_raw(Box::new(UpvalueCell::new(value, initialized)));
        self.session_cell_ptrs.borrow_mut().push(ptr);
        ptr
    }

    /// 释放全部 session 堆 upvalue cell box（收尾兜底，幂等）。仅在完全隔离重置
    /// （`full_reset`）时调用，此时没有存活的 cell_stack / 函数对象 upvalues 会引用
    /// 它们；mark-sweep 已释放的死 cell 已出表，表内残留为收集后仍登记的存活 cell。
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
///
/// 符号下标 `0..WELL_KNOWN_SYMBOL_COUNT` 保留给 well-known symbol（描述取内建
/// 名表），用户 `Symbol()`/`Symbol.for()` 的下标自此区间之后递增，
/// `symbol_descriptions` 只存用户符号描述。
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
        // 用户符号下标顺延到 well-known 保留区间之后，与 well-known 下标空间不重叠。
        let idx = WELL_KNOWN_SYMBOL_COUNT + self.symbol_descriptions.len() as u32;
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
        // well-known 下标区间取内建名表，用户区间按偏移查描述槽。
        if id < WELL_KNOWN_SYMBOL_COUNT {
            return oxide_runtime_api::well_known_symbol_name(id);
        }
        self.symbol_descriptions
            .get((id - WELL_KNOWN_SYMBOL_COUNT) as usize)
            .and_then(|s| s.as_deref())
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
    /// 内置包装器快路径：迭代器为 Array/String/TA/Map/Set 内置包装器时非 None，
    /// DONE/NEXT 直步不经 native 调用。
    pub(crate) fast: Option<BuiltinIterKind>,
    /// 快路径本迭代产出的元素值（仅 DONE→NEXT 之间有效，不跨迭代）。
    pub(crate) fast_value: JsValue,
    /// 快路径 inner 快照：init 期读 `__inner__` 槽后固化，步内不重读槽。
    /// 是 GC 边（session 对象指针），须随 fast_value 接 mark。
    pub(crate) fast_inner: JsValue,
    /// 快路径游标：步内推进的迭代下标，替代每步读写 `__index__` 槽。
    /// Map/Set 族耗尽哨兵为 i32::MAX（口径与慢路径槽一致）。
    pub(crate) fast_cursor: usize,
}

/// for-in / for-of 的活跃迭代器状态。
pub(crate) struct IterState {
    pub(crate) for_in_iters: Vec<*mut ForInIter>,
    pub(crate) for_of_iters: Vec<ForOfEntry>,
}

impl IterState {
    pub(crate) fn reset(&mut self) {
        for iter in self.for_in_iters.drain(..) {
            if iter.is_null() {
                continue;
            }
            // SAFETY: 指针是堆上迭代器体，表独占持有，清表释放恰好一次。
            unsafe {
                drop(Box::from_raw(iter));
            }
        }
        self.for_of_iters.clear();
    }

    pub(crate) fn push_for_in(&mut self, iter: *mut ForInIter) {
        self.for_in_iters.push(iter);
    }

    pub(crate) fn pop_for_in(&mut self) {
        if let Some(iter) = self.for_in_iters.pop() {
            if iter.is_null() {
                return;
            }
            // SAFETY: 指针是堆上迭代器体，表独占持有，出表释放恰好一次。
            unsafe {
                drop(Box::from_raw(iter));
            }
        }
    }

    pub(crate) fn last_for_in(&self) -> *mut ForInIter {
        self.for_in_iters.last().copied().unwrap_or(std::ptr::null_mut())
    }

    /// 压入新迭代器条目：`last_result` 初始为 undefined（尚未执行任何 next()）。
    /// `is_async` 标记 for-await-of 的异步迭代器（异步逃出关闭走独立机制）；
    /// `fast` 为内置包装器种类（None = 慢路径），`fast_inner` 为快路径 inner 快照
    /// （慢路径传 undefined，游标恒 0）。
    pub(crate) fn push_for_of(
        &mut self, iterator: JsValue, is_async: bool, fast: Option<BuiltinIterKind>, fast_inner: JsValue,
    ) {
        self.for_of_iters.push(ForOfEntry {
            iterator,
            last_result: JsValue::undefined(),
            is_async,
            fast,
            fast_value: JsValue::undefined(),
            fast_inner,
            fast_cursor: 0,
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

/// 指令周期采样状态：采样周期（2 的幂）与样本记录表。
///
/// 开启时 dispatch 主循环每 `period` 条指令记一条样本（flat_id、pc、opcode、
/// frames 深度、表代际），run 末按 (flat_id, 表代际) 聚合并输出 top-K 直方图
/// （stderr）。关闭（`period == 0`）时热路径仅一次可预测分支，零写零分配。
pub(crate) struct SampleState {
    /// 采样周期（2 的幂，0 = 关闭）。
    pub(crate) period: u64,
    /// 直方图 top-K 大小（默认 10）。
    pub(crate) top_k: usize,
    /// 样本记录：(flat_id, pc, opcode, frames 深度, 表代际)，run 边界清空。
    pub(crate) records: Vec<(u32, u32, u8, u32, u32)>,
}

impl SampleState {
    /// 清空样本记录（run 边界）。只清记录：period 与 top_k 是调用方
    /// 设置的配置，不随 run 边界重置。
    pub(crate) fn clear_records(&mut self) {
        self.records.clear();
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
