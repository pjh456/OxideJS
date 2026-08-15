use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::CompiledModule;

use crate::vm::{CallFrame, FrameContinuation, InlineSyncState, Vm};
use crate::{vm_debug, vm_info, vm_trace, vm_warn};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// 按 `flat_id` 下标收集整棵子模块树为平表，供 `run()` 装载。
///
/// 子模块节点以 `Arc` 与调用方模块树共享：只做 Arc::clone（O(1) 引用计数），
/// 不再深拷贝 constants/upvalue_captures 等。顶层模块本身以浅拷贝包 Arc——
/// 其自有 constants 仍逐 run 复制，但整棵子树零深拷贝。
fn collect_flat_modules(module: &CompiledModule) -> Vec<Arc<CompiledModule>> {
    let mut out: Vec<Option<Arc<CompiledModule>>> = Vec::new();
    let top = Arc::new(module.clone());
    place_flat(&top, &mut out);
    out.into_iter().map(|m| m.expect("flat_id slot must be filled")).collect()
}

fn place_flat(module: &Arc<CompiledModule>, out: &mut Vec<Option<Arc<CompiledModule>>>) {
    let idx = module.flat_id as usize;
    if idx >= out.len() {
        out.resize(idx + 1, None);
    }
    out[idx] = Some(Arc::clone(module));
    for sub in &module.sub_modules {
        place_flat(sub, out);
    }
}

/// save 方向的字段取值表达式（字段名由调用方以 ident 位置提供，本宏只产出值）。
/// `regs` 取窗口副本：值来自 `save_inline_state` 预填充的 `window_regs` 局部缓冲
/// （经宏参数注入，绕开 macro hygiene），从池中取出后即 move 进 `InlineSyncState`。
macro_rules! inline_save_field {
    ($recv:ident, $window_regs:ident, regs, boxed_window) => { std::mem::take(&mut $window_regs).into_boxed_slice() };
    ($recv:ident, $window_regs:ident, saved_this, copy) => { $recv.regs[254] };
    ($recv:ident, $window_regs:ident, saved_new_target, copy) => { $recv.regs[255] };
    ($recv:ident, $window_regs:ident, pc, copy) => { $recv.pc };
    ($recv:ident, $window_regs:ident, bytecode, move_field) => { std::mem::take(&mut $recv.bytecode) };
    ($recv:ident, $window_regs:ident, active_immutables, copy) => { $recv.active_immutables };
    ($recv:ident, $window_regs:ident, active_reg_limit, copy) => { $recv.active_reg_limit };
    ($recv:ident, $window_regs:ident, root_reg_limit, copy) => { $recv.root_reg_limit };
    ($recv:ident, $window_regs:ident, try_stack, move_field) => { std::mem::take(&mut $recv.try_stack) };
    ($recv:ident, $window_regs:ident, frames, frames_values) => { std::mem::take(&mut $recv.frames) };
    ($recv:ident, $window_regs:ident, exception_value, opt_take) => { $recv.exception_value.take() };
    ($recv:ident, $window_regs:ident, pending_exception, opt_take) => { $recv.pending_exception.take() };
    ($recv:ident, $window_regs:ident, pending_error_kind, opt_take) => { $recv.pending_error_kind.take() };
    ($recv:ident, $window_regs:ident, pending_completion, opt_copy) => { $recv.pending_completion };
    ($recv:ident, $window_regs:ident, for_in_iters, for_in_keys) => { std::mem::take(&mut $recv.iters.for_in_iters) };
    ($recv:ident, $window_regs:ident, for_of_iters, iter_take) => { std::mem::take(&mut $recv.iters.for_of_iters) };
    ($recv:ident, $window_regs:ident, last_for_of_result, iter_copy) => { $recv.iters.last_for_of_result };
    ($recv:ident, $window_regs:ident, saved_bytecode_stack, move_field) => { std::mem::take(&mut $recv.saved_bytecode_stack) };
    ($recv:ident, $window_regs:ident, saved_immutables_stack, move_field) => { std::mem::take(&mut $recv.saved_immutables_stack) };
    ($recv:ident, $window_regs:ident, save_stack, move_field) => { std::mem::take(&mut $recv.save_stack) };
    ($recv:ident, $window_regs:ident, spill_stack, move_field) => { std::mem::take(&mut $recv.spill_stack) };
    ($recv:ident, $window_regs:ident, cell_stack, move_field) => { std::mem::take(&mut $recv.cell_stack) };
    ($recv:ident, $window_regs:ident, inline_callee, opt_copy) => { $recv.inline_callee };
    ($recv:ident, $window_regs:ident, inline_args_base, copy) => { $recv.inline_args_base };
    ($recv:ident, $window_regs:ident, inline_args_count, copy) => { $recv.inline_args_count };
    ($recv:ident, $window_regs:ident, accessor_frame_target_reg, copy) => { $recv.accessor_frame_target_reg };
}

/// restore 方向的字段写回语句。`regs` 只回拷窗口并把缓冲归还池。
macro_rules! inline_restore_field {
    ($recv:ident, $saved:ident, regs, boxed_window) => {
        $recv.regs[..$saved.regs.len()].copy_from_slice(&$saved.regs);
        $recv.inline_reg_pool = Some($saved.regs.into_vec());
    };
    ($recv:ident, $saved:ident, saved_this, copy) => { $recv.regs[254] = $saved.saved_this };
    ($recv:ident, $saved:ident, saved_new_target, copy) => { $recv.regs[255] = $saved.saved_new_target };
    ($recv:ident, $saved:ident, pc, copy) => { $recv.pc = $saved.pc };
    ($recv:ident, $saved:ident, bytecode, move_field) => { $recv.bytecode = $saved.bytecode };
    ($recv:ident, $saved:ident, active_immutables, copy) => { $recv.active_immutables = $saved.active_immutables };
    ($recv:ident, $saved:ident, active_reg_limit, copy) => { $recv.active_reg_limit = $saved.active_reg_limit };
    ($recv:ident, $saved:ident, root_reg_limit, copy) => { $recv.root_reg_limit = $saved.root_reg_limit };
    ($recv:ident, $saved:ident, try_stack, move_field) => { $recv.try_stack = $saved.try_stack };
    ($recv:ident, $saved:ident, frames, frames_values) => { $recv.frames = $saved.frames };
    ($recv:ident, $saved:ident, exception_value, opt_take) => { $recv.exception_value = $saved.exception_value };
    ($recv:ident, $saved:ident, pending_exception, opt_take) => { $recv.pending_exception = $saved.pending_exception };
    ($recv:ident, $saved:ident, pending_error_kind, opt_take) => { $recv.pending_error_kind = $saved.pending_error_kind };
    ($recv:ident, $saved:ident, pending_completion, opt_copy) => { $recv.pending_completion = $saved.pending_completion };
    ($recv:ident, $saved:ident, for_in_iters, for_in_keys) => { $recv.iters.for_in_iters = $saved.for_in_iters };
    ($recv:ident, $saved:ident, for_of_iters, iter_take) => { $recv.iters.for_of_iters = $saved.for_of_iters };
    ($recv:ident, $saved:ident, last_for_of_result, iter_copy) => { $recv.iters.last_for_of_result = $saved.last_for_of_result };
    ($recv:ident, $saved:ident, saved_bytecode_stack, move_field) => { $recv.saved_bytecode_stack = $saved.saved_bytecode_stack };
    ($recv:ident, $saved:ident, saved_immutables_stack, move_field) => { $recv.saved_immutables_stack = $saved.saved_immutables_stack };
    ($recv:ident, $saved:ident, save_stack, move_field) => { $recv.save_stack = $saved.save_stack };
    ($recv:ident, $saved:ident, spill_stack, move_field) => { $recv.spill_stack = $saved.spill_stack };
    ($recv:ident, $saved:ident, cell_stack, move_field) => { $recv.cell_stack = $saved.cell_stack };
    ($recv:ident, $saved:ident, inline_callee, opt_copy) => { $recv.inline_callee = $saved.inline_callee };
    ($recv:ident, $saved:ident, inline_args_base, copy) => { $recv.inline_args_base = $saved.inline_args_base };
    ($recv:ident, $saved:ident, inline_args_count, copy) => { $recv.inline_args_count = $saved.inline_args_count };
    ($recv:ident, $saved:ident, accessor_frame_target_reg, copy) => { $recv.accessor_frame_target_reg = $saved.accessor_frame_target_reg };
}

/// 内联同步调用可搬移执行核心字段的单一登记表。save/restore 双向由本宏展开；
/// 新增字段只加一行（字段名, 操作符）。注释 = 该字段语义（M 搬移 / V 含 JsValue）。
/// `$window_regs` 只在 save 方向被 `regs` 字段消费。
macro_rules! inline_core_fields {
    ($recv:ident, $saved:ident, $ops:ident, $window_regs:ident) => {
        $ops!($recv, $saved, $window_regs,
            (regs, boxed_window), // V M
            (saved_this, copy), // V
            (saved_new_target, copy), // V
            (pc, copy), // M
            (bytecode, move_field), // V M
            (active_immutables, copy), // M
            (active_reg_limit, copy), // M
            (root_reg_limit, copy), // M
            (try_stack, move_field), // M
            (frames, frames_values), // V M
            (exception_value, opt_take), // V M
            (pending_exception, opt_take), // V M
            (pending_error_kind, opt_take), // M
            (pending_completion, opt_copy), // V M
            (for_in_iters, for_in_keys), // V M
            (for_of_iters, iter_take), // V M
            (last_for_of_result, iter_copy), // V M
            (saved_bytecode_stack, move_field), // M
            (saved_immutables_stack, move_field), // M
            (save_stack, move_field), // V M
            (spill_stack, move_field), // V M
            (cell_stack, move_field), // V M
            (inline_callee, opt_copy), // V M
            (inline_args_base, copy), // M
            (inline_args_count, copy), // M
            (accessor_frame_target_reg, copy), // M
        )
    };
}

/// 把字段表展开为 `InlineSyncState` 结构体字面量（字段名在 ident 位置，值由
/// `inline_save_field` 产出）。
macro_rules! inline_save {
    ($recv:ident, $saved:ident, $window_regs:ident, $(($field:ident, $op:ident)),* $(,)?) => {
        InlineSyncState {
            $($field: inline_save_field!($recv, $window_regs, $field, $op)),*
        }
    };
}

/// 把字段表展开为写回语句序列。
macro_rules! inline_restore {
    ($recv:ident, $saved:ident, $window_regs:ident, $(($field:ident, $op:ident)),* $(,)?) => {
        { $(inline_restore_field!($recv, $saved, $field, $op);)* }
    };
}

impl Vm {
    /// 保存当前 VM 执行状态到堆上（内联同步调用与生成器恢复共用）。
    ///
    /// 寄存器只保存窗口 `regs[0..min(regs_end, 254)]` 的副本，`regs[254]/[255]`
    /// 单独存入 `saved_this`/`saved_new_target`。窗口缓冲取自 `inline_reg_pool`
    /// 复用，热回调循环内零分配；嵌套时池为空则新分配。
    ///
    /// # 边界与前提
    /// - `regs_end` ≤ 254 表示窗口化保存；传 256（全量）等价于保存全部寄存器
    ///   （254 个通用槽 0..=253 + 254/255 单独存）。
    /// - 调用方保证 `regs_end ≤ 256`；窗口上限 254 覆盖 callee 写入集
    ///   （`n_registers ≤ 254`，含 RegAlloc 最高合法物理槽 253）。
    pub(crate) fn save_inline_state(&mut self, regs_end: usize) -> Box<InlineSyncState> {
        vm_trace!("save_inline_state: pc={} depth={}", self.pc, self.frames.len());
        let window = regs_end.min(254);
        let mut window_regs = self.inline_reg_pool.take().unwrap_or_default();
        window_regs.clear();
        window_regs.extend_from_slice(&self.regs[..window]);
        let vm = self;
        Box::new(inline_core_fields!(vm, vm, inline_save, window_regs))
    }

    /// 把 [`save_inline_state`] 保存的状态恢复回 VM。窗口回拷 + `regs[254]/[255]`
    /// 单回，窗口外寄存器 callee 未触碰无需恢复。
    pub(crate) fn restore_inline_state(&mut self, saved: Box<InlineSyncState>) {
        vm_trace!("restore_inline_state: pc={}", saved.pc);
        let vm = self;
        let _window_regs: Vec<JsValue> = Vec::new();
        inline_core_fields!(vm, saved, inline_restore, _window_regs);
    }

    /// 首次执行的 VM 就绪：清空执行核心（regs/pc/bytecode/各栈段/迭代器/内联态），
    /// 压入首帧。参数初始化步与 async/gen 调度标志由调用方管理。
    ///
    /// # 边界
    /// callee 非函数 / sub_idx 越界 / 超调用深度时由 push_bytecode_frame 返回 Err。
    pub(crate) fn prepare_execution_initial(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue],
    ) -> Result<(), String> {
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode = Arc::default();
        self.active_reg_limit = 1;
        self.root_reg_limit = 1;
        self.try_stack.clear();
        self.iters.for_in_iters.clear();
        self.iters.for_of_iters.clear();
        self.iters.last_for_of_result = JsValue::undefined();
        self.spill_stack.clear();
        self.save_stack.clear();
        self.saved_bytecode_stack.clear();
        self.saved_immutables_stack.clear();
        self.cell_stack.clear();
        self.inline_callee = None;
        self.push_bytecode_frame(
            callee,
            this_value,
            args,
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::None,
            0,
        )
    }

    pub(crate) fn call_bytecode_function_inline(
        &mut self, callee: JsValue, callee_obj: &JsObject, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        if callee_obj.sub_module_index() == 0 {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }
        let sub_idx = callee_obj.sub_module_index() as usize;
        vm_debug!(
            "call_bytecode_function_inline: sub_idx={}, args={}, depth={}",
            sub_idx,
            args.len(),
            self.frames.len()
        );
        if sub_idx >= self.sub_modules.len() {
            return Err(format!(
                "accessor sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        if self.frames.len() >= self.kernel_core.config.max_call_depth {
            self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
            return Ok(JsValue::undefined());
        }
        if self.native_call_depth >= self.kernel_core.config.max_call_depth {
            self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
            return Ok(JsValue::undefined());
        }
        // 异步生成器函数调用返回异步生成器迭代器对象，next/return/throw 返回 Promise。
        if self.sub_modules[sub_idx].is_generator && self.sub_modules[sub_idx].is_async {
            return self.create_async_generator_object(callee, receiver, args);
        }
        // 生成器函数调用返回迭代器对象，不执行函数体。
        if self.sub_modules[sub_idx].is_generator {
            return self.create_generator_object(callee, receiver, args);
        }
        // 异步函数调用返回 capability promise，立即同步执行 body 到首个 await。
        if self.sub_modules[sub_idx].is_async {
            return self.create_async_object(callee, receiver, args);
        }
        self.native_call_depth += 1;

        let subs = Arc::clone(&self.sub_modules);
        let sub = &subs[sub_idx];

        vm_trace!("call_bytecode: saving state pc={} depth={}", self.pc, self.frames.len());
        // 窗口 = max(调用方活跃寄存器, callee 寄存器数)：restore 只回拷窗口，正好
        // 覆盖 callee 写入区（regs[0..n_registers] + 254/255）与调用方活动区。
        let window = self.active_reg_limit.max(sub.n_registers).max(1) as usize;
        let saved = self.save_inline_state(window);

        // 只零填 callee 读区 regs[0..n_registers]；高位残留调用方旧值无害
        // （callee 字节码只访问自身 n_registers 内）。
        for r in 0..sub.n_registers as usize {
            self.regs[r] = JsValue::undefined();
        }
        self.pc = 0;
        self.bytecode = Arc::clone(&sub.bytecode);
        self.activate_immutables(sub_idx, &sub.constants);
        self.active_reg_limit = sub.n_registers.max(1);
        self.root_reg_limit = self.active_reg_limit;
        self.cell_stack.push(Vec::with_capacity(sub.cells_needed as usize));
        self.inline_callee = Some(callee);
        // inline 同步调用无 CallFrame：完整实参写入 spill 栈实参区，供 CREATE_ARGUMENTS 读取。
        self.inline_args_base = self.spill_stack.len() as u32;
        self.spill_stack.extend_from_slice(args);
        self.inline_args_count = args.len().min(u16::MAX as usize) as u16;
        for i in 0..sub.n_args as usize {
            self.regs[sub.param_base as usize + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        self.regs[254] = if sub.is_arrow { callee_obj.captured_this() } else { receiver };
        self.regs[255] = JsValue::undefined();
        for (name, reg) in &sub.builtin_reg_map {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let global = self.session.global_object();
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }
        let _ = callee;

        vm_trace!(
            "call_bytecode: dispatching sub_idx={} n_regs={} n_args={} arrow={}",
            sub_idx,
            sub.n_registers,
            sub.n_args,
            sub.is_arrow
        );
        let result = self.dispatch();
        self.native_call_depth -= 1;

        vm_trace!("call_bytecode: restoring state pc={} result={:?}", saved.pc, result.as_ref().ok());
        self.restore_inline_state(saved);
        result
    }

    pub(crate) fn restore_frame(&mut self, frame: CallFrame) {
        vm_trace!(
            "restore_frame: return_addr={} caller_reg_limit={}",
            frame.return_addr,
            frame.caller_reg_limit
        );
        if let Some(saved_bc) = self.saved_bytecode_stack.pop() {
            self.bytecode = saved_bc;
        }
        vm_debug!(
            "restore_frame: return_addr={} bc_len={} saved_stack={} fn={:?}",
            frame.return_addr,
            self.bytecode.len(),
            self.saved_bytecode_stack.len(),
            self.kernel_core
                .perm_interner()
                .lookup(frame.function_name)
                .map(|s| s.to_string())
        );
        if let Some(saved_imm) = self.saved_immutables_stack.pop() {
            self.active_immutables = saved_imm;
        }
        let offset = frame.saved_reg_offset as usize;
        let len = frame.caller_reg_limit as usize;
        self.regs[..len].copy_from_slice(&self.save_stack[offset..offset + len]);
        self.save_stack.truncate(offset);
        // spill 帧边界截断：子函数写入的 spill 数据在帧恢复后丢弃。
        self.spill_stack.truncate(frame.spill_offset as usize);
        self.regs[254] = frame.saved_this;
        self.regs[255] = frame.saved_new_target;
        // active_reg_limit 还原为调用方真实值（caller_reg_limit 可能被存活上界截断）。
        self.active_reg_limit = frame.caller_active_reg_limit;
        self.pc = frame.return_addr;
    }

    /// 重新执行当前已加载的 bytecode：清空执行状态并重置 IC 缓存后再次 dispatch。
    pub fn rerun(&mut self) -> Result<JsValue, String> {
        vm_info!("rerun: clearing IC caches");
        self.clear_execution_state();
        self.active_reg_limit = self.root_reg_limit;
        crate::ic_helper::clear_ic_caches(self.bytecode_mut());
        self.dispatch()
    }

    /// 加载并执行一个已编译模块，返回模块顶层执行结果或未捕获异常消息。
    ///
    /// 内部初始化寄存器/bytecode/immutables 与 builtin 寄存器预绑定，然后进入
    /// dispatch 主循环；执行完成或异常展开后返回。
    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        vm_debug!("run: starting bytecode execution, {} instructions", module.bytecode.len());
        self.clear_execution_state();
        self.cell_stack.clear();
        self.cell_stack.push(Vec::new());
        self.sub_modules = Arc::new(collect_flat_modules(module));
        self.immutables_cache = (0..self.sub_modules.len()).map(|_| OnceLock::new()).collect();
        self.bytecode = Arc::clone(&module.bytecode);
        self.activate_immutables(0, &module.constants);
        self.root_reg_limit = module.n_registers.max(1);
        self.active_reg_limit = self.root_reg_limit;

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let global = self.session.global_object();
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        // 顶层 this：脚本为全局对象（ECMA-262 全局执行上下文）；
        // ES module 顶层环境 GetThisBinding 返回 undefined。
        let global = self.session.global_object();
        self.regs[254] = if module.is_es_module {
            JsValue::undefined()
        } else {
            JsValue::from_js_object(global.as_ptr() as *mut JsObject)
        };

        let result = self.dispatch();
        // 顶层执行结束后 drain 微任务队列：Promise reactions 与 thenable 委托在此执行。
        self.drain_job_queue();
        result
    }

    pub(crate) fn unwind(&mut self) -> Result<(), String> {
        vm_debug!("unwind: {} try handlers on stack", self.try_stack.len());
        while let Some(mut handler) = self.try_stack.pop() {
            if handler.frame_depth > self.frames.len() {
                // 残留 handler 所属帧已返回（return 逃出泄漏的兜底）：丢弃防跳回
                // 死函数的 catch/finally——否则 catch 再抛会反复弹出同一 handler
                // 形成死循环。其 for_of_depth 快照属于已死帧，也不可据此关闭
                // 当前帧的迭代器，故整体跳过。
                continue;
            }
            while self.frames.len() > handler.frame_depth {
                if let Some(frame) = self.frames.pop() {
                    self.cell_stack.pop();
                    self.restore_frame(frame);
                }
            }
            // 对该处理器作用域内被中断的 for-of 循环执行 IteratorClose。
            self.close_for_of_above(handler.for_of_depth);
            if let Some(finally_pc) = handler.finally_pc {
                if handler.finally_active {
                    // 新异常在 finally 体内抛出：覆盖在途异常与完成，继续向外展开，
                    // 不再重入本 finally（否则已执行的 finally 会重复运行）。
                    self.pending_exception = None;
                    self.pending_error_kind = None;
                    self.pending_completion = None;
                    continue;
                }
                vm_trace!("unwind: entering finally at pc={}", finally_pc);
                self.pending_exception = Some(self.exception_value.take().unwrap_or(JsValue::undefined()));
                handler.finally_active = true;
                self.try_stack.push(handler);
                self.pc = finally_pc;
                return Ok(());
            }
            if let Some(catch_pc) = handler.catch_pc {
                vm_trace!("unwind: caught at pc={}", catch_pc);
                let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
                self.regs[0] = exc;
                self.pc = catch_pc;
                return Ok(());
            }
        }
        self.close_for_of_above(0);
        while let Some(frame) = self.frames.pop() {
            self.cell_stack.pop();
            self.restore_frame(frame);
        }
        let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
        // 保留逃逸的 JsValue，使 for-of 的 next() 抛出时可重新抛出原值而非展平后的
        // 字符串（由 dispatch_for_of_done/next 消费）。
        self.last_uncaught_value = Some(exc);
        let kind_str = self.pending_error_kind.take().unwrap_or("Error");
        let exc_text = self.error_text(exc);
        let msg = if exc_text.starts_with(kind_str) {
            format!("uncaught {exc_text}")
        } else {
            format!("uncaught {kind_str}: {exc_text}")
        };
        vm_warn!("unwind: uncaught {}: {}", kind_str, exc_text);
        Err(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::Completion;
    use oxide_types::value::JsValue;

    #[test]
    fn save_restore_inline_round_trip() {
        // 构造带哨兵的 VM → save → 恢复 → 断言全部字段逐字往返（含 accessor_frame_target_reg）。
        let mut vm = Vm::new();
        vm.regs[3] = JsValue::float(1.0);
        vm.regs[254] = JsValue::float(254.0);
        vm.regs[255] = JsValue::float(255.0);
        vm.pc = 7;
        vm.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        vm.active_reg_limit = 4;
        vm.root_reg_limit = 5;
        vm.exception_value = Some(JsValue::float(6.0));
        vm.pending_exception = Some(JsValue::float(7.0));
        vm.pending_error_kind = Some("TypeError");
        vm.pending_completion = Some(Completion::Return {
            value: JsValue::float(8.0),
            remaining_finally: 1,
        });
        vm.iters.last_for_of_result = JsValue::float(9.0);
        vm.spill_stack.push(JsValue::float(10.0));
        vm.save_stack.push(JsValue::float(11.0));
        vm.inline_callee = Some(JsValue::float(12.0));
        vm.inline_args_base = 2;
        vm.inline_args_count = 3;
        vm.accessor_frame_target_reg = Some(9);

        let saved = vm.save_inline_state(256);
        vm.regs[3] = JsValue::float(99.0);
        vm.accessor_frame_target_reg = None;
        vm.restore_inline_state(saved);

        assert_eq!(vm.regs[3], JsValue::float(1.0));
        assert_eq!(vm.regs[254], JsValue::float(254.0));
        assert_eq!(vm.regs[255], JsValue::float(255.0));
        assert_eq!(vm.pc, 7);
        assert_eq!(vm.active_reg_limit, 4);
        assert_eq!(vm.root_reg_limit, 5);
        assert_eq!(vm.exception_value, Some(JsValue::float(6.0)));
        assert_eq!(vm.pending_exception, Some(JsValue::float(7.0)));
        assert_eq!(vm.pending_error_kind, Some("TypeError"));
        assert!(matches!(
            vm.pending_completion,
            Some(Completion::Return { value, remaining_finally: 1 }) if value == JsValue::float(8.0)
        ));
        assert_eq!(vm.iters.last_for_of_result, JsValue::float(9.0));
        assert_eq!(vm.spill_stack, vec![JsValue::float(10.0)]);
        assert_eq!(vm.save_stack, vec![JsValue::float(11.0)]);
        assert_eq!(vm.inline_callee, Some(JsValue::float(12.0)));
        assert_eq!(vm.inline_args_base, 2);
        assert_eq!(vm.inline_args_count, 3);
        assert_eq!(vm.accessor_frame_target_reg, Some(9));
    }

    #[test]
    fn save_restore_inline_window_254_preserves_r253() {
        // 窗口化路径边界：调用方 active_reg_limit = 254 时窗口覆盖物理槽 253
        // （RegAlloc 最高合法色），save/restore 必须往返 regs[253]，且与 254/255
        // 单存槽位互不重叠。
        let mut vm = Vm::new();
        vm.regs[253] = JsValue::float(253.0);
        vm.regs[254] = JsValue::float(254.0);
        vm.regs[255] = JsValue::float(255.0);

        let saved = vm.save_inline_state(254);
        vm.regs[253] = JsValue::float(99.0);
        vm.regs[254] = JsValue::float(98.0);
        vm.regs[255] = JsValue::float(97.0);
        vm.restore_inline_state(saved);

        assert_eq!(vm.regs[253], JsValue::float(253.0));
        assert_eq!(vm.regs[254], JsValue::float(254.0));
        assert_eq!(vm.regs[255], JsValue::float(255.0));
    }
}
