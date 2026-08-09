use crate::vm::{FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::{ic_trace, vm_trace};
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
        vm_trace!("ordinary_get_inner: shape={} prop_si={}", obj.shape_id(), prop_name_si);
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        let mut current = Some(obj);
        let mut depth = 0usize;
        while let Some(obj) = current {
            if obj.is_array() && prop_name_si == length_si {
                return Ok(JsValue::int(obj.prop_count() as i32));
            }
            if obj.is_array() {
                if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                    // 数组元素区：hole（删除标记）视为不存在，落到原型链。
                    if index < obj.array_prop_count && !obj.prop_meta_at(index).is_some_and(|m| m.is_hole()) {
                        // 数组元素为访问器属性（defineProperty getter）时须触发
                        // getter，而非直接读数据槽。
                        if let Some(meta) = obj.prop_meta_at(index) {
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
                        return Ok(obj.get_prop_at(index));
                    }
                }
            }
            if obj.is_typed_array_obj() {
                if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                    return oxide_builtins::typed_array::typed_array_element_get(self, obj, index);
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
            if let Some(proto_obj) = current {
                vm_trace!("ordinary_get proto step: depth={} shape={}", depth, proto_obj.shape_id());
            }
        }
        Ok(JsValue::undefined())
    }

    pub(crate) fn push_bytecode_getter_frame(
        &mut self, getter: JsValue, receiver: JsValue, target_reg: u8,
    ) -> Result<bool, String> {
        vm_trace!(
            "push_bytecode_getter_frame: target_reg={} native={}",
            target_reg,
            getter.is_object() && unsafe { &*getter.as_js_object_ptr() }.native_fn().is_some()
        );
        if !getter.is_object() {
            return Err(self.error_message_text("TypeError", "getter is not callable"));
        }
        let getter_obj = unsafe { &*getter.as_js_object_ptr() };
        if !getter_obj.is_function() {
            return Err(self.error_message_text("TypeError", "getter is not callable"));
        }

        if getter_obj.native_fn().is_some() {
            let result = match self.call_function_sync(getter, receiver, &[]) {
                Ok(v) => v,
                Err(err) => self.raise_call_error(&err)?,
            };
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
        vm_trace!("inherited_property_meta: shape={} prop_si={}", obj.shape_id(), prop_name_si);
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
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, false)
    }

    /// 分发期入口：调用方（dispatch_set_prop 等）已对值做过 promote。
    pub(crate) fn ordinary_set_dispatch(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, true)
    }

    fn ordinary_set_inner(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, use_frame_push: bool,
    ) -> Result<(), String> {
        vm_trace!(
            "ordinary_set_inner: shape={} prop_si={} frame_push={}",
            obj.shape_id(),
            prop_name_si,
            use_frame_push
        );
        // TypedArray 整数索引：receiver 为 TA 本体时写底层 buffer（越界静默忽略）；
        // receiver 非 TA 时按规范把写入落到 receiver 对象，不碰 TA buffer。
        if obj.is_typed_array_obj() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                if receiver.as_js_object_ptr() != obj as *const JsObject as *mut JsObject {
                    return self.set_to_receiver(obj, prop_name_si, val, receiver, index as usize, use_frame_push);
                }
                return oxide_builtins::typed_array::typed_array_element_set(self, obj, index, val);
            }
        }
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
            // pos 是存储索引（get_own_property_slot 对数组已加元素区偏移）。
            obj.set_prop_storage(pos as usize, val);
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

    /// TypedArray 整数索引在 `receiver` ≠ TA 时的 [[Set]] 语义：越界或非对象
    /// receiver 直接返回 true（不写、不 ToNumber）；界内对象 receiver 走普通 set
    /// 把属性落到 receiver 自身。
    fn set_to_receiver(
        &mut self, ta_obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, index: usize,
        use_frame_push: bool,
    ) -> Result<(), String> {
        let Some((_, length)) = oxide_builtins::typed_array::typed_array_integer_index(self, ta_obj, prop_name_si)
        else {
            return Ok(());
        };
        if index >= length {
            return Ok(());
        }
        let receiver_ptr = receiver.as_js_object_ptr();
        if receiver_ptr.is_null() {
            return Ok(());
        }
        let promoted = self.promote_if_needed_for_write_ptr(receiver_ptr, val);
        let receiver_obj = unsafe { &mut *receiver_ptr };
        self.ordinary_set_inner(receiver_obj, prop_name_si, promoted, receiver, use_frame_push)
    }

    pub(crate) fn call_or_push_setter(
        &mut self, setter: JsValue, receiver: JsValue, val: JsValue, use_frame_push: bool,
    ) -> Result<(), String> {
        vm_trace!("call_or_push_setter: frame_push={}", use_frame_push);
        // native setter 抛错经 call_function_sync 以 String 返回（未 unwind）——
        // 在此经 raise_call_error 恢复为可捕获的 JS 异常（与 getter 路径对称）。
        let call_native = |vm: &mut Self| -> Result<(), String> {
            if let Err(err) = vm.call_function_sync(setter, receiver, &[val]) {
                vm.raise_call_error(&err)?;
            }
            Ok(())
        };
        if !use_frame_push {
            return call_native(self);
        }
        if !setter.is_object() {
            return self.raise_type_error("setter is not callable");
        }
        let setter_obj = unsafe { &*setter.as_js_object_ptr() };
        if !setter_obj.is_function() {
            return self.raise_type_error("setter is not callable");
        }
        if setter_obj.native_fn().is_some() {
            return call_native(self);
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
        vm_trace!("read_member_prop: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let ext0 = self.bytecode[self.pc];
        let ext1 = self.bytecode[self.pc + 1];
        let ext2 = self.bytecode[self.pc + 2];
        self.pc += 3;
        if obj.has_prop_meta() {
            return self.ordinary_get(obj, prop_name_si, receiver);
        }
        let cached_shape_id = ext0 & 0x00FF_FFFF;
        let cached_slot = ext1;
        let cached_depth = (ext2 & 0xFF) as u8;

        let val = if let Some(v) = crate::ic_helper::ic_get_hit(obj, cached_shape_id, cached_slot, cached_depth) {
            v
        } else if let Some(template) = self.kernel_core.prop_forge().get_template(obj.shape_id()) {
            if template.prop_name != prop_name_si {
                self.proto_chain_ic_get(obj, prop_name_si, receiver)?
            } else if template.position < obj.prop_vec_len() as u32 {
                crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), template.position, 0);
                obj.get_prop_shape(template.position)
            } else {
                self.proto_chain_ic_get(obj, prop_name_si, receiver)?
            }
        } else {
            self.proto_chain_ic_get(obj, prop_name_si, receiver)?
        };
        Ok(val)
    }

    /// 执行 ordinary_get 并把 IC 按原型链深度写回。
    fn proto_chain_ic_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String> {
        let resolved = self.ordinary_get(obj, prop_name_si, receiver)?;
        // 快路径：自身属性（depth=0）。
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            if !obj.is_accessor_meta(pos) {
                crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos, 0);
            }
            return Ok(resolved);
        }
        // 沿原型链查找继承属性。
        let mut cursor = obj.proto().as_js_object_ptr();
        let mut depth = 1u8;
        while !cursor.is_null() {
            let co = unsafe { &*cursor };
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(co.shape_id(), prop_name_si) {
                if !co.is_accessor_meta(pos) {
                    crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, co.shape_id(), pos, depth);
                }
                break;
            }
            if !co.proto().is_object() {
                break;
            }
            cursor = co.proto().as_js_object_ptr();
            depth += 1;
        }
        Ok(resolved)
    }

    pub(crate) fn set_member_prop(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String> {
        ic_trace!("set_member_prop: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            if obj.has_prop_meta() {
                self.ordinary_set(obj, prop_name_si, val, receiver)?;
                return Ok(());
            }
            obj.set_prop_shape(pos, val);
            crate::ic_helper::write_ic_back(&mut self.bytecode, self.pc, obj.shape_id(), pos, 0);
        } else {
            self.ordinary_set(obj, prop_name_si, val, receiver)?;
        }
        Ok(())
    }

    pub(crate) fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue) {
        vm_trace!("set_or_create_prop_value: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        // TypedArray 整数索引写 buffer（越界忽略），不进入 shape/prop 槽。
        if obj.is_typed_array_obj() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                let _ = oxide_builtins::typed_array::typed_array_element_set(self, obj, index, val);
                return;
            }
        }
        // 数组下标键写入元素区（维护 array_prop_count），不进入 shape 链。
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                obj.set_prop_at(index, val);
                return;
            }
        }
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_shape(pos, val);
        } else {
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            // 数组对象：属性追加到 hash_props 属性区（元素之后），array_prop_count 不变。
            obj.push_prop(val);
            obj.bump_generation();
        }
    }

    pub(crate) fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        vm_trace!("define_data_property: shape={} prop_si={}", obj.shape_id(), prop_name_si);
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        // TypedArray 整数索引：走元素定义（界内写 buffer，越界/非法描述符拒绝）。
        if obj.is_typed_array_obj() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                return oxide_builtins::typed_array::typed_array_element_define(self, obj, index, val);
            }
        }
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                // 数组索引属性存元素区并维护 array_prop_count（元素数随索引增长）。
                return self.define_array_index_element(
                    obj,
                    index,
                    val,
                    attributes,
                    false,
                    JsValue::undefined(),
                    JsValue::undefined(),
                );
            }
        }
        let pos = if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            // shape 槽位 → 存储索引（数组属性在元素区之后）。
            if obj.is_array() {
                obj.array_prop_count as usize + pos as usize
            } else {
                pos as usize
            }
        } else {
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(JsValue::undefined()) as usize
        };
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable() {
                if current.is_accessor {
                    return Err("cannot redefine non-configurable property".to_string());
                }
                if current.attributes.enumerable() != attributes.enumerable() {
                    return Err("cannot redefine non-configurable property".to_string());
                }
                if !current.attributes.writable()
                    && (attributes.writable() || !coercion::same_value(obj.get_prop_at(pos), val))
                {
                    return Err("cannot redefine non-configurable property".to_string());
                }
                if current.attributes.configurable() != attributes.configurable() {
                    return Err("cannot redefine non-configurable property".to_string());
                }
            }
        }
        obj.set_prop_storage(pos, val);
        obj.set_data_meta(pos, attributes);
        obj.bump_generation();
        Ok(())
    }

    pub(crate) fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        vm_trace!("define_accessor_property: shape={} prop_si={}", obj.shape_id(), prop_name_si);
        let target_ptr = obj as *mut JsObject;
        let get = self.promote_if_needed_for_write_ptr(target_ptr, get);
        let set = self.promote_if_needed_for_write_ptr(target_ptr, set);
        if obj.is_array() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                return self.define_array_index_element(obj, index, JsValue::undefined(), attributes, true, get, set);
            }
        }
        let pos = if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            // shape 槽位 → 存储索引（数组属性在元素区之后）。
            if obj.is_array() {
                obj.array_prop_count as usize + pos as usize
            } else {
                pos as usize
            }
        } else {
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            obj.push_prop(JsValue::undefined()) as usize
        };
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable()
                && (!current.is_accessor
                    || current.attributes.enumerable() != attributes.enumerable()
                    || current.attributes.configurable() != attributes.configurable()
                    || current.get != get
                    || current.set != set)
            {
                return Err("cannot redefine non-configurable property".to_string());
            }
        }
        obj.set_prop_storage(pos, JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, attributes);
        obj.bump_generation();
        Ok(())
    }

    /// 数组索引属性（`"0"`~`"4294967294"`）的 define 路径：存入元素区并维护
    /// `array_prop_count`（length 随最高索引增长），meta 与元素槽对齐。
    fn define_array_index_element(
        &mut self, obj: &mut JsObject, index: u32, val: JsValue, attributes: PropAttributes, is_accessor: bool,
        get: JsValue, set: JsValue,
    ) -> Result<(), String> {
        let pos = index as usize;
        if pos > oxide_types::object::MAX_DENSE_PROPS {
            return Err("array index out of dense range".to_string());
        }
        let pos = pos as u32;
        if let Some(current) = obj.prop_meta_at(pos) {
            if !current.attributes.configurable() {
                if current.is_accessor != is_accessor {
                    return Err("cannot redefine non-configurable property".to_string());
                }
                if current.attributes.enumerable() != attributes.enumerable() {
                    return Err("cannot redefine non-configurable property".to_string());
                }
                if current.attributes.configurable() != attributes.configurable() {
                    return Err("cannot redefine non-configurable property".to_string());
                }
                if is_accessor {
                    if current.get != get || current.set != set {
                        return Err("cannot redefine non-configurable property".to_string());
                    }
                } else if !current.attributes.writable()
                    && (attributes.writable() || !coercion::same_value(obj.get_prop_at(pos), val))
                {
                    return Err("cannot redefine non-configurable property".to_string());
                }
            }
        }
        obj.set_prop_at(pos, val);
        if is_accessor {
            obj.set_accessor_meta(pos, get, set, attributes);
        } else {
            obj.set_data_meta(pos, attributes);
        }
        obj.bump_generation();
        Ok(())
    }
}
