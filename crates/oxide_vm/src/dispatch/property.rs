use oxide_compiler::opcode::OpCode;
use oxide_kernel::prop_forge::PropTemplate;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::vm::Vm;

impl Vm {
    pub(crate) fn dispatch_property_op(
        &mut self,
        op: OpCode,
        rd: usize,
        a: usize,
        b: usize,
    ) -> Result<(), String> {
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
                if let Some(resolved) = self.resolve_property(proto, prop_name_si) {
                    self.regs[a] = resolved;
                    self.pc += 3;
                    return Ok(());
                }
            }
            if val.is_int() || val.is_double() {
                let proto_ptr = self.kernel.builtin_world().number_proto.as_ptr() as *mut JsObject;
                let proto = unsafe { &*proto_ptr };
                let prop_name_si = self.property_key_si(self.regs[b]);
                if let Some(resolved) = self.resolve_property(proto, prop_name_si) {
                    self.regs[a] = resolved;
                    self.pc += 3;
                    return Ok(());
                }
            }
            self.raise_type_error("IC_GET_PROP on non-object")?;
            return Ok(());
        }

        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

        if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0 {
            self.regs[a] = unsafe { *(cached_ptr as *const JsValue) };
        } else if let Some(template) = self.kernel.prop_forge().get_template(obj.shape_id()) {
            if template.prop_name == prop_name_si {
                if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                    self.write_ic_back(obj.shape_id(), ptr);
                    self.regs[a] = unsafe { *ptr };
                } else {
                    self.regs[a] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }
            } else {
                self.regs[a] = self
                    .resolve_property(obj, prop_name_si)
                    .unwrap_or(JsValue::undefined());
            }
        } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
            if let Some(pos) = self
                .kernel
                .shape_forge()
                .lookup_position(obj.shape_id(), prop_name_si)
            {
                if let Some(ptr) = obj.prop_ptr_at(pos) {
                    self.write_ic_back(obj.shape_id(), ptr);
                }
            }
            self.regs[a] = val;
        } else {
            self.regs[a] = JsValue::undefined();
        }

        Ok(())
    }

    fn dispatch_ic_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("IC_SET_PROP on non-object")?;
            return Ok(());
        }

        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

        if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0 {
            unsafe {
                *(cached_ptr as *mut JsValue) = self.regs[a];
            }
        } else if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            obj.set_prop_at(pos, self.regs[a]);
            if let Some(ptr) = obj.prop_ptr_at(pos) {
                self.write_ic_back(obj.shape_id(), ptr);
            }
        } else {
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            let new_pos = obj.push_prop(self.regs[a]);
            obj.bump_generation();
            if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                self.write_ic_back(new_shape_id, ptr);
                self.kernel.prop_forge().upsert(
                    new_shape_id,
                    PropTemplate {
                        shape_id: new_shape_id,
                        prop_name: prop_name_si,
                        position: new_pos,
                        generation: obj.generation(),
                    },
                );
            }
        }

        Ok(())
    }

    fn dispatch_get_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("GET_PROP on non-object")?;
            return Ok(());
        }
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        self.regs[a] = self
            .resolve_property(obj, prop_name_si)
            .unwrap_or(JsValue::undefined());
        Ok(())
    }

    fn dispatch_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("SET_PROP on non-object")?;
            return Ok(());
        }
        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[b]);
        if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            obj.set_prop_at(pos, self.regs[a]);
        } else {
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(self.regs[a]);
            obj.bump_generation();
        }
        Ok(())
    }

    fn dispatch_get_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("GET_PROP_DYNAMIC on non-object")?;
            return Ok(());
        }
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[a]);
        self.regs[b] = self
            .resolve_property(obj, prop_name_si)
            .unwrap_or(JsValue::undefined());
        Ok(())
    }

    fn dispatch_set_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("SET_PROP_DYNAMIC on non-object")?;
            return Ok(());
        }
        let obj = unsafe { &mut *obj_ptr };
        let prop_name_si = self.property_key_si(self.regs[a]);
        if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            obj.set_prop_at(pos, self.regs[b]);
        } else {
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(self.regs[b]);
            obj.bump_generation();
        }
        Ok(())
    }

    fn dispatch_set_elem(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            self.raise_type_error("SET_ELEM on non-object")?;
            return Ok(());
        }
        let idx = self.regs[a].as_int().max(0) as u32;
        let obj = unsafe { &mut *obj_ptr };
        obj.set_prop_at(idx, self.regs[b]);
        Ok(())
    }
}
