use crate::vm::{FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use oxide_runtime_api as coercion;
use oxide_types::object::{JsObject, PropAttributes, PropMetaEntry};
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn ordinary_get(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<JsValue, String> {
        self.ordinary_get_inner(obj, prop_name_si, receiver, None)
    }

    pub(crate) fn ordinary_get_with_target(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue, target_reg: u8,
    ) -> Result<JsValue, String> {
        self.ordinary_get_inner(obj, prop_name_si, receiver, Some(target_reg))
    }

    fn ordinary_get_inner(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue, target_reg: Option<u8>,
    ) -> Result<JsValue, String> {
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        let mut current = Some(obj);
        let mut depth = 0usize;
        while let Some(obj) = current {
            if obj.is_array() && prop_name_si == length_si {
                return Ok(JsValue::int(obj.prop_count() as i32));
            }
            if obj.is_array() {
                if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                    if index < obj.prop_vec_len() as u32 {
                        return Ok(obj.get_prop_at(index));
                    }
                }
            }
            if let Some(pos) = self.get_own_property_slot(obj, prop_name_si) {
                if let Some(meta) = obj.prop_meta_at(pos) {
                    if meta.is_accessor {
                        return if meta.get.is_undefined() {
                            Ok(JsValue::undefined())
                        } else if let Some(tr) = target_reg {
                            let getter = meta.get;
                            let pushed = self.push_bytecode_getter_frame(getter, receiver, tr)?;
                            if pushed {
                                return Ok(JsValue::undefined());
                            }
                            Ok(self.regs[tr as usize])
                        } else {
                            self.call_function_sync(meta.get, receiver, &[])
                        };
                    }
                }
                return Ok(obj.get_prop_at(pos));
            }
            if depth >= MAX_PROTO_CHAIN_DEPTH {
                break;
            }
            depth += 1;
            let proto = obj.proto();
            current = proto.is_object().then(|| unsafe { &*proto.as_js_object_ptr() });
        }
        Ok(JsValue::undefined())
    }

    fn push_bytecode_getter_frame(
        &mut self, getter: JsValue, receiver: JsValue, target_reg: u8,
    ) -> Result<bool, String> {
        if !getter.is_object() {
            return Err(self.error_message_text("TypeError", "getter is not callable"));
        }
        let getter_obj = unsafe { &*getter.as_js_object_ptr() };
        if !getter_obj.is_function() {
            return Err(self.error_message_text("TypeError", "getter is not callable"));
        }

        if getter_obj.native_fn().is_some() {
            let result = self.call_function_sync(getter, receiver, &[])?;
            self.regs[target_reg as usize] = result;
            return Ok(false);
        }

        self.push_bytecode_frame(
            getter,
            receiver,
            &[],
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::AccessorGet { target_reg },
        )?;
        self.accessor_frame_target_reg = Some(target_reg);
        Ok(true)
    }

    fn inherited_property_meta(&self, obj: &JsObject, prop_name_si: u32) -> Option<PropMetaEntry> {
        let mut proto = obj.proto();
        let mut depth = 0usize;
        while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
            depth += 1;
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self.get_own_property_slot(proto_obj, prop_name_si) {
                return proto_obj.prop_meta_at(pos);
            }
            proto = proto_obj.proto();
        }
        None
    }

    pub(crate) fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, false)
    }

    pub(crate) fn ordinary_set_dispatch(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, true)
    }

    fn ordinary_set_inner(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, use_frame_push: bool,
    ) -> Result<(), String> {
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        if let Some(pos) = self.get_own_property_slot(obj, prop_name_si) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    if meta.set.is_undefined() {
                        return self.raise_type_error("property has no setter");
                    }
                    return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
                }
                if !meta.attributes.writable() {
                    return self.raise_type_error("cannot assign to read-only property");
                }
            }
            obj.set_prop_at(pos, val);
            return Ok(());
        }

        if let Some(meta) = self.inherited_property_meta(obj, prop_name_si) {
            if meta.is_accessor {
                if meta.set.is_undefined() {
                    return self.raise_type_error("property has no setter");
                }
                return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
            }
            if !meta.attributes.writable() {
                return self.raise_type_error("cannot assign to read-only property");
            }
        }

        self.set_or_create_prop_value(obj, prop_name_si, val);
        Ok(())
    }

    fn call_or_push_setter(
        &mut self, setter: JsValue, receiver: JsValue, val: JsValue, use_frame_push: bool,
    ) -> Result<(), String> {
        if !use_frame_push {
            self.call_function_sync(setter, receiver, &[val])?;
            return Ok(());
        }
        if !setter.is_object() {
            return self.raise_type_error("setter is not callable");
        }
        let setter_obj = unsafe { &*setter.as_js_object_ptr() };
        if !setter_obj.is_function() {
            return self.raise_type_error("setter is not callable");
        }
        if setter_obj.native_fn().is_some() {
            self.call_function_sync(setter, receiver, &[val])?;
            return Ok(());
        }
        self.push_bytecode_frame(
            setter,
            receiver,
            &[val],
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::AccessorSet,
        )?;
        Ok(())
    }

    pub(crate) fn read_member_prop(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<JsValue, String> {
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let _ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        if obj.has_prop_meta() {
            return self.ordinary_get(obj, prop_name_si, receiver);
        }
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_slot = ext1;

        let val =
            if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_slot < obj.prop_vec_len() as u32 {
                obj.get_prop_at(cached_slot)
            } else if let Some(template) = self.kernel_core.prop_forge().get_template(obj.shape_id()) {
                if template.prop_name != prop_name_si {
                    self.ordinary_get(obj, prop_name_si, receiver)?
                } else if template.position < obj.prop_vec_len() as u32 {
                    crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), template.position);
                    obj.get_prop_at(template.position)
                } else {
                    self.ordinary_get(obj, prop_name_si, receiver)?
                }
            } else {
                let resolved = self.ordinary_get(obj, prop_name_si, receiver)?;
                if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                    if !obj.is_accessor_meta(pos) {
                        crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos);
                    }
                }
                resolved
            };
        Ok(val)
    }

    pub(crate) fn set_member_prop(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            if obj.has_prop_meta() {
                self.ordinary_set(obj, prop_name_si, val, receiver)?;
                return Ok(());
            }
            obj.set_prop_at(pos, val);
            crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos);
        } else {
            self.ordinary_set(obj, prop_name_si, val, receiver)?;
        }
        Ok(())
    }

    pub(crate) fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue) {
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_at(pos, val);
        } else {
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(val);
            obj.bump_generation();
        }
    }

    pub(crate) fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        let pos = if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            pos
        } else {
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(JsValue::undefined())
        };
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable() {
                if current.is_accessor {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
                if current.attributes.enumerable() != attributes.enumerable() {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
                if !current.attributes.writable()
                    && (attributes.writable() || !coercion::same_value(obj.get_prop_at(pos), val))
                {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
                if current.attributes.configurable() != attributes.configurable() {
                    return self.raise_type_error("cannot redefine non-configurable property");
                }
            }
        }
        obj.set_prop_at(pos, val);
        obj.set_data_meta(pos, attributes);
        obj.bump_generation();
        Ok(())
    }

    pub(crate) fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        let target_ptr = obj as *mut JsObject;
        let get = self.promote_if_needed_for_write_ptr(target_ptr, get);
        let set = self.promote_if_needed_for_write_ptr(target_ptr, set);
        let pos = if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            pos
        } else {
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(JsValue::undefined())
        };
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable()
                && (!current.is_accessor
                    || current.attributes.enumerable() != attributes.enumerable()
                    || current.attributes.configurable() != attributes.configurable()
                    || current.get != get
                    || current.set != set)
            {
                return self.raise_type_error("cannot redefine non-configurable property");
            }
        }
        obj.set_prop_at(pos, JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, attributes);
        obj.bump_generation();
        Ok(())
    }
}
