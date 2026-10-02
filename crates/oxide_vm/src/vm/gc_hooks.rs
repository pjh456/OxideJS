//! GC 根遍历与执行期收集钩子：根统一枚举、搬移后指针重写、执行期两档
//! 收集入口（安全点门控）与 session GC 账目统计访问器。

use oxide_types::object::{Cell, JsObject, JsString};
use oxide_types::value::JsValue;

use super::frames::Completion;
use super::Vm;
use crate::session_gc::SessionGc;

impl Vm {
    /// GC 根收集的统一遍历（对象与字符串都产出）。与 `rewrite_values` 字段一一对应。
    /// 覆盖执行核心的全部 JsValue 持有点：regs/帧/各栈段/cell/在途异常与完成/
    /// 挂起信号/迭代器/微任务/global。新增执行字段必须同时登记在此与
    /// `rewrite_values`。
    pub(crate) fn for_each_value(&self, mut f: impl FnMut(JsValue)) {
        for value in &self.regs {
            f(*value);
        }
        // 各代际 immutables 缓存含 session BigInt（new_bigint 分配），未入根则被
        // sweep 释放 → 常量池加载时悬垂。perm 字符串无害（不在 session 集合中）。
        for table in self.tables.values() {
            for once_lock in &table.immutables {
                if let Some(immutable_vec) = once_lock.get() {
                    for &value in immutable_vec.iter() {
                        f(value);
                    }
                }
            }
        }
        for frame in &self.frames {
            f(frame.saved_this);
            f(frame.saved_new_target);
            f(frame.callee);
            f(frame.constructed_this.unwrap_or(JsValue::undefined()));
        }
        for &v in &self.save_stack {
            f(v);
        }
        // spill 栈是 session GC 根（漏根 → 溢出值被回收 → use-after-free）。
        for &v in &self.spill_stack {
            f(v);
        }
        for cell_vec in &self.cell_stack {
            for &cell_ptr in cell_vec {
                if cell_ptr.is_null() {
                    continue;
                }
                // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
                f(unsafe { &*cell_ptr }.value);
            }
        }
        f(self.exception_value.unwrap_or(JsValue::undefined()));
        f(self.pending_exception.unwrap_or(JsValue::undefined()));
        f(self.last_uncaught_value.unwrap_or(JsValue::undefined()));
        f(self.pending_length_exception.unwrap_or(JsValue::undefined()));
        // 悬挂的 return 完成持有返回值，是 GC 根。
        if let Some(Completion::Return { value, .. }) = self.pending_completion {
            f(value);
        }
        f(self.generator_suspended.unwrap_or(JsValue::undefined()));
        f(self.delegated_iterator.unwrap_or(JsValue::undefined()));
        f(self.async_context.unwrap_or(JsValue::undefined()));
        f(self.async_gen_context.unwrap_or(JsValue::undefined()));
        // 在途异步逃出的 promise/完成值/剩余迭代器都是 GC 根。
        if let Some(pend) = &self.pending_async_escape {
            f(pend.close_promise);
            if let Completion::Return { value, .. } = pend.completion {
                f(value);
            }
            for &v in &pend.remaining {
                f(v);
            }
        }
        f(self.inline_callee.unwrap_or(JsValue::undefined()));
        // 标签模板对象缓存：命中的模板对象是 GC 根（未根 → sweep 搬移/回收悬垂）。
        for &cached in self.template_objects.values() {
            f(cached);
        }
        for &entry in &self.iters.for_of_iters {
            f(entry.iterator);
            f(entry.last_result);
            f(entry.fast_value);
        }
        // 微任务队列中的处理器/能力/值都是 GC 根。
        for job in &self.job_queue {
            crate::promise::for_each_job_value(job, &mut f);
        }
        // Atomics waiter 表登记的 promise 是 GC 根（run 内跨调用存活）。
        for promises in self.atomics_waiters.values() {
            for p in promises {
                f(*p);
            }
        }
        for iter in &self.iters.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放堆上迭代器体，VM 表独占持有。
            unsafe {
                for (v, _si) in (*(*iter)).keys.iter() {
                    f(*v);
                }
            }
        }
        f(JsValue::from_js_object(self.session.global_object().as_ptr() as *mut JsObject));
    }

    /// GC 指针重写（session 搬移后调用）。与 `for_each_value` 字段一一对应。
    #[allow(dead_code)]
    pub(crate) fn rewrite_values(&mut self, mut rewrite: impl FnMut(JsValue) -> JsValue) {
        for value in &mut self.regs {
            *value = rewrite(*value);
        }
        for frame in &mut self.frames {
            frame.saved_this = rewrite(frame.saved_this);
            frame.saved_new_target = rewrite(frame.saved_new_target);
            frame.callee = rewrite(frame.callee);
            frame.constructed_this = frame.constructed_this.map(&mut rewrite);
        }
        for v in &mut self.save_stack {
            *v = rewrite(*v);
        }
        for v in &mut self.spill_stack {
            *v = rewrite(*v);
        }
        for cell_vec in &mut self.cell_stack {
            for &mut cell_ptr in cell_vec.iter_mut() {
                if cell_ptr.is_null() {
                    continue;
                }
                // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
                let cell = unsafe { &mut *cell_ptr };
                cell.value = rewrite(cell.value);
            }
        }
        self.exception_value = self.exception_value.map(&mut rewrite);
        self.pending_exception = self.pending_exception.map(&mut rewrite);
        self.last_uncaught_value = self.last_uncaught_value.map(&mut rewrite);
        self.pending_length_exception = self.pending_length_exception.map(&mut rewrite);
        for cached in self.template_objects.values_mut() {
            *cached = rewrite(*cached);
        }
        self.pending_completion = self.pending_completion.map(|completion| match completion {
            Completion::Return {
                value,
                remaining_finally,
                for_of_count,
                for_in_count,
            } => Completion::Return {
                value: rewrite(value),
                remaining_finally,
                for_of_count,
                for_in_count,
            },
            other => other,
        });
        self.generator_suspended = self.generator_suspended.map(&mut rewrite);
        self.delegated_iterator = self.delegated_iterator.map(&mut rewrite);
        self.async_context = self.async_context.map(&mut rewrite);
        self.async_gen_context = self.async_gen_context.map(&mut rewrite);
        // 在途异步逃出的 promise/完成值/剩余迭代器随 sweep 重写（与 mark 段一一对应）。
        self.pending_async_escape = self.pending_async_escape.as_mut().map(|pend| {
            let completion = match pend.completion {
                Completion::Return {
                    value,
                    remaining_finally,
                    for_of_count,
                    for_in_count,
                } => Completion::Return {
                    value: rewrite(value),
                    remaining_finally,
                    for_of_count,
                    for_in_count,
                },
                other => other,
            };
            super::PendingAsyncEscape {
                close_promise: rewrite(pend.close_promise),
                completion,
                remaining: pend.remaining.iter().copied().map(&mut rewrite).collect(),
            }
        });
        self.inline_callee = self.inline_callee.map(&mut rewrite);
        for entry in &mut self.iters.for_of_iters {
            entry.iterator = rewrite(entry.iterator);
            entry.last_result = rewrite(entry.last_result);
            entry.fast_value = rewrite(entry.fast_value);
        }
        // 微任务队列中的值随 sweep 重写。
        for job in &mut self.job_queue {
            crate::promise::rewrite_job_values(job, &mut rewrite);
        }
        // Atomics waiter 表中的 promise 随 sweep 重写（与 mark 段一一对应）。
        for promises in self.atomics_waiters.values_mut() {
            for p in promises {
                *p = rewrite(*p);
            }
        }
        for iter in &mut self.iters.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放堆上迭代器体，VM 表独占持有。
            unsafe {
                for (v, _si) in (*(*iter)).keys.iter_mut() {
                    *v = rewrite(*v);
                }
            }
        }
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        if !global_ptr.is_null() {
            // SAFETY: KernelSession 在 VM 生命周期内拥有 global_object。
            unsafe {
                (*global_ptr).rewrite_object_values(rewrite);
            }
        }
    }

    /// GC 根统一枚举入口：遍历的字段清单与 `rewrite_values` 一一对应。
    pub(crate) fn for_each_root(&self, f: impl FnMut(JsValue)) {
        self.for_each_value(f);
    }

    /// 执行一次 session GC（是否真正回收由 `SessionGc` 的水位门控决定）。
    ///
    /// `session_gc` 经 `mem::take` 借出后再放回：收集需要 `&mut Vm`，而
    /// `SessionGc` 是 `Vm` 的内部字段，借出以避开借用冲突。
    pub(crate) fn maybe_collect_session_gc(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.maybe_collect(self);
        self.gc_state.session_gc = session_gc;
    }

    /// 执行期字符串阈值回收：仅回收 session 字符串（跳过对象搬移）。热路径只在
    /// 超阈值后进入，`mem::take` 不承担每次分配的开销。
    pub(crate) fn maybe_collect_session_strings(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.maybe_collect_strings_only(self);
        self.gc_state.session_gc = session_gc;
    }

    /// 执行期原地 sweep 收集的 dispatch 安全点入口：仅在循环顶
    /// （`native_call_depth == 0`，无 builtin 局部裸指针、dispatch 未重入）
    /// 由触发块在水位命中后调用，无门控直接收集。
    ///
    /// # 边界与前提
    /// - for-in 迭代器体是堆上 Box：活跃形经 VM 表、挂起形经状态盒边进入
    ///   根收集，键引用被 mark 标活，原地清扫不搬移对象、不误释放，
    ///   无需拦截；
    /// - 无对象可回收时仍跑一轮（mark + 原地清扫），水位同点抬高，
    ///   触发间距由包络增量控制。
    pub(crate) fn maybe_collect_in_run(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.collect_in_run(self);
        self.gc_state.session_gc = session_gc;
    }

    /// 只读访问 session GC 的统计（回收次数、存活/死亡对象数、释放字节等）。
    pub fn session_gc_stats(&self) -> &SessionGc {
        &self.gc_state.session_gc
    }

    /// 当前 session arena 中存活（已晋升）的对象数量。
    pub fn session_object_count(&self) -> usize {
        self.gc_state.session_object_ptrs.len()
    }

    /// session 当前分配的字节数（对象 + 存活字符串）。
    pub fn session_bytes_allocated(&self) -> usize {
        self.gc_state.session_bytes_allocated
    }

    /// 执行期 session 堆账目的峰值高水位（顶层指令边界采样，全量重置清零）。
    pub fn session_bytes_peak(&self) -> usize {
        self.gc_state.session_bytes_peak
    }

    /// 本 run 累计分配字节的高水位：`run_alloc_bytes` 的顶层指令边界
    /// 采样上界，run 边界（reset/full_reset）重起算。留存内存观测锚。
    pub fn run_alloc_peak(&self) -> usize {
        self.gc_state.run_alloc_peak
    }

    /// 本 run 累计分配字节：session 手工堆账目（session 对象及其属性向量 +
    /// session 串）+ BigInt 表长 × `size_of::<BigInt>` + cell 表长 ×
    /// `size_of::<Cell>`。三分量两两不相交（BigInt 与 cell 不在手工账目内），
    /// O(1) 读，供单 run 分配上限判定。收集释放后读数回落存活集量级；
    /// 轻量 reset 保留账目与表（仅复位峰值与水位），full_reset 清零重起算。
    ///
    /// 注意：手工堆账目只在对象分配/字符串分配/GC 回收点更新，执行期对象
    /// 属性区（元素/属性向量扩容）增长对其不可见——上限判定须配合
    /// [`Self::run_alloc_bytes_full`] 的深采样层。
    pub(crate) fn run_alloc_bytes(&self) -> usize {
        self.gc_state.session_bytes_allocated
            + self.gc_state.session_bigint_ptrs.borrow().len() * std::mem::size_of::<num_bigint::BigInt>()
            + self.gc_state.session_cell_ptrs.borrow().len() * std::mem::size_of::<Cell>()
    }

    /// 本 run 累计分配字节的全量重算版：各分量按当前值各计一次、互不相交——
    /// 存活对象头加堆数据（属性/元素/meta 向量容量、upvalue 列表容量、native
    /// 状态盒）+ 存活串（头加 payload）+ BigInt 表长 × 尺寸 + cell 表长 ×
    /// 尺寸。重算即属性区扩容等账目盲区的兜底，读数为当前真实总量，零增长时
    /// 与 [`Self::run_alloc_bytes`] 的轻层公式逐位相等。
    pub(crate) fn run_alloc_bytes_full(&self) -> u64 {
        let mut bytes = 0u64;
        for &ptr in self.gc_state.session_object_ptrs.iter() {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自对象表登记，dispatch 安全点处仍有效。
            let obj = unsafe { &*ptr };
            bytes += std::mem::size_of::<JsObject>() as u64 + SessionGc::object_heap_data_bytes(obj);
        }
        for &ptr in &self.gc_state.session_string_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 在字符串表登记，收尾前有效。
            bytes += (std::mem::size_of::<JsString>() + unsafe { (*ptr).payload_bytes() }) as u64;
        }
        bytes += (self.gc_state.session_bigint_ptrs.borrow().len() * std::mem::size_of::<num_bigint::BigInt>()) as u64;
        bytes += (self.gc_state.session_cell_ptrs.borrow().len() * std::mem::size_of::<Cell>()) as u64;
        bytes
    }

    /// 无条件执行一次完整 session GC（mark + 对象原地清扫 + 串/BigInt 清扫）。
    ///
    /// # 副作用
    /// - 存活对象地址不变、根与原生盒免改写；
    ///   `session_bytes_allocated` 重置为清扫后的存活字节。
    ///
    /// # 注意事项
    /// - 须在执行外的安全点调用（无在途 builtin 局部裸指针、dispatch 未重入）；
    ///   执行期触发仍走水位路径，本入口供事后观测（如基准测 workload 后留存堆）。
    pub fn collect_session_gc(&mut self) {
        let mut session_gc = std::mem::take(&mut self.gc_state.session_gc);
        session_gc.collect(self);
        self.gc_state.session_gc = session_gc;
    }

    /// 当前 epoch 中已分配并跟踪的对象数量。对象分配统一入口 Box 化后
    /// 全部对象入 session 表，epoch 表退役，恒返 0（采集点保留，恒零口径）。
    pub fn epoch_object_count(&self) -> usize {
        0
    }

    /// inline cache 命中率（0.0~1.0），用于观测 IC 预热效果。
    pub fn ic_hit_rate(&self) -> f64 {
        self.profiling.ic_hit_rate()
    }

    /// 累计执行的指令数（profiling）。
    pub fn instruction_count(&self) -> u64 {
        self.profiling.instruction_count
    }

    /// 全局 symbol 注册表中已注册的 key 数量。
    pub fn symbol_registry_len(&self) -> usize {
        self.symbols.registry_len()
    }

    /// inline cache 命中次数。
    pub fn ic_hit_count(&self) -> u64 {
        self.profiling.ic_hits.get()
    }

    /// inline cache 未命中次数。
    pub fn ic_miss_count(&self) -> u64 {
        self.profiling.ic_misses.get()
    }
}
