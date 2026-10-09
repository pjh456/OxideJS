//! 挂起执行上下文快照：生成器 / 异步函数 / 异步生成器 三份状态结构体共享的执行核心。
//!
//! 挂起时把 VM 的执行核心（regs/pc/bytecode/各栈段/在途异常与完成）整体搬入，
//! 恢复时搬回；GC 侧对快照的根收集统一走 for_each_value。
//! 约定：本结构是"值"的容器，不含调度标志（generator_dispatch/async_*_dispatch/
//! async_context 等仍由各恢复包装函数管理，避免跨状态机污染）。

use std::sync::Arc;

use oxide_bytecode::opcode;
use oxide_types::object::Cell;
use oxide_types::value::JsValue;

use crate::vm::{CallFrame, Completion, ForInIter, PendingAsyncEscape, TryHandler, Vm};
use crate::vm_state::ForOfEntry;

/// 挂起执行上下文快照（三份状态结构体共享的执行核心）。
pub(crate) struct SuspendedFrame {
    pub regs: Box<[JsValue; 256]>,
    pub pc: usize,
    pub bytecode: Arc<[opcode::Instr]>,
    /// 模块 flat_id（callee 表代际平表中的下标；恢复时按 callee 记录的代际
    /// 解析平表并重激活 immutables）。
    pub sub_idx: u32,
    pub active_reg_limit: u8,
    pub root_reg_limit: u8,
    /// 挂起帧（push_bytecode_frame 压入，快照时弹出存此）。
    pub frame: Option<CallFrame>,
    pub spill_stack: Vec<JsValue>,
    pub save_stack: Vec<JsValue>,
    pub cell_stack: Vec<Vec<*mut Cell>>,
    pub try_stack: Vec<TryHandler>,
    pub for_in_iters: Vec<*mut ForInIter>,
    pub for_of_iters: Vec<ForOfEntry>,
    /// `yield*` 委托中的内层迭代器；异步函数恒为 None。
    pub delegated_iterator: Option<JsValue>,
    pub saved_bytecode_stack: Vec<Arc<[opcode::Instr]>>,
    pub saved_immutables_stack: Vec<*const [JsValue]>,
    pub exception_value: Option<JsValue>,
    pub pending_exception: Option<JsValue>,
    pub pending_error_kind: Option<&'static str>,
    pub pending_completion: Option<Completion>,
    /// 逃出 for-await-of 的异步关闭在途状态（随挂起快照跨微任务存活）。
    pub pending_async_escape: Option<PendingAsyncEscape>,
}

impl SuspendedFrame {
    /// 全空帧（New 阶段构造状态盒时用）。
    pub fn new_empty() -> Self {
        SuspendedFrame {
            regs: Box::new([JsValue::undefined(); 256]),
            pc: 0,
            bytecode: Arc::default(),
            sub_idx: 0,
            active_reg_limit: 0,
            root_reg_limit: 0,
            frame: None,
            spill_stack: Vec::new(),
            save_stack: Vec::new(),
            cell_stack: Vec::new(),
            try_stack: Vec::new(),
            for_in_iters: Vec::new(),
            for_of_iters: Vec::new(),
            delegated_iterator: None,
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            exception_value: None,
            pending_exception: None,
            pending_error_kind: None,
            pending_completion: None,
            pending_async_escape: None,
        }
    }

    /// 从当前 VM 快照：弹出挂起帧，搬入各栈段、迭代器与在途异常/完成。
    ///
    /// # 前提
    /// 嵌套 dispatch 已让出，VM 只含当前状态机的执行数据（与现有 snapshot_* 的
    /// 语义逐字一致）。`callee` 用于计算 sub_idx。
    ///
    /// # 边界
    /// frames 为空返回 Err（挂起点缺帧）。
    pub fn save_from(&mut self, vm: &mut Vm, callee: JsValue) -> Result<(), String> {
        let frame = vm.frames.pop().ok_or_else(|| "frame missing on suspend".to_string())?;
        self.frame = Some(frame);
        // 挂起弹帧不经 restore_frame：帧已弹入状态盒，活动镜像清零（null 空切片）。
        vm.active_upvalues = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        *self.regs = vm.regs;
        self.pc = vm.pc;
        self.bytecode = std::mem::take(&mut vm.bytecode);
        self.sub_idx = if callee.is_object() {
            unsafe { (*callee.as_js_object_ptr()).sub_module_index() }
        } else {
            0
        };
        self.active_reg_limit = vm.active_reg_limit;
        self.root_reg_limit = vm.root_reg_limit;
        self.spill_stack = std::mem::take(&mut vm.spill_stack);
        self.save_stack = std::mem::take(&mut vm.save_stack);
        self.cell_stack = std::mem::take(&mut vm.cell_stack);
        self.try_stack = std::mem::take(&mut vm.try_stack);
        self.for_in_iters = std::mem::take(&mut vm.iters.for_in_iters);
        self.for_of_iters = std::mem::take(&mut vm.iters.for_of_iters);
        self.delegated_iterator = std::mem::take(&mut vm.delegated_iterator);
        self.saved_bytecode_stack = std::mem::take(&mut vm.saved_bytecode_stack);
        self.saved_immutables_stack = std::mem::take(&mut vm.saved_immutables_stack);
        self.exception_value = vm.exception_value.take();
        self.pending_exception = vm.pending_exception.take();
        self.pending_error_kind = vm.pending_error_kind.take();
        self.pending_completion = vm.pending_completion.take();
        self.pending_async_escape = std::mem::take(&mut vm.pending_async_escape);
        Ok(())
    }

    /// 恢复进 VM：写回各栈段、在途异常/完成，按 callee 记录的表代际重激活
    /// immutables，并把执行键维度（表代际 / flat_id）还原到被恢复模块。
    ///
    /// # 边界
    /// - `callee` 须为挂起函数对象值（状态盒 `callee` 槽）：代际按对象创建期
    ///   记录解析，存活函数对象按代际保活其表，跨 run 恢复命中同一张表。
    /// - callee 非对象（gen 0 哨兵）、代际表已被回收或下标越界返回 Err，
    ///   调用方须按各自路径回滚并报错（保持现有三处行为）。
    ///
    /// # 副作用
    /// - `active_table_gen` / `active_flat_id` 还原为被恢复模块的 (代际,
    ///   flat_id)：恢复体后续发射的标签模板须按本模块 (代际, flat_id) 命中
    ///   模板缓存——不还原则同顶层连续恢复两代挂起体撞调用方侧同键，静默
    ///   取回他侧模板对象。
    pub fn restore_into(&mut self, vm: &mut Vm, callee: JsValue) -> Result<(), String> {
        vm.regs = *self.regs;
        vm.pc = self.pc;
        vm.bytecode = std::mem::take(&mut self.bytecode);
        let gen = if callee.is_object() {
            // SAFETY: callee 为执行核心产出的函数对象值，session 生命周期内指针有效。
            unsafe { (*callee.as_js_object_ptr()).table_gen() }
        } else {
            vm.current_gen
        };
        let table = vm.tables.get(&gen);
        let module = table.and_then(|t| t.modules.get(self.sub_idx as usize));
        let constants = module.map(|m| m.constants.clone());
        match constants {
            Some(c) => vm.activate_immutables(gen, self.sub_idx as usize, &c),
            None => return Err("suspended state module table is no longer available".into()),
        }
        // 模板缓存键维度随模块恢复：与 activate_immutables 同位还原。
        vm.active_table_gen = gen;
        vm.active_flat_id = self.sub_idx;
        vm.active_reg_limit = self.active_reg_limit;
        vm.root_reg_limit = self.root_reg_limit;
        vm.frames.clear();
        // 帧弹回前取被恢复函数的表，弹回后置回活动镜像（帧空即顶层空切片）。
        let up = self
            .frame
            .as_ref()
            .map(|f| f.upvalues)
            .unwrap_or(std::ptr::slice_from_raw_parts(std::ptr::null(), 0));
        if let Some(frame) = self.frame.take() {
            vm.frames.push(frame);
        }
        vm.active_upvalues = up;
        vm.spill_stack = std::mem::take(&mut self.spill_stack);
        vm.save_stack = std::mem::take(&mut self.save_stack);
        vm.cell_stack = std::mem::take(&mut self.cell_stack);
        vm.try_stack = std::mem::take(&mut self.try_stack);
        vm.iters.for_in_iters = std::mem::take(&mut self.for_in_iters);
        vm.iters.for_of_iters = std::mem::take(&mut self.for_of_iters);
        vm.delegated_iterator = std::mem::take(&mut self.delegated_iterator);
        vm.saved_bytecode_stack = std::mem::take(&mut self.saved_bytecode_stack);
        vm.saved_immutables_stack = std::mem::take(&mut self.saved_immutables_stack);
        vm.exception_value = self.exception_value.take();
        vm.pending_exception = self.pending_exception.take();
        vm.pending_error_kind = self.pending_error_kind.take();
        vm.pending_completion = self.pending_completion.take();
        vm.pending_async_escape = std::mem::take(&mut self.pending_async_escape);
        Ok(())
    }

    /// GC 根遍历：产出全部 JsValue（对象与字符串都产出），含 cell/for_in 解引用。
    pub fn for_each_value(&self, mut f: impl FnMut(JsValue)) {
        for v in self.regs.iter() {
            f(*v);
        }
        if let Some(frame) = &self.frame {
            f(frame.saved_this);
            f(frame.saved_new_target);
            f(frame.callee);
            if let Some(ct) = frame.constructed_this {
                f(ct);
            }
        }
        for v in &self.spill_stack {
            f(*v);
        }
        for v in &self.save_stack {
            f(*v);
        }
        for cells in &self.cell_stack {
            for &p in cells {
                if p.is_null() {
                    continue;
                }
                // SAFETY: cell 经 alloc_cell 独立堆分配，本 session 内指针有效。
                f(unsafe { &*p }.value);
            }
        }
        for entry in &self.for_of_iters {
            f(entry.iterator);
            f(entry.last_result);
            f(entry.fast_value);
            f(entry.fast_inner);
        }
        if let Some(it) = self.delegated_iterator {
            f(it);
        }
        if let Some(v) = self.exception_value {
            f(v);
        }
        if let Some(v) = self.pending_exception {
            f(v);
        }
        if let Some(Completion::Return { value, .. }) = self.pending_completion {
            f(value);
        }
        if let Some(pend) = &self.pending_async_escape {
            f(pend.close_promise);
            if let Completion::Return { value, .. } = pend.completion {
                f(value);
            }
            for &v in &pend.remaining {
                f(v);
            }
        }
        for iter in &self.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放堆上迭代器体，状态盒独占持有。
            for (v, _si) in unsafe { (*(*iter)).keys.iter() } {
                f(*v);
            }
        }
    }

    /// 堆字节数（drop 记账用：bytecode + spill/save/for_of 容量）。
    pub fn heap_bytes(&self) -> u64 {
        self.bytecode.len() as u64 * std::mem::size_of::<opcode::Instr>() as u64
            + self.spill_stack.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
            + self.save_stack.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
            + self.for_of_iters.capacity() as u64 * std::mem::size_of::<ForOfEntry>() as u64
    }
}

impl Drop for SuspendedFrame {
    /// 释放状态盒持有的 for-in 迭代器体：体是堆上 `Box`，每个状态盒独占
    /// 持有自身体，随状态盒释放逐条释放（null 跳过）。restore 已把向量
    /// 移出时字段为空，Drop 不再触碰，无双放。
    fn drop(&mut self) {
        for iter in self.for_in_iters.drain(..) {
            if iter.is_null() {
                continue;
            }
            // SAFETY: 指针是堆上迭代器体，状态盒独占持有，释放恰好一次。
            unsafe {
                drop(Box::from_raw(iter));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{FrameContinuation, Vm};
    fn vm() -> Vm {
        Vm::new()
    }

    fn sentinel_frame(tag: JsValue) -> CallFrame {
        CallFrame {
            return_addr: 7,
            function_name: 1,
            caller_reg_limit: 2,
            caller_active_reg_limit: 2,
            saved_reg_offset: 3,
            spill_offset: 4,
            arguments_base: 5,
            arguments_count: 6,
            saved_this: tag,
            saved_new_target: tag,
            callee: tag,
            construct_result_reg: None,
            constructed_this: Some(tag),
            is_derived_constructor: false,
            super_called: false,
            strict: false,
            continuation: FrameContinuation::None,
            upvalues: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
        }
    }

    #[test]
    fn save_restore_round_trip() {
        let mut machine = vm();
        machine.regs[0] = JsValue::float(42.0);
        machine.pc = 9;
        machine.frames.push(sentinel_frame(JsValue::float(43.0)));
        machine.spill_stack.push(JsValue::float(44.0));
        machine.delegated_iterator = Some(JsValue::float(45.0));
        machine.pending_completion = Some(Completion::Return {
            value: JsValue::float(46.0),
            remaining_finally: 1,
            for_of_count: 0,
            for_in_count: 0,
            dispose_count: 0,
        });

        let mut frame = SuspendedFrame::new_empty();
        frame.save_from(&mut machine, JsValue::undefined()).unwrap();
        assert_eq!(frame.regs[0], JsValue::float(42.0));
        assert_eq!(frame.pc, 9);
        assert_eq!(frame.delegated_iterator, Some(JsValue::float(45.0)));
        assert!(matches!(
            frame.pending_completion,
            Some(Completion::Return { value, remaining_finally: 1, .. }) if value == JsValue::float(46.0)
        ));

        // gen 0 空表占位 + sub_idx = 0 越界：恢复应报错（保持现有三处行为）。
        let mut machine2 = vm();
        let mut frame2 = SuspendedFrame::new_empty();
        frame2.sub_idx = 0;
        let err = frame2.restore_into(&mut machine2, JsValue::undefined());
        assert!(err.is_err());
    }
}
