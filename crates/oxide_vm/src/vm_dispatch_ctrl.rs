use crate::vm::{Completion, FrameArgs, FrameContinuation, TryHandler, Vm};
use crate::vm_trace;
use oxide_bytecode::opcode;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::make_private_name_id;
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn dispatch_call(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        let callee_reg = rd;
        let this_reg = a as u8;
        let first_arg_reg = b as u8;
        let callee = self.regs[callee_reg];

        if callee.is_object() {
            let obj_ptr = callee.as_js_object_ptr();
            if !obj_ptr.is_null() {
                let obj = unsafe { &*obj_ptr };
                if obj.is_function() {
                    if obj.is_class_constructor() {
                        return self
                            .raise_type_error("class constructor cannot be invoked without 'new'")
                            .map(|_| true);
                    }
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;
                    // ext 高 8 位 = 调用点存活上界（0 = 未编码/全量），压帧窗口按此截断。
                    let call_window = (ext >> 8) as u8;
                    crate::vm_debug!("CALL rd={} this={} args={} depth={}", rd, this_reg, arg_count, self.frames.len());

                    if obj.native_fn().is_some() {
                        self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count)?;
                        return Ok(true);
                    } else if obj.sub_module_index() > 0 {
                        let sub_idx = obj.sub_module_index() as usize;
                        let this_value = self.regs[this_reg as usize];
                        let is_generator = sub_idx < self.sub_modules.len() && self.sub_modules[sub_idx].is_generator;
                        let is_async = sub_idx < self.sub_modules.len() && self.sub_modules[sub_idx].is_async;
                        // 生成器/异步路径需把实参物化存进状态盒；普通字节码调用直接
                        // 引用寄存器连续区间，免临时堆 Vec（每次调用省 1 次分配）。
                        if is_generator || is_async {
                            let args: Vec<JsValue> = (0..arg_count)
                                .map(|i| self.regs[first_arg_reg.wrapping_add(i as u8) as usize])
                                .collect();
                            // 异步生成器函数调用返回异步生成器迭代器对象。
                            if is_generator && is_async {
                                let gen_pc = self.pc;
                                let gen = self.create_async_generator_object(callee, this_value, &args)?;
                                // 参数初始化抛错已就地展开（unwind 改写 pc 至 catch）：异常值已
                                // 写入 catch 参数，再写结果寄存器会覆盖之——仅在初始化成功（pc 未动）
                                // 时交付生成器对象。
                                if self.pc == gen_pc {
                                    self.regs[0] = gen;
                                }
                                return Ok(false);
                            }
                            // 生成器函数调用返回迭代器对象，不执行函数体。
                            if is_generator {
                                let gen_pc = self.pc;
                                let gen = self.create_generator_object(callee, this_value, &args)?;
                                // 参数初始化抛错已就地展开时不得覆盖 catch 参数（同上）。
                                if self.pc == gen_pc {
                                    self.regs[0] = gen;
                                }
                                return Ok(false);
                            }
                            // 异步函数调用返回 capability promise，立即同步执行 body 到首个 await。
                            let promise = self.create_async_object(callee, this_value, &args)?;
                            self.regs[0] = promise;
                            return Ok(false);
                        }
                        self.push_bytecode_frame(
                            callee,
                            this_value,
                            FrameArgs::RegRange {
                                first: first_arg_reg,
                                count: arg_count,
                            },
                            None,
                            None,
                            JsValue::undefined(),
                            FrameContinuation::None,
                            call_window,
                        )?;
                        return Ok(true);
                    }
                }
            }
        }

        let fn_flag: u8 = if callee.is_object() {
            unsafe { (*callee.as_js_object_ptr()).is_function() as u8 }
        } else {
            2
        };
        crate::vm_debug!(
            "CALL target not callable: rd={} a={} b={} callee={:?} fn_flag={}",
            rd,
            a,
            b,
            callee,
            fn_flag
        );
        self.raise_type_error("CALL target is not callable").map(|_| true)
    }

    pub(crate) fn dispatch_call_native(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let callee_reg = rd;
        let this_reg = a as u8;
        let first_arg_reg = b as u8;
        let callee = self.regs[callee_reg];

        if !callee.is_object() {
            return self.raise_type_error("CALL_NATIVE target is not an object");
        }
        let obj_ptr = callee.as_js_object_ptr();
        if obj_ptr.is_null() {
            return self.raise_type_error("CALL_NATIVE target is null");
        }
        let obj = unsafe { &*obj_ptr };
        // 目标为普通 JS 函数（如用户覆盖了 builtin 名）：回退到通用调用路径，
        // 复用 dispatch_call 的 JS 函数调用逻辑而非强制走 native。
        // pc 此刻指向 ext 字（与 CALL 指令一致），dispatch_call 直接读取。
        if obj.is_function() && obj.native_fn().is_none() {
            return self.dispatch_call(rd, a, b).map(|_| ());
        }
        if !obj.is_function() || obj.native_fn().is_none() {
            return self.raise_type_error("CALL_NATIVE target is not a native function");
        }

        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;
        crate::vm_debug!("CALL_NATIVE rd={} args={}", rd, arg_count);
        self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count)
    }

    pub(crate) fn dispatch_define_accessor(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("DEFINE_ACCESSOR rd={} getter={} setter={}", rd, a, b);
        let prop_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        if prop_idx >= self.immutables().len() {
            return self.raise_type_error("DEFINE_ACCESSOR constant index out of bounds");
        }
        let key_val = self.immutables()[prop_idx];
        // 私有访问器：key 常量编码为 Int（私有名局部 id），映射到私有键高半区；
        // 普通访问器 key 是字符串常量，走 interner 键。
        let prop_name_si = if key_val.is_int() {
            make_private_name_id(key_val.as_int().max(0) as u32)
        } else {
            self.property_key_si(key_val)?
        };
        self.dispatch_define_accessor_common(rd, a, b, prop_name_si)
    }

    /// 计算键访问器：ext[0] 高位标记 `0x8000_0000 | key_reg`，键值运行时从寄存器读取。
    pub(crate) fn dispatch_define_accessor_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("DEFINE_ACCESSOR_DYNAMIC rd={} getter={} setter={}", rd, a, b);
        let key_word = self.bytecode[self.pc];
        self.pc += 1;
        let key_reg = (key_word & 0x7FFF_FFFF) as usize;
        let prop_name_si = self.property_key_si(self.regs[key_reg])?;
        self.dispatch_define_accessor_common(rd, a, b, prop_name_si)
    }

    /// 访问器定义公共路径：读 get/set 槽、合并已有访问器、写属性。
    fn dispatch_define_accessor_common(
        &mut self, rd: usize, a: usize, b: usize, prop_name_si: u32,
    ) -> Result<(), String> {
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return self.raise_type_error("DEFINE_ACCESSOR target is not object");
        }
        let getter = self.regs[a];
        let setter = self.regs[b];
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let existing = self
            .get_own_property_slot(obj, prop_name_si)
            .and_then(|pos| obj.prop_meta_at(pos))
            .filter(|meta| meta.is_accessor);
        let get = if getter.is_undefined() {
            existing.map(|meta| meta.get).unwrap_or(JsValue::undefined())
        } else {
            getter
        };
        let set = if setter.is_undefined() {
            existing.map(|meta| meta.set).unwrap_or(JsValue::undefined())
        } else {
            setter
        };
        match self.define_accessor_property(obj, prop_name_si, get, set, PropAttributes::DEFAULT_DATA) {
            Ok(()) => Ok(()),
            Err(msg) => self.raise_error_kind("TypeError", &msg),
        }
    }

    /// define 数据属性：rd=目标对象，a=值，b=键。与 SET_PROP 不同，不触发原型链
    /// setter，走 `define_data_property`（默认属性 writable/enumerable/configurable）。
    pub(crate) fn dispatch_define_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("DEFINE_PROP rd={} value={} key={}", rd, a, b);
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return self.raise_type_error("DEFINE_PROP target is not object");
        }
        let prop_name_si = self.property_key_si(self.regs[b])?;
        let value = self.regs[a];
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        match self.define_data_property(obj, prop_name_si, value, PropAttributes::DEFAULT_DATA) {
            Ok(()) => Ok(()),
            Err(msg) => self.raise_error_kind("TypeError", &msg),
        }
    }

    /// define 数据属性并指定描述符：ext 字为 attrs（bit0=writable, bit1=enumerable, bit2=configurable，与 PropAttributes 位一致）。
    pub(crate) fn dispatch_define_prop_attrs(
        &mut self, rd: usize, a: usize, b: usize, attrs: u8,
    ) -> Result<(), String> {
        vm_trace!("DEFINE_PROP_ATTRS rd={} value={} key={} attrs={:#04b}", rd, a, b, attrs);
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return self.raise_type_error("DEFINE_PROP_ATTRS target is not object");
        }
        let prop_name_si = self.property_key_si(self.regs[b])?;
        let value = self.regs[a];
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        match self.define_data_property(obj, prop_name_si, value, PropAttributes(attrs)) {
            Ok(()) => Ok(()),
            Err(msg) => self.raise_error_kind("TypeError", &msg),
        }
    }

    /// 定义访问器属性并指定描述符：ext = [key 常量下标, attrs]，key 编码同 DEFINE_ACCESSOR。
    pub(crate) fn dispatch_define_accessor_attrs(
        &mut self, rd: usize, a: usize, b: usize, key_word: u32, attrs: u8,
    ) -> Result<(), String> {
        vm_trace!("DEFINE_ACCESSOR_ATTRS rd={} getter={} setter={} attrs={:#04b}", rd, a, b, attrs);
        let prop_idx = key_word as usize;
        if prop_idx >= self.immutables().len() {
            return self.raise_type_error("DEFINE_ACCESSOR_ATTRS constant index out of bounds");
        }
        let key_val = self.immutables()[prop_idx];
        let prop_name_si = if key_val.is_int() {
            make_private_name_id(key_val.as_int().max(0) as u32)
        } else {
            self.property_key_si(key_val)?
        };
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return self.raise_type_error("DEFINE_ACCESSOR_ATTRS target is not object");
        }
        let getter = self.regs[a];
        let setter = self.regs[b];
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        // 类内 get x 与 set x 分两条指令定义同一属性：第二次到达时已存在访问器，
        // 未提供的半边（undefined）须继承既有值，否则后定义覆盖前定义的 get/set。
        let existing = self
            .get_own_property_slot(obj, prop_name_si)
            .and_then(|pos| obj.prop_meta_at(pos))
            .filter(|meta| meta.is_accessor);
        let get = if getter.is_undefined() {
            existing.map(|meta| meta.get).unwrap_or(JsValue::undefined())
        } else {
            getter
        };
        let set = if setter.is_undefined() {
            existing.map(|meta| meta.set).unwrap_or(JsValue::undefined())
        } else {
            setter
        };
        match self.define_accessor_property(obj, prop_name_si, get, set, PropAttributes(attrs)) {
            Ok(()) => Ok(()),
            Err(msg) => self.raise_error_kind("TypeError", &msg),
        }
    }

    /// 定义全局 var 绑定数据属性：rd=全局对象，a=值，b=键。
    /// 属性可写/可枚举/不可配置（CreateGlobalVarBinding 的属性描述符）。
    ///
    /// # 边界与前提
    /// - 已有同名不可写属性（NaN/undefined/Infinity 等内置）时静默跳过：规范中
    ///   var 绑定仍建立（寄存器侧），全局对象属性保持原样，不抛错。
    /// - 已有同名可写数据属性（重复 var 声明）时按定义更新值。
    pub(crate) fn dispatch_define_global_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("DEFINE_GLOBAL_PROP rd={} value={} key={}", rd, a, b);
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return Ok(());
        }
        let prop_name_si = self.property_key_si(self.regs[b])?;
        let value = self.regs[a];
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let _ = self.define_data_property(obj, prop_name_si, value, PropAttributes::new(true, true, false));
        Ok(())
    }

    /// 定义隐式全局数据属性：ext 字 = 键常量池下标（u16），a 槽 = 值寄存器。
    /// 全局对象由 session 直接解析——函数体内隐式全局写不依赖 this（regs[254]
    /// 不一定是全局对象）。属性可写/可枚举/可配置（未声明标识符 PutValue 与
    /// eval 脚本 var/函数声明的属性描述符）。
    ///
    /// # 步骤
    /// 1. 属性已存在：CreateGlobalVarBinding 不改既有描述符——保持原属性只更新
    ///    值（不可写数据/访问器：strict 抛 TypeError、sloppy 静默 no-op）。
    /// 2. 属性缺失：新建可写/可枚举/可配置属性；全局对象不可扩展 → TypeError
    ///    （两模式均抛，CreateGlobalVarBinding 语义）。
    ///
    /// # 边界与前提
    /// - 键常量必须是字符串（emit 侧保证）；非字符串按错误返回。
    pub(crate) fn dispatch_define_global_prop_c(&mut self, a: usize, key_idx: u16) -> Result<(), String> {
        let idx = key_idx as usize;
        vm_trace!("DEFINE_GLOBAL_PROP_C value={} idx={}", a, idx);
        let key_val = self.immutables().get(idx).copied().unwrap_or(JsValue::undefined());
        if !key_val.is_string() {
            return Err(format!("DEFINE_GLOBAL_PROP_C constant index {idx} is not a string key"));
        }
        let si = self.property_key_si(key_val)?;
        let value = self.regs[a];
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        // SAFETY: 全局对象钉在 session 永久区，指针在 VM 生命周期内有效。
        let obj = unsafe { &mut *global_ptr };
        // 属性已存在：保持原描述符只更新值，不升级 configurable（规范：
        // CreateGlobalVarBinding 对既有属性不改描述符）。
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), si) {
            if let Some(current) = obj.prop_meta_at(pos) {
                let writable = !current.is_accessor && current.attributes.writable();
                if !writable {
                    return if self.current_strict() {
                        self.raise_error_kind("TypeError", "cannot assign to read-only property")
                    } else {
                        Ok(())
                    };
                }
                return self.define_data_property(obj, si, value, current.attributes);
            }
        }
        // 属性缺失：新建；唯一失败面是全局对象不可扩展（两模式均抛 TypeError）。
        match self.define_data_property(obj, si, value, PropAttributes::new(true, true, true)) {
            Ok(()) => Ok(()),
            Err(msg) => self.raise_error_kind("TypeError", &msg),
        }
    }

    /// break 完成：`crossed`（rd 槽）为 emit 词法算出的逃出 finally 域数。
    /// 逐个穿越 finally 后跳转到目标；crossed 为 0 时直接跳转。
    /// ext 字携带逃出的迭代器层数：无 finally 穿越时立即关闭后跳转，有 finally
    /// 时计数随 Completion 悬挂，由 TRY_FINALLY_END 在穿越完成后消费。
    pub(crate) fn dispatch_break(&mut self, instr: u32) -> Result<(), String> {
        let (for_of_count, for_in_count) = self.read_escape_counts();
        let offset = opcode::offset16(instr) as isize;
        let target_pc = ((self.pc as isize) + offset - 1) as usize;
        let crossed = opcode::rd(instr) as usize;
        if let Some(finally_pc) = self.record_completion(Completion::Break {
            target_pc,
            remaining_finally: crossed,
            for_of_count,
            for_in_count,
        }) {
            self.pc = finally_pc;
        } else {
            // 无 finally 穿越：关闭逃出迭代器后跳转；return() 抛错被接管时
            // unwind 已设 pc，跳过跳转。
            match self.close_escaped_iters(for_of_count, for_in_count) {
                Ok(true) => self.pc = target_pc,
                Ok(false) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// continue 完成：同 break，目标为循环继续位置。
    pub(crate) fn dispatch_continue(&mut self, instr: u32) -> Result<(), String> {
        let (for_of_count, for_in_count) = self.read_escape_counts();
        let offset = opcode::offset16(instr) as isize;
        let target_pc = ((self.pc as isize) + offset - 1) as usize;
        let crossed = opcode::rd(instr) as usize;
        if let Some(finally_pc) = self.record_completion(Completion::Continue {
            target_pc,
            remaining_finally: crossed,
            for_of_count,
            for_in_count,
        }) {
            self.pc = finally_pc;
        } else {
            // 无 finally 穿越：关闭逃出迭代器后跳转；return() 抛错被接管时
            // unwind 已设 pc，跳过跳转。
            match self.close_escaped_iters(for_of_count, for_in_count) {
                Ok(true) => self.pc = target_pc,
                Ok(false) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// 读取 BREAK/CONTINUE/RETURN 的 ext 字逃出计数（低 16 位 for-of，高 16 位
    /// for-in）。调用时机：主循环已把 pc 推进到 ext 字位置；两条跳转/返回路径
    /// 都会覆盖 pc，此处无需再推进。
    ///
    /// # 边界与前提
    /// - 手工构造的 IR 可能让 RETURN/BREAK/CONTINUE 落在末位且无 ext 字（pc == len），
    ///   此时按 (0, 0) 处理：无迭代器逃出，语义正确。
    fn read_escape_counts(&self) -> (usize, usize) {
        if self.pc >= self.bytecode.len() {
            return (0, 0);
        }
        let packed = self.bytecode[self.pc];
        ((packed & 0xFFFF) as usize, (packed >> 16) as usize)
    }

    /// 记录一次控制流完成：穿越 `remaining_finally` 个 finally 体后执行完成本身。
    ///
    /// # 步骤
    /// 1. 新完成覆盖在途异常/完成（break/continue/return 是新的突然完成）。
    /// 2. 从栈顶向下扫描：逃出的 catch-only handler 弹出；正在执行的 finally 体
    ///    （`finally_active`）被覆盖弹出并计数；未进入的包裹 finally 计数后进入。
    /// 3. 计数耗尽（全部跨越的 finally 已进入或已覆盖）→ 返回 None（快速路径）。
    ///
    /// # 边界与前提
    /// - 只扫描当前帧深度（`frames.len()`）的 handler；调用者的 handler 不参与——
    ///   break/continue 词法不跨函数，return 也只逃出本函数 finally。
    ///
    /// # 副作用
    /// - 清空 `pending_exception`/`pending_completion`，弹出被逃出的 handler。
    pub(crate) fn record_completion(&mut self, c: Completion) -> Option<usize> {
        self.pending_exception = None;
        self.pending_error_kind = None;
        self.pending_completion = None;
        let depth = self.frames.len();
        let mut remaining = c.remaining_finally();
        loop {
            if remaining == 0 {
                return None;
            }
            let (frame_depth, finally_pc, finally_active) = {
                let h = self.try_stack.last()?;
                (h.frame_depth, h.finally_pc, h.finally_active)
            };
            if frame_depth != depth {
                return None;
            }
            let Some(fp) = finally_pc else {
                // 逃出的 catch-only handler：丢弃防泄漏。
                self.try_stack.pop();
                continue;
            };
            remaining -= 1;
            if finally_active {
                // 正在执行本 finally 体：本完成覆盖它，弹出。
                self.try_stack.pop();
                continue;
            }
            self.try_stack.last_mut().unwrap().finally_active = true;
            self.pending_completion = Some(c.with_remaining(remaining));
            return Some(fp);
        }
    }

    /// return 完成：`crossed` = 当前帧深度内全部 finally handler 数（return 逃出整个
    /// 函数，途中被覆盖的 finally 体也在内），穿越后实际返回。
    /// ext 字携带逃出的迭代器层数，由 TRY_FINALLY_END 在穿越完成后关闭。
    pub(crate) fn dispatch_return(&mut self, instr: u32) -> Result<Option<JsValue>, String> {
        let rd = opcode::rd(instr) as usize;
        let result = self.regs[rd];
        let (for_of_count, for_in_count) = self.read_escape_counts();
        crate::vm_debug!(
            "RETURN depth={} saved_pc={}",
            self.frames.len(),
            self.frames.last().map(|f| f.return_addr).unwrap_or(0)
        );
        // 纯 catch handler 的清理延后到完成消费处：record_completion 在 finally
        // 穿越路径弹出逃出的 catch-only handler；无 finally 时 do_return 弹帧前
        // 兜底清理。若在此提前弹出，return() 抛错经 unwind 展开时将找不到
        // 外围 catch（规范要求新错误替代完成值继续被捕获）。
        let crossed = self
            .try_stack
            .iter()
            .filter(|h| h.frame_depth == self.frames.len() && h.finally_pc.is_some())
            .count();
        if let Some(finally_pc) = self.record_completion(Completion::Return {
            value: result,
            remaining_finally: crossed,
            for_of_count,
            for_in_count,
        }) {
            self.pc = finally_pc;
            return Ok(None);
        }
        // 无 finally 穿越：关闭逃出迭代器后返回；return() 抛错被接管时原完成值
        // 被新错误替代，跳过实际返回。
        match self.close_escaped_iters(for_of_count, for_in_count) {
            Ok(true) => self.do_return(result),
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// 弹出当前帧内所有残留的纯 catch handler（无 finally 域），供 return 逃出
    /// 本函数时清理。保留 finally handler（供完成穿越）与其它帧的 handler。
    ///
    /// # 边界与前提
    /// - 只处理 `frame_depth == frames.len()` 的 handler：return 只逃出本帧，
    ///   调用者的 handler 必须保留。
    /// - 纯 catch 判定为 `finally_pc.is_none()`（不携带 finally 的 TRY_BEGIN）。
    pub(crate) fn pop_frame_catch_handlers(&mut self) {
        let depth = self.frames.len();
        let mut kept = Vec::with_capacity(self.try_stack.len());
        for h in self.try_stack.drain(..) {
            if h.frame_depth == depth && h.finally_pc.is_none() {
                continue;
            }
            kept.push(h);
        }
        self.try_stack = kept;
    }

    /// 实际执行返回：弹出当前帧并交付返回值（供 dispatch_return 与 finally 完成
    /// 恢复共用）。
    pub(crate) fn do_return(&mut self, result: JsValue) -> Result<Option<JsValue>, String> {
        // 兜底：return 完成恢复路径（dispatch_try_finally_end 直达）也可能携带
        // 未清理的纯 catch handler，先弹出再弹帧。
        self.pop_frame_catch_handlers();
        if let Some(frame) = self.frames.pop() {
            self.cell_stack.pop();
            let construct_result_reg = frame.construct_result_reg;
            let constructed_this = frame.constructed_this;
            let is_derived_constructor = frame.is_derived_constructor;
            let super_called = frame.super_called;
            let continuation = frame.continuation;
            vm_trace!("RETURN frame: continuation={:?}, derived={}", continuation, is_derived_constructor);
            self.restore_frame(frame);
            if let (Some(target_reg), Some(constructed_this)) = (construct_result_reg, constructed_this) {
                // 规范 §9.2.2.2：derived 构造器返回非对象值（含 undefined/null/原始值）
                // 且未调用 super() 时抛 ReferenceError；调过 super() 则回退构造 this。
                if is_derived_constructor && !result.is_object() && !super_called {
                    self.raise_error_kind("ReferenceError", "derived constructor must call super()")?;
                    return Ok(None);
                }
                self.regs[target_reg as usize] = if result.is_object() { result } else { constructed_this };
                self.regs[0] = self.regs[target_reg as usize];
                vm_trace!("RETURN constructor: target_reg={} regs[0]={:?}", target_reg, self.regs[0]);
            } else {
                match continuation {
                    FrameContinuation::None => self.regs[0] = result,
                    FrameContinuation::AccessorGet { target_reg } => {
                        self.regs[target_reg as usize] = result;
                        vm_trace!("RETURN accessor_get: target_reg={}", target_reg);
                    }
                    FrameContinuation::AccessorSet => {
                        vm_trace!("RETURN accessor_set");
                    }
                }
            }
            // 父构造器正常返回：super() 调用完成，置位调用方 derived 帧的 super_called。
            // 识别依据：construct_result_reg == Some(254) 仅 SUPER_CALL 压帧产生（emit
            // 从寄存器 ≥1 分配，254 保留给 this）；父构造器抛错走 unwind 弹帧不进
            // 本函数，不置位——后续 this 访问 / 再调 super 仍按未初始化报错。
            if construct_result_reg == Some(254) {
                if let Some(caller) = self.frames.last_mut() {
                    if caller.is_derived_constructor {
                        caller.super_called = true;
                    }
                }
            }
            // 生成器/异步/构造内嵌 dispatch：帧全部弹出后把结果交付恢复方，而非继续执行。
            if self.frames.is_empty() && (self.generator_dispatch || self.async_dispatch || self.construct_dispatch) {
                // 构造内嵌：弹帧已把构造结果（非对象回退 this）写入 regs[0]，直接交付。
                let delivered = if self.construct_dispatch { self.regs[0] } else { result };
                return Ok(Some(delivered));
            }
            Ok(None)
        } else {
            vm_trace!("RETURN top-level: regs[0]={:?}", result);
            Ok(Some(result))
        }
    }

    pub(crate) fn dispatch_throw(&mut self, rd: usize) -> Result<bool, String> {
        let exc_value = self.regs[rd];
        let kind = self.thrown_error_kind(exc_value);
        vm_trace!("THROW pc={} kind={}", self.pc, kind);
        self.exception_value = Some(exc_value);
        self.pending_error_kind = Some(kind);
        self.unwind().map(|_| true)
    }

    pub(crate) fn dispatch_try_begin(&mut self, instr: u32) {
        crate::vm_trace!("TRY_BEGIN frame_depth={}", self.frames.len());
        let offset = opcode::offset16(instr) as isize;
        let catch_pc = if offset == 0 {
            None
        } else {
            Some(((self.pc as isize) + offset - 1) as usize)
        };
        self.try_stack.push(TryHandler {
            catch_pc,
            finally_pc: None,
            finally_active: false,
            frame_depth: self.frames.len(),
            for_of_depth: self.iters.for_of_iters.len(),
        });
    }

    pub(crate) fn dispatch_try_end(&mut self) {
        vm_trace!("TRY_END");
        self.try_stack.pop();
    }

    pub(crate) fn dispatch_try_finally_begin(&mut self, instr: u32) {
        let offset = opcode::offset16(instr) as isize;
        let finally_pc = ((self.pc as isize) + offset - 1) as usize;
        vm_trace!("TRY_FINALLY_BEGIN finally_pc={}", finally_pc);
        self.try_stack.push(TryHandler {
            catch_pc: None,
            finally_pc: Some(finally_pc),
            finally_active: false,
            frame_depth: self.frames.len(),
            for_of_depth: self.iters.for_of_iters.len(),
        });
    }

    /// finally 体入口标记：置位栈顶 try handler 的 `finally_active`。
    ///
    /// # 注意事项
    /// - 所有进入 finally 体的路径（try/catch 体 JMP、catch 体直落、异常 unwind、
    ///   完成穿越、生成器/async 恢复）都先执行本指令，统一完成置位，消除对跳转
    ///   目标匹配的依赖。栈顶在标记执行时必然是本 finally 的 handler。
    /// - 幂等：finally 体已置位（unwind/完成穿越路径先行置位）时重复置 true 无害。
    pub(crate) fn dispatch_try_finally_enter(&mut self) {
        vm_trace!("TRY_FINALLY_ENTER");
        if let Some(h) = self.try_stack.last_mut() {
            h.finally_active = true;
        }
    }

    /// finally 完成分发点：弹出当前 handler 后统一恢复在途异常或控制流完成。
    ///
    /// # 步骤
    /// 1. 弹出刚执行完的 finally handler。
    /// 2. 优先恢复在途异常（unwind 继续向外展开）。
    /// 3. 否则查 `pending_completion`：仍有剩余 finally 体则进入下一个（递减计数）；
    ///    耗尽则执行完成本身（break/continue 跳转目标，return 交付返回值）。
    ///
    /// # 返回值
    /// `Ok(Some(value))` = 函数返回；`Ok(None)` = 正常继续 dispatch。
    pub(crate) fn dispatch_try_finally_end(&mut self) -> Result<Option<JsValue>, String> {
        vm_trace!("TRY_FINALLY_END has_pending_exc={}", self.pending_exception.is_some());
        self.try_stack.pop();
        if self.pending_exception.is_some() && self.exception_value.is_none() {
            self.exception_value = self.pending_exception.take();
            self.unwind()?;
            return Ok(None);
        }
        self.pending_exception = None;
        let Some(c) = self.pending_completion.take() else {
            return Ok(None);
        };
        if c.remaining_finally() > 0 {
            let next = c.with_remaining(c.remaining_finally() - 1);
            if let Some(finally_pc) = self.find_next_finally() {
                self.pending_completion = Some(next);
                self.pc = finally_pc;
                return Ok(None);
            }
        }
        match c {
            Completion::Break {
                target_pc,
                for_of_count,
                for_in_count,
                ..
            }
            | Completion::Continue {
                target_pc,
                for_of_count,
                for_in_count,
                ..
            } => {
                // 全部 finally 穿越完成、真正跳转前关闭逃出迭代器——与规范顺序
                // （循环体完成值已含内部 try-finally 处理，IteratorClose 在后）一致。
                // return() 抛错被接管时 unwind 已设 pc，跳过跳转。
                match self.close_escaped_iters(for_of_count, for_in_count) {
                    Ok(true) => self.pc = target_pc,
                    Ok(false) => {}
                    Err(e) => return Err(e),
                }
                Ok(None)
            }
            Completion::Return {
                value,
                for_of_count,
                for_in_count,
                ..
            } => match self.close_escaped_iters(for_of_count, for_in_count) {
                Ok(true) => self.do_return(value),
                Ok(false) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// 逃出关闭：按 LIFO 弹出逃出的 for-in 迭代器（无 return() 语义），再关闭逃出的
    /// for-of 迭代器。异步迭代器条目（for-await-of）跳过——其 return() 返回 promise
    /// 须 await 后结算，走独立异步关闭机制。逃出路径非 suppress：return() 抛错经
    /// raise_call_error 展开，剩余迭代器由 unwind 的 close_for_of_above 以 suppress
    /// 继续关闭——新错误替代原完成值向外传播。
    ///
    /// # 返回值
    /// `Ok(true)` = 全部关闭完成，调用方继续跳转/返回动作；`Ok(false)` = return()
    /// 抛错且异常已被外围 catch/finally 接管（unwind 已改写 pc），调用方须丢弃
    /// 原完成动作；`Err` = 无处理器，异常逃逸。
    pub(crate) fn close_escaped_iters(&mut self, for_of_count: usize, for_in_count: usize) -> Result<bool, String> {
        for _ in 0..for_in_count {
            self.iters.pop_for_in();
        }
        for _ in 0..for_of_count {
            let Some(entry) = self.iters.pop_for_of() else {
                break;
            };
            // 异步迭代器不在此关闭：for-await-of 的逃出（labeled/return）由
            // 异步关闭机制处理，此处同步关闭会跳过 return() promise 的等待。
            if entry.is_async {
                continue;
            }
            let pc_before = self.pc;
            match self.close_for_of_iterator(entry.iterator, false) {
                Ok(()) => {}
                Err(e) => return Err(e),
            }
            // return() 抛错被接管：unwind 把 pc 改写为 catch/finally 入口，或挂起
            // 在途异常等 finally 穿越。正常关闭时 pc 不变（call_function_sync 保存
            // 并恢复调用方 pc）。
            if self.pc != pc_before || self.pending_exception.is_some() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// 找下一个未被覆盖的包裹 finally 并进入（resume 路径用）：弹出途中被逃出的
    /// catch-only 与防御性的 active handler。无更多返回 None。
    fn find_next_finally(&mut self) -> Option<usize> {
        let depth = self.frames.len();
        loop {
            let (frame_depth, finally_pc, finally_active) = {
                let h = self.try_stack.last()?;
                (h.frame_depth, h.finally_pc, h.finally_active)
            };
            if frame_depth != depth {
                return None;
            }
            let Some(fp) = finally_pc else {
                self.try_stack.pop();
                continue;
            };
            if finally_active {
                self.try_stack.pop();
                continue;
            }
            self.try_stack.last_mut().unwrap().finally_active = true;
            return Some(fp);
        }
    }
}
