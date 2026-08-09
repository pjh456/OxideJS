use crate::native::NativeFn;
use crate::vm::{native_fn_ptr_to_fn, CallFrame, FrameContinuation, Vm};
use crate::{vm_debug, vm_trace};
use oxide_builtins::iterator::make_iterator_for_value;
use oxide_builtins::{builtins_debug, builtins_trace};
use oxide_bytecode::opcode;
use oxide_runtime_api::{to_boolean, NativeResult};
use oxide_types::object::{Cell, JsObject, PropAttributes};
use oxide_types::value::JsValue;
use std::sync::Arc;

impl Vm {
    #[inline(always)]
    pub(crate) fn build_native_args(first_arg_reg: u8, arg_count: usize, this_reg: u8) -> ([u8; 257], usize) {
        let mut args_buf = [0u8; 257];
        args_buf[0] = this_reg;
        let n = arg_count.min(256);
        for i in 0..n {
            args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
        }
        (args_buf, n + 1)
    }

    #[inline(always)]
    pub(crate) fn dispatch_native_call(
        &mut self, obj: &JsObject, callee: JsValue, this_reg: u8, first_arg_reg: u8, arg_count: usize,
    ) -> Result<(), String> {
        if self.native_call_depth >= self.kernel_core.config.max_call_depth {
            return self.raise_error_kind("RangeError", "Maximum call stack size exceeded");
        }
        let (args_buf, len) = Self::build_native_args(first_arg_reg, arg_count, this_reg);
        let args_slice = &args_buf[..len];
        builtins_debug!("native_call depth={} args={}", self.native_call_depth, arg_count);

        // SAFETY: native_fn 经 set_native_fn 以合法 NativeFn 指针设置；
        // native_fn_ptr_to_fn 是 NativeFnPtr → NativeFn 的唯一强制转换点。
        let func: NativeFn = unsafe { native_fn_ptr_to_fn(obj.native_fn().unwrap()) };
        // regs[254] 是调用方的 `this` 寄存器，同时充当分发型 builtin
        // （Function.prototype.bind/call/apply）读取的"当前 callee"槽。native 调用与
        // 调用方共享扁平寄存器文件，因此先快照调用方 `this`，在 native 执行期间暴露
        // callee，随后恢复——否则每次 native 调用都会把自己的函数对象留在调用方的
        // `this` 寄存器里。
        let saved_this = self.regs[254];
        self.regs[254] = callee;
        self.native_call_depth += 1;
        let result = func(self, args_slice);
        self.native_call_depth -= 1;
        self.regs[254] = saved_this;
        match result {
            NativeResult::Ok(val) => {
                builtins_trace!("native_call ok depth={}", self.native_call_depth);
                self.regs[0] = val;
                Ok(())
            }
            NativeResult::Err(err_val) => {
                let (error, kind) = if err_val.is_object() {
                    (err_val, self.thrown_error_kind(err_val))
                } else {
                    let msg = if err_val.is_string() {
                        // SAFETY: err_val 是字符串值。
                        unsafe { (*err_val.as_string_ptr()).data.clone() }
                    } else {
                        format!("{err_val}")
                    };
                    builtins_debug!("native_call err={}", msg);
                    (oxide_builtins::error::create_error(self, &msg), "Error")
                };
                self.exception_value = Some(error);
                self.pending_error_kind = Some(kind);
                self.unwind()
            }
            NativeResult::TailCall { callee, this, args } => {
                if callee.is_object() {
                    let obj = unsafe { &*callee.as_js_object_ptr() };
                    if obj.native_fn().is_some() {
                        match self.call_function_sync(callee, this, &args) {
                            Ok(val) => {
                                self.regs[0] = val;
                                return Ok(());
                            }
                            Err(_) => {
                                // call_function_sync 已把 native 错误展平为 String，
                                // 原始 JsValue 暂存在 last_uncaught_value——恢复为 JS 异常，
                                // 使外围 try/catch 能捕获（而非作为引擎错误上抛）。
                                let exc = self
                                    .last_uncaught_value
                                    .take()
                                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, "call failed"));
                                let kind = self.thrown_error_kind(exc);
                                self.exception_value = Some(exc);
                                self.pending_error_kind = Some(kind);
                                self.unwind()?;
                                return Ok(());
                            }
                        }
                    }
                }
                self.push_bytecode_frame(callee, this, &args, None, None, JsValue::undefined(), FrameContinuation::None)
            }
        }
    }
}

impl Vm {
    pub(crate) fn dispatch_create_closure(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let sub_idx = opcode::imm16(instr) as u32;
        vm_trace!("CREATE_CLOSURE rd={} sub_idx={}", rd, sub_idx);
        if sub_idx == 0 || (sub_idx as usize) >= self.sub_modules.len() {
            // 逃逸闭包（函数对象在定义模块之外被创建）时 sub_idx 相对定义模块，
            // 超出当前 sub_modules 上下文——按运行时错误处理而非索引越界 panic。
            return Err(format!(
                "CREATE_CLOSURE: sub_module_index {} out of bounds (max {})",
                sub_idx,
                self.sub_modules.len()
            ));
        }
        let sub = &self.sub_modules[sub_idx as usize];
        let is_arrow = sub.is_arrow;
        let is_class_constructor = sub.is_class_constructor;
        let is_derived_constructor = sub.is_derived_constructor;
        let needs_home_object = sub.needs_home_object;
        let upvalue_captures = sub.upvalue_captures.clone();
        let function_name = sub.function_name.clone();
        let function_length = sub.function_length;
        let result = self.create_function_object(
            sub_idx,
            is_arrow,
            is_class_constructor,
            is_derived_constructor,
            needs_home_object,
        );
        // 函数名推断：emit 端在变量声明/对象属性赋值点设置 function_name。
        let func_obj = unsafe { &mut *result.as_js_object_ptr() };
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        // length/name 描述符均为不可写、不可枚举、可配置（SetFunctionLength /
        // SetFunctionName 语义）；length 先于 name 定义保证属性序 [prototype, length, name]。
        let attrs = PropAttributes::new(false, false, true);
        let length_val = JsValue::int(function_length as i32);
        self.define_data_property(func_obj, length_si, length_val, attrs)?;
        let name_val = function_name
            .as_deref()
            .map(|n| self.new_string(n))
            .unwrap_or_else(|| self.new_string(""));
        self.define_data_property(func_obj, name_si, name_val, attrs)?;
        if !upvalue_captures.is_empty() {
            // 链式捕获（parent_uv_idx）：从父闭包（创建者）的 upvalues 取 cell。
            let parent_upvalues: Vec<*mut Cell> = match self.current_callee() {
                Some(callee) if callee.is_object() => unsafe { &*callee.as_js_object_ptr() }.upvalues_slice().to_vec(),
                _ => Vec::new(),
            };
            if let Some(current_cells) = self.cell_stack.last_mut() {
                let mut upvals = Box::new(Vec::with_capacity(upvalue_captures.len()));
                for capture in &upvalue_captures {
                    let cell_ptr = if let Some(puv) = capture.parent_uv_idx {
                        parent_upvalues.get(puv as usize).copied().unwrap_or(std::ptr::null_mut())
                    } else {
                        let cell_idx = capture.cell_idx as usize;
                        if cell_idx >= current_cells.len() {
                            current_cells.resize(cell_idx + 1, std::ptr::null_mut());
                        }
                        // cell 可能尚未 MAKE_CELL（hoisting 顺序：CREATE_CLOSURE 先于
                        // MAKE_CELL）——建占位 Cell，后续 MAKE_CELL 更新同一 Cell 的值，
                        // 保证 upvalue 始终指向定义方表里的稳定 cell（而非调用方表）。
                        if current_cells[cell_idx].is_null() {
                            // 占位 Cell 保持未初始化（TDZ）直到 MAKE_CELL 置位；
                            // 若绑定为 var 则其初始化 MAKE_CELL 在函数序言先于任何读取执行。
                            let cell = self.gc_state.session_epoch.alloc(Cell::new(JsValue::undefined(), false));
                            current_cells[cell_idx] = cell as *mut Cell;
                        }
                        current_cells[cell_idx]
                    };
                    upvals.push(cell_ptr);
                }
                let func_obj = unsafe { &mut *result.as_js_object_ptr() };
                func_obj.set_upvalues(upvals);
            }
        }
        self.regs[rd] = result;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn dispatch_make_cell(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let cell_idx = opcode::imm16(instr) as usize;
        let value = self.regs[rd];
        let current = self.cell_stack.last_mut().unwrap();
        while current.len() <= cell_idx {
            current.push(std::ptr::null_mut());
        }
        if current[cell_idx].is_null() {
            // 无占位 cell：新建。
            let cell = self.gc_state.session_epoch.alloc(Cell::new(value, true));
            current[cell_idx] = cell as *mut Cell;
        } else {
            // 更新占位 cell（CREATE_CLOSURE 已建），使闭包 upvalue 指向的
            // cell 值跟随初始化；赋值即解除 TDZ。
            unsafe {
                let cell = &mut *current[cell_idx];
                cell.value = value;
                cell.set_initialized(true);
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn dispatch_cell_get(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let cell_idx = b;
        let current = self.cell_stack.last_mut().unwrap();
        while current.len() <= cell_idx {
            current.push(std::ptr::null_mut());
        }
        if current[cell_idx].is_null() {
            let val = self.regs[a];
            let cell = self.gc_state.session_epoch.alloc(Cell::new(val, true));
            current[cell_idx] = cell as *mut Cell;
        }
        let c = unsafe { &*current[cell_idx] };
        if !c.is_initialized() {
            return self.raise_error_kind("ReferenceError", "Cannot access variable before initialization");
        }
        self.regs[rd] = c.value;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn dispatch_cell_set(&mut self, a: usize, b: usize) -> Result<(), String> {
        let cell_idx = b;
        let src_val = self.regs[a];
        let current = self.cell_stack.last_mut().unwrap();
        if cell_idx >= current.len() {
            return Ok(());
        }
        let cell_ptr = current[cell_idx];
        if cell_ptr.is_null() {
            return Ok(());
        }
        unsafe {
            let cell = &mut *cell_ptr;
            cell.value = src_val;
            cell.set_initialized(true);
        }
        Ok(())
    }

    /// 当前执行函数的闭包对象：普通路径取 frames 栈顶帧，inline（sync 回调）路径
    /// frames 被隔离为空，取 `inline_callee`。
    pub(crate) fn current_callee(&self) -> Option<JsValue> {
        self.frames.last().map(|f| f.callee).or(self.inline_callee)
    }

    #[allow(dead_code)]
    pub(crate) fn dispatch_load_upvalue(&mut self, rd: usize, instr: u32) -> Result<(), String> {
        let uv_idx = opcode::imm16(instr) as usize;
        if let Some(callee) = self.current_callee() {
            if callee.is_object() {
                let obj = unsafe { &*callee.as_js_object_ptr() };
                let upvals = obj.upvalues_slice();
                if uv_idx < upvals.len() {
                    let cell = upvals[uv_idx];
                    if !cell.is_null() {
                        let c = unsafe { &*cell };
                        if !c.is_initialized() {
                            return self
                                .raise_error_kind("ReferenceError", "Cannot access variable before initialization");
                        }
                        self.regs[rd] = c.value;
                        return Ok(());
                    }
                }
            }
        }
        // cell 尚未创建（hoisting 顺序：CREATE_CLOSURE 先于 MAKE_CELL），
        // 用调用方寄存器值惰性创建 cell。
        self.lazy_create_upvalue_cell(rd, uv_idx)
    }

    fn lazy_create_upvalue_cell(&mut self, rd: usize, uv_idx: usize) -> Result<(), String> {
        if let Some(callee) = self.current_callee() {
            if callee.is_object() {
                let obj = unsafe { &mut *callee.as_js_object_ptr() };
                let upvals = obj.upvalues_slice_mut();
                if uv_idx < upvals.len() {
                    // 尝试从调用方 cell 表取初值（MAKE_CELL 在 CREATE_CLOSURE 之后执行）。
                    let val = if self.cell_stack.len() >= 2 {
                        let caller_cells = &self.cell_stack[self.cell_stack.len() - 2];
                        if uv_idx < caller_cells.len() && !caller_cells[uv_idx].is_null() {
                            unsafe { (*caller_cells[uv_idx]).value }
                        } else {
                            self.regs[rd]
                        }
                    } else {
                        self.regs[rd]
                    };
                    let cell = self.gc_state.session_epoch.alloc(Cell::new(val, true));
                    upvals[uv_idx] = cell as *mut Cell;
                    self.regs[rd] = cell.value;
                    return Ok(());
                }
            }
        }
        self.regs[rd] = JsValue::undefined();
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn dispatch_store_upvalue(&mut self, a: usize, b: usize) -> Result<(), String> {
        let uv_idx = b;
        let src_val = self.regs[a];
        if let Some(callee) = self.current_callee() {
            if callee.is_object() {
                let obj = unsafe { &mut *callee.as_js_object_ptr() };
                let upvals = obj.upvalues_slice_mut();
                if uv_idx < upvals.len() {
                    if !upvals[uv_idx].is_null() {
                        unsafe {
                            let cell = &mut *upvals[uv_idx];
                            cell.value = src_val;
                            cell.set_initialized(true);
                        }
                        vm_debug!("STORE_UPVALUE len={} wrote existing", upvals.len());
                    } else {
                        let cell = self.gc_state.session_epoch.alloc(Cell::new(src_val, true));
                        upvals[uv_idx] = cell as *mut Cell;
                    }
                }
            }
        }
        Ok(())
    }
}

impl Vm {
    pub(crate) fn dispatch_create_regexp(&mut self, rd: usize, a: usize, b: usize) -> Result<Option<JsValue>, String> {
        vm_trace!("CREATE_REGEXP rd={}", rd);
        let pat_val = self.regs[a];
        let flags_val = self.regs[b];
        let ctor_ptr = self.session.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
        let ctor = unsafe { &*ctor_ptr };
        let Some(native_fn) = ctor.native_fn() else {
            self.raise_error_kind("TypeError", "RegExp constructor unavailable")?;
            return Ok(Some(JsValue::undefined()));
        };
        let saved_0 = self.regs[0];
        let saved_1 = self.regs[1];
        let saved_2 = self.regs[2];
        self.regs[0] = JsValue::undefined();
        self.regs[1] = pat_val;
        self.regs[2] = flags_val;
        let func = unsafe { native_fn_ptr_to_fn(native_fn) };
        let result = match func(self, &[0, 1, 2]) {
            NativeResult::Ok(v) => v,
            NativeResult::Err(e) => {
                // 保留原始错误对象与类型（非法正则字面量须抛 SyntaxError，非 TypeError）。
                self.exception_value = Some(e);
                self.pending_error_kind = Some(self.thrown_error_kind(e));
                return self.unwind().map(|_| None);
            }
            NativeResult::TailCall { .. } => {
                return Err(self.error_message_text("TypeError", "unexpected tail call"));
            }
        };
        self.regs[0] = saved_0;
        self.regs[1] = saved_1;
        self.regs[2] = saved_2;
        self.regs[rd] = result;
        Ok(None)
    }

    pub(crate) fn dispatch_super_call(&mut self, rd: usize, a: usize) -> Result<bool, String> {
        vm_debug!("SUPER_CALL depth={}", self.frames.len());
        let first_arg_reg = a as u8;
        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;

        let Some(frame) = self.frames.last() else {
            self.raise_error_kind("ReferenceError", "super() used outside class constructor")?;
            return Ok(true);
        };
        if !frame.is_derived_constructor {
            self.raise_error_kind("ReferenceError", "super() used outside derived constructor")?;
            return Ok(true);
        }
        if !self.regs[254].is_undefined() {
            self.raise_error_kind("ReferenceError", "super() called more than once")?;
            return Ok(true);
        }
        let Some(derived_this) = frame.constructed_this else {
            self.raise_error_kind("ReferenceError", "super() without derived this")?;
            return Ok(true);
        };

        let new_target = self.regs[255];
        if !new_target.is_object() {
            self.raise_error_kind("TypeError", "super() new.target is not an object")?;
            return Ok(true);
        }
        let new_target_obj = unsafe { &*new_target.as_js_object_ptr() };
        let super_ctor = new_target_obj.proto();
        if !super_ctor.is_object() {
            self.raise_error_kind("TypeError", "super constructor is not an object")?;
            return Ok(true);
        }
        let super_obj = unsafe { &*super_ctor.as_js_object_ptr() };
        if !super_obj.is_function() {
            self.raise_error_kind("TypeError", "super constructor is not a function")?;
            return Ok(true);
        }

        if super_obj.native_fn().is_some() {
            self.regs[253] = derived_this;
            self.regs[254] = super_ctor; // bind_dispatcher 把 regs[254] 当作包装 callee 读取
            let (args_buf, len) = Self::build_native_args(first_arg_reg, arg_count, 253);
            // SAFETY: native_fn 经 set_native_fn 以合法 NativeFn 指针设置；
            // native_fn_ptr_to_fn 是 NativeFnPtr → NativeFn 的唯一强制转换点。
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(super_obj.native_fn().unwrap()) };
            match func(self, &args_buf[..len]) {
                NativeResult::Ok(val) => {
                    // super() 返回实例的 [[Prototype]] 须设为 new.target.prototype
                    //（native 构造器不知道 new.target，由调用方设置）。
                    let instance = if val.is_object() { val } else { derived_this };
                    if instance.is_object() {
                        self.set_constructed_proto(instance, new_target_obj)?;
                    }
                    self.regs[254] = instance;
                    self.regs[rd] = self.regs[254];
                }
                NativeResult::Err(err_val) => {
                    self.exception_value = Some(err_val);
                    self.pending_error_kind = Some(self.thrown_error_kind(err_val));
                    match self.unwind() {
                        Ok(()) => return Ok(true),
                        Err(e) => return Err(e),
                    }
                }
                NativeResult::TailCall { callee, this, args } => {
                    // 如 bound 函数：解析尾调用，用其返回值作为构造实例
                    // （或回退到 derived_this）。
                    match self.call_function_sync(callee, this, &args) {
                        Ok(val) => {
                            let instance = if val.is_object() { val } else { derived_this };
                            if instance.is_object() {
                                self.set_constructed_proto(instance, new_target_obj)?;
                            }
                            self.regs[254] = instance;
                            self.regs[rd] = self.regs[254];
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        } else if super_obj.sub_module_index() > 0 {
            let sub_idx = super_obj.sub_module_index() as usize;
            if sub_idx >= self.sub_modules.len() {
                return Err(format!(
                    "SUPER_CALL: sub_module_index {} out of bounds (max {})",
                    sub_idx,
                    self.sub_modules.len()
                ));
            }
            if self.frames.len() >= self.kernel_core.config.max_call_depth {
                return Err(self.error_message_text("RangeError", "Maximum call stack size exceeded"));
            }

            let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
            let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
            let sub_n_registers = self.sub_modules[sub_idx].n_registers;
            let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
            let caller_reg_limit = self.active_reg_limit.max(1);
            let saved_reg_offset = self.save_stack.len() as u32;
            self.save_stack.extend_from_slice(&self.regs[..caller_reg_limit as usize]);
            let saved_this = self.regs[254];
            let saved_new_target = self.regs[255];

            // 完整实参写入 spill 栈实参区（在帧的 spill 区之前），供 CREATE_ARGUMENTS 使用。
            let args_base = self.spill_stack.len() as u32;
            for i in 0..arg_count {
                let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                self.spill_stack.push(self.regs[src_reg]);
            }
            let args_count = arg_count.min(u16::MAX as usize) as u16;

            for i in 0..sub_n_args {
                let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                self.regs[sub_param_base + i] = self.regs[src_reg];
            }
            self.regs[254] = derived_this;
            self.regs[255] = new_target;

            self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
            self.saved_immutables_stack.push(self.active_immutables);

            let function_name = self.sub_modules[sub_idx]
                .function_name
                .as_deref()
                .map(|name| self.kernel_core.perm_interner().intern(name).0)
                .unwrap_or(0);

            self.frames.push(CallFrame {
                return_addr: self.pc,
                function_name,
                caller_reg_limit,
                saved_reg_offset,
                spill_offset: self.spill_stack.len() as u32,
                arguments_base: args_base,
                arguments_count: args_count,
                saved_this,
                saved_new_target,
                callee: super_ctor,
                construct_result_reg: Some(254),
                constructed_this: Some(derived_this),
                is_derived_constructor: super_obj.is_derived_constructor(),
                continuation: FrameContinuation::None,
            });

            self.bytecode = sub_bytecode;
            let subs = Arc::clone(&self.sub_modules);
            self.activate_immutables(sub_idx, &subs[sub_idx].constants);
            self.cell_stack.push(Vec::with_capacity(subs[sub_idx].cells_needed as usize));
            for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map.clone() {
                let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
                let global = self.session.global_object();
                if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                    self.regs[*reg as usize] = global.get_prop_at(pos);
                }
            }

            self.active_reg_limit = sub_n_registers.max(1);
            self.pc = 0;
            return Ok(true);
        } else {
            self.raise_error_kind("TypeError", "super constructor is not callable")?;
            return Ok(true);
        }
        Ok(false)
    }

    /// super() 返回的构造实例其 [[Prototype]] 设为 new.target.prototype。
    fn set_constructed_proto(&mut self, instance: JsValue, new_target_obj: &JsObject) -> Result<(), String> {
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(new_target_obj, proto_si) {
            if proto_val.is_object() {
                let obj = unsafe { &mut *instance.as_js_object_ptr() };
                obj.set_proto(proto_val).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub(crate) fn dispatch_super_get_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        vm_trace!("SUPER_GET_PROP rd={} a={} b={}", rd, a, b);
        let key_val = self.regs[b];
        let prop_name_si = self.property_key_si(key_val);
        let Some(frame) = self.frames.last() else {
            self.raise_error_kind("ReferenceError", "super property used outside function")?;
            return Ok(true);
        };
        if !frame.callee.is_object() {
            self.raise_error_kind("ReferenceError", "super property has no home object")?;
            return Ok(true);
        }
        let callee_obj = unsafe { &*frame.callee.as_js_object_ptr() };
        let home_object = callee_obj.home_object();
        if !home_object.is_object() {
            self.raise_error_kind("ReferenceError", "super property has no home object")?;
            return Ok(true);
        }
        let home_obj = unsafe { &*home_object.as_js_object_ptr() };
        let super_base = home_obj.proto();
        if !super_base.is_object() {
            self.regs[rd] = JsValue::undefined();
        } else {
            let super_obj = unsafe { &*super_base.as_js_object_ptr() };
            let val = self.ordinary_get_with_target(super_obj, prop_name_si, self.regs[a], rd as u8)?;
            if self.accessor_frame_target_reg.take().is_none() {
                self.regs[rd] = val;
            }
        }
        Ok(false)
    }

    pub(crate) fn dispatch_set_home_object(&mut self, rd: usize, a: usize) -> Result<bool, String> {
        vm_trace!("SET_HOME_OBJECT rd={} a={}", rd, a);
        let func_val = self.regs[rd];
        let home_val = self.regs[a];
        if !func_val.is_object() || !home_val.is_object() {
            self.raise_error_kind("TypeError", "SET_HOME_OBJECT expects function and object")?;
            return Ok(true);
        }
        let home_val = self.promote_if_needed_for_write_ptr(func_val.as_js_object_ptr(), home_val);
        let func_obj = unsafe { &mut *func_val.as_js_object_ptr() };
        if !func_obj.is_function() {
            self.raise_error_kind("TypeError", "SET_HOME_OBJECT target is not a function")?;
            return Ok(true);
        }
        func_obj.set_home_object(home_val);
        Ok(false)
    }
}

impl Vm {
    /// 解析 spread 调用系 ext：读走 `nstatic | (nspread<<8)` 首字与 `nstatic+nspread`
    /// 个有序实参字（静态字 = 寄存器号，spread 字高位标记）。
    fn read_spread_ext(&mut self) -> Vec<usize> {
        let header = self.bytecode[self.pc];
        self.pc += 1;
        let nstatic = (header & 0xFF) as usize;
        let nspread = (header >> 8) as usize;
        let mut words = Vec::with_capacity(nstatic + nspread);
        for _ in 0..nstatic + nspread {
            words.push(self.bytecode[self.pc] as usize);
            self.pc += 1;
        }
        words
    }

    /// 按源码求值序物化完整实参 Vec：静态字直接读寄存器，spread 字迭代展开。
    ///
    /// # 步骤
    /// 1. 静态实参逐个从寄存器读入；spread 源经 `make_iterator_for_value` 迭代追加。
    /// 2. 迭代中途抛错 → 当前迭代器经 return() 关闭（IteratorClose），异常恢复为 JS
    ///    异常后 unwind。
    /// 3. 展开总数超过 u16 上限（arguments_count 字段）→ RangeError。
    ///
    /// # 返回值
    /// - `Ok(Some(vec))` 成功；
    /// - `Ok(None)` 异常已展开（调用方按 continue 处理）；
    /// - `Err` 引擎错误直接传播。
    fn materialize_spread_args(&mut self, words: &[usize]) -> Result<Option<Vec<JsValue>>, String> {
        let mut args = Vec::with_capacity(words.len());
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let value_si = self.kernel_core.perm_interner().intern("value").0;
        for &w in words {
            if w >> 31 == 1 {
                let value = self.regs[w & 0x7FFF_FFFF];
                let iterator = match make_iterator_for_value(self, value) {
                    Ok(it) => it,
                    Err(err) => {
                        self.exception_value = Some(err);
                        self.pending_error_kind = Some(self.thrown_error_kind(err));
                        return self.unwind().map(|_| None);
                    }
                };
                loop {
                    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
                    let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
                        Ok(v) => v,
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    let result = match self.call_function_sync(next_fn, iterator, &[]) {
                        Ok(v) => v,
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    if !result.is_object() {
                        return self.spread_iter_error(
                            iterator,
                            self.error_message_text("TypeError", "iterator result is not an object"),
                            "TypeError",
                        );
                    }
                    let result_obj = unsafe { &*result.as_js_object_ptr() };
                    let done = match self.ordinary_get(result_obj, done_si, result) {
                        Ok(v) => to_boolean(v),
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    if done {
                        break;
                    }
                    let val = match self.ordinary_get(result_obj, value_si, result) {
                        Ok(v) => v,
                        Err(e) => return self.spread_iter_error(iterator, e, "Error"),
                    };
                    args.push(val);
                    if args.len() > u16::MAX as usize {
                        self.raise_error_kind("RangeError", "Too many arguments in function call")?;
                        return Ok(None);
                    }
                }
            } else {
                args.push(self.regs[w]);
                if args.len() > u16::MAX as usize {
                    self.raise_error_kind("RangeError", "Too many arguments in function call")?;
                    return Ok(None);
                }
            }
        }
        Ok(Some(args))
    }

    /// spread 迭代中途异常：先关闭当前迭代器（suppress return 自身错误，保留在途异常），
    /// 再把原异常（native 错误经 last_uncaught_value 恢复）重新抛出并展开。
    fn spread_iter_error(
        &mut self, iterator: JsValue, msg: String, fallback_kind: &'static str,
    ) -> Result<Option<Vec<JsValue>>, String> {
        let exc = self
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_kind_error(self, fallback_kind, &msg));
        self.close_for_of_iterator(iterator, true)?;
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(self.thrown_error_kind(exc));
        self.unwind().map(|_| None)
    }

    /// CALL_SPREAD：运行期物化完整实参后按目标分派（native 走值传递，字节码走帧）。
    pub(crate) fn dispatch_call_spread(&mut self, rd: usize, a: usize, _b: usize) -> Result<bool, String> {
        let callee = self.regs[rd];
        let this_value = self.regs[a];
        let words = self.read_spread_ext();
        let args = match self.materialize_spread_args(&words)? {
            Some(args) => args,
            None => return Ok(true),
        };
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
                    if obj.native_fn().is_some() {
                        match self.call_function_sync(callee, this_value, &args) {
                            Ok(v) => {
                                self.regs[0] = v;
                                return Ok(false);
                            }
                            Err(_) => {
                                let exc = self
                                    .last_uncaught_value
                                    .take()
                                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, "call failed"));
                                self.exception_value = Some(exc);
                                self.pending_error_kind = Some(self.thrown_error_kind(exc));
                                return self.unwind().map(|_| true);
                            }
                        }
                    } else if obj.sub_module_index() > 0 {
                        self.push_bytecode_frame(
                            callee,
                            this_value,
                            &args,
                            None,
                            None,
                            JsValue::undefined(),
                            FrameContinuation::None,
                        )?;
                        return Ok(true);
                    }
                }
            }
        }
        self.raise_type_error("CALL target is not callable").map(|_| true)
    }

    /// NEW_EXPRESSION_SPREAD：物化实参后按构造函数目标分派（native 走值传递，字节码走帧）。
    pub(crate) fn dispatch_new_expression_spread(&mut self, rd: usize, a: usize, _b: usize) -> Result<bool, String> {
        let constructor_reg = a;
        let constructor = self.regs[constructor_reg];

        if !constructor.is_object() {
            return self
                .raise_type_error("NEW_EXPRESSION: constructor is not an object")
                .map(|_| true);
        }
        let ctor_ptr = constructor.as_js_object_ptr();
        if ctor_ptr.is_null() {
            return self.raise_type_error("NEW_EXPRESSION: constructor is null").map(|_| true);
        }
        let ctor_obj = unsafe { &*ctor_ptr };
        if !ctor_obj.is_function() {
            return self
                .raise_type_error("NEW_EXPRESSION: constructor is not a function")
                .map(|_| true);
        }
        if ctor_obj.is_arrow() {
            return self
                .raise_type_error("arrow functions cannot be used as constructors")
                .map(|_| true);
        }
        // native 方法（非构造器）不可 new。
        if ctor_obj.native_fn().is_some() && ctor_obj.type_tag != oxide_types::object::JsObject::OBJ_TYPE_CONSTRUCTOR {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }

        let words = self.read_spread_ext();
        let args = match self.materialize_spread_args(&words)? {
            Some(args) => args,
            None => return Ok(true),
        };

        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let new_obj = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(ctor_obj, proto_si) {
            if proto_val.is_object() {
                let new_obj_mut = unsafe { &mut *new_obj };
                let proto_obj_ptr = proto_val.as_js_object_ptr();
                let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
            }
        }
        let new_obj_val = JsValue::object(new_obj as *mut u8);

        if ctor_obj.native_fn().is_some() {
            // native 构造器：receiver 为新对象，值传递调用。
            match self.call_function_sync(constructor, new_obj_val, &args) {
                Ok(v) => {
                    self.regs[rd] = if v.is_object() { v } else { new_obj_val };
                    Ok(false)
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, "constructor call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    self.unwind().map(|_| true)
                }
            }
        } else if ctor_obj.sub_module_index() > 0 {
            if self.frames.len() >= self.kernel_core.config.max_call_depth {
                return Err(self.error_message_text("RangeError", "Maximum call stack size exceeded"));
            }
            let this_value = if ctor_obj.is_derived_constructor() {
                JsValue::undefined()
            } else {
                new_obj_val
            };
            self.push_bytecode_frame(
                constructor,
                this_value,
                &args,
                Some(rd as u8),
                Some(new_obj_val),
                constructor,
                FrameContinuation::None,
            )?;
            Ok(true)
        } else {
            let error =
                oxide_builtins::error::create_error(self, "NEW_EXPRESSION: bytecode constructors not yet supported");
            self.exception_value = Some(error);
            self.pending_error_kind = Some(self.thrown_error_kind(error));
            self.unwind().map(|_| true)
        }
    }

    /// SUPER_CALL_SPREAD：物化实参后按父构造器目标分派（native 走值传递，字节码走帧）。
    pub(crate) fn dispatch_super_call_spread(&mut self, rd: usize, _a: usize) -> Result<bool, String> {
        let words = self.read_spread_ext();

        let Some(frame) = self.frames.last() else {
            self.raise_error_kind("ReferenceError", "super() used outside class constructor")?;
            return Ok(true);
        };
        if !frame.is_derived_constructor {
            self.raise_error_kind("ReferenceError", "super() used outside derived constructor")?;
            return Ok(true);
        }
        if !self.regs[254].is_undefined() {
            self.raise_error_kind("ReferenceError", "super() called more than once")?;
            return Ok(true);
        }
        let Some(derived_this) = frame.constructed_this else {
            self.raise_error_kind("ReferenceError", "super() without derived this")?;
            return Ok(true);
        };
        let new_target = self.regs[255];
        if !new_target.is_object() {
            self.raise_error_kind("TypeError", "super() new.target is not an object")?;
            return Ok(true);
        }
        let new_target_obj = unsafe { &*new_target.as_js_object_ptr() };
        let super_ctor = new_target_obj.proto();
        if !super_ctor.is_object() {
            self.raise_error_kind("TypeError", "super constructor is not an object")?;
            return Ok(true);
        }
        let super_obj = unsafe { &*super_ctor.as_js_object_ptr() };
        if !super_obj.is_function() {
            self.raise_error_kind("TypeError", "super constructor is not a function")?;
            return Ok(true);
        }

        let args = match self.materialize_spread_args(&words)? {
            Some(args) => args,
            None => return Ok(true),
        };

        if super_obj.native_fn().is_some() {
            match self.call_function_sync(super_ctor, derived_this, &args) {
                Ok(val) => {
                    // super() 返回实例的 [[Prototype]] 须设为 new.target.prototype
                    let instance = if val.is_object() { val } else { derived_this };
                    if instance.is_object() {
                        self.set_constructed_proto(instance, new_target_obj)?;
                    }
                    self.regs[254] = instance;
                    self.regs[rd] = instance;
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, "super() call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    self.unwind().map(|_| true)?;
                }
            }
            Ok(false)
        } else if super_obj.sub_module_index() > 0 {
            if self.frames.len() >= self.kernel_core.config.max_call_depth {
                return Err(self.error_message_text("RangeError", "Maximum call stack size exceeded"));
            }
            self.push_bytecode_frame(
                super_ctor,
                derived_this,
                &args,
                Some(254),
                Some(derived_this),
                new_target,
                FrameContinuation::None,
            )?;
            Ok(true)
        } else {
            self.raise_error_kind("TypeError", "super constructor is not callable")?;
            Ok(true)
        }
    }
}
