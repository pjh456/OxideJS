use crate::native::NativeFn;
use crate::vm::{native_fn_ptr_to_fn, CallFrame, FrameContinuation, Vm};
use oxide_bytecode::opcode;
use oxide_kernel::{builtins_debug, builtins_trace};
use oxide_runtime_api::NativeResult;
use oxide_types::object::JsObject;
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

        // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
        // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
        let func: NativeFn = unsafe { native_fn_ptr_to_fn(obj.native_fn().unwrap()) };
        self.regs[254] = callee;
        self.native_call_depth += 1;
        match func(self, args_slice) {
            NativeResult::Ok(val) => {
                self.native_call_depth -= 1;
                builtins_trace!("native_call ok depth={}", self.native_call_depth);
                self.regs[0] = val;
                Ok(())
            }
            NativeResult::Err(err_val) => {
                self.native_call_depth -= 1;
                let (error, kind) = if err_val.is_object() {
                    (err_val, self.thrown_error_kind(err_val))
                } else {
                    let msg = if err_val.is_string() {
                        // SAFETY: err_val is a string value.
                        unsafe { (*err_val.as_string_ptr()).data.clone() }
                    } else {
                        format!("{err_val}")
                    };
                    builtins_debug!("native_call err={}", msg);
                    (crate::builtins::error::create_error(self, &msg), "Error")
                };
                self.exception_value = Some(error);
                self.pending_error_kind = Some(kind);
                self.unwind()
            }
            NativeResult::TailCall { callee, this, args } => {
                self.native_call_depth -= 1;
                if callee.is_object() {
                    let obj = unsafe { &*callee.as_js_object_ptr() };
                    if obj.native_fn().is_some() {
                        let result = self.call_function_sync(callee, this, &args)?;
                        self.regs[0] = result;
                        return Ok(());
                    }
                }
                self.push_bytecode_frame(callee, this, &args, None, None, JsValue::undefined(), FrameContinuation::None)
            }
        }
    }
}

impl Vm {
    pub(crate) fn dispatch_create_closure(&mut self, rd: usize, instr: u32) {
        let sub_idx = opcode::imm16(instr) as u32;
        debug_assert!(sub_idx > 0 && (sub_idx as usize) <= self.sub_modules.len());
        let sub = &self.sub_modules[sub_idx as usize - 1];
        let result = self.create_function_object(
            sub_idx,
            sub.is_arrow,
            sub.is_class_constructor,
            sub.is_derived_constructor,
            sub.needs_home_object,
        );
        self.regs[rd] = result;
    }

    pub(crate) fn dispatch_create_regexp(&mut self, rd: usize, a: usize, b: usize) -> Result<Option<JsValue>, String> {
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
                return Err(self.error_message_text("TypeError", &self.error_text(e)));
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
            self.regs[254] = super_ctor; // bind_dispatcher reads regs[254] as the wrapper callee
            let (args_buf, len) = Self::build_native_args(first_arg_reg, arg_count, 253);
            // SAFETY: native_fn was set via set_native_fn with a valid NativeFn pointer;
            // native_fn_ptr_to_fn is the single coercion point for NativeFnPtr → NativeFn.
            let func: NativeFn = unsafe { native_fn_ptr_to_fn(super_obj.native_fn().unwrap()) };
            match func(self, &args_buf[..len]) {
                NativeResult::Ok(val) => {
                    self.regs[254] = if val.is_object() { val } else { derived_this };
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
                    // e.g. bound function: resolve the tail call, use its return
                    // value as the constructed instance (or fall back to derived_this).
                    match self.call_function_sync(callee, this, &args) {
                        Ok(val) => {
                            self.regs[254] = if val.is_object() { val } else { derived_this };
                            self.regs[rd] = self.regs[254];
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        } else if super_obj.sub_module_index() > 0 {
            let sub_idx = super_obj.sub_module_index() as usize - 1;
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

            for i in 0..sub_n_args {
                let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                self.regs[sub_param_base + i] = self.regs[src_reg];
            }
            self.regs[254] = derived_this;
            self.regs[255] = new_target;

            self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
            self.saved_immutables_stack.push(self.active_immutables);
            self.frames.push(CallFrame {
                return_addr: self.pc,
                function_name: self.sub_modules[sub_idx]
                    .function_name
                    .as_deref()
                    .map(|name| self.kernel_core.perm_interner().intern(name).0)
                    .unwrap_or(0),
                caller_reg_limit,
                saved_reg_offset,
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
            self.activate_immutables(sub_idx + 1, &subs[sub_idx].constants);
            for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
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

    pub(crate) fn dispatch_super_get_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
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
