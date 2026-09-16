//! 同步调用与压帧：native/字节码函数同步调用入口、大实参溢出区、
//! 压帧窗口与形参回写、内联窗口上界与严格模式判定。

use std::sync::Arc;

use oxide_runtime_api::NativeResult;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::frames::{CallFrame, FrameArgs, FrameContinuation};
use super::{native_fn_ptr_to_fn, Vm};
use crate::native::NativeFn;
use crate::{vm_debug, vm_trace};

impl Vm {
    fn pack_sync_native_call_args(&mut self, receiver: JsValue, callee: JsValue, args: &[JsValue]) -> Vec<u8> {
        self.regs[253] = receiver;
        self.regs[254] = callee;

        let mut arg_regs = Vec::with_capacity(args.len() + 1);
        arg_regs.push(253);
        for (idx, arg) in args.iter().enumerate() {
            let reg = (Self::SYNC_NATIVE_ARG_BASE + idx) as u8;
            self.regs[reg as usize] = *arg;
            arg_regs.push(reg);
        }
        arg_regs
    }

    pub(crate) fn is_session_ptr(&self, obj_ptr: *mut JsObject) -> bool {
        if obj_ptr.is_null() {
            return false;
        }
        // SAFETY: obj_ptr 非空且指向本 session 拥有的 JsObject。
        unsafe { (*obj_ptr).is_session_epoch() }
    }

    pub(crate) fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject {
        let ptr = self.epoch.alloc(obj);
        self.gc_state.track_epoch_object(ptr);
        ptr
    }

    pub(crate) fn call_function_sync(
        &mut self, callee: JsValue, receiver: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        vm_debug!("call_function_sync: args={} callee_is_object={}", args.len(), callee.is_object());
        if !callee.is_object() {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        if !callee_obj.is_function() {
            return Err(self.error_message_text("TypeError", "accessor is not callable"));
        }

        if let Some(native_fn) = callee_obj.native_fn() {
            if self.native_call_depth >= self.kernel_core.config.max_call_depth {
                self.raise_error_kind("RangeError", "Maximum call stack size exceeded")?;
                return Ok(JsValue::undefined());
            }
            // 大实参集（超过寄存器窗口）：窗口外的实参转存 spill 栈溢出区（GC 根），
            // native 侧经 native_arg_count/native_arg_at 读取；窗口内仍按寄存器
            // 协议打包，保证未迁移的 builtin 行为不变。
            let overflow_base = self.spill_stack.len();
            let saved_overflow_base = self.native_overflow_base;
            let saved_overflow_count = self.native_overflow_count;
            if args.len() > Self::SYNC_NATIVE_ARG_LIMIT {
                self.spill_stack.extend_from_slice(&args[Self::SYNC_NATIVE_ARG_LIMIT..]);
                self.native_overflow_base = overflow_base;
                self.native_overflow_count = args.len() - Self::SYNC_NATIVE_ARG_LIMIT;
            }
            // native 回调只写 regs[0..args.len()] 实参区 + regs[253]/[254]（receiver/callee），
            // 窗口 = 调用方活动寄存器 ∪ 实参写入区；窗口外槽回调不触碰，无需保存。
            let window = (self.active_reg_limit as usize).max(args.len() + 3).min(253);
            let mut saved_window = self.inline_reg_pool.take().unwrap_or_default();
            saved_window.clear();
            saved_window.extend_from_slice(&self.regs[..window]);
            let saved_r253 = self.regs[253];
            let saved_r254 = self.regs[254];
            let pack_limit = args.len().min(Self::SYNC_NATIVE_ARG_LIMIT);
            let arg_regs = self.pack_sync_native_call_args(receiver, callee, &args[..pack_limit]);
            // SAFETY: native_fn 经 set_native_fn 以合法 NativeFn 指针设置；
            // native_fn_ptr_to_fn 是 NativeFnPtr → NativeFn 的唯一强制转换点。
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(native_fn) };
            self.native_call_depth += 1;
            let result = func(self, &arg_regs);
            self.native_call_depth -= 1;
            // 溢出区随本次调用结束截断回收，并还原外层的溢出区描述（嵌套调用安全）。
            if args.len() > Self::SYNC_NATIVE_ARG_LIMIT {
                self.spill_stack.truncate(overflow_base);
            }
            self.native_overflow_base = saved_overflow_base;
            self.native_overflow_count = saved_overflow_count;
            // 窗口回拷 + regs[253]/[254] 单回，缓冲归还池复用。
            self.regs[..window].copy_from_slice(&saved_window);
            self.regs[253] = saved_r253;
            self.regs[254] = saved_r254;
            self.inline_reg_pool = Some(saved_window);
            return match result {
                NativeResult::Ok(val) => Ok(val),
                NativeResult::Err(err) => {
                    self.last_uncaught_value = Some(err);
                    Err(self.error_text(err))
                }
                NativeResult::TailCall { callee, this, args } => self.call_function_sync(callee, this, &args),
            };
        }

        // 字节码函数在自身（同一 epoch）内联执行，防止 use-after-free。
        // 独立子 VM 拥有不同 epoch：返回含子 VM epoch 指针的 JsValue 后再销毁子 VM，
        // 会在 release 构建中产生悬垂指针 / 访问违规。
        self.call_bytecode_function_inline(callee, callee_obj, receiver, args)
    }

    /// 压帧窗口上界：调用方活动寄存器与调用点存活上界取 min。
    ///
    /// # 边界与前提
    /// - `call_window` 为 CALL ext 高 8 位编码的存活上界；0 = 未编码（回退全量）。
    /// - 窗口至少为 1（reg 0 恒为调用结果槽）。
    pub(crate) fn call_window_limit(&self, caller_active_reg_limit: u8, call_window: u8) -> u8 {
        if call_window == 0 {
            caller_active_reg_limit
        } else {
            caller_active_reg_limit.min(call_window)
        }
        .max(1)
    }

    /// 当前执行上下文的严格模式标志：属性写失败路径据此分派（严格抛
    /// TypeError，sloppy 静默 no-op）。
    ///
    /// # 边界与前提
    /// - inline 执行期间帧表可能非空（调用方帧隔离在外），须以
    ///   `inline_frames_base` 为基线区分"inline 前已有帧"与"inline 内新压帧"
    ///   （CALL/accessor 帧严格性归目标函数，最内层执行上下文即帧栈顶）。
    /// - 无帧且无 inline 时为顶层脚本执行，取 `top_level_strict`。
    pub(crate) fn current_strict(&self) -> bool {
        if self.inline_callee.is_some() {
            if self.frames.len() > self.inline_frames_base {
                return self.frames.last().unwrap().strict;
            }
            return self.inline_strict;
        }
        if let Some(frame) = self.frames.last() {
            return frame.strict;
        }
        self.top_level_strict
    }

    #[expect(clippy::too_many_arguments)]
    pub(crate) fn push_bytecode_frame(
        &mut self, callee: JsValue, this_value: JsValue, args: FrameArgs, construct_result_reg: Option<u8>,
        constructed_this: Option<JsValue>, new_target: JsValue, continuation: FrameContinuation, call_window: u8,
    ) -> Result<(), String> {
        vm_trace!(
            "push_bytecode_frame: depth={}, args={}, continuation={:?}",
            self.frames.len(),
            args.len(),
            continuation
        );
        if !callee.is_object() {
            return Err(self.error_message_text("TypeError", "CALL target is not callable"));
        }
        let obj = unsafe { &*callee.as_js_object_ptr() };
        if !obj.is_function() || obj.sub_module_index() == 0 {
            return Err(self.error_message_text("TypeError", "CALL target is not callable"));
        }
        let sub_idx = obj.sub_module_index() as usize;
        let gen = obj.table_gen();
        let table = match self.tables.get(&gen) {
            Some(t) => t,
            None => return Err(format!("CALL: module table gen {} not available", gen)),
        };
        if sub_idx >= table.modules.len() {
            return Err(format!("CALL: sub_module_index {} out of bounds (max {})", sub_idx, table.modules.len()));
        }
        if self.frames.len() >= self.kernel_core.config.max_call_depth {
            return self.raise_error_kind("RangeError", "Maximum call stack size exceeded");
        }

        // 被调模块取 Arc 克隆（与调用方共享同一编译产物）：后续 activate_immutables
        // 等 &mut self 调用期间不持有对注册表的借用。
        let sub = Arc::clone(&table.modules[sub_idx]);
        let sub_bytecode = Arc::clone(&sub.bytecode);
        let sub_n_args = sub.n_args as usize;
        let sub_n_registers = sub.n_registers;
        let sub_param_base = sub.param_base as usize;
        let sub_is_arrow = sub.is_arrow;
        let sub_is_strict = sub.is_strict;
        // 窗口 = min(调用方活动寄存器, 存活上界)；call_window=0 表示调用方全量
        // （运行时发起路径 / 未编码的旧模块）。恢复按窗口回拷，active_reg_limit
        // 仍还原为调用方真实值（caller_active_reg_limit）。
        let caller_active_reg_limit = self.active_reg_limit.max(1);
        let caller_reg_limit = self.call_window_limit(caller_active_reg_limit, call_window);
        let saved_reg_offset = self.save_stack.len() as u32;
        self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
        let saved_this = self.regs[254];
        let saved_new_target = self.regs[255];

        // 完整实参先写入 spill 栈实参区（在帧的 spill 区之前）：CREATE_ARGUMENTS 据此
        // 构建 arguments 对象，帧恢复时随 spill 区截断一起丢弃。spill 在前、形参在后，
        // 且形参源改读 spill 区——实参源区间与形参写入区在共享寄存器文件内重叠时
        // 不会先写后读串值（nested callee identity 高位 param_base 可落入实参区间）。
        let args_base = self.spill_stack.len() as u32;
        match args {
            FrameArgs::Slice(s) => self.spill_stack.extend_from_slice(s),
            FrameArgs::RegRange { first, count } => {
                for i in 0..count {
                    self.spill_stack.push(self.regs[first.wrapping_add(i as u8) as usize]);
                }
            }
        }
        let args_count = args.len().min(u16::MAX as usize) as u16;

        // 形参拷贝：源为 spill 实参区（与调用方寄存器隔离），实参不足补 undefined。
        for i in 0..sub_n_args {
            let v = if i < args_count as usize {
                self.spill_stack[args_base as usize + i]
            } else {
                JsValue::undefined()
            };
            self.regs[sub_param_base + i] = v;
        }
        // this 绑定：箭头函数恒用词法捕获；sloppy 普通函数 this 为 null/undefined 时
        // 替换为全局对象（ECMA-262 10.4.3）；严格模式与显式方法/构造 this 原样保留。
        self.regs[254] = if sub_is_arrow {
            obj.captured_this()
        } else if !sub_is_strict && this_value.is_nullish() {
            JsValue::from_js_object(self.session.global_object().as_ptr() as *mut JsObject)
        } else {
            this_value
        };
        self.regs[255] = new_target;

        self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
        self.saved_immutables_stack.push(self.active_immutables);
        // 记录调用方 flat_id 与表代际，进入被调模块（标签模板 site 缓存按
        // (代际, flat_id) 隔离）。
        self.saved_flat_id_stack.push(self.active_flat_id);
        self.saved_table_gen_stack.push(self.active_table_gen);
        self.active_flat_id = sub_idx as u32;
        self.active_table_gen = gen;

        let function_name = sub
            .function_name
            .as_deref()
            .map(|name| self.kernel_core.perm_interner().intern(name).0)
            .unwrap_or(0);

        self.frames.push(CallFrame {
            return_addr: self.pc,
            function_name,
            caller_reg_limit,
            caller_active_reg_limit,
            saved_reg_offset,
            spill_offset: self.spill_stack.len() as u32,
            arguments_base: args_base,
            arguments_count: args_count,
            saved_this,
            saved_new_target,
            callee,
            construct_result_reg,
            constructed_this,
            is_derived_constructor: obj.is_derived_constructor(),
            super_called: false,
            strict: sub_is_strict,
            continuation,
        });

        self.pc = 0;
        self.bytecode = sub_bytecode;
        self.activate_immutables(gen, sub_idx, &sub.constants);
        self.cell_stack.push(Vec::with_capacity(sub.cells_needed as usize));
        self.reload_builtin_mirror_slots(&sub.builtin_reg_map);

        self.active_reg_limit = sub_n_registers.max(1);
        self.pc = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use oxide_bytecode::module::CompiledModule;
    use oxide_bytecode::opcode;
    use oxide_runtime_api::VmHost;
    use oxide_types::object::NativeFnPtr;

    fn native_return_last_arg(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let reg = *args.last().expect("receiver + args");
        NativeResult::Ok(vm.reg(reg))
    }

    fn native_return_arg_count(_vm: &mut Vm, args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(args.len().saturating_sub(1) as i32))
    }

    fn native_return_full_arg_count(vm: &mut Vm, args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(vm.native_arg_count(args) as i32))
    }

    fn native_return_last_full_arg(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let n = vm.native_arg_count(args);
        NativeResult::Ok(vm.native_arg_at(args, n - 1))
    }

    fn native_nested_inline_254(vm: &mut Vm, args: &[u8]) -> NativeResult {
        // 外层 native 回调：receiver 落 regs[253]（native 分支单存槽），内嵌执行
        // n_registers=254 的 inline 字节码回调后，receiver 槽必须保持外层值。
        let receiver = vm.reg(args[0]);
        let callee = vm.reg(args[1]);
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        let mut call_args = Vec::with_capacity(args.len().saturating_sub(2));
        for &r in &args[2..] {
            call_args.push(vm.reg(r));
        }
        let kept = match vm.call_bytecode_function_inline(callee, callee_obj, receiver, &call_args) {
            Ok(_) => vm.regs[253] == receiver,
            Err(_) => false,
        };
        NativeResult::Ok(JsValue::int(if kept { 1 } else { 0 }))
    }

    fn native_function(vm: &mut Vm, f: crate::native::NativeFn) -> JsValue {
        let proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let mut obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.set_function(true);
        // SAFETY: f 是 NativeFn 函数项，可作为 NativeFnPtr 存储。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(f as *const ()) }));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    #[test]
    fn call_function_sync_passes_high_arity_native_args_without_truncation() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_last_arg);
        let args: Vec<JsValue> = (0..20).map(JsValue::int).collect();

        let result = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("high-arity sync call should succeed");

        assert_eq!(result, JsValue::int(19));
    }

    #[test]
    fn call_function_sync_reports_actual_native_arg_count() {
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_arg_count);
        let args: Vec<JsValue> = (0..32).map(JsValue::int).collect();

        let result = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("arg count should be preserved");

        assert_eq!(result, JsValue::int(32));
    }

    #[test]
    fn call_function_sync_overflow_preserves_large_native_arity_and_registers() {
        // 大实参集（超寄存器窗口 253）经 spill 溢出区完整送达 native：
        // 全量计数与末位实参都可读，且调用方寄存器不被打包过程污染。
        let mut vm = Vm::new();
        let callee = native_function(&mut vm, native_return_full_arg_count);
        vm.set_reg(1, JsValue::int(7));
        vm.set_reg(253, JsValue::int(11));
        vm.set_reg(254, JsValue::int(12));
        vm.set_reg(255, JsValue::int(13));

        let args: Vec<JsValue> = (0..(Vm::SYNC_NATIVE_ARG_LIMIT + 100)).map(|i| JsValue::int(i as i32)).collect();
        let count = vm
            .call_function_sync(callee, JsValue::undefined(), &args)
            .expect("大实参集应经 spill 溢出区完整送达 native");
        assert_eq!(count, JsValue::int(args.len() as i32));

        let last_callee = native_function(&mut vm, native_return_last_full_arg);
        let last = vm
            .call_function_sync(last_callee, JsValue::undefined(), &args)
            .expect("溢出区末位实参应可读");
        assert_eq!(last, JsValue::int((args.len() - 1) as i32));

        assert_eq!(vm.reg(1), JsValue::int(7));
        assert_eq!(vm.reg(253), JsValue::int(11));
        assert_eq!(vm.reg(254), JsValue::int(12));
        assert_eq!(vm.reg(255), JsValue::int(13));
        // 溢出区随调用结束截断回收，不残留 spill 增长。
        assert!(vm.spill_stack.is_empty(), "溢出区应已回收: {:?}", vm.spill_stack);
        assert_eq!(vm.native_overflow_count, 0);
    }

    /// 构造 `n_registers = 254` 的子模块：RegAlloc 合法产物（builtin 槽落 253 或
    /// 高活度着色），其函数体写物理槽 253 后返回。
    fn sub_module_254_with_r253_write() -> CompiledModule {
        CompiledModule {
            bytecode: Arc::from(vec![
                opcode::encode(opcode::OpCode::MOV, 253, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ]),
            n_registers: 254,
            ..CompiledModule::new()
        }
    }

    #[test]
    fn inline_callee_254_registers_preserves_caller_active_r253() {
        // 窗口化边界回归：inline 回调 callee 的 n_registers = 254（写物理槽 253）
        // 且调用方 active_reg_limit = 254（regs[253] 为活动值）时，调用后
        // regs[253] 必须恢复为调用方值，不得被 callee 写值覆盖。
        let mut vm = Vm::new();
        vm.install_module_table_for_test(Arc::new(vec![
            Arc::new(CompiledModule::new()),
            Arc::new(sub_module_254_with_r253_write()),
        ]));
        vm.active_reg_limit = 254;
        vm.regs[253] = JsValue::int(42);

        let callee = vm.create_function_object(1, vm.current_gen, false, false, false, false);
        let callee_obj = unsafe { &*callee.as_js_object_ptr() };
        let result = vm
            .call_bytecode_function_inline(callee, callee_obj, JsValue::undefined(), &[])
            .expect("inline call should succeed");

        assert_eq!(result, JsValue::undefined());
        assert_eq!(vm.regs[253], JsValue::int(42), "调用方 regs[253] 活动值不得被 callee 覆盖");
        assert_eq!(vm.active_reg_limit, 254, "restore 应还原调用方活动寄存器上限");
    }

    #[test]
    fn native_callback_nested_254_register_inline_callee_keeps_receiver() {
        // 嵌套回归防线：native 回调体（receiver 落 regs[253]）内嵌 n_registers=254
        // 的 inline 字节码回调时，内层写 regs[253] 不得污染外层 receiver 槽。
        let mut vm = Vm::new();
        vm.install_module_table_for_test(Arc::new(vec![
            Arc::new(CompiledModule::new()),
            Arc::new(sub_module_254_with_r253_write()),
        ]));
        vm.active_reg_limit = 254;
        vm.regs[253] = JsValue::int(7);
        vm.regs[254] = JsValue::int(8);

        let inner_callee = vm.create_function_object(1, vm.current_gen, false, false, false, false);
        let outer_native = native_function(&mut vm, native_nested_inline_254);
        let result = vm
            .call_function_sync(outer_native, JsValue::int(99), &[inner_callee])
            .expect("native callback should succeed");

        assert_eq!(result, JsValue::int(1), "内嵌 inline 回调后外层 receiver 槽必须保持原值");
        assert_eq!(vm.regs[253], JsValue::int(7), "native 分支恢复后调用方 regs[253] 保持");
        assert_eq!(vm.regs[254], JsValue::int(8), "native 分支恢复后调用方 regs[254] 保持");
    }

    #[test]
    fn push_bytecode_frame_param_overlap_reads_spill_first() {
        // 实参源区间 regs[1..3) 与 callee 形参写入区 regs[2..4) 重叠（first < param_base）：
        // 压帧必须先拷 spill 实参区、形参再从 spill 区取源，避免边写形参边读实参
        // 造成先写后读串值（形参 b 与 spill 实参区都取错）。
        let mut vm = Vm::new();
        // sub_modules[1] = callee：2 个形参，param_base=2（与调用方实参槽 2 重叠）
        let mut callee_mod = CompiledModule::new();
        callee_mod.n_args = 2;
        callee_mod.param_base = 2;
        callee_mod.n_registers = 5;
        callee_mod.bytecode = Arc::from(vec![opcode::encode(opcode::OpCode::RETURN, 0, 0, 0)]);
        vm.install_module_table_for_test(Arc::new(vec![Arc::new(CompiledModule::new()), Arc::new(callee_mod)]));
        vm.active_reg_limit = 8;
        // 调用方实参区 regs[1..3)：arg0=10, arg1=20；regs[2] 同时是 callee 形参槽（param_base=2）
        vm.regs[1] = JsValue::int(10);
        vm.regs[2] = JsValue::int(20);

        let callee = vm.create_function_object(1, vm.current_gen, false, false, false, false);
        vm.push_bytecode_frame(
            callee,
            JsValue::undefined(),
            super::FrameArgs::RegRange { first: 1, count: 2 },
            None,
            None,
            JsValue::undefined(),
            super::FrameContinuation::None,
            0,
        )
        .expect("压帧成功");
        // 形参从 spill 实参区取源：regs[2]=arg0=10，regs[3]=arg1=20（不得被先写覆盖）
        assert_eq!(vm.regs[2], JsValue::int(10), "形参 a 应为实参 arg0");
        assert_eq!(vm.regs[3], JsValue::int(20), "形参 b 应为实参 arg1（不受先写覆盖）");
        // spill 实参区（arguments 对象源）保持原实参值
        let frame = vm.frames.last().expect("压帧后应有帧");
        let base = frame.arguments_base as usize;
        assert_eq!(vm.spill_stack[base], JsValue::int(10), "spill 实参区 arg0");
        assert_eq!(vm.spill_stack[base + 1], JsValue::int(20), "spill 实参区 arg1");
    }
}
