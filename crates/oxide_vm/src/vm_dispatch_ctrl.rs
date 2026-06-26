use crate::vm::{FrameContinuation, TryHandler, Vm};
use oxide_bytecode::opcode;
use oxide_types::object::PropAttributes;
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
                    crate::vm_debug!("CALL rd={} this={} args={} depth={}", rd, this_reg, arg_count, self.frames.len());

                    if obj.native_fn().is_some() {
                        self.dispatch_native_call(obj, callee, this_reg, first_arg_reg, arg_count)?;
                        return Ok(true);
                    } else if obj.sub_module_index() > 0 {
                        let args: Vec<JsValue> = (0..arg_count)
                            .map(|i| self.regs[first_arg_reg.wrapping_add(i as u8) as usize])
                            .collect();
                        self.push_bytecode_frame(
                            callee,
                            self.regs[this_reg as usize],
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
        let prop_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        if prop_idx >= self.immutables().len() {
            return self.raise_type_error("DEFINE_ACCESSOR constant index out of bounds");
        }
        let obj_val = self.regs[rd];
        if !obj_val.is_object() {
            return self.raise_type_error("DEFINE_ACCESSOR target is not object");
        }
        let key_val = self.immutables()[prop_idx];
        let prop_name_si = self.property_key_si(key_val);
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
        self.define_accessor_property(obj, prop_name_si, get, set, PropAttributes::DEFAULT_DATA)
    }

    pub(crate) fn dispatch_return(&mut self, rd: usize) -> Result<Option<JsValue>, String> {
        let result = self.regs[rd];
        crate::vm_debug!("RETURN depth={}", self.frames.len());
        if let Some(frame) = self.frames.pop() {
            let construct_result_reg = frame.construct_result_reg;
            let constructed_this = frame.constructed_this;
            let is_derived_constructor = frame.is_derived_constructor;
            let continuation = frame.continuation;
            let callee_this = self.regs[254];
            self.restore_frame(frame);
            if let (Some(target_reg), Some(constructed_this)) = (construct_result_reg, constructed_this) {
                if is_derived_constructor && result.is_undefined() && callee_this.is_undefined() {
                    self.raise_error_kind("ReferenceError", "derived constructor must call super()")?;
                    return Ok(None);
                }
                self.regs[target_reg as usize] = if result.is_object() { result } else { constructed_this };
                self.regs[0] = self.regs[target_reg as usize];
            } else {
                match continuation {
                    FrameContinuation::None => self.regs[0] = result,
                    FrameContinuation::AccessorGet { target_reg } => self.regs[target_reg as usize] = result,
                    FrameContinuation::AccessorSet => {}
                }
            }
            Ok(None)
        } else {
            Ok(Some(result))
        }
    }

    pub(crate) fn dispatch_throw(&mut self, rd: usize) -> Result<bool, String> {
        crate::vm_debug!("THROW pc={}", self.pc);
        let exc_value = self.regs[rd];
        self.exception_value = Some(exc_value);
        self.pending_error_kind = Some(self.thrown_error_kind(exc_value));
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
            frame_depth: self.frames.len(),
        });
    }

    pub(crate) fn dispatch_try_end(&mut self) {
        self.try_stack.pop();
    }

    pub(crate) fn dispatch_try_finally_begin(&mut self, instr: u32) {
        let offset = opcode::offset16(instr) as isize;
        let finally_pc = ((self.pc as isize) + offset - 1) as usize;
        self.try_stack.push(TryHandler {
            catch_pc: None,
            finally_pc: Some(finally_pc),
            frame_depth: self.frames.len(),
        });
    }

    pub(crate) fn dispatch_try_finally_end(&mut self) -> Result<bool, String> {
        self.try_stack.pop();
        if self.pending_exception.is_some() && self.exception_value.is_none() {
            self.exception_value = self.pending_exception.take();
            self.unwind().map(|_| true)
        } else {
            self.pending_exception = None;
            Ok(false)
        }
    }
}
