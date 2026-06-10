use oxide_compiler::opcode::OpCode;
use oxide_kernel::prop_forge::PropTemplate;
use oxide_types::object::JsObject;

use crate::vm::Vm;

impl Vm {
    pub(crate) fn dispatch_property_op(&mut self, op: OpCode, rd: usize, a: usize, b: usize) -> Result<(), String> {
        match op {
            OpCode::IC_GET_PROP => self.dispatch_ic_get_prop(a, b),
            OpCode::IC_SET_PROP => self.dispatch_ic_set_prop(rd, a, b),
            OpCode::GET_PROP => self.dispatch_get_prop(rd, a, b),
            OpCode::SET_PROP => self.dispatch_set_prop(rd, a, b),
            OpCode::GET_PROP_DYNAMIC => self.dispatch_get_prop_dynamic(rd, a, b),
            OpCode::SET_PROP_DYNAMIC => self.dispatch_set_prop_dynamic(rd, a, b),
            OpCode::SET_ELEM => self.dispatch_set_elem(rd, a, b),
            _ => unreachable!("non-property opcode passed to dispatch_property_op"),
        }
    }

    fn dispatch_ic_get_prop(&mut self, a: usize, b: usize) -> Result<(), String> {
        let val = self.regs[a];
        let obj_ptr = val.as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            if val.is_string() {
                let proto_ptr = self.kernel.builtin_world().string_proto.as_ptr() as *mut JsObject;
                let proto = unsafe { &*proto_ptr };
                let prop_name_si = self.property_key_si(self.regs[b]);
                let resolved = self.ordinary_get(proto, prop_name_si, val)?;
                if !resolved.is_undefined() {
                    self.regs[a] = resolved;
                    self.pc += 3;
                    return Ok(());
                }
            }
            if val.is_int() || val.is_double() {
                let proto_ptr = self.kernel.builtin_world().number_proto.as_ptr() as *mut JsObject;
                let proto = unsafe { &*proto_ptr };
                let prop_name_si = self.property_key_si(self.regs[b]);
                let resolved = self.ordinary_get(proto, prop_name_si, val)?;
                if !resolved.is_undefined() {
                    self.regs[a] = resolved;
                    self.pc += 3;
                    return Ok(());
                }
            }
            self.raise_type_error("IC_GET_PROP on non-object")?;
            return Ok(());
        }
        let addr = obj_ptr as usize;
        if addr < 0x10000 || addr % std::mem::align_of::<JsObject>() != 0 {
            self.raise_type_error("IC_GET_PROP on non-object")?;
            return Ok(());
        }

        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let _ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        if obj.has_prop_meta() {
            let val = self.ordinary_get_with_target(obj, prop_name_si, val, a as u8)?;
            if self.accessor_frame_target_reg.take().is_none() {
                self.regs[a] = val;
            }
            return Ok(());
        }
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_slot = ext1;

        if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_slot < obj.prop_vec_len() as u32 {
            self.regs[a] = obj.get_prop_at(cached_slot);
        } else if let Some(template) = self.kernel.prop_forge().get_template(obj.shape_id()) {
            if template.prop_name == prop_name_si {
                if template.position < obj.prop_vec_len() as u32 {
                    self.write_ic_back(obj.shape_id(), template.position);
                    self.regs[a] = obj.get_prop_at(template.position);
                } else {
                    self.regs[a] = self.ordinary_get(obj, prop_name_si, val)?;
                }
            } else {
                self.regs[a] = self.ordinary_get(obj, prop_name_si, val)?;
            }
        } else {
            let resolved = self.ordinary_get(obj, prop_name_si, val)?;
            if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                self.write_ic_back(obj.shape_id(), pos);
            }
            self.regs[a] = resolved;
        }

        Ok(())
    }

    fn dispatch_ic_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "IC_SET_PROP on non-object")? else {
            return Ok(());
        };

        let prop_name_si = self.property_key_si(self.regs[b]);
        if self.kernel.string_forge().lookup(prop_name_si).as_deref() == Some("__proto__") {
            let obj = unsafe { &mut *obj_ptr };
            if self.is_object_prototype(obj_ptr) && !self.regs[a].is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                self.pc += 3;
                return Ok(());
            }
            obj.set_proto(self.regs[a]).map_err(|e| e.to_string())?;
            self.pc += 3;
            return Ok(());
        }

        let obj = unsafe { &mut *obj_ptr };
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let _ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        let value = self.regs[a];
        let receiver = self.regs[rd];
        if obj.has_prop_meta() {
            self.ordinary_set_dispatch(obj, prop_name_si, value, receiver)?;
            return Ok(());
        }
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_slot = ext1;

        if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_slot < obj.prop_vec_len() as u32 {
            obj.set_prop_at(cached_slot, value);
        } else if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_at(pos, value);
            self.write_ic_back(obj.shape_id(), pos);
        } else {
            let old_shape = obj.shape_id();
            self.ordinary_set_dispatch(obj, prop_name_si, value, receiver)?;
            if old_shape != obj.shape_id() {
                if let Some(pos) = self.kernel.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                    self.write_ic_back(obj.shape_id(), pos);
                    self.kernel.prop_forge().upsert(
                        obj.shape_id(),
                        PropTemplate {
                            shape_id: obj.shape_id(),
                            prop_name: prop_name_si,
                            position: pos,
                            generation: obj.generation(),
                        },
                    );
                }
            }
        }

        Ok(())
    }

    fn dispatch_get_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "GET_PROP on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        let val = self.ordinary_get_with_target(obj, prop_name_si, self.regs[rd], a as u8)?;
        if self.accessor_frame_target_reg.take().is_none() {
            self.regs[a] = val;
        }
        Ok(())
    }

    fn dispatch_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "SET_PROP on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        if self.kernel.string_forge().lookup(prop_name_si).as_deref() == Some("__proto__") {
            if self.is_object_prototype(obj_ptr) && !self.regs[a].is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                return Ok(());
            }
            obj.set_proto(self.regs[a]).map_err(|e| e.to_string())?;
            return Ok(());
        }
        self.ordinary_set_dispatch(obj, prop_name_si, self.regs[a], self.regs[rd])?;
        Ok(())
    }

    fn dispatch_get_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "GET_PROP_DYNAMIC on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[a]);
        let val = self.ordinary_get_with_target(obj, prop_name_si, self.regs[rd], b as u8)?;
        if self.accessor_frame_target_reg.take().is_none() {
            self.regs[b] = val;
        }
        Ok(())
    }

    fn dispatch_set_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "SET_PROP_DYNAMIC on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[a]);
        if self.kernel.string_forge().lookup(prop_name_si).as_deref() == Some("__proto__") {
            if self.is_object_prototype(obj_ptr) && !self.regs[b].is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                return Ok(());
            }
            obj.set_proto(self.regs[b]).map_err(|e| e.to_string())?;
            return Ok(());
        }
        self.ordinary_set_dispatch(obj, prop_name_si, self.regs[b], self.regs[rd])?;
        Ok(())
    }

    fn dispatch_set_elem(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "SET_ELEM on non-object")? else {
            return Ok(());
        };
        let idx = self.regs[a].as_int().max(0) as u32;
        let obj = unsafe { &mut *obj_ptr };
        obj.set_prop_at(idx, self.regs[b]);
        Ok(())
    }
}
