//! 挂起执行上下文快照：生成器 / 异步函数 / 异步生成器 三份状态结构体共享的执行核心。
//!
//! 挂起时把 VM 的执行核心（regs/pc/bytecode/各栈段/在途异常与完成）整体搬入，
//! 恢复时搬回；GC 侧对快照的根收集与指针重写也统一走本结构的方法。
//! 约定：本结构是"值"的容器，不含调度标志（generator_dispatch/async_*_dispatch/
//! async_context 等仍由各恢复包装函数管理，避免跨状态机污染）。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode;
use oxide_types::object::Cell;
use oxide_types::value::JsValue;

use crate::vm::{CallFrame, Completion, ForInIter, TryHandler, Vm};
use crate::vm_state::ForOfEntry;

/// 挂起执行上下文快照（三份状态结构体共享的执行核心）。
pub(crate) struct SuspendedFrame {
    pub regs: Box<[JsValue; 256]>,
    pub pc: usize,
    pub bytecode: Arc<[opcode::Instr]>,
    /// 模块 flat_id（恢复时经 sub_modules 重激活 immutables；跨 run 失效据此报错）。
    pub sub_idx: u32,
    pub active_reg_limit: u8,
    pub root_reg_limit: u8,
    /// 挂起帧（push_bytecode_frame 压入，快照时弹出存此）。
    pub frame: Option<CallFrame>,
    pub spill_stack: Vec<JsValue>,
    pub save_stack: Vec<JsValue>,
    pub cell_stack: Vec<Vec<*mut Cell>>,
    pub try_stack: Vec<TryHandler>,
    pub for_in_iters: Vec<*mut ForInIter<'static>>,
    pub for_of_iters: Vec<ForOfEntry>,
    /// `yield*` 委托中的内层迭代器；异步函数恒为 None。
    pub delegated_iterator: Option<JsValue>,
    pub saved_bytecode_stack: Vec<Arc<[opcode::Instr]>>,
    pub saved_immutables_stack: Vec<*const [JsValue]>,
    pub exception_value: Option<JsValue>,
    pub pending_exception: Option<JsValue>,
    pub pending_error_kind: Option<&'static str>,
    pub pending_completion: Option<Completion>,
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
        Ok(())
    }

    /// 恢复进 VM：写回各栈段、在途异常/完成，按 sub_idx 重激活 immutables。
    ///
    /// # 边界
    /// `sub_idx >= sub_modules.len()`（挂起状态跨 run）返回 Err，调用方须按各自
    /// 路径回滚并报错（保持现有三处行为）。
    pub fn restore_into(&mut self, vm: &mut Vm, sub_modules: &Arc<Vec<Arc<CompiledModule>>>) -> Result<(), String> {
        vm.regs = *self.regs;
        vm.pc = self.pc;
        vm.bytecode = std::mem::take(&mut self.bytecode);
        let subs = Arc::clone(sub_modules);
        if (self.sub_idx as usize) < subs.len() {
            vm.activate_immutables(self.sub_idx as usize, &subs[self.sub_idx as usize].constants);
        } else {
            return Err("suspended state across runs is no longer valid".into());
        }
        vm.active_reg_limit = self.active_reg_limit;
        vm.root_reg_limit = self.root_reg_limit;
        vm.frames.clear();
        if let Some(frame) = self.frame.take() {
            vm.frames.push(frame);
        }
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
        Ok(())
    }

    /// GC 根遍历：产出全部 JsValue（对象与字符串都产出），含 cell/for_in 解引用。
    /// 与 `rewrite_values` 字段一一对应。
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
                // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
                f(unsafe { &*p }.value);
            }
        }
        for entry in &self.for_of_iters {
            f(entry.iterator);
            f(entry.last_result);
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
        for iter in &self.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放由当前 VM epoch 拥有的存活迭代器指针。
            for (v, _si) in unsafe { (*(*iter)).keys.iter() } {
                f(*v);
            }
        }
    }

    /// GC 指针重写：与 `for_each_value` 字段一一对应。
    pub fn rewrite_values(&mut self, mut rewrite: impl FnMut(JsValue) -> JsValue) {
        for v in self.regs.iter_mut() {
            *v = rewrite(*v);
        }
        if let Some(frame) = &mut self.frame {
            frame.saved_this = rewrite(frame.saved_this);
            frame.saved_new_target = rewrite(frame.saved_new_target);
            frame.callee = rewrite(frame.callee);
            frame.constructed_this = frame.constructed_this.map(&mut rewrite);
        }
        for v in &mut self.spill_stack {
            *v = rewrite(*v);
        }
        for v in &mut self.save_stack {
            *v = rewrite(*v);
        }
        for cells in &mut self.cell_stack {
            for &mut p in cells.iter_mut() {
                if p.is_null() {
                    continue;
                }
                // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
                let cell = unsafe { &mut *p };
                cell.value = rewrite(cell.value);
            }
        }
        for entry in &mut self.for_of_iters {
            entry.iterator = rewrite(entry.iterator);
            entry.last_result = rewrite(entry.last_result);
        }
        self.delegated_iterator = self.delegated_iterator.map(&mut rewrite);
        self.exception_value = self.exception_value.map(&mut rewrite);
        self.pending_exception = self.pending_exception.map(&mut rewrite);
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
        for iter in &mut self.for_in_iters {
            if iter.is_null() {
                continue;
            }
            // SAFETY: for_in_iters 存放由当前 VM epoch 拥有的存活迭代器指针。
            for (v, _si) in unsafe { (*(*iter)).keys.iter_mut() } {
                *v = rewrite(*v);
            }
        }
    }

    /// 深拷贝（promote / sweep 搬移用，替代 async/asyncgen 各自的 clone_*_with_rewrite）。
    pub fn clone_with_rewrite(&self, mut rewrite: impl FnMut(JsValue) -> JsValue) -> Self {
        SuspendedFrame {
            regs: Box::new({
                let mut regs = [JsValue::undefined(); 256];
                for (i, v) in self.regs.iter().enumerate() {
                    regs[i] = rewrite(*v);
                }
                regs
            }),
            pc: self.pc,
            bytecode: self.bytecode.clone(),
            sub_idx: self.sub_idx,
            active_reg_limit: self.active_reg_limit,
            root_reg_limit: self.root_reg_limit,
            frame: self.frame.as_ref().map(|f| CallFrame {
                return_addr: f.return_addr,
                function_name: f.function_name,
                caller_reg_limit: f.caller_reg_limit,
                caller_active_reg_limit: f.caller_active_reg_limit,
                saved_reg_offset: f.saved_reg_offset,
                spill_offset: f.spill_offset,
                arguments_base: f.arguments_base,
                arguments_count: f.arguments_count,
                saved_this: rewrite(f.saved_this),
                saved_new_target: rewrite(f.saved_new_target),
                callee: rewrite(f.callee),
                construct_result_reg: f.construct_result_reg,
                constructed_this: f.constructed_this.map(&mut rewrite),
                is_derived_constructor: f.is_derived_constructor,
                super_called: f.super_called,
                strict: f.strict,
                continuation: f.continuation,
            }),
            spill_stack: self.spill_stack.iter().copied().map(&mut rewrite).collect(),
            save_stack: self.save_stack.iter().copied().map(&mut rewrite).collect(),
            cell_stack: self.cell_stack.clone(),
            try_stack: self.try_stack.clone(),
            for_in_iters: self.for_in_iters.clone(),
            for_of_iters: self
                .for_of_iters
                .iter()
                .copied()
                .map(|entry| ForOfEntry {
                    iterator: rewrite(entry.iterator),
                    last_result: rewrite(entry.last_result),
                    is_async: entry.is_async,
                })
                .collect(),
            delegated_iterator: self.delegated_iterator.map(&mut rewrite),
            saved_bytecode_stack: self.saved_bytecode_stack.clone(),
            saved_immutables_stack: self.saved_immutables_stack.clone(),
            exception_value: self.exception_value.map(&mut rewrite),
            pending_exception: self.pending_exception.map(&mut rewrite),
            pending_error_kind: self.pending_error_kind,
            pending_completion: self.pending_completion.map(|completion| match completion {
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
            }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{FrameContinuation, Vm};
    use std::collections::HashSet;

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
        }
    }

    fn fill_frame(frame: &mut SuspendedFrame) {
        frame.regs[0] = JsValue::float(1.0);
        frame.regs[7] = JsValue::float(2.0);
        frame.pc = 11;
        frame.bytecode = Arc::from(vec![0u32]);
        frame.sub_idx = 3;
        frame.active_reg_limit = 4;
        frame.root_reg_limit = 5;
        frame.frame = Some(sentinel_frame(JsValue::float(3.0)));
        frame.spill_stack.push(JsValue::float(4.0));
        frame.save_stack.push(JsValue::float(5.0));
        frame.cell_stack.push(Vec::new());
        frame.try_stack.push(TryHandler {
            catch_pc: None,
            finally_pc: Some(2),
            finally_active: false,
            frame_depth: 0,
            for_of_depth: 0,
        });
        frame.for_of_iters.push(ForOfEntry {
            iterator: JsValue::float(6.0),
            last_result: JsValue::float(7.0),
            is_async: false,
        });
        frame.delegated_iterator = Some(JsValue::float(8.0));
        frame.saved_bytecode_stack.push(Arc::from(vec![0u32]));
        frame
            .saved_immutables_stack
            .push(std::ptr::slice_from_raw_parts(std::ptr::null(), 0));
        frame.exception_value = Some(JsValue::float(9.0));
        frame.pending_exception = Some(JsValue::float(10.0));
        frame.pending_error_kind = Some("Error");
        frame.pending_completion = Some(Completion::Return {
            value: JsValue::float(11.0),
            remaining_finally: 0,
            for_of_count: 0,
            for_in_count: 0,
        });
    }

    #[test]
    fn for_each_rewrite_cover_same_fields() {
        let mut frame = SuspendedFrame::new_empty();
        fill_frame(&mut frame);

        // for_each_value 与 rewrite_values 必须覆盖同一组字段：先统计 for_each 访问
        // 的位点数，再把 rewrite 访问的每个位点改写为唯一值，最后 for_each 收集应
        // 恰好产出全部唯一值——若两侧字段不对称则计数不等。
        let mut for_each_count = 0usize;
        frame.for_each_value(|_| for_each_count += 1);

        let mut rewrite_count = 0usize;
        let mut n = 1000.0f64;
        frame.rewrite_values(|_v| {
            rewrite_count += 1;
            n += 1.0;
            JsValue::float(n)
        });
        assert_eq!(rewrite_count, for_each_count);

        let mut collected = HashSet::new();
        frame.for_each_value(|v| {
            collected.insert(v.to_bits());
        });
        assert_eq!(collected.len(), rewrite_count);
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

        // sub_modules 为空表：恢复应报"跨 run"错（保持现有三处行为）。
        let mut machine2 = vm();
        let subs = std::sync::Arc::new(vec![]);
        let mut frame2 = SuspendedFrame::new_empty();
        frame2.sub_idx = 0;
        let err = frame2.restore_into(&mut machine2, &subs);
        assert!(err.is_err());
    }
}
