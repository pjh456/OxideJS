use crate::native::{NativeFn, NativeResult};
use crate::vm::{native_fn_ptr_to_fn, CallFrame, ForInIter, FrameContinuation, Vm};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn dispatch_new_expression(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        let constructor_reg = a;
        let first_arg_reg = b as u8;
        oxide_kernel::vm_debug!("NEW_EXPRESSION rd={}", rd);

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

        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;

        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let new_obj = self.epoch.alloc(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));

        let proto_si = self.kernel_core.string_forge().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(ctor_obj, proto_si) {
            if proto_val.is_object() {
                let new_obj_mut = unsafe { &mut *new_obj };
                let proto_obj_ptr = proto_val.as_js_object_ptr();
                let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
            }
        }

        if ctor_obj.native_fn().is_some() {
            let new_obj_val = JsValue::object(new_obj as *mut u8);
            self.regs[255] = new_obj_val;

            let mut args_buf = [0u8; 257];
            args_buf[0] = 255u8;
            for i in 0..arg_count.min(256) {
                args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
            }
            let args_slice = &args_buf[..arg_count + 1];

            let func: NativeFn = unsafe { native_fn_ptr_to_fn(ctor_obj.native_fn().unwrap()) };
            self.regs[254] = constructor;
            match func(self, args_slice) {
                NativeResult::Ok(val) => {
                    self.regs[rd] = if val.is_object() { val } else { new_obj_val };
                    Ok(false)
                }
                NativeResult::Err(err_val) => {
                    self.exception_value = Some(err_val);
                    self.pending_error_kind = Some("Error");
                    self.unwind().map(|_| true)
                }
                NativeResult::TailCall { .. } => {
                    self.raise_type_error("constructor tail call not supported").map(|_| true)
                }
            }
        } else if ctor_obj.sub_module_index() > 0 {
            let sub_idx = ctor_obj.sub_module_index() as usize - 1;
            if sub_idx >= self.sub_modules.len() {
                return Err(format!(
                    "NEW_EXPRESSION: sub_module_index {} out of bounds (max {})",
                    sub_idx,
                    self.sub_modules.len()
                ));
            }

            if self.frames.len() >= self.kernel_core.config.max_call_depth {
                return Err("RangeError: Maximum call stack size exceeded".into());
            }

            let new_obj_val = JsValue::object(new_obj as *mut u8);
            let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
            let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
            let sub_n_registers = self.sub_modules[sub_idx].n_registers;
            let sub_constants = self.sub_modules[sub_idx].constants.clone();
            let sub_param_base = self.sub_modules[sub_idx].param_base as usize;
            let caller_reg_limit = self.active_reg_limit.max(1);
            let saved_regs = self.regs[..caller_reg_limit as usize].to_vec().into_boxed_slice();
            let saved_this = self.regs[254];
            let saved_new_target = self.regs[255];

            for i in 0..sub_n_args {
                let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                self.regs[sub_param_base + i] = self.regs[src_reg];
            }
            self.regs[254] = if ctor_obj.is_derived_constructor() {
                JsValue::undefined()
            } else {
                new_obj_val
            };
            self.regs[255] = constructor;

            let converted_sub_constants = self.convert_constants(&sub_constants)?;
            self.saved_bytecode_stack.push(std::mem::take(&mut self.bytecode));
            self.saved_constants_stack.push(std::mem::take(&mut self.constants));

            self.frames.push(CallFrame {
                return_addr: self.pc,
                function_name: self.sub_modules[sub_idx]
                    .function_name
                    .as_deref()
                    .map(|name| self.kernel_core.string_forge().intern(name).0)
                    .unwrap_or(0),
                caller_reg_limit,
                saved_regs,
                saved_this,
                saved_new_target,
                callee: constructor,
                construct_result_reg: Some(rd as u8),
                constructed_this: Some(new_obj_val),
                is_derived_constructor: ctor_obj.is_derived_constructor(),
                continuation: FrameContinuation::None,
            });

            self.bytecode = sub_bytecode;
            self.constants = converted_sub_constants;

            for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
                let si = self.kernel_core.string_forge().intern(name.as_str()).0;
                let global = self.session.global_object();
                if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
                    self.regs[*reg as usize] = global.get_prop_at(pos);
                }
            }

            self.active_reg_limit = sub_n_registers.max(1);
            self.pc = 0;
            Ok(true)
        } else {
            let error =
                crate::builtins::error::create_error(self, "NEW_EXPRESSION: bytecode constructors not yet supported");
            self.exception_value = Some(error);
            self.pending_error_kind = Some("Error");
            self.unwind().map(|_| true)
        }
    }

    pub(crate) fn dispatch_template_str(&mut self, rd: usize) {
        let header = self.bytecode[self.pc];
        self.pc += 1;
        let segment_count = (header >> 16) as usize;
        let len_hint = (header & 0xFFFF) as usize;

        let mut result = String::with_capacity(len_hint.max(16));
        for _ in 0..segment_count {
            let seg = self.bytecode[self.pc];
            self.pc += 1;
            if (seg >> 31) == 1 {
                let reg = (seg & 0x7F) as u8;
                let val = self.regs[reg as usize];
                let s = if val.is_string() {
                    self.kernel_core
                        .string_forge()
                        .lookup(val.as_string_index())
                        .unwrap_or_default()
                } else {
                    format!("{}", val)
                };
                result.push_str(&s);
            } else {
                let const_idx = (seg & 0x7FFF_FFFF) as usize;
                if const_idx < self.constants.len() {
                    let val = self.constants[const_idx];
                    if val.is_string() {
                        let s = self
                            .kernel_core
                            .string_forge()
                            .lookup(val.as_string_index())
                            .unwrap_or_default();
                        result.push_str(&s);
                    }
                }
            }
        }
        let si = self.kernel_core.string_forge().intern(&result).0;
        self.regs[rd] = JsValue::string(si, 0);
    }

    pub(crate) fn dispatch_instanceof(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let lhs_val = self.regs[a];
        let rhs_val = self.regs[b];

        if !rhs_val.is_object() {
            return self.raise_type_error("INSTANCEOF right-hand side is not callable");
        }
        if !lhs_val.is_object() {
            self.regs[rd] = JsValue::bool(false);
            return Ok(());
        }

        let rhs_obj = unsafe { &*rhs_val.as_js_object_ptr() };
        let proto_si = self.kernel_core.string_forge().intern("prototype").0;
        let ctor_proto = self.resolve_property(rhs_obj, proto_si);

        let ctor_proto_ptr = match ctor_proto {
            Some(v) if v.is_object() => v.as_js_object_ptr(),
            _ => {
                self.regs[rd] = JsValue::bool(false);
                return Ok(());
            }
        };

        let mut proto = unsafe { &*lhs_val.as_js_object_ptr() }.proto();
        loop {
            if !proto.is_object() {
                self.regs[rd] = JsValue::bool(false);
                break;
            }
            let proto_ptr = proto.as_js_object_ptr();
            if proto_ptr == ctor_proto_ptr {
                self.regs[rd] = JsValue::bool(true);
                break;
            }
            proto = unsafe { &*proto_ptr }.proto();
        }
        Ok(())
    }

    pub(crate) fn dispatch_in(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let key_val = self.regs[a];
        let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            return self.raise_type_error("IN right-hand side is not an object");
        }
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(key_val);
        let found = self.resolve_property(obj, prop_name_si).is_some();
        self.regs[rd] = JsValue::bool(found);
        Ok(())
    }

    pub(crate) fn dispatch_for_in_init(&mut self, a: usize) -> Result<(), String> {
        let obj_val = self.regs[a];
        if !obj_val.is_object() {
            return self.raise_type_error("for-in right-hand side is not an object");
        }

        let mut keys_vec = bumpalo::collections::Vec::new_in(self.epoch.bump());
        let mut seen = std::collections::HashSet::new();
        let mut current = obj_val;

        loop {
            if !current.is_object() {
                break;
            }
            let cur = unsafe { &*current.as_js_object_ptr() };
            let mut cursor = Some(cur.shape_id());
            while let Some(id) = cursor {
                if id == oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
                    break;
                }
                if let Some(shape) = self.kernel_core.shape_forge().get_shape(id) {
                    if shape.property_name != u32::MAX && seen.insert(shape.property_name) {
                        let enumerable = self
                            .kernel_core
                            .shape_forge()
                            .lookup_position(cur.shape_id(), shape.property_name)
                            .and_then(|pos| cur.prop_meta_at(pos))
                            .map(|meta| meta.attributes.enumerable())
                            .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                        if enumerable {
                            let hash = self.kernel_core.string_forge().get_hash(shape.property_name).unwrap_or(0);
                            keys_vec.push(JsValue::string(shape.property_name, hash));
                        }
                    }
                    cursor = shape.parent;
                } else {
                    break;
                }
            }
            current = cur.proto();
        }

        let iter = self.epoch.alloc(ForInIter { keys: keys_vec, index: 0 });
        self.for_in_iters.push(iter.cast::<ForInIter<'static>>());
        Ok(())
    }

    pub(crate) fn dispatch_for_in_next(&mut self, rd: usize) -> Result<(), String> {
        let iter_ptr = self.for_in_iters.last().copied().unwrap_or(std::ptr::null_mut());
        if iter_ptr.is_null() {
            return Err("FOR_IN_NEXT without active iterator".into());
        }
        let iter = unsafe { &mut *iter_ptr };
        if iter.index < iter.keys.len() {
            self.regs[rd] = iter.keys[iter.index];
            iter.index += 1;
        } else {
            self.regs[rd] = JsValue::undefined();
        }
        Ok(())
    }

    pub(crate) fn dispatch_for_in_done(&mut self, rd: usize) {
        let iter_ptr = self.for_in_iters.last().copied().unwrap_or(std::ptr::null_mut());
        if iter_ptr.is_null() {
            self.regs[rd] = JsValue::bool(true);
        } else {
            let iter = unsafe { &*iter_ptr };
            self.regs[rd] = JsValue::bool(iter.index >= iter.keys.len());
        }
    }

    pub(crate) fn dispatch_for_in_cleanup(&mut self) {
        self.for_in_iters.pop();
    }
}
