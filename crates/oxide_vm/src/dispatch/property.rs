use crate::{ic_debug, ic_trace, vm_trace};
use oxide_bytecode::opcode::OpCode;
use oxide_kernel::prop_forge::PropTemplate;
use oxide_types::object::JsObject;
use oxide_types::private_key::make_private_name_id;
use oxide_types::value::JsValue;

use crate::ic_helper::{self, ic_get_hit, ic_set_hit};
use crate::vm::{Vm, MAX_PROTO_CHAIN_DEPTH};

#[cold]
fn prop_cache_miss() {}

impl Vm {
    pub(crate) fn dispatch_property_op(&mut self, op: OpCode, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("PROPERTY_OP {:?} rd={} a={} b={}", op, rd, a, b);
        match op {
            OpCode::IC_GET_PROP => self.dispatch_ic_get_prop(a, b),
            OpCode::IC_SET_PROP => self.dispatch_ic_set_prop(rd, a, b),
            OpCode::GET_PROP => self.dispatch_get_prop(rd, a, b),
            OpCode::SET_PROP => self.dispatch_set_prop(rd, a, b),
            OpCode::GET_PROP_DYNAMIC => self.dispatch_get_prop_dynamic(rd, a, b),
            OpCode::SET_PROP_DYNAMIC => self.dispatch_set_prop_dynamic(rd, a, b),
            OpCode::SET_ELEM => self.dispatch_set_elem(rd, a, b),
            OpCode::GET_PRIVATE => self.dispatch_get_private(rd, a, b),
            OpCode::SET_PRIVATE => self.dispatch_set_private(rd, a, b),
            OpCode::INIT_PRIVATE => self.dispatch_init_private(rd, a, b),
            OpCode::PRIVATE_BRAND_IN => self.dispatch_private_brand_in(rd, a, b),
            _ => unreachable!("non-property opcode passed to dispatch_property_op"),
        }
    }

    fn private_key_from_reg(&self, reg: usize) -> u32 {
        let value = self.regs[reg];
        let local_id = if value.is_int() {
            value.as_int().max(0) as u32
        } else if value.is_double() {
            value.as_double().max(0.0) as u32
        } else {
            0
        };
        make_private_name_id(local_id)
    }

    fn primitive_property_get(&mut self, val: JsValue, prop_name_si: u32) -> Result<Option<JsValue>, String> {
        if val.is_string() {
            let length_si = self.kernel_core.perm_interner().intern("length").0;
            if prop_name_si == length_si {
                // SAFETY: val is a string value.
                let len = unsafe { (*val.as_string_ptr()).data.encode_utf16().count() };
                return Ok(Some(JsValue::int(len as i32)));
            }
            let proto_ptr = self.session.builtin_world().string_proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &*proto_ptr };
            return self.ordinary_get(proto, prop_name_si, val).map(Some);
        }
        if val.is_int() || val.is_double() {
            let proto_ptr = self.session.builtin_world().number_proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &*proto_ptr };
            return self.ordinary_get(proto, prop_name_si, val).map(Some);
        }
        if val.is_bool() {
            let proto_ptr = self.session.builtin_world().boolean_proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &*proto_ptr };
            return self.ordinary_get(proto, prop_name_si, val).map(Some);
        }
        Ok(None)
    }

    fn private_slot(&self, obj: &JsObject, private_key: u32) -> Option<u32> {
        self.kernel_core.shape_forge().lookup_position(obj.shape_id(), private_key)
    }

    fn resolve_private_value(&self, obj: &JsObject, private_key: u32) -> Option<JsValue> {
        if let Some(pos) = self.private_slot(obj, private_key) {
            return Some(obj.get_prop_at(pos));
        }
        let mut proto = obj.proto();
        let mut depth = 0usize;
        while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
            depth += 1;
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self.private_slot(proto_obj, private_key) {
                return Some(proto_obj.get_prop_at(pos));
            }
            proto = proto_obj.proto();
        }
        None
    }

    fn dispatch_get_private(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GET_PRIVATE rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[a], "private field access on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &*obj_ptr };
        let private_key = self.private_key_from_reg(b);
        let Some(value) = self.resolve_private_value(obj, private_key) else {
            return self.raise_type_error("private field brand check failed");
        };
        self.regs[rd] = value;
        Ok(())
    }

    fn dispatch_set_private(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SET_PRIVATE rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "private field assignment on non-object")? else {
            return Ok(());
        };
        let private_key = self.private_key_from_reg(b);
        let obj = unsafe { &mut *obj_ptr };
        let Some(pos) = self.private_slot(obj, private_key) else {
            return self.raise_type_error("private field brand check failed");
        };
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let obj = unsafe { &mut *obj_ptr };
        obj.set_prop_at(pos, value);
        Ok(())
    }

    fn dispatch_init_private(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("INIT_PRIVATE rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "private field initialization on non-object")?
        else {
            return Ok(());
        };
        let private_key = self.private_key_from_reg(b);
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let obj = unsafe { &mut *obj_ptr };
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), private_key) {
            obj.set_prop_at(pos, value);
        } else {
            self.set_or_create_prop_value(obj, private_key, value);
        }
        Ok(())
    }

    fn dispatch_private_brand_in(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("PRIVATE_BRAND_IN rd={} a={} b={}", rd, a, b);
        let obj_val = self.regs[a];
        if !obj_val.is_object() {
            self.regs[rd] = JsValue::bool(false);
            return Ok(());
        }
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        let private_key = self.private_key_from_reg(b);
        self.regs[rd] = JsValue::bool(self.resolve_private_value(obj, private_key).is_some());
        Ok(())
    }

    fn dispatch_ic_get_prop(&mut self, a: usize, b: usize) -> Result<(), String> {
        let val = self.regs[a];
        let obj_ptr = val.as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            let prop_name_si = self.property_key_si(self.regs[b]);
            if let Some(resolved) = self.primitive_property_get(val, prop_name_si)? {
                self.regs[a] = resolved;
                self.pc += 3;
                return Ok(());
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
        let (cached_shape_id, cached_slot) = ic_helper::read_ic_entry(&self.bytecode, &mut self.pc);
        if obj.has_prop_meta() {
            let val = self.ordinary_get_with_target(obj, prop_name_si, val, a as u8)?;
            if self.accessor_frame_target_reg.take().is_none() {
                self.regs[a] = val;
            }
            return Ok(());
        }

        if let Some(value) = ic_get_hit(obj, cached_shape_id, cached_slot) {
            self.regs[a] = value;
            self.profiling.record_ic_hit();
            ic_trace!("IC_GET hit shape={} slot={}", cached_shape_id, cached_slot);
        } else if let Some(template) = self.kernel_core.prop_forge().get_template(obj.shape_id()) {
            self.profiling.record_ic_miss();
            prop_cache_miss();
            if template.prop_name == prop_name_si {
                if template.position < obj.prop_vec_len() as u32 {
                    ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), template.position);
                    ic_debug!(
                        "IC_GET propforge hit shape={} prop={} slot={}",
                        obj.shape_id(),
                        prop_name_si,
                        template.position
                    );
                    self.regs[a] = obj.get_prop_at(template.position);
                } else {
                    ic_debug!("IC_GET miss shape={} prop={}", obj.shape_id(), prop_name_si);
                    self.regs[a] = self.ordinary_get(obj, prop_name_si, val)?;
                }
            } else {
                ic_debug!("IC_GET miss shape={} prop={}", obj.shape_id(), prop_name_si);
                self.regs[a] = self.ordinary_get(obj, prop_name_si, val)?;
            }
        } else {
            prop_cache_miss();
            ic_debug!("IC_GET miss shape={} prop={}", obj.shape_id(), prop_name_si);
            let resolved = self.ordinary_get(obj, prop_name_si, val)?;
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos);
            }
            self.regs[a] = resolved;
        }

        Ok(())
    }

    fn dispatch_ic_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "Cannot create property on non-object")? else {
            return Ok(());
        };

        let prop_name_si = self.property_key_si(self.regs[b]);
        if self.kernel_core.perm_interner().lookup(prop_name_si) == Some("__proto__") {
            let proto_value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
            if self.is_object_prototype(obj_ptr) && !proto_value.is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                self.pc += 3;
                return Ok(());
            }
            let obj = unsafe { &mut *obj_ptr };
            obj.set_proto(proto_value).map_err(|e| e.to_string())?;
            self.pc += 3;
            return Ok(());
        }

        let (cached_shape_id, cached_slot) = ic_helper::read_ic_entry(&self.bytecode, &mut self.pc);
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let receiver = self.regs[rd];
        let obj = unsafe { &mut *obj_ptr };
        if obj.has_prop_meta() {
            self.ordinary_set_dispatch(obj, prop_name_si, value, receiver)?;
            return Ok(());
        }

        if ic_set_hit(obj, cached_shape_id, cached_slot, value) {
            self.profiling.record_ic_hit();
            ic_trace!("IC_SET hit shape={} slot={}", cached_shape_id, cached_slot);
        } else if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            self.profiling.record_ic_miss();
            prop_cache_miss();
            obj.set_prop_at(pos, value);
            ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos);
            ic_debug!("IC_SET write-back shape={} slot={}", obj.shape_id(), pos);
        } else {
            self.profiling.record_ic_miss();
            prop_cache_miss();
            let old_shape = obj.shape_id();
            self.ordinary_set_dispatch(obj, prop_name_si, value, receiver)?;
            if old_shape != obj.shape_id() {
                if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                    ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos);
                    ic_debug!("IC_SET write-back shape={} slot={}", obj.shape_id(), pos);
                    self.kernel_core.prop_forge().upsert(
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
        vm_trace!("GET_PROP rd={} a={} b={}", rd, a, b);
        let prop_name_si = self.property_key_si(self.regs[b]);
        if let Some(value) = self.primitive_property_get(self.regs[rd], prop_name_si)? {
            self.regs[a] = value;
            return Ok(());
        }
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "GET_PROP on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &*obj_ptr };
        let val = self.ordinary_get_with_target(obj, prop_name_si, self.regs[rd], a as u8)?;
        if self.accessor_frame_target_reg.take().is_none() {
            self.regs[a] = val;
        }
        Ok(())
    }

    fn dispatch_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SET_PROP rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "Cannot create property on non-object")? else {
            return Ok(());
        };
        let prop_name_si = self.property_key_si(self.regs[b]);
        if self.kernel_core.perm_interner().lookup(prop_name_si) == Some("__proto__") {
            let proto_value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
            if self.is_object_prototype(obj_ptr) && !proto_value.is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                return Ok(());
            }
            let obj = unsafe { &mut *obj_ptr };
            obj.set_proto(proto_value).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let obj = unsafe { &mut *obj_ptr };
        self.ordinary_set_dispatch(obj, prop_name_si, value, self.regs[rd])?;
        Ok(())
    }

    fn dispatch_get_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GET_PROP_DYNAMIC rd={} a={} b={}", rd, a, b);
        let prop_name_si = self.property_key_si(self.regs[a]);
        if let Some(value) = self.primitive_property_get(self.regs[rd], prop_name_si)? {
            self.regs[b] = value;
            return Ok(());
        }
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "GET_PROP_DYNAMIC on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &*obj_ptr };
        let val = self.ordinary_get_with_target(obj, prop_name_si, self.regs[rd], b as u8)?;
        if self.accessor_frame_target_reg.take().is_none() {
            self.regs[b] = val;
        }
        Ok(())
    }

    fn dispatch_set_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SET_PROP_DYNAMIC rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "Cannot create property on non-object")? else {
            return Ok(());
        };
        let prop_name_si = self.property_key_si(self.regs[a]);
        if self.kernel_core.perm_interner().lookup(prop_name_si) == Some("__proto__") {
            let proto_value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[b]);
            if self.is_object_prototype(obj_ptr) && !proto_value.is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                return Ok(());
            }
            let obj = unsafe { &mut *obj_ptr };
            obj.set_proto(proto_value).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[b]);
        let obj = unsafe { &mut *obj_ptr };
        self.ordinary_set_dispatch(obj, prop_name_si, value, self.regs[rd])?;
        Ok(())
    }

    fn dispatch_set_elem(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SET_ELEM rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "Cannot create property on non-object")? else {
            return Ok(());
        };
        let idx = if self.regs[a].is_int() {
            self.regs[a].as_int().max(0) as u32
        } else if self.regs[a].is_double() {
            self.regs[a].as_double().max(0.0) as u32
        } else {
            0
        };
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[b]);
        let obj = unsafe { &mut *obj_ptr };
        obj.set_prop_at(idx, value);
        Ok(())
    }
}

impl Vm {
    pub(crate) fn dispatch_delete_prop_static(&mut self, rd: usize) -> Result<bool, String> {
        vm_trace!("DELETE_PROP_STATIC rd={}", rd);
        let prop_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        let key_val = self.immutables().get(prop_idx).copied().unwrap_or_else(JsValue::undefined);
        let prop_name_si = self.property_key_si(key_val);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "delete on non-object")? else {
            return Ok(true);
        };
        let obj = unsafe { &mut *obj_ptr };
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_at(pos, JsValue::undefined());
        }
        self.regs[rd] = JsValue::bool(true);
        Ok(false)
    }

    pub(crate) fn dispatch_delete_prop_dynamic(&mut self, rd: usize, b: usize) -> Result<bool, String> {
        vm_trace!("DELETE_PROP_DYNAMIC rd={}", rd);
        let prop_name_si = self.property_key_si(self.regs[b]);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "delete on non-object")? else {
            return Ok(true);
        };
        let obj = unsafe { &mut *obj_ptr };
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_at(pos, JsValue::undefined());
        }
        self.regs[rd] = JsValue::bool(true);
        Ok(false)
    }
}
