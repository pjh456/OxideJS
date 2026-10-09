use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode::OpCode;

use crate::vm::{CallFrame, FrameArgs, FrameContinuation, InlineSyncState, TableGen, Vm};
use crate::{vm_debug, vm_info, vm_trace, vm_warn};
use oxide_types::object::{Cell, JsObject};
use oxide_types::value::JsValue;

/// 按 `flat_id` 下标收集整棵子模块树为平表，供 `run()` 装载。
///
/// 顶层与子模块一律 `Arc` 共享：只做 Arc::clone（O(1) 引用计数），run 期
/// 零模块拷贝。唯一可写面 = bytecode 缓冲，经 `bytecode_mut` 的 COW 写时
/// 复制——dispatch 期平表与 `Vm.bytecode` 双持（refcount ≥ 2），写必落在
/// 私有拷贝上，宿主侧共享缓冲永不被写。
fn collect_flat_modules(module: &Arc<CompiledModule>) -> Vec<Arc<CompiledModule>> {
    let mut out: Vec<Option<Arc<CompiledModule>>> = Vec::new();
    let top = Arc::clone(module);
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
    ($recv:ident, $window_regs:ident, regs, boxed_window) => {
        std::mem::take(&mut $window_regs).into_boxed_slice()
    };
    ($recv:ident, $window_regs:ident, saved_this, copy) => {
        $recv.regs[254]
    };
    ($recv:ident, $window_regs:ident, saved_new_target, copy) => {
        $recv.regs[255]
    };
    ($recv:ident, $window_regs:ident, pc, copy) => {
        $recv.pc
    };
    ($recv:ident, $window_regs:ident, bytecode, move_field) => {
        std::mem::take(&mut $recv.bytecode)
    };
    ($recv:ident, $window_regs:ident, active_immutables, copy) => {
        $recv.active_immutables
    };
    ($recv:ident, $window_regs:ident, active_reg_limit, copy) => {
        $recv.active_reg_limit
    };
    ($recv:ident, $window_regs:ident, root_reg_limit, copy) => {
        $recv.root_reg_limit
    };
    ($recv:ident, $window_regs:ident, try_stack, move_field) => {
        std::mem::take(&mut $recv.try_stack)
    };
    ($recv:ident, $window_regs:ident, frames, frames_values) => {
        std::mem::take(&mut $recv.frames)
    };
    ($recv:ident, $window_regs:ident, exception_value, opt_take) => {
        $recv.exception_value.take()
    };
    ($recv:ident, $window_regs:ident, pending_exception, opt_take) => {
        $recv.pending_exception.take()
    };
    ($recv:ident, $window_regs:ident, pending_error_kind, opt_take) => {
        $recv.pending_error_kind.take()
    };
    ($recv:ident, $window_regs:ident, pending_completion, opt_copy) => {
        $recv.pending_completion
    };
    ($recv:ident, $window_regs:ident, for_in_iters, for_in_keys) => {
        std::mem::take(&mut $recv.iters.for_in_iters)
    };
    ($recv:ident, $window_regs:ident, for_of_iters, iter_take) => {
        std::mem::take(&mut $recv.iters.for_of_iters)
    };
    ($recv:ident, $window_regs:ident, saved_bytecode_stack, move_field) => {
        std::mem::take(&mut $recv.saved_bytecode_stack)
    };
    ($recv:ident, $window_regs:ident, saved_immutables_stack, move_field) => {
        std::mem::take(&mut $recv.saved_immutables_stack)
    };
    ($recv:ident, $window_regs:ident, save_stack, move_field) => {
        std::mem::take(&mut $recv.save_stack)
    };
    ($recv:ident, $window_regs:ident, spill_stack, move_field) => {
        std::mem::take(&mut $recv.spill_stack)
    };
    ($recv:ident, $window_regs:ident, cell_stack, move_field) => {
        std::mem::take(&mut $recv.cell_stack)
    };
    ($recv:ident, $window_regs:ident, inline_callee, opt_copy) => {
        $recv.inline_callee
    };
    ($recv:ident, $window_regs:ident, active_upvalues, copy) => {
        $recv.active_upvalues
    };
    ($recv:ident, $window_regs:ident, generator_dispatch, flag_zero) => {{
        let prev = $recv.generator_dispatch;
        $recv.generator_dispatch = false;
        prev
    }};
    ($recv:ident, $window_regs:ident, async_dispatch, flag_zero) => {{
        let prev = $recv.async_dispatch;
        $recv.async_dispatch = false;
        prev
    }};
    ($recv:ident, $window_regs:ident, construct_dispatch, flag_zero) => {{
        let prev = $recv.construct_dispatch;
        $recv.construct_dispatch = false;
        prev
    }};
    ($recv:ident, $window_regs:ident, inline_strict, copy) => {
        $recv.inline_strict
    };
    ($recv:ident, $window_regs:ident, inline_frames_base, copy) => {
        $recv.inline_frames_base
    };
    ($recv:ident, $window_regs:ident, inline_args_base, copy) => {
        $recv.inline_args_base
    };
    ($recv:ident, $window_regs:ident, inline_args_count, copy) => {
        $recv.inline_args_count
    };
    ($recv:ident, $window_regs:ident, accessor_frame_target_reg, copy) => {
        $recv.accessor_frame_target_reg
    };
    ($recv:ident, $window_regs:ident, active_flat_id, copy) => {
        $recv.active_flat_id
    };
    ($recv:ident, $window_regs:ident, active_table_gen, copy) => {
        $recv.active_table_gen
    };
}

/// restore 方向的字段写回语句。`regs` 只回拷窗口并把缓冲归还池。
macro_rules! inline_restore_field {
    ($recv:ident, $saved:ident, regs, boxed_window) => {
        $recv.regs[..$saved.regs.len()].copy_from_slice(&$saved.regs);
        $recv.inline_reg_pool = Some(std::mem::take(&mut $saved.regs).into_vec());
    };
    ($recv:ident, $saved:ident, saved_this, copy) => {
        $recv.regs[254] = $saved.saved_this
    };
    ($recv:ident, $saved:ident, saved_new_target, copy) => {
        $recv.regs[255] = $saved.saved_new_target
    };
    ($recv:ident, $saved:ident, pc, copy) => {
        $recv.pc = $saved.pc
    };
    ($recv:ident, $saved:ident, bytecode, move_field) => {
        $recv.bytecode = std::mem::take(&mut $saved.bytecode)
    };
    ($recv:ident, $saved:ident, active_immutables, copy) => {
        $recv.active_immutables = $saved.active_immutables
    };
    ($recv:ident, $saved:ident, active_reg_limit, copy) => {
        $recv.active_reg_limit = $saved.active_reg_limit
    };
    ($recv:ident, $saved:ident, root_reg_limit, copy) => {
        $recv.root_reg_limit = $saved.root_reg_limit
    };
    ($recv:ident, $saved:ident, try_stack, move_field) => {
        $recv.try_stack = std::mem::take(&mut $saved.try_stack)
    };
    ($recv:ident, $saved:ident, frames, frames_values) => {
        $recv.frames = std::mem::take(&mut $saved.frames)
    };
    ($recv:ident, $saved:ident, exception_value, opt_take) => {
        $recv.exception_value = $saved.exception_value
    };
    ($recv:ident, $saved:ident, pending_exception, opt_take) => {
        $recv.pending_exception = $saved.pending_exception
    };
    ($recv:ident, $saved:ident, pending_error_kind, opt_take) => {
        $recv.pending_error_kind = $saved.pending_error_kind
    };
    ($recv:ident, $saved:ident, pending_completion, opt_copy) => {
        $recv.pending_completion = $saved.pending_completion
    };
    ($recv:ident, $saved:ident, for_in_iters, for_in_keys) => {
        // unwind 不弹 for-in 体：被调函数 for-in 异常逃出且未被接管时残留体留在
        // 表内，快照回写是整向量替换。先逐条释放残留体再搬回快照，堆 Box 恰好
        // 释放一次；for-of 栈随后由独立臂从快照还原，本臂顺带清位不影响终态。
        $recv.iters.reset();
        $recv.iters.for_in_iters = std::mem::take(&mut $saved.for_in_iters)
    };
    ($recv:ident, $saved:ident, for_of_iters, iter_take) => {
        $recv.iters.for_of_iters = std::mem::take(&mut $saved.for_of_iters)
    };
    ($recv:ident, $saved:ident, saved_bytecode_stack, move_field) => {
        $recv.saved_bytecode_stack = std::mem::take(&mut $saved.saved_bytecode_stack)
    };
    ($recv:ident, $saved:ident, saved_immutables_stack, move_field) => {
        $recv.saved_immutables_stack = std::mem::take(&mut $saved.saved_immutables_stack)
    };
    ($recv:ident, $saved:ident, save_stack, move_field) => {
        $recv.save_stack = std::mem::take(&mut $saved.save_stack)
    };
    ($recv:ident, $saved:ident, spill_stack, move_field) => {
        $recv.spill_stack = std::mem::take(&mut $saved.spill_stack)
    };
    ($recv:ident, $saved:ident, cell_stack, move_field) => {
        $recv.cell_stack = std::mem::take(&mut $saved.cell_stack)
    };
    ($recv:ident, $saved:ident, inline_callee, opt_copy) => {
        $recv.inline_callee = $saved.inline_callee
    };
    ($recv:ident, $saved:ident, active_upvalues, copy) => {
        $recv.active_upvalues = $saved.active_upvalues
    };
    ($recv:ident, $saved:ident, generator_dispatch, flag_zero) => {
        $recv.generator_dispatch = $saved.generator_dispatch
    };
    ($recv:ident, $saved:ident, async_dispatch, flag_zero) => {
        $recv.async_dispatch = $saved.async_dispatch
    };
    ($recv:ident, $saved:ident, construct_dispatch, flag_zero) => {
        $recv.construct_dispatch = $saved.construct_dispatch
    };
    ($recv:ident, $saved:ident, inline_strict, copy) => {
        $recv.inline_strict = $saved.inline_strict
    };
    ($recv:ident, $saved:ident, inline_frames_base, copy) => {
        $recv.inline_frames_base = $saved.inline_frames_base
    };
    ($recv:ident, $saved:ident, inline_args_base, copy) => {
        $recv.inline_args_base = $saved.inline_args_base
    };
    ($recv:ident, $saved:ident, inline_args_count, copy) => {
        $recv.inline_args_count = $saved.inline_args_count
    };
    ($recv:ident, $saved:ident, accessor_frame_target_reg, copy) => {
        $recv.accessor_frame_target_reg = $saved.accessor_frame_target_reg
    };
    ($recv:ident, $saved:ident, active_flat_id, copy) => {
        $recv.active_flat_id = $saved.active_flat_id
    };
    ($recv:ident, $saved:ident, active_table_gen, copy) => {
        $recv.active_table_gen = $saved.active_table_gen
    };
}

/// 内联同步调用可搬移执行核心字段的单一登记表。save/restore 双向由本宏展开；
/// 新增字段只加一行（字段名, 操作符）。注释 = 该字段语义（M 搬移 / V 含 JsValue）。
/// `$window_regs` 只在 save 方向被 `regs` 字段消费。
macro_rules! inline_core_fields {
    ($recv:ident, $saved:ident, $ops:ident, $window_regs:ident) => {
        $ops!(
            $recv,
            $saved,
            $window_regs,
            (regs, boxed_window),                 // V M
            (saved_this, copy),                   // V
            (saved_new_target, copy),             // V
            (pc, copy),                           // M
            (bytecode, move_field),               // V M
            (active_immutables, copy),            // M
            (active_reg_limit, copy),             // M
            (root_reg_limit, copy),               // M
            (try_stack, move_field),              // M
            (frames, frames_values),              // V M
            (exception_value, opt_take),          // V M
            (pending_exception, opt_take),        // V M
            (pending_error_kind, opt_take),       // M
            (pending_completion, opt_copy),       // V M
            (for_in_iters, for_in_keys),          // V M
            (for_of_iters, iter_take),            // V M
            (saved_bytecode_stack, move_field),   // M
            (saved_immutables_stack, move_field), // M
            (save_stack, move_field),             // V M
            (spill_stack, move_field),            // V M
            (cell_stack, move_field),             // V M
            (inline_callee, opt_copy),            // V M
            (active_upvalues, copy),              // M
            (generator_dispatch, flag_zero),      // M 调度标志：快照属主值，嵌套期间清零
            (async_dispatch, flag_zero),          // M 调度标志：快照属主值，嵌套期间清零
            (construct_dispatch, flag_zero),      // M 调度标志：快照属主值，嵌套期间清零
            (inline_strict, copy),                // M
            (inline_frames_base, copy),           // M
            (inline_args_base, copy),             // M
            (inline_args_count, copy),            // M
            (accessor_frame_target_reg, copy),    // M
            (active_flat_id, copy),               // M
            (active_table_gen, copy),             // M
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
        // 固化不变量：内联 state-swap 边界不携带在途异步逃出（挂起态快照前已搬入
        // 状态盒，settle 前恒为 None）。
        debug_assert!(self.pending_async_escape.is_none());
        let window = regs_end.min(254);
        let mut window_regs = self.inline_reg_pool.take().unwrap_or_default();
        window_regs.clear();
        window_regs.extend_from_slice(&self.regs[..window]);
        let vm = self;
        Box::new(inline_core_fields!(vm, vm, inline_save, window_regs))
    }

    /// 把 [`save_inline_state`] 保存的状态恢复回 VM。窗口回拷 + `regs[254]/[255]`
    /// 单回，窗口外寄存器 callee 未触碰无需恢复。
    pub(crate) fn restore_inline_state(&mut self, mut saved: Box<InlineSyncState>) {
        vm_trace!("restore_inline_state: pc={}", saved.pc);
        // 固化不变量：内联 state-swap 边界不携带在途异步逃出（与 save 侧同）。
        debug_assert!(self.pending_async_escape.is_none());
        let vm = self;
        let _window_regs: Vec<JsValue> = Vec::new();
        inline_core_fields!(vm, saved, inline_restore, _window_regs);
        // 窗口回拷已复活调用方陈旧镜像槽：重载调用方模块的 builtin 名集
        // （active_flat_id 已随宏回写还原到调用方）；外层 native pack 实参区
        // 在飞时跳过该域，实参值由窗口拷回还原、不在此刷新。
        vm.reload_active_module_mirror_slots(vm.native_pack_end);
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
        self.iters.reset();
        self.spill_stack.clear();
        self.save_stack.clear();
        self.saved_bytecode_stack.clear();
        self.saved_immutables_stack.clear();
        self.cell_stack.clear();
        self.inline_callee = None;
        self.push_bytecode_frame(
            callee,
            this_value,
            FrameArgs::Slice(args),
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::None,
            0,
        )
    }

    /// 内联同步调用字节码函数：不起新 CallFrame，直接在调用方寄存器窗口内执行被调模块。
    ///
    /// 平表按被调对象创建期 `table_gen` 解析（跨 run 调用仍命中定义模块所在原表）；
    /// 生成器与异步生成器函数不执行函数体，分别返回迭代器/异步迭代器对象；异步函数返回
    /// capability promise 并立即同步执行函数体到首个 await。
    /// 普通函数经 `save_inline_state` 保存窗口后在调用方寄存器文件上执行子模块，结束
    /// 时 `restore_inline_state` 回拷窗口。
    ///
    /// # 步骤
    /// 1. `sub_module_index == 0` 为 native 哨兵（accessor 不可调用），直接返回 TypeError。
    /// 2. 按创建期代际取平表并校验下标；调用深度达上限抛 RangeError。
    /// 3. 生成器/异步/异步生成器分流到对应对象构造，不进入 dispatch。
    /// 4. 保存窗口与 pc，零填 callee 读区，装载字节码/常量/平表 id，实参写入 spill 栈实参区。
    /// 5. 绑定 `regs[254]`：箭头函数用创建期捕获的 `this`，sloppy 普通函数在 nullish
    ///    receiver 上替换全局对象，严格模式与显式 receiver 原样保留；`regs[255]` 置 undefined。
    /// 6. 执行 dispatch 后恢复调用方状态并返回结果。
    ///
    /// # 边界与前提
    /// - 表代际未注册或 `sub_module_index` 越界返回 `Err`，不 panic。
    /// - 实参超出 `n_args` 的部分忽略；缺位参数填 undefined。
    ///
    /// # 副作用
    /// - 改写全局寄存器文件、pc、字节码/常量/平表 id、spill 栈与 `cell_stack`；
    ///   结束前经 `restore_inline_state` 全部还原。
    pub(crate) fn call_bytecode_function_inline(
        &mut self, callee: JsValue, callee_obj: &JsObject, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        if callee_obj.sub_module_index() == 0 {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }
        let sub_idx = callee_obj.sub_module_index() as usize;
        let gen = callee_obj.table_gen();
        vm_debug!(
            "call_bytecode_function_inline: sub_idx={}, args={}, depth={}",
            sub_idx,
            args.len(),
            self.frames.len()
        );
        let table = match self.tables.get(&gen) {
            Some(t) => t,
            None => return Err(format!("accessor: module table gen {} not available", gen)),
        };
        if sub_idx >= table.modules.len() {
            return Err(format!(
                "accessor sub_module_index {} out of bounds (max {})",
                sub_idx,
                table.modules.len()
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
        // 被调模块取 Arc 克隆（与调用方共享同一编译产物）：后续 &mut self
        // 调用期间不持有对注册表的借用。
        let sub = Arc::clone(&table.modules[sub_idx]);
        // 异步生成器函数调用返回异步生成器迭代器对象，next/return/throw 返回 Promise。
        if sub.is_generator && sub.is_async {
            return self.create_async_generator_object(callee, receiver, args);
        }
        // 生成器函数调用返回迭代器对象，不执行函数体。
        if sub.is_generator {
            return self.create_generator_object(callee, receiver, args);
        }
        // 异步函数调用返回 capability promise，立即同步执行 body 到首个 await。
        if sub.is_async {
            return self.create_async_object(callee, receiver, args);
        }
        self.native_call_depth += 1;

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
        self.activate_immutables(gen, sub_idx, &sub.constants);
        self.active_flat_id = sub_idx as u32;
        self.active_table_gen = gen;
        self.active_reg_limit = sub.n_registers.max(1);
        self.root_reg_limit = self.active_reg_limit;
        self.cell_stack.push(Vec::with_capacity(sub.cells_needed as usize));
        self.inline_callee = Some(callee);
        // 内联无帧：活动镜像直接取被调对象表（非闭包为 null 空切片）。
        self.active_upvalues = callee_obj.upvalues_slice() as *const [*mut Cell];
        // inline 无 CallFrame：目标函数严格模式单独记录，内联执行期间的写路径
        // strict/sloppy 判定据此分派（嵌套内联时外层值由 InlineSyncState 恢复）。
        self.inline_strict = sub.is_strict;
        // 帧基线快照：内联期间新压的帧（CALL/accessor）越过基线，其严格性归帧栈顶。
        self.inline_frames_base = self.frames.len();
        // inline 同步调用无 CallFrame：完整实参写入 spill 栈实参区，供 CREATE_ARGUMENTS 读取。
        self.inline_args_base = self.spill_stack.len() as u32;
        self.spill_stack.extend_from_slice(args);
        self.inline_args_count = args.len().min(u16::MAX as usize) as u16;
        for i in 0..sub.n_args as usize {
            self.regs[sub.param_base as usize + i] = args.get(i).copied().unwrap_or(JsValue::undefined());
        }
        // this 绑定：箭头函数恒用词法捕获；sloppy 普通函数 this 为 null/undefined 时
        // 替换为全局对象，其余原始值经 ToObject 盒装（ECMA-262 10.4.3）；严格模式
        // this 原样保留。
        self.regs[254] = if sub.is_arrow {
            callee_obj.captured_this()
        } else if !sub.is_strict && receiver.is_nullish() {
            JsValue::from_js_object(self.realm.session.borrow().global_object().as_ptr() as *mut JsObject)
        } else if sub.is_strict {
            receiver
        } else {
            oxide_runtime_api::to_object(receiver, self)?
        };
        self.regs[255] = JsValue::undefined();
        self.reload_builtin_mirror_slots(&sub.builtin_reg_map, 0);
        let _ = callee;

        vm_trace!(
            "call_bytecode: dispatching sub_idx={} n_regs={} n_args={} arrow={}",
            sub_idx,
            sub.n_registers,
            sub.n_args,
            sub.is_arrow
        );
        // 体自身 try 处理器基线：异常逃出体内时先由体内处理器捕获
        // （catch_pc 在体字节码内，状态还原后指向调用方字节码即悬空）。
        let try_base = self.try_stack.len();
        let mut result = self.dispatch();
        loop {
            if result.is_ok() || self.try_stack.len() <= try_base {
                break;
            }
            // 体内处理器捕获：pc 落体字节码 catch/finally，继续执行体余部。
            // 捕获体再抛（re-throw）时展开会消费外层处理器并跳外字节码 catch_pc，
            // 与活动字节码不一致——re-dispatch 返回 Err 即该形态，还原状态后由
            // 调用边界按外层处理器正常展开（与修复前同口径）。
            if self.unwind().is_err() {
                break;
            }
            result = self.dispatch();
            if result.is_err() {
                break;
            }
        }
        self.native_call_depth -= 1;

        vm_trace!("call_bytecode: restoring state pc={} result={:?}", saved.pc, result.as_ref().ok());
        self.restore_inline_state(saved);
        result
    }

    /// 从调用帧恢复调用方执行状态：弹回字节码/平表 id/表代际/常量四组保存栈，
    /// 回拷调用方寄存器窗口并按 `return_addr` 回写 pc。
    ///
    /// 帧恢复只负责执行核心状态；被调函数的返回值传递由调用路径另行处理。
    ///
    /// # 边界与前提
    /// - 保存栈为空时对应字段保持原值（不 panic）；`saved_reg_offset` 与
    ///   `caller_reg_limit` 由压帧时按调用方活动上界写入，须与其匹配。
    ///
    /// # 副作用
    /// - 改写 pc、字节码、活动平表 id/代际、活动常量、寄存器窗口与 `regs[254]/[255]`、
    ///   save_stack/spill_stack、`active_reg_limit`，并重载调用方 builtin 镜像槽。
    pub(crate) fn restore_frame(&mut self, frame: CallFrame) {
        vm_trace!(
            "restore_frame: return_addr={} caller_reg_limit={}",
            frame.return_addr,
            frame.caller_reg_limit
        );
        // 调用返回：sloppy 函数在 dispatch_create_arguments 把 own arguments 属性
        // 设为本调用的 arguments 对象；返回后须清空（置 undefined），否则强引用
        // 保活最后一次调用的 arguments 对象，跨 epoch 晋升后泄漏。仅当属性在场时
        // 清空（strict 函数无 own arguments，避免误建）。
        if !frame.strict && frame.callee.is_object() {
            let arguments_si = self.kernel_core.perm_interner().intern("arguments").0;
            let callee_ref = unsafe { &*frame.callee.as_js_object_ptr() };
            if let Some(slot) = self.get_own_property_slot(callee_ref, arguments_si) {
                let callee_mut = unsafe { &mut *frame.callee.as_js_object_ptr() };
                callee_mut.set_prop_at(slot, JsValue::undefined());
            }
        }
        // 弹帧后活动镜像切回新栈顶的表（帧空即顶层空切片）。
        self.active_upvalues = self
            .frames
            .last()
            .map(|f| f.upvalues)
            .unwrap_or(std::ptr::slice_from_raw_parts(std::ptr::null(), 0));
        if let Some(saved_bc) = self.saved_bytecode_stack.pop() {
            self.bytecode = saved_bc;
        }
        if let Some(saved_flat) = self.saved_flat_id_stack.pop() {
            self.active_flat_id = saved_flat;
        }
        if let Some(saved_gen) = self.saved_table_gen_stack.pop() {
            self.active_table_gen = saved_gen;
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
        // 窗口回拷已复活调用方陈旧镜像槽：重载调用方模块的 builtin 名集
        // （active_flat_id 已随保存栈弹回还原到调用方）；外层 native pack
        // 实参区在飞时跳过该域，实参值由窗口拷回还原、不在此刷新。
        self.reload_active_module_mirror_slots(self.native_pack_end);
    }

    /// 重新执行当前已加载的 bytecode：清空执行状态并重置 IC 缓存后再次 dispatch。
    ///
    /// # 注意事项
    /// - 清空寄存器文件后按 run 初始状态恢复顶层 this（regs[254]）：不恢复则
    ///   重执行中依赖 this 的顶层写（顶层 var 全局同步写）在 undefined 上
    ///   静默 no-op，循环计数不推进。
    pub fn rerun(&mut self) -> Result<JsValue, String> {
        vm_info!("rerun: clearing IC caches");
        self.clear_execution_state();
        self.active_reg_limit = self.root_reg_limit;
        self.regs[254] = self.top_level_this;
        // 寄存器文件清空后镜像槽须重载（与 run 入口同语义），否则裸读回陈旧值。
        self.reload_active_module_mirror_slots(0);
        crate::ic_helper::clear_ic_caches(self.bytecode_mut());
        self.dispatch()
    }

    /// run 边界回收无存活函数对象引用的子模块表代际：扫 session 对象表收
    /// 存活函数对象引用的表代际集，注册表中其余代际 drop。表内存由此有界于
    /// 存活函数对象数，不随 run 数单调增；存活函数按创建期代际仍命中原表。
    ///
    /// # 注意事项
    /// - 须在换表注册新代际**之前**调用（此刻 current_gen 仍指上一 run 的表，
    ///   新表未注册天然不被回收）。
    /// - 执行外安全点调用（dispatch 未重入、无在途 builtin 局部裸指针），
    ///   session 对象表此刻全部有效。
    pub(crate) fn reclaim_unreferenced_tables(&mut self) {
        // 存活代际集：函数对象只经创建点直落 session 对象表登记（或晋升克隆
        // 随行），扫全表即完备；native 函数哨兵 sub_module_index == 0 不计数。
        let mut live_gens: Vec<u32> = Vec::new();
        for &ptr in &self.realm.gc.borrow().session_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: session 对象表此刻未清空，指针指向 session arena 内合法对象。
            let obj = unsafe { &*ptr };
            if obj.is_function() && obj.sub_module_index() != 0 {
                live_gens.push(obj.table_gen());
            }
        }
        // 当前代际恒保留（本 run 执行仍在用，即便尚无函数引用）。
        live_gens.push(self.current_gen);
        self.tables.retain(|gen, _| live_gens.contains(gen));
    }

    /// 执行一个 JS 任务：闭包是任务体，本方法执行它并原样返回结果。
    ///
    /// 任务体在 `run()` 是 `dispatch` 主循环，在 worker 消息是单次函数调用。
    /// 本方法是任务边界原语，不排空微任务队列（排空时机由事件循环决定）。
    ///
    /// # 边界与前提
    /// - 闭包返回 `Err`（未捕获异常）时原样上交，不吞并、不改写。
    ///
    /// # 副作用
    /// - 任务体对 `self` 的全部改写（寄存器 / 帧栈 / 对象表 / 异常侧通道）原样保留。
    pub fn execute_task(&mut self, task: impl FnOnce(&mut Self) -> Result<JsValue, String>) -> Result<JsValue, String> {
        task(self)
    }

    /// 加载并执行一个已编译模块，返回模块顶层执行结果或未捕获异常消息。
    ///
    /// 模块以 `Arc` 与调用方共享（如 CodeForge 缓存条目）：平表装载只做
    /// Arc::clone，顶层字节码按引用计数条件复制（共享时装载期显式深拷贝，
    /// 独占时零拷贝）。内部初始化寄存器/bytecode/immutables 与
    /// builtin 寄存器预绑定，然后进入 dispatch 主循环；执行完成或异常展开后返回。
    pub fn run(&mut self, module: &Arc<CompiledModule>) -> Result<JsValue, String> {
        vm_debug!("run: starting bytecode execution, {} instructions", module.bytecode.len());
        self.clear_execution_state();
        // 模板对象缓存按 run 清空：缓存只留本次 run 的条目，规模有界（键含代际
        // 维度后跨 run 不再误命中，清空仅为规模约束）。
        self.template_objects.clear();
        self.saved_flat_id_stack.clear();
        self.saved_table_gen_stack.clear();
        self.active_flat_id = 0;
        self.cell_stack.clear();
        self.cell_stack.push(Vec::new());
        // run 边界换表前先回收无存活函数对象引用的表代际（此刻 current_gen 仍
        // 指上一 run 的表，存活代际集由 session 对象表扫描得出）。
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        self.reclaim_unreferenced_tables();
        // 新表注册于 current_gen + 1：本 run 创建的函数对象记录该代际，跨 run
        // 调用按创建期代际解析。
        self.current_gen = self.current_gen.wrapping_add(1);
        let modules = Arc::new(collect_flat_modules(module));
        self.tables.insert(
            self.current_gen,
            Box::new(TableGen {
                immutables: (0..modules.len()).map(|_| OnceLock::new()).collect(),
                si_tables: (0..modules.len()).map(|_| OnceLock::new()).collect(),
                modules,
            }),
        );
        self.active_table_gen = self.current_gen;
        // 顶层脚本严格模式：无帧且无 inline 时写路径的 strict/sloppy 判定来源。
        self.top_level_strict = module.is_strict;
        // 顶层字节码按模块引用计数条件复制：模块被宿主共享（外层 Arc 计数 > 1，
        // 如 CodeForge 缓存条目与调用方各持一份）时装载期显式深拷贝，把 COW 深拷贝
        // 从 dispatch 主循环热路径挪到 run 装载期（批量、可预测）；模块独占（计数 1，
        // 如 worker 新编译模块）保持 Arc::clone，无宿主共享缓冲需保护，复制无收益。
        if Arc::strong_count(module) > 1 {
            self.bytecode = Arc::from(&module.bytecode[..]);
        } else {
            self.bytecode = Arc::clone(&module.bytecode);
        }
        self.activate_immutables(self.current_gen, 0, &module.constants);
        self.root_reg_limit = module.n_registers.max(1);
        self.active_reg_limit = self.root_reg_limit;

        self.reload_builtin_mirror_slots(&module.builtin_reg_map, 0);

        // 顶层 this：脚本为全局对象（ECMA-262 全局执行上下文）；
        // ES module 顶层环境 GetThisBinding 返回 undefined。记录到
        // top_level_this：rerun 清空寄存器文件后按此恢复顶层 this。
        let global_ptr = self.realm.session.borrow().global_object().as_ptr() as *mut JsObject;
        let this_val = if module.is_es_module {
            JsValue::undefined()
        } else {
            JsValue::from_js_object(global_ptr)
        };
        self.top_level_this = this_val;
        self.regs[254] = this_val;

        // 「正在求值模块」cyclic 守卫标记：入口模块函数对象在 run 不可得（顶层
        // CREATE_CLOSURE 产物），用 undefined 哨兵——sentinel 检查只看 is_some()，
        // 依赖模块的 real-function 检查恒不匹配 undefined，安全。save/restore 保
        // 嵌套/重入安全，dispatch 返回即还原（微任务 drain 前模块已非求值中）。
        let saved_evaluating = self.evaluating_module.take();
        self.evaluating_module = Some(JsValue::undefined());
        let result = self.execute_task(|vm| vm.dispatch());
        self.evaluating_module = saved_evaluating;
        // 顶层执行结束后 drain 微任务队列：Promise reactions 与 thenable 委托在此执行。
        self.drain_microtasks();
        // 指令周期采样：run 末聚合本 run 的样本并输出 top-K 直方图（关闭时零开销
        // 短路）。放 dispatch 返回后而非循环内：嵌套 dispatch 会多次返回，直方图
        // 只应输出一次。
        self.print_sample_histogram();
        result
    }

    /// 按 (flat_id, 表代际) 聚合采样记录并向 stderr 输出 top-K 热点直方图。
    ///
    /// 聚合键取 (flat_id, 表代际, pc)：样本数按 (flat_id, 表代际) 汇总，top pc
    /// 取该键下样本数最多的字节码偏移（与反汇编 offset 列同单位）。函数名经样本
    /// 记录表代际的平表解析（顶层为 0），未知 flat_id 输出 `<flat N>`。
    ///
    /// # 边界
    /// - 采样关闭（`period == 0`）或无样本时直接返回，零输出零分配。
    /// - 只读采样记录与平表，不改写任何执行状态。
    pub(crate) fn print_sample_histogram(&self) {
        if self.sampling.period == 0 || self.sampling.records.is_empty() {
            return;
        }
        // 聚合：(flat_id, 表代际, pc) → 样本数与 opcode；
        // (flat_id, 表代际) → 最大 frames 深度。
        let mut pc_hist: HashMap<(u32, u32, u32), (u64, u8)> = HashMap::new();
        let mut flat_max_frames: HashMap<(u32, u32), u32> = HashMap::new();
        for &(flat, pc, op, frames, gen) in &self.sampling.records {
            let entry = pc_hist.entry((flat, gen, pc)).or_insert((0, op));
            entry.0 += 1;
            let top = flat_max_frames.entry((flat, gen)).or_insert(frames);
            if frames > *top {
                *top = frames;
            }
        }
        // 按 (flat_id, 表代际) 汇总样本数，降序排列后取 top-K。
        let mut flat_total: HashMap<(u32, u32), u64> = HashMap::new();
        for (&(flat, gen, _), &(count, _)) in &pc_hist {
            *flat_total.entry((flat, gen)).or_insert(0) += count;
        }
        let mut ranked: Vec<((u32, u32), u64)> = flat_total.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let k = self.sampling.top_k.min(ranked.len());
        eprintln!(
            "sample top {k} (period {}, {} samples, {} instructions):",
            self.sampling.period,
            self.sampling.records.len(),
            self.profiling.instruction_count
        );
        for (rank, &((flat, gen), count)) in ranked.iter().take(k).enumerate() {
            // top pc：该 (flat_id, 表代际) 下样本数最多的字节码偏移（并列取哈希
            // 遍历首个，非确定，诊断用途可接受）。
            let mut top_pc: Option<(u64, u32, u8)> = None;
            for (&(f, g, pc), &(c, op)) in &pc_hist {
                if f != flat || g != gen {
                    continue;
                }
                match top_pc {
                    None => top_pc = Some((c, pc, op)),
                    Some((tc, _, _)) if c > tc => top_pc = Some((c, pc, op)),
                    Some(_) => {}
                }
            }
            let (top_count, top_pc, top_op) = top_pc.unwrap_or((0, 0, 0));
            let op_name = OpCode::try_from(top_op)
                .map(|o| o.to_string())
                .unwrap_or_else(|_| format!("op{top_op}"));
            let name = self
                .tables
                .get(&gen)
                .and_then(|t| t.modules.get(flat as usize))
                .and_then(|m| m.function_name.clone())
                .unwrap_or_else(|| format!("<flat {flat}>"));
            let frames = flat_max_frames.get(&(flat, gen)).copied().unwrap_or(0);
            eprintln!(
                "  {rank:>2}. {name:<24} (flat {flat}) {count:>8} samples  top pc={top_pc} {op_name} ({top_count})  frames={frames}"
            );
        }
    }

    /// 异常展开：沿 `try_stack` 查找最近可接管的 catch/finally，无则逐帧回退并返回未捕获错误。
    ///
    /// 与 `TRY_FINALLY_END` 的完成穿越逻辑对应：finally 体在途异常经 `pending_exception`
    /// 悬挂，`finally_active` 标记其已进入，防止同一 finally 重复执行；处理器作用域内被
    /// 中断的 for-of 循环先经 `close_for_of_above` 执行 IteratorClose。
    ///
    /// # 步骤
    /// 1. 逐 handler 展开：所属帧已返回（`frame_depth` 超出当前帧数）的残留 handler 跳过。
    /// 2. 弹帧至 handler 深度，逐帧弹 `cell_stack` 并 `restore_frame` 回写调用方状态。
    /// 3. 关闭该作用域内被中断的 for-of 迭代器。
    /// 4. 有 finally：未进入则挂起在途异常并转入 finally 体；已进入（finally 体内新异常）
    ///    则丢弃在途异常继续向外展开，不重入本 finally。
    /// 5. 有 catch：在途异常写入 `regs[0]`，pc 转向 catch。
    /// 6. 无可用处理器：关闭全部迭代器、回退所有帧，记 `last_uncaught_value` 并返回 `Err`。
    ///
    /// # 边界与前提
    /// - `exception_value` 为空时按 undefined 处理；`try_stack` 为空直接走未捕获路径。
    ///
    /// # 副作用
    /// - 弹出 `try_stack`/`frames`/`cell_stack`，改写 pc 与寄存器窗口；未捕获时把逃逸
    ///   异常值存入 `last_uncaught_value`（for-of 的 next() 抛出经此重抛原值而非展平
    ///   字符串），并消费 `pending_error_kind`。
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
                // 在途异常已进入 finally 处理流程：槽作废，防残留值被后续 take 误取。
                self.last_uncaught_value = None;
                return Ok(());
            }
            if let Some(catch_pc) = handler.catch_pc {
                vm_trace!("unwind: caught at pc={}", catch_pc);
                let exc = self.exception_value.take().unwrap_or(JsValue::undefined());
                self.regs[0] = exc;
                self.pc = catch_pc;
                // 在途异常已写入 catch 参数：槽作废，防残留值被后续 take 误取。
                self.last_uncaught_value = None;
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

    fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source).expect("parse");
        oxide_compiler::compiler::Compiler::new().compile(&program).expect("compile")
    }

    #[test]
    fn top_level_module_entry_shares_host_arc() {
        // 顶层平表条目 = 宿主模块 Arc 同一实例：run 期零模块克隆，
        // 克隆会产出不同 Arc，本钉在此情形必失败。
        let mut vm = Vm::new();
        let module = Arc::new(compile("var o = { a: 1 }; o.a + o.a"));
        vm.run(&module).expect("run1");
        assert!(Arc::ptr_eq(&vm.current_table().modules[0], &module), "顶层条目须与宿主 Arc 同一实例");
        vm.full_reset();
        vm.run(&module).expect("run2");
        assert!(Arc::ptr_eq(&vm.current_table().modules[0], &module), "full_reset 后二次 run 仍共享");
    }

    #[test]
    fn top_level_run_keeps_host_bytecode_ic_words_zero() {
        // 宿主侧 bytecode 的 IC 扩展字在 run 后恒零：IC 写回只发生在
        // Vm.bytecode 的 COW 私有拷贝上——dispatch 期平表与 Vm.bytecode 双持
        // 共享缓冲（refcount ≥ 2），make_mut 必先深拷贝再写，宿主共享缓冲
        // 结构上不可写。
        let mut vm = Vm::new();
        let module = Arc::new(compile("var o = { a: 1 }; o.a + o.a"));
        vm.run(&module).expect("run");
        assert!(vm.ic_miss_count() > 0, "源含属性读取，应发生 IC miss 与写回");
        // 以 clear_ic_caches 的扫描口径作 oracle：IC 扩展字若已全零，清零是 no-op。
        let original: Vec<oxide_bytecode::opcode::Instr> = module.bytecode.to_vec();
        let mut probe = original.clone();
        crate::ic_helper::clear_ic_caches(&mut probe);
        assert_eq!(probe, original, "宿主侧 IC 扩展字须全零");
        // 写回未落宿主缓冲：run 末 Vm.bytecode 是 COW 私有拷贝（不同分配），
        // 而非宿主共享缓冲本身。
        assert!(
            !Arc::ptr_eq(&module.bytecode, &vm.bytecode),
            "Vm.bytecode 须为 COW 私有拷贝，写回不得落宿主缓冲"
        );
        assert_eq!(vm.bytecode.len(), module.bytecode.len());
    }

    #[test]
    fn run_copies_bytecode_when_shared() {
        // 顶层字节码装载期显式副本：共享模块（引用计数 > 1）在 run 装载期即深拷贝，
        // 即使零 IC miss（无写回触发惰性 COW）Vm.bytecode 也须是私有 Arc 实例。
        // 本钉在惰性 COW 行为下必失败（无写回则不拷贝，保持与宿主同一实例）。
        let mut vm = Vm::new();
        let module = Arc::new(compile("var x = 1; x + 1"));
        let shared = Arc::clone(&module); // 双 Arc 计数，模拟宿主缓存共享
        vm.run(&shared).expect("run");
        assert!(
            !Arc::ptr_eq(&module.bytecode, &vm.bytecode),
            "共享模块装载期须显式复制，Vm.bytecode 不得与宿主共享同一 Arc"
        );
        assert_eq!(vm.bytecode.len(), module.bytecode.len());
    }

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
            for_of_count: 0,
            for_in_count: 0,
        });
        vm.iters.for_of_iters.push(crate::vm_state::ForOfEntry {
            iterator: JsValue::float(8.5),
            last_result: JsValue::float(9.0),
            is_async: false,
            fast: None,
            fast_value: JsValue::undefined(),
            fast_inner: JsValue::undefined(),
            fast_cursor: 0,
        });
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
            Some(Completion::Return { value, remaining_finally: 1, .. }) if value == JsValue::float(8.0)
        ));
        assert_eq!(vm.iters.for_of_iters.len(), 1);
        assert_eq!(vm.iters.for_of_iters[0].iterator, JsValue::float(8.5));
        assert_eq!(vm.iters.for_of_iters[0].last_result, JsValue::float(9.0));
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

    fn global_value(vm: &mut Vm, name: &str) -> JsValue {
        let key = vm.kernel_core.perm_interner().intern(name).0;
        let session = vm.realm.session.borrow();
        let global = session.global_object();
        let pos = vm
            .kernel_core
            .shape_forge()
            .lookup_position(global.shape_id(), key)
            .expect("global slot");
        global.get_prop_at(pos)
    }

    fn as_number(v: &JsValue) -> f64 {
        if v.is_int() {
            v.as_int() as f64
        } else {
            v.as_double()
        }
    }

    #[test]
    fn execute_task_returns_closure_result() {
        // 任务体为单次 call_function_sync 调用，execute_task 原样返回闭包结果。
        let mut vm = Vm::new();
        let module = Arc::new(compile("function f() { return 42; }"));
        vm.run(&module).expect("run");
        let f = global_value(&mut vm, "f");
        let result = vm.execute_task(|vm| vm.call_function_sync(f, JsValue::undefined(), &[]));
        assert_eq!(as_number(&result.expect("execute_task")), 42.0);
    }

    #[test]
    fn execute_task_propagates_closure_error() {
        // 任务体抛未捕获异常：execute_task 原样上交 Err，last_uncaught_value 侧通道按既有语义填充。
        let mut vm = Vm::new();
        let module = Arc::new(compile("function f() { throw new Error('boom'); }"));
        vm.run(&module).expect("run");
        let f = global_value(&mut vm, "f");
        let result = vm.execute_task(|vm| vm.call_function_sync(f, JsValue::undefined(), &[]));
        assert!(result.is_err(), "未捕获异常须以 Err 上交");
        assert!(vm.last_uncaught_value.is_some(), "last_uncaught_value 侧通道须填充");
    }

    #[test]
    fn drain_microtasks_runs_promise_reactions() {
        // 任务体建 promise 并挂 .then：execute_task 不排空微任务队列，反应留在队列，
        // drain_microtasks 执行之，侧效应发生。
        let mut vm = Vm::new();
        let module = Arc::new(compile(
            "var done = false; \
             function make() { Promise.resolve(1).then(function (x) { done = true; }); }",
        ));
        vm.run(&module).expect("run");
        let make = global_value(&mut vm, "make");
        vm.execute_task(|vm| vm.call_function_sync(make, JsValue::undefined(), &[]))
            .expect("execute_task");
        assert!(!vm.job_queue.is_empty(), "反应应已入队");
        vm.drain_microtasks();
        assert!(vm.job_queue.is_empty(), "drain 后队列应清空");
        assert_eq!(global_value(&mut vm, "done"), JsValue::bool(true), "promise 反应应已置 done = true");
    }

    #[test]
    fn drain_microtasks_empty_queue_idempotent() {
        // run 后队列为空，连续两次 drain_microtasks 无副作用。
        let mut vm = Vm::new();
        let module = Arc::new(compile("var x = 1;"));
        vm.run(&module).expect("run");
        assert!(vm.job_queue.is_empty());
        vm.drain_microtasks();
        vm.drain_microtasks();
        assert!(vm.job_queue.is_empty());
    }

    #[test]
    fn using_declaration_registers_dispose_stack() {
        // using 声明经 DISPOSE_REGISTER 把资源值压入释放栈；run 边界（下次 run
        // 入口）清栈，上一 run 的残留条目不得跨 run 可见。
        let mut vm = Vm::new();
        let module = Arc::new(compile("function f() { using x = { a: 1 }; } f();"));
        vm.run(&module).expect("run");
        assert_eq!(vm.dispose_stack.len(), 1, "using 声明应登记一个资源值");
        assert!(vm.dispose_stack[0].is_object(), "登记值应为资源对象");

        // 二次 run 入口清栈后重新登记：残留条目不跨 run 存活。
        vm.run(&module).expect("run2");
        assert_eq!(vm.dispose_stack.len(), 1, "二次 run 应恰好登记一条新条目");

        // 执行状态清空同样清释放栈。
        vm.clear_execution_state();
        assert!(vm.dispose_stack.is_empty(), "执行状态清空应清空释放栈");
    }
}
