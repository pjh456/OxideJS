use crate::vm::{FrameArgs, FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::{ic_trace, vm_trace};
use oxide_kernel::prop_forge::PropTemplate;
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
        let length_si = self.length_si;
        let mut current = Some(obj);
        let mut depth = 0usize;
        while let Some(obj) = current {
            if obj.is_array() && prop_name_si == length_si {
                return Ok(obj.logical_len_value());
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
                                    match self.call_function_sync(meta.get, receiver, &[]) {
                                        Ok(v) => Ok(v),
                                        Err(err) => Ok(self.raise_call_error(&err)?),
                                    }
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
                            match self.call_function_sync(meta.get, receiver, &[]) {
                                Ok(v) => Ok(v),
                                Err(err) => Ok(self.raise_call_error(&err)?),
                            }
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
            FrameArgs::Slice(&[]),
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::AccessorGet { target_reg },
            0,
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

    /// 全局 builtin 属性 A 侧写成功后反向同步当前帧镜像槽（成员写 / define /
    /// delete 成功后调用）。
    ///
    /// # 边界与前提
    /// - 仅当接收者为会话全局对象时生效：非全局写经指针判等短路（热路径成本
    ///   = 一次指针比较）。
    /// - 名集以活动模块 `builtin_reg_map` 为准（与编译期登记同源）：键不在表
    ///   内即无镜像槽，不写。
    /// # 副作用
    /// - 写当前帧寄存器文件镜像槽。
    /// # 注意事项
    /// - 只可挂在真实 A 侧写发生之后；写失败路径（只读 no-op / 抛错）不得
    ///   调用，否则污染镜像槽。
    pub(crate) fn sync_global_builtin_mirror(&mut self, obj: &JsObject, key_si: u32, val: JsValue) {
        if !std::ptr::eq(obj as *const JsObject, self.session.global_object().as_ptr()) {
            return;
        }
        let Some(module) = self.active_module() else {
            return;
        };
        for (name, reg) in &module.builtin_reg_map {
            if self.kernel_core.perm_interner().intern(name.as_str()).0 == key_si {
                self.regs[*reg as usize] = val;
                return;
            }
        }
    }

    /// IC 命中快路径的防御变体（命中分支不解析键）：按 (shape, slot) 定位被
    /// 写属性，自镜像名集反查键后同步。
    ///
    /// # 边界与前提
    /// - 仅当写落在全局对象自身时生效：原型链目标命中（depth > 0）不改全局
    ///   自身属性，不同步。
    /// - 全局对象自构造起恒带 prop_meta，IC 命中分支对今日不可达；本挂点防
    ///   该不变量被未来优化破坏后镜像静默失步。
    pub(crate) fn sync_global_builtin_mirror_slot(
        &mut self, obj: &JsObject, shape_id: u32, slot: u32, depth: u8, val: JsValue,
    ) {
        if !std::ptr::eq(obj as *const JsObject, self.session.global_object().as_ptr()) {
            return;
        }
        if depth != 0 || shape_id != obj.shape_id() {
            return;
        }
        let Some(module) = self.active_module() else {
            return;
        };
        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            if self.kernel_core.shape_forge().lookup_position(shape_id, si) == Some(slot) {
                self.regs[*reg as usize] = val;
                return;
            }
        }
    }

    /// 重载活动帧模块（`active_flat_id`）的 builtin 镜像槽：先按值取出名集再
    /// 写寄存器，避免借用交叉。帧恢复 / 重执行边界调用。
    pub(crate) fn reload_active_module_mirror_slots(&mut self) {
        let map = self.active_module().map(|m| m.builtin_reg_map.clone()).unwrap_or_default();
        self.reload_builtin_mirror_slots(&map);
    }

    /// 从全局对象 A 侧重载模块 builtin 名集的镜像槽：属性在位取原始存储值，
    /// 缺位写 undefined。
    ///
    /// # 边界与前提
    /// - 缺位臂语义为"写 undefined"，各入口（run / 帧 / inline / 恢复）统一，
    ///   使成员形删除后的裸读与入口预载看到同一值。
    /// # 副作用
    /// - 只写镜像槽寄存器（槽下标受编译期登记约束）。
    pub(crate) fn reload_builtin_mirror_slots(&mut self, map: &[(String, u32)]) {
        if map.is_empty() {
            return;
        }
        let global = self.session.global_object();
        for (name, reg) in map {
            let si = self.kernel_core.perm_interner().intern(name.as_str()).0;
            let val = self
                .kernel_core
                .shape_forge()
                .lookup_position(global.shape_id(), si)
                .map(|pos| global.get_prop_at(pos))
                .unwrap_or_else(JsValue::undefined);
            self.regs[*reg as usize] = val;
        }
    }

    /// `strict` 为写方（执行赋值的那个函数/脚本）的严格模式：写失败时严格抛
    /// TypeError，sloppy 静默 no-op（ECMA-262 OrdinarySetOwnProperty 失败分支）。
    pub(crate) fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, false, strict)
    }

    /// 分发期入口：调用方（dispatch_set_prop 等）已对值做过 promote。
    pub(crate) fn ordinary_set_dispatch(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, true, strict)
    }

    fn ordinary_set_inner(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, use_frame_push: bool,
        strict: bool,
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
                if !std::ptr::eq(receiver.as_js_object_ptr(), obj as *mut JsObject) {
                    return self.set_to_receiver(
                        obj,
                        prop_name_si,
                        val,
                        receiver,
                        index as usize,
                        use_frame_push,
                        strict,
                    );
                }
                return oxide_builtins::typed_array::typed_array_element_set(self, obj, index, val);
            }
        }
        // 数组 length 赋值：ArraySetLength 语义（ToUint32 + 调整元素区）。
        // 旧行为会把 length 存成影子命名属性，导致 `arr.length = N` 后
        // prop_count/迭代/内置方法看到的长度不一致。
        let length_si = self.length_si;
        if obj.is_array() && prop_name_si == length_si {
            // 冻结数组的 length 属性不可写（writable=false），赋值直接失败；
            // 两模式统一抛 TypeError（length 冻结检查的 strict/sloppy 差异不在本路径范围）。
            if obj.is_frozen() {
                return self.raise_type_error("Cannot assign to read only property 'length'");
            }
            let pc_before = self.pc;
            let number_len = self.coerce_number_bounded(val)?;
            // ToPrimitive 抛错（valueOf/toString throw）已被 unwind 定向到外围 catch 时
            // pc 指向 catch 入口：主 dispatch 约定异常后 opcode 不得继续 raise，直接
            // 返回由 dispatch 继续执行 catch，避免二次抛错覆盖原异常。
            if self.pc != pc_before {
                return Ok(());
            }
            let raw_new_len = if number_len == 0.0 || !number_len.is_finite() {
                0
            } else {
                number_len.trunc().rem_euclid(4_294_967_296.0) as u32 as usize
            };
            // ToUint32 != ToNumber (e.g. 1.5 / NaN / Infinity / negative / 2**32) -> RangeError.
            if raw_new_len as f64 != number_len {
                return self.raise_error_kind("RangeError", "Invalid array length");
            }
            let old_logical = obj.logical_len() as usize;
            let old_count = obj.array_prop_count as usize;
            // ArraySetLength：增长（newLen > oldLen）要求对象可扩展，不可扩展时
            // 整个赋值失败且不修改（length 失败两模式均静默 no-op，不抛）。
            if raw_new_len > old_logical && !obj.is_extensible() {
                return Ok(());
            }
            // ArraySetLength：收缩时若 [newLen, oldLen) 内存在不可配置元素，整个收缩
            // 失败且不做任何修改（length 失败两模式均静默 no-op，不抛）。
            if raw_new_len < old_logical {
                for idx in raw_new_len..old_count {
                    if let Some(meta) = obj.prop_meta_at(idx) {
                        if !meta.attributes.configurable() {
                            return Ok(());
                        }
                    }
                }
            }
            // Dense storage caps at MAX_DENSE_PROPS; larger lengths stay at the cap.
            let new_len_u = raw_new_len.min(oxide_types::object::MAX_DENSE_PROPS);
            obj.set_prop_count(new_len_u);
            // Grown slots are sparse holes: HasProperty / prototype reads must treat
            // them as absent.
            for idx in old_count..new_len_u {
                obj.mark_hole_at(idx);
            }
            // Logical length override: lengths beyond the dense cap are recorded
            // separately so `a.length` reads return the real value.
            if raw_new_len > oxide_types::object::MAX_DENSE_PROPS {
                obj.set_array_len_override(raw_new_len as u32);
            } else {
                obj.clear_array_len_override();
            }
            // 同步旧版可能残留的影子 length 属性（shape 槽），保证 IC 快路径读到新值。
            if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), length_si) {
                let store_idx = obj.array_prop_count as usize + pos as usize;
                obj.set_prop_storage(store_idx, obj.logical_len_value());
            }
            return Ok(());
        }
        if let Some(pos) = self.get_own_property_slot(obj, prop_name_si) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    if meta.set.is_undefined() {
                        // 无 setter：严格抛错，sloppy 静默 no-op。
                        if strict {
                            return self.raise_type_error("property has no setter");
                        }
                        return Ok(());
                    }
                    return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
                }
                if !meta.attributes.writable() {
                    // 只读数据属性：严格抛错，sloppy 静默 no-op。
                    if strict {
                        return self.raise_type_error("cannot assign to read-only property");
                    }
                    return Ok(());
                }
            }
            // pos 是存储索引（get_own_property_slot 对数组已加元素区偏移）。
            obj.set_prop_storage(pos as usize, val);
            self.sync_global_builtin_mirror(obj, prop_name_si, val);
            return Ok(());
        }

        if let Some(meta) = self.inherited_property_meta(obj, prop_name_si) {
            if meta.is_accessor {
                if meta.set.is_undefined() {
                    // 继承无 setter：严格抛错，sloppy 静默 no-op。
                    if strict {
                        return self.raise_type_error("property has no setter");
                    }
                    return Ok(());
                }
                return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
            }
            if !meta.attributes.writable() {
                // 继承只读数据属性：严格抛错，sloppy 静默 no-op（不遮蔽）。
                if strict {
                    return self.raise_type_error("cannot assign to read-only property");
                }
                return Ok(());
            }
        }

        // 新属性（自身与原型链均无同名）：须对象可扩展（OrdinarySet 的 extensible
        // 检查），不可扩展时赋值失败（严格抛错，sloppy 静默 no-op）。
        if !obj.is_extensible() {
            if strict {
                return self.raise_type_error("object is not extensible");
            }
            return Ok(());
        }
        self.set_or_create_prop_value(obj, prop_name_si, val);
        Ok(())
    }

    /// TypedArray 整数索引在 `receiver` ≠ TA 时的 [[Set]] 语义：越界或非对象
    /// receiver 直接返回 true（不写、不 ToNumber）；界内对象 receiver 走普通 set
    /// 把属性落到 receiver 自身。
    #[allow(clippy::too_many_arguments)]
    fn set_to_receiver(
        &mut self, ta_obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, index: usize,
        use_frame_push: bool, strict: bool,
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
        self.ordinary_set_inner(receiver_obj, prop_name_si, promoted, receiver, use_frame_push, strict)
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
            FrameArgs::Slice(&[val]),
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::AccessorSet,
            0,
        )?;
        Ok(())
    }

    /// 读取 member 复合操作数（obj.x / obj["x"]），返回 (值, IC 扩展字起始位置)。
    ///
    /// `ext_pc` 供同一指令的写侧 [`Self::set_member_prop`] 使用：读侧负责推进 pc
    /// 越过扩展字，写侧显式接收该位置、不再接触 pc，消除隐式 pc 契约。
    pub(crate) fn read_member_prop(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<(JsValue, usize), String> {
        vm_trace!("read_member_prop: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let (cached_shape_id, cached_slot, cached_depth) =
            crate::ic_helper::read_ic_slot0(&self.bytecode, &mut self.pc);
        let ic_pc = self.pc;
        if obj.has_prop_meta() {
            return self.ordinary_get(obj, prop_name_si, receiver).map(|v| (v, ic_pc));
        }

        let val = if let Some(v) = crate::ic_helper::ic_get_hit(obj, cached_shape_id, cached_slot, cached_depth) {
            v
        } else if let Some(v) =
            crate::ic_helper::ic_get_hit_poly(obj, &self.bytecode, ic_pc - oxide_bytecode::opcode::IC_EXT_WORDS)
        {
            v
        } else if let Some(template) = self.kernel_core.prop_forge().get_template(obj.shape_id()) {
            if template.prop_name != prop_name_si {
                self.proto_chain_ic_get(obj, prop_name_si, receiver)?
            } else if template.position < obj.prop_vec_len() as u32 {
                crate::ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, obj.shape_id(), template.position, 0);
                obj.get_prop_shape(template.position)
            } else {
                self.proto_chain_ic_get(obj, prop_name_si, receiver)?
            }
        } else {
            self.proto_chain_ic_get(obj, prop_name_si, receiver)?
        };
        Ok((val, ic_pc))
    }

    /// 执行 ordinary_get 并把 IC 按原型链深度写回（own 属性 depth=0，原型链按层计数）。
    pub(crate) fn proto_chain_ic_get(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<JsValue, String> {
        let ic_pc = self.pc;
        let resolved = self.ordinary_get(obj, prop_name_si, receiver)?;
        // 快路径：自身属性（depth=0）。
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            if !obj.is_accessor_meta(pos) {
                crate::ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, obj.shape_id(), pos, 0);
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
                    crate::ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, co.shape_id(), pos, depth);
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

    /// member 复合写（obj.x += 1 / obj.x++ 等）的写侧：优先 IC 写命中（depth==0 槽）
    /// 直写返回，miss 走查表/写新属性快路径。
    ///
    /// `ext_pc` 由调用方从 [`Self::read_member_prop`] 返回值带出（读侧已把 pc 推进
    /// 越过本 IC 指令扩展字后的位置）；写侧不再接触 pc，避免二次推进导致指令定位错位。
    pub(crate) fn set_member_prop(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, ext_pc: usize,
    ) -> Result<(), String> {
        ic_trace!("set_member_prop: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        // 写方即当前执行函数（赋值语义），strict/sloppy 判定取当前上下文。
        let strict = self.current_strict();
        if obj.has_prop_meta() {
            self.ordinary_set(obj, prop_name_si, val, receiver, strict)?;
            return Ok(());
        }
        if crate::ic_helper::ic_set_hit_own(obj, &self.bytecode, ext_pc, val) {
            self.profiling.record_ic_hit();
            // IC 写命中快路径防御：今日全局对象带 meta 不可达，防不变量被破坏
            // 后静默失步。
            self.sync_global_builtin_mirror(obj, prop_name_si, val);
            return Ok(());
        }
        self.profiling.record_ic_miss();
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_shape(pos, val);
            self.sync_global_builtin_mirror(obj, prop_name_si, val);
            crate::ic_helper::write_ic_back(self.bytecode_mut(), ext_pc, obj.shape_id(), pos, 0);
        } else if self.named_prop_create_needs_ordinary_set(obj, prop_name_si) {
            // 数组 length / 整数索引键写新属性：分流回 ordinary_set（ArraySetLength /
            // 元素区写），与 dispatch_ic_set_prop 对称，防快路径建影子槽破坏数组语义。
            self.ordinary_set(obj, prop_name_si, val, receiver, strict)?;
        } else {
            self.create_named_prop_fast(obj, prop_name_si, val, receiver, ext_pc, false, strict)?;
        }
        Ok(())
    }

    /// 写新属性的 CreateDataProperty 快路径（shape 转换，IC 命中在数学上无法覆盖）：
    /// 原型链同名 accessor/只读检查后，一次 make_shape + 追加槽 + IC 写回 + 模板 upsert。
    ///
    /// # 边界与前提
    /// - 调用方已确认属性不存在于 shape 链（lookup_position miss）。
    /// - 数组 length / 数组与 TA 整数索引键不进入本路径（调用方先分流到 ordinary_set）。
    /// - `use_frame_push` 与 ordinary_set* 入口一致（member 写 false / IC_SET 分发 true）。
    /// - `strict` 为写方的严格模式：原型链只读/无 setter/不可扩展时严格抛错、sloppy 静默 no-op。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_named_prop_fast(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, ext_pc: usize,
        use_frame_push: bool, strict: bool,
    ) -> Result<(), String> {
        // 原型链同名 accessor 须触发 setter、只读 data 须报错（与 ordinary_set 一致），
        // 命中时不得创建 own 属性（规范 [[Set]] shadow 语义）。
        if let Some(meta) = self.inherited_property_meta(obj, prop_name_si) {
            if meta.is_accessor {
                if meta.set.is_undefined() {
                    // 继承无 setter：严格抛错，sloppy 静默 no-op。
                    if strict {
                        return self.raise_type_error("property has no setter");
                    }
                    return Ok(());
                }
                return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
            }
            if !meta.attributes.writable() {
                // 继承只读数据属性：严格抛错，sloppy 静默 no-op（不遮蔽）。
                if strict {
                    return self.raise_type_error("cannot assign to read-only property");
                }
                return Ok(());
            }
        }
        // 新属性创建要求对象可扩展（规范 extensible 检查在原型链 setter/只读判定之后）。
        if !obj.is_extensible() {
            // 不可扩展：严格抛错，sloppy 静默 no-op。
            if strict {
                return self.raise_type_error("object is not extensible");
            }
            return Ok(());
        }
        // 新 shape 槽位 = 追加前命名属性数（与 push_prop 的追加位置一致）。
        let slot = obj.prop_vec_len() as u32;
        let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
        obj.set_shape_id(new_shape_id);
        obj.push_prop(val);
        obj.bump_generation();
        self.sync_global_builtin_mirror(obj, prop_name_si, val);
        crate::ic_helper::write_ic_back(self.bytecode_mut(), ext_pc, new_shape_id, slot, 0);
        self.kernel_core.prop_forge().upsert(
            new_shape_id,
            PropTemplate {
                shape_id: new_shape_id,
                prop_name: prop_name_si,
                position: slot,
                generation: obj.generation(),
            },
        );
        Ok(())
    }

    /// 写新属性须分流到完整 ordinary_set 语义的场景：数组 length（ArraySetLength）与
    /// 数组/TA 整数索引键（元素区 / buffer 写）——这些键不在 shape 链上，CreateDataProperty
    /// 快路径会错误地为其建命名属性。
    pub(crate) fn named_prop_create_needs_ordinary_set(&self, obj: &JsObject, prop_name_si: u32) -> bool {
        (obj.is_array() && prop_name_si == self.length_si)
            || (obj.is_typed_array_obj() || obj.is_array())
                && self.array_index_from_property_key(prop_name_si).is_some()
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
                // 新元素（越界或 hole 空洞）要求对象可扩展；已有元素覆盖不受限制。
                // 常规入口（ordinary_set）已预先拦截，此处为 REST/SPREAD/builtin
                // 内部等直调方的兜底。
                let is_new = index >= obj.array_prop_count || obj.prop_meta_at(index).is_some_and(|m| m.is_hole());
                if is_new && !obj.is_extensible() {
                    return;
                }
                obj.set_prop_at(index, val);
                return;
            }
        }
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            obj.set_prop_shape(pos, val);
            self.sync_global_builtin_mirror(obj, prop_name_si, val);
        } else {
            // 不可扩展对象禁止新增命名属性（兜底路径，常规入口已拦截）。
            if !obj.is_extensible() {
                return;
            }
            let new_shape_id = self.kernel_core.shape_forge().make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            // 数组对象：属性追加到 hash_props 属性区（元素之后），array_prop_count 不变。
            obj.push_prop(val);
            obj.bump_generation();
            self.sync_global_builtin_mirror(obj, prop_name_si, val);
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
        // 新命名属性（shape 链 lookup miss）且对象不可扩展 → 拒绝定义
        // （Object.defineProperty 抛 TypeError；Reflect.defineProperty 自动转 false）。
        if self
            .kernel_core
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
            .is_none()
            && !obj.is_extensible()
        {
            return Err("object is not extensible".to_string());
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
        self.sync_global_builtin_mirror(obj, prop_name_si, val);
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
        // 新命名属性（shape 链 lookup miss）且对象不可扩展 → 拒绝定义。
        if self
            .kernel_core
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
            .is_none()
            && !obj.is_extensible()
        {
            return Err("object is not extensible".to_string());
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
        // accessor 化同步原始存储值（undefined），与入口预载读裸槽同不变式；
        // getter 穿透属镜像模型结构残面，不在本挂点扩面。
        self.sync_global_builtin_mirror(obj, prop_name_si, JsValue::undefined());
        Ok(())
    }

    /// 数组索引属性（`"0"`~`"4294967294"`）的 define 路径：存入元素区并维护
    /// `array_prop_count`（length 随最高索引增长），meta 与元素槽对齐。
    #[allow(clippy::too_many_arguments)]
    fn define_array_index_element(
        &mut self, obj: &mut JsObject, index: u32, val: JsValue, attributes: PropAttributes, is_accessor: bool,
        get: JsValue, set: JsValue,
    ) -> Result<(), String> {
        let pos = index as usize;
        if pos > oxide_types::object::MAX_DENSE_PROPS {
            return Err("array index out of dense range".to_string());
        }
        // 新元素（越界或 hole 空洞）要求对象可扩展；已有元素重定义不受限（走下方
        // non-configurable 校验）。
        let is_new = pos >= obj.array_prop_count as usize || obj.prop_meta_at(pos).is_some_and(|m| m.is_hole());
        if is_new && !obj.is_extensible() {
            return Err("object is not extensible".to_string());
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
