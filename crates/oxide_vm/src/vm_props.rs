use crate::vm::{FrameArgs, FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::{ic_trace, vm_trace};
use oxide_kernel::prop_forge::PropTemplate;
use oxide_runtime_api as coercion;
use oxide_types::object::{JsObject, PropAttributes, PropMetaEntry};
use oxide_types::value::JsValue;

impl Vm {
    /// 属性读入口：解析 `obj[prop_name_si]`，依次尝试数组 length 虚拟属性、
    /// 数组元素区、TypedArray 整数索引、命名属性槽，最后沿原型链查找。
    ///
    /// # 步骤
    /// 1. 数组 length 键返回逻辑长度值，不落属性存储。
    /// 2. 数组整数索引在元素区命中且非 hole 时返回元素值；访问器元素触发 getter。
    /// 3. 顶层 TypedArray 经统一数值键门：界内整数读底层 buffer，数字无效键
    ///    立即 undefined，非规范数字串落普通路径。
    /// 4. 命名属性槽命中返回槽值；accessor 触发 getter。
    /// 5. 全部 miss 时沿原型链逐层查找，深度以 `MAX_PROTO_CHAIN_DEPTH` 为界。
    ///
    /// # 边界与前提
    /// - 元素区 hole（删除标记）与越界索引视同不存在，继续落原型链；
    /// - getter 未定义时返回 undefined；整条原型链 miss 返回 undefined。
    ///
    /// # 副作用
    /// - getter 经 `call_function_sync` 同步调用执行：native 直接调用，字节码
    ///   函数同 epoch 内联。
    ///
    /// # 注意事项
    /// - 本入口不带目标寄存器，字节码 getter 的帧化变体见
    ///   `ordinary_get_with_target`。
    pub(crate) fn ordinary_get(
        &mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue,
    ) -> Result<JsValue, String> {
        self.ordinary_get_inner(obj, prop_name_si, receiver, None)
    }

    /// 带目标寄存器的读入口：访问器 getter 为字节码函数时帧化执行，结果由
    /// `target_reg` 承接；其余解析逻辑同 `ordinary_get`。
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
            // 模块命名空间 exotic [[Get]]：条目表是权威状态，预注册但未初始化的
            // 导出读抛 ReferenceError；非 live ns（无表）落普通属性路径零行为变化。
            if obj.is_module_namespace() {
                if let Some(state) = oxide_builtins::module::module_ns_export(obj, prop_name_si) {
                    return match state {
                        oxide_builtins::module::ModuleNsQuery::Initialized(v) => Ok(v),
                        oxide_builtins::module::ModuleNsQuery::Uninitialized => {
                            let msg = oxide_builtins::module::NS_UNINITIALIZED_MESSAGE;
                            if self.native_call_depth == 0 {
                                self.raise_error_kind("ReferenceError", msg)?;
                            } else {
                                return Err(self.error_message_text("ReferenceError", msg));
                            }
                            Ok(JsValue::undefined())
                        }
                    };
                }
            }
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
            // 统一数值键门（exotic [[Get]]）：界内整数读底层 buffer；数字无效
            // 键（负/分数/±Infinity/NaN/越界，含 "-0" 特例）立即 undefined 不
            // 查自身命名属性也不走原型链；非规范数字串落下方普通属性路径。
            // 门只落在顶层对象：原型链上的 TA 维持普通 shape 槽查找。
            if obj.is_typed_array_obj() && depth == 0 {
                match oxide_builtins::typed_array::ta_index_gate(self, obj, prop_name_si) {
                    oxide_builtins::typed_array::TaIndexGate::NumericValid(index) => {
                        return oxide_builtins::typed_array::typed_array_element_get(self, obj, index);
                    }
                    oxide_builtins::typed_array::TaIndexGate::NumericInvalid => {
                        return Ok(JsValue::undefined());
                    }
                    oxide_builtins::typed_array::TaIndexGate::Ordinary => {}
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

    /// 将访问器 getter 帧化执行：native getter 同步调用并写回 `target_reg`，
    /// 字节码 getter 压入 `AccessorGet` 帧由主循环执行。
    ///
    /// # 步骤
    /// 1. 校验 getter 为可调用函数对象，否则抛 TypeError。
    /// 2. native getter：同步调用 `call_function_sync`，结果写入 `target_reg`，
    ///    返回 `false`（调用方在当前帧继续）。
    /// 3. 字节码 getter：压入 `AccessorGet` 帧并记录目标寄存器，返回 `true`。
    ///
    /// # 边界与前提
    /// - `target_reg` 须在寄存器文件范围内；调用方按返回值区分「结果已写回」
    ///   与「帧化待执行」两种状态。
    ///
    /// # 副作用
    /// - native 路径写 `target_reg`；字节码路径改写帧栈与 pc，并设置
    ///   `accessor_frame_target_reg`。
    ///
    /// # 注意事项
    /// - native getter 抛错经 `raise_call_error` 恢复为可捕获的 JS 异常；
    ///   返回 `true` 时结果寄存器尚无值，调用方不得读取。
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

    /// 全局 builtin 属性在原始存储侧（全局对象自身属性槽，下文简称 A 侧）写成功后
    /// 反向同步当前帧镜像槽（成员写 / define / delete 成功后调用）。
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
    /// 写寄存器，避免借用交叉。帧恢复 / 重执行边界调用；`pack_end` 为外层
    /// native pack 实参区上界（独占），落域镜像槽跳过。
    pub(crate) fn reload_active_module_mirror_slots(&mut self, pack_end: usize) {
        let map = self.active_module().map(|m| m.builtin_reg_map.clone()).unwrap_or_default();
        self.reload_builtin_mirror_slots(&map, pack_end);
    }

    /// 从全局对象 A 侧重载模块 builtin 名集的镜像槽：属性在位取原始存储值，
    /// 缺位写 undefined。
    ///
    /// # 边界与前提
    /// - 缺位臂语义为"写 undefined"，各入口（run / 帧 / inline / 恢复）统一；
    ///   裸读已路由 A 侧全局对象属性，槽为写侧/不变式维护（槽 = A 侧原始存储），
    ///   不再被读消费。
    /// - `pack_end` 为 native pack 实参区上界（独占）：落 [0, pack_end) 的镜像
    ///   槽跳过，实参值只存在于寄存器、无刷新源，窗口拷回负责其还原。
    /// # 副作用
    /// - 只写镜像槽寄存器（槽下标受编译期登记约束）。
    pub(crate) fn reload_builtin_mirror_slots(&mut self, map: &[(String, u32)], pack_end: usize) {
        if map.is_empty() {
            return;
        }
        let global = self.session.global_object();
        for (name, reg) in map {
            if (*reg as usize) < pack_end {
                continue;
            }
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
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, false, strict, false)
    }

    /// builtin 内部写入口：失败一律返回格式化 `Err` 由调用边界恢复为异常对象，
    /// 不就地 `unwind`——builtin 执行在重入的字节码上下文中，就地展开会跳入
    /// 调用方 catch 并让 builtin 继续，随后覆盖异常寄存器。
    pub(crate) fn ordinary_set_builtin(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        let val = self.promote_if_needed_for_write_ptr(obj as *mut JsObject, val);
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, false, strict, true)
    }

    /// 分发期入口：调用方（dispatch_set_prop 等）已对值做过 promote。
    pub(crate) fn ordinary_set_dispatch(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, strict: bool,
    ) -> Result<(), String> {
        self.ordinary_set_inner(obj, prop_name_si, val, receiver, true, strict, false)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "写路径须透传 receiver、帧化与 builtin 三组调用契约标志"
    )]
    fn ordinary_set_inner(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, use_frame_push: bool,
        strict: bool, builtin: bool,
    ) -> Result<(), String> {
        vm_trace!(
            "ordinary_set_inner: shape={} prop_si={} frame_push={}",
            obj.shape_id(),
            prop_name_si,
            use_frame_push
        );
        // 模块命名空间 exotic [[Set]] 恒 false（规范 10.4.6.8）：严格抛 TypeError，
        // sloppy 静默 no-op；Reflect.set 以 strict=true 调用并投影为 false。
        if obj.is_module_namespace() {
            if strict {
                return self.write_protection_failure(builtin, "Cannot assign to a module namespace export");
            }
            return Ok(());
        }
        // TypedArray 整数索引：receiver 为 TA 本体时写底层 buffer（越界静默忽略）；
        // receiver 非 TA 时按规范把写入落到 receiver 对象，不碰 TA buffer。
        // buffer 已 detach 的 TA：数值键（含非规范数值串）写静默忽略、不落
        // 命名属性；非数字串与 symbol 键落普通属性路径。
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
                        builtin,
                    );
                }
                return oxide_builtins::typed_array::typed_array_element_set(self, obj, index, val);
            }
            if std::ptr::eq(receiver.as_js_object_ptr(), obj as *mut JsObject)
                && oxide_builtins::typed_array::typed_array_detached_numeric_key_noop(self, obj, prop_name_si)
            {
                return Ok(());
            }
        }
        // 数组 length 赋值：走 ArraySetLength 语义（两次数值强转、可写性判定与
        // 元素区调整），不得落影子命名属性——否则 prop_count / 迭代 / 内置方法
        // 看到的长度与元素区不一致。
        let length_si = self.length_si;
        if obj.is_array() && prop_name_si == length_si {
            return self.set_array_length_value(obj, val, strict, builtin);
        }
        // 字符串 exotic 的 length 是 [[Writable]]: false 的固有数据属性（规范
        // String exotic [[Set]]）：写不成立，严格抛 TypeError、sloppy 静默
        // no-op——不得落命名属性区。
        if obj.is_string_obj() && prop_name_si == length_si {
            if strict {
                return self.write_protection_failure(builtin, "cannot assign to read-only property");
            }
            return Ok(());
        }
        if let Some(pos) = self.get_own_property_slot(obj, prop_name_si) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    if meta.set.is_undefined() {
                        // 无 setter：严格抛错，sloppy 静默 no-op。
                        if strict {
                            return self.write_protection_failure(builtin, "property has no setter");
                        }
                        return Ok(());
                    }
                    return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
                }
                if !meta.attributes.writable() {
                    // 只读数据属性：严格抛错，sloppy 静默 no-op。
                    if strict {
                        return self.write_protection_failure(builtin, "cannot assign to read-only property");
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
                        return self.write_protection_failure(builtin, "property has no setter");
                    }
                    return Ok(());
                }
                return self.call_or_push_setter(meta.set, receiver, val, use_frame_push);
            }
            if !meta.attributes.writable() {
                // 继承只读数据属性：严格抛错，sloppy 静默 no-op（不遮蔽）。
                if strict {
                    return self.write_protection_failure(builtin, "cannot assign to read-only property");
                }
                return Ok(());
            }
        }

        // 新属性（自身与原型链均无同名）：须对象可扩展（OrdinarySet 的 extensible
        // 检查先于 length 增长判定），不可扩展时赋值失败（严格抛错，sloppy 静默 no-op）。
        if !obj.is_extensible() {
            if strict {
                return self.write_protection_failure(builtin, "object is not extensible");
            }
            return Ok(());
        }

        // 数组索引增长（index >= 当前 length）过 length 可写性检查：与 define 侧
        // `define_array_index_element` 同序；length 不可写时严格抛 TypeError、
        // sloppy 静默。索引小于 length 的已有元素写入在自身槽位命中即返回，
        // 不达此处。
        if let Some(index) = self.array_index_from_property_key(prop_name_si) {
            if obj.is_array() && index >= obj.logical_len() && !obj.is_length_writable() {
                return self.array_length_write_failure(
                    strict,
                    builtin,
                    "Cannot add property, array length is not writable",
                );
            }
        }
        self.set_or_create_prop_value(obj, prop_name_si, val);
        Ok(())
    }

    /// 数组 length 赋值路径：`a.length = v` 的 `[[Set]]`，按 ArraySetLength 语义
    /// 调整元素区（与 define 侧 [`Self::define_array_length`] 共用截断/扩洞逻辑）。
    ///
    /// # 步骤
    /// 1. 入口可写位检查：已不可写时按模式直接返回（严格抛 TypeError、sloppy 静默），
    ///    不执行强转——可写位判定先于 ArrayLength，用户 valueOf/toPrimitive 不触发。
    /// 2. `ToUint32` 与 `ToNumber` 两次强转各自执行一次（均可触发用户代码）。
    /// 3. 两次结果不等（非整数、负数、NaN/Infinity、2^32 等）抛 RangeError。
    /// 4. 强转完成后复查可写位：强转期用户代码可能已收窄 length；失败按模式返回
    ///    且不做任何截断（写不可写数据描述符先于 ArraySetLength 失败）。
    /// 5. 收缩求最高不可配置阻挡索引：有则以「阻挡索引 + 1」部分截断后按模式返回
    ///    失败；无则完整截断/扩 hole 到目标长度。
    ///
    /// # 边界与前提
    /// - 仅由 `ordinary_set_inner` 对 `is_array()` 对象且键为 length 时调用。
    /// - 两次强转之后须复查可写位：用户代码可能在强转期收窄 length 可写位。
    ///
    /// # 副作用
    /// - 修改元素区与元素元数据、可能置/清 `array_len_override`、bump 世代。
    fn set_array_length_value(
        &mut self, obj: &mut JsObject, val: JsValue, strict: bool, builtin: bool,
    ) -> Result<(), String> {
        // 可写位判定先于 ArrayLength：入口即不可写时不执行强转——用户强转副作用
        // 不触发，强转期抛出的自定义错误也不得穿透 kind。
        if !obj.is_length_writable() {
            return self.array_length_write_failure(strict, builtin, "Cannot assign to read only property 'length'");
        }

        // ToNumber(BigInt) 抛 TypeError：在通用强转近似接受 BigInt 之前拦截。
        if val.is_bigint() {
            return self.write_protection_failure(builtin, "Cannot convert a BigInt value to a number");
        }

        let pc_before = self.pc;
        let new_len = self.coerce_uint32_bounded(val)?;
        // 强转抛错在主 dispatch 下已 unwind 到外围 catch，此时 opcode 不得继续，
        // 直接返回由 dispatch 执行 catch，避免二次抛错覆盖原异常。
        if self.pc != pc_before {
            return Ok(());
        }
        let number_len = self.coerce_number_bounded(val)?;
        if self.pc != pc_before {
            return Ok(());
        }
        if new_len as f64 != number_len {
            if builtin {
                return Err(self.error_message_text("RangeError", "Invalid array length"));
            }
            return self.raise_error_kind("RangeError", "Invalid array length");
        }

        // 两次强转后重新判定：强转期间用户代码可能已把 length 收窄为不可写。
        if !obj.is_length_writable() {
            return self.array_length_write_failure(strict, builtin, "Cannot assign to read only property 'length'");
        }

        let old_logical = obj.logical_len();
        let target_len = Self::array_length_shrink_target(obj, new_len, old_logical);
        Self::apply_array_length(obj, target_len);
        obj.bump_generation();

        if target_len != new_len {
            return self.array_length_write_failure(strict, builtin, "Cannot assign to read only property 'length'");
        }
        Ok(())
    }

    /// 属性写保护失败的 strict/sloppy 分派：严格模式抛 TypeError（非 builtin 就地
    /// 展开到最近 catch），sloppy 静默 no-op。
    fn array_length_write_failure(&mut self, strict: bool, builtin: bool, msg: &str) -> Result<(), String> {
        if !strict {
            return Ok(());
        }
        self.write_protection_failure(builtin, msg)
    }

    /// TypeError 的两态出口：非 builtin 就地抛可捕获异常并展开，builtin 内部返回
    /// 格式化 `Err` 由调用边界恢复为异常对象（builtin 在重入字节码上下文中，就地
    /// 展开会跳入调用方 catch 并让 builtin 继续，随后覆盖异常寄存器）。
    fn write_protection_failure(&mut self, builtin: bool, msg: &str) -> Result<(), String> {
        if builtin {
            Err(self.error_message_text("TypeError", msg))
        } else {
            self.raise_type_error(msg)
        }
    }

    /// TypedArray 整数索引在 `receiver` ≠ TA 时的 [[Set]] 语义：越界或非对象
    /// receiver 直接返回 true（不写、不 ToNumber）；界内对象 receiver 走普通 set
    /// 把属性落到 receiver 自身。
    #[allow(clippy::too_many_arguments)]
    fn set_to_receiver(
        &mut self, ta_obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue, index: usize,
        use_frame_push: bool, strict: bool, builtin: bool,
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
        self.ordinary_set_inner(receiver_obj, prop_name_si, promoted, receiver, use_frame_push, strict, builtin)
    }

    /// 调用或帧化访问器 setter：native（或禁止帧化的调用方）同步执行，
    /// 字节码 setter 压入 `AccessorSet` 帧。
    ///
    /// # 步骤
    /// 1. `use_frame_push=false`：直接同步调用 setter 并返回。
    /// 2. 校验 setter 为可调用函数对象，否则抛 TypeError。
    /// 3. native setter 同步调用；字节码 setter 压入 `AccessorSet` 帧并携带实参。
    ///
    /// # 边界与前提
    /// - 帧化路径要求 setter 为可调用对象；`use_frame_push` 由写入口按场景给出
    ///   （member 写传 false / IC 分发传 true）。
    ///
    /// # 副作用
    /// - 同步路径执行 setter 体；帧化路径改写帧栈与 pc。
    ///
    /// # 注意事项
    /// - native setter 抛错经 `call_function_sync` 以 `Err(String)` 返回，
    ///   此处经 `raise_call_error` 恢复为可捕获的 JS 异常（与 getter 路径对称）。
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
    /// 数组/TA 整数索引键（元素区 / buffer 写）、detach 后 TA 的数值键（[[Set]]
    /// 静默臂，非规范数值串如 "1.1"/"-0" 不在 shape 链上，CreateDataProperty
    /// 快路径会错误地为其建命名属性）。
    pub(crate) fn named_prop_create_needs_ordinary_set(&self, obj: &JsObject, prop_name_si: u32) -> bool {
        (obj.is_array() && prop_name_si == self.length_si)
            || (obj.is_typed_array_obj() || obj.is_array())
                && self.array_index_from_property_key(prop_name_si).is_some()
            || obj.is_typed_array_obj()
                && self.array_index_from_property_key(prop_name_si).is_none()
                && oxide_builtins::typed_array::typed_array_detached_numeric_key_noop_ro(self, obj, prop_name_si)
    }

    /// 值写入直调路径：REST / SPREAD / builtin 内部等已确定目标对象的场景，
    /// 跳过 `ordinary_set` 的严格模式、原型链 setter 与只读检查直接落值。
    ///
    /// # 步骤
    /// 1. TypedArray 整数索引写底层 buffer（越界静默忽略）。
    /// 2. 数组整数索引写元素区并维护 `array_prop_count`；新元素要求对象可扩展。
    /// 3. 命名属性槽命中直写槽值。
    /// 4. 命名属性 miss 且对象可扩展时 `make_shape` 追加新槽。
    ///
    /// # 边界与前提
    /// - 调用方须已确认接收者即目标对象（无原型链 setter / 只读遮蔽）。
    /// - 不可扩展对象的新属性写入静默忽略（常规入口已预先拦截，此处为兜底）。
    ///
    /// # 副作用
    /// - 写属性存储 / 元素区 / buffer；新增属性时修改 shape 与世代计数，
    ///   并同步全局 builtin 镜像槽。
    ///
    /// # 注意事项
    /// - 与 `ordinary_set` 的差异在于不做 writable / setter / 严格模式判定，
    ///   仅供语义已确定的内部调用方使用。
    pub(crate) fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue) {
        vm_trace!("set_or_create_prop_value: shape_id={} prop_name_si={}", obj.shape_id(), prop_name_si);
        // TypedArray 整数索引写 buffer（越界忽略），不进入 shape/prop 槽；
        // buffer 已 detach 的数值键写静默忽略（与 ordinary_set_inner 同臂）。
        if obj.is_typed_array_obj() {
            if let Some(index) = self.array_index_from_property_key(prop_name_si) {
                let _ = oxide_builtins::typed_array::typed_array_element_set(self, obj, index, val);
                return;
            }
            if oxide_builtins::typed_array::typed_array_detached_numeric_key_noop(self, obj, prop_name_si) {
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

    /// `defineProperty` 数据属性共享路径：写入或重定义数据属性并施加给定
    /// attributes。
    ///
    /// # 步骤
    /// 1. TypedArray 整数索引分流到元素定义；数组整数索引分流到元素区定义。
    /// 2. 新命名属性（shape 链 miss）且对象不可扩展时拒绝定义。
    /// 3. 命中现有槽且属性不可配置时做重定义校验。
    /// 4. 写槽值、数据属性 meta 与世代计数，并同步全局 builtin 镜像槽。
    ///
    /// # 边界与前提
    /// - `attributes` 为最终属性描述符，调用方已按 `defineProperty` 规则填充缺省值。
    /// - 不可配置属性的校验：不可把 accessor 改为数据属性；枚举性、可配置性不得
    ///   放宽；只读属性不可改为可写，改值须满足 `same_value`。
    ///
    /// # 副作用
    /// - 写属性存储与 meta；可能新增 shape 槽并 bump 世代；同步镜像槽。
    ///
    /// # 注意事项
    /// - 错误以 `Err(String)` 返回，由 `Object.defineProperty` 转 TypeError、
    ///   `Reflect.defineProperty` 转 `false`。
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
        // 数组 length 是无 shape 槽的虚拟数据属性：按 ArraySetLength 语义应用，
        // 不得走通用命名属性路径建影子槽（会造成读写分叉）。
        if obj.is_array() && prop_name_si == self.length_si {
            return self.define_array_length(obj, val, attributes);
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

    /// `defineProperty` 访问器属性共享路径：写入或重定义 getter/setter 并施加
    /// 给定 attributes。
    ///
    /// # 步骤
    /// 1. 数组整数索引分流到元素区访问器定义。
    /// 2. 新命名属性（shape 链 miss）且对象不可扩展时拒绝定义。
    /// 3. 命中现有槽且属性不可配置时做重定义校验。
    /// 4. 写空值槽、accessor meta 与世代计数，并同步全局 builtin 镜像槽。
    ///
    /// # 边界与前提
    /// - 不可配置 accessor 仅在 get/set 不变、枚举性与可配置性均不变时才允许
    ///   重定义；不可配置数据属性不可改为 accessor。
    ///
    /// # 副作用
    /// - 属性存储写 `undefined`，accessor meta 记录 get/set；可能新增 shape 槽
    ///   并 bump 世代；同步镜像槽。
    ///
    /// # 注意事项
    /// - 错误以 `Err(String)` 返回，由 `Object.defineProperty` 转 TypeError、
    ///   `Reflect.defineProperty` 转 `false`。
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
        // 数组 length 当前为不可配置数据属性，禁止转为访问器属性。
        if obj.is_array() && prop_name_si == self.length_si {
            return Err("cannot redefine non-configurable property".to_string());
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
        // 数组 exotic [[DefineOwnProperty]]：索引达到/超过当前 length 时须增长 length，
        // length 不可写则拒绝；索引小于 length 的元素重定义不受此限。
        if pos as u32 >= obj.logical_len() && !obj.is_length_writable() {
            return Err("cannot define property beyond non-writable length".to_string());
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

    /// 数组 length 虚拟属性的 define 路径：按 `ArraySetLength` 语义把描述符值
    /// 强转、校验并应用到元素区与逻辑长度。
    ///
    /// # 步骤
    /// 1. BigInt 值在强转前拦截为 TypeError；随后 `ToUint32` 与 `ToNumber` 两次
    ///    强转（均可触发用户代码），结果不等抛 RangeError，强转严格早于描述符校验。
    /// 2. 校验当前非可配置数据属性的收窄：configurable/enumerable 不得置真；
    ///    当前不可写时不得再请求 writable 或改动值。
    /// 3. 收缩时按规范删除循环语义求最高不可配置索引：存在则以「阻挡索引 + 1」
    ///    为最终长度部分截断（删除其上全部可配置元素），再返回失败；全部可配置
    ///    时完整截断到新长度。
    /// 4. 应用新逻辑长度（截断/扩 hole、dense 上限覆盖）并按描述符写可写位。
    ///
    /// # 边界与前提
    /// - 仅由 `define_data_property` 对 `is_array()` 对象且键为 `length_si` 时调用。
    /// - 描述符缺 `value` 时调用方以当前逻辑长度为哨兵值，数值强转为恒等无副作用
    ///   转换。
    ///
    /// # 副作用
    /// - 截断/扩展元素区与元素元数据、可能置/清 `array_len_override`、写 length
    ///   可写位、bump 世代。
    ///
    /// # 注意事项
    /// - 非法长度以 `"RangeError: "` 前缀标记 kind，强转失败以 `"TypeError: "`
    ///   前缀标记；用户代码抛出的原始异常值转存 VM 专用槽，由 Object/Reflect
    ///   入口取出并原值重抛。
    fn define_array_length(
        &mut self, obj: &mut JsObject, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String> {
        // BigInt 无 ToNumber 语义（ToNumber(BigInt) 抛 TypeError）：在强转前拦截，
        // 避免通用 to_number 的近似分支把 1n 当作 1.0 接受。
        if val.is_bigint() {
            return Err(self.error_message_text("TypeError", "Cannot convert a BigInt value to a number"));
        }

        // [[Value]] 两次强转都要实际执行（可触发用户代码），且须早于描述符校验；
        // 两次强转之间用户代码可能把 length 收窄为不可写。清空未捕获槽后执行，
        // 槽内值必为本次强转产生；用户抛出时原值转存专用槽供入口原值重抛，
        // 引擎转换失败（如 Symbol）留空由消息前缀重建 TypeError。
        self.last_uncaught_value = None;
        let new_len = match self.coerce_uint32_bounded(val) {
            Ok(v) => v,
            Err(msg) => {
                self.pending_length_exception = self.last_uncaught_value.take();
                return Err(msg);
            }
        };
        let number_len = match self.coerce_number_bounded(val) {
            Ok(v) => v,
            Err(msg) => {
                self.pending_length_exception = self.last_uncaught_value.take();
                return Err(msg);
            }
        };
        if new_len as f64 != number_len {
            return Err(self.error_message_text("RangeError", "Invalid array length"));
        }

        let old_logical = obj.logical_len();
        // 当前 length 是不可配置、不可枚举的数据属性；writable 由冻结标志与独立位决定。
        if attributes.configurable() || attributes.enumerable() {
            return Err("cannot redefine non-configurable property".to_string());
        }
        if !obj.is_length_writable() && (attributes.writable() || new_len != old_logical) {
            return Err("cannot redefine non-configurable property".to_string());
        }

        // 收缩按 ArraySetLength 删除循环语义求目标长度（部分截断 / 完整截断）。
        let target_len = Self::array_length_shrink_target(obj, new_len, old_logical);

        Self::apply_array_length(obj, target_len);
        obj.set_length_non_writable(!attributes.writable());
        obj.bump_generation();

        if target_len != new_len {
            return Err("cannot redefine non-configurable property".to_string());
        }
        Ok(())
    }

    /// 数组 length 收缩的目标长度：`[new_len, old_len)` 内存在不可配置元素时取
    /// 最高阻挡索引 + 1（部分截断，其上可配置元素已删除），否则完整截断到 `new_len`。
    ///
    /// # 边界与前提
    /// - `new_len >= old_logical` 时直接返回 `new_len`（非收缩路径）。
    fn array_length_shrink_target(obj: &JsObject, new_len: u32, old_logical: u32) -> u32 {
        if new_len < old_logical {
            match Self::highest_non_configurable_index(obj, new_len, old_logical) {
                Some(blocker) => blocker + 1,
                None => new_len,
            }
        } else {
            new_len
        }
    }

    /// 返回 `[from, old_len)` 内最高的不可配置自身元素索引；无则 `None`。
    ///
    /// # 边界与前提
    /// - 扫描上界取逻辑长度与稠密元素数的较小者：逻辑长度超过
    ///   `MAX_DENSE_PROPS` 时，其上的索引不存于稠密元素区，无自身属性可阻挡。
    /// - hole 标记为可配置（删除成功），不计入阻挡。
    fn highest_non_configurable_index(obj: &JsObject, from: u32, old_len: u32) -> Option<u32> {
        let scan_hi = (old_len as usize).min(obj.array_prop_count as usize);
        (from as usize..scan_hi)
            .rev()
            .find(|&idx| obj.prop_meta_at(idx).is_some_and(|m| !m.attributes.configurable()))
            .map(|idx| idx as u32)
    }

    /// 把数组逻辑长度与物理元素区收敛到 `final_len`：截断/补齐稠密元素并维护
    /// `array_len_override`。
    ///
    /// # 边界与前提
    /// - 稠密物理槽以 `MAX_DENSE_PROPS` 封顶，超出部分仅记逻辑长度覆盖。
    /// - 不写 length 可写位、不 bump 世代，由调用方统一处理。
    fn apply_array_length(obj: &mut JsObject, final_len: u32) {
        let old_count = obj.array_prop_count as usize;
        let new_phys = (final_len as usize).min(oxide_types::object::MAX_DENSE_PROPS);
        obj.set_prop_count(new_phys);

        // 扩出的槽是稀疏 hole：存在性检查与原型链读取须视同不存在。
        for idx in old_count..new_phys {
            obj.mark_hole_at(idx);
        }

        if final_len as usize > oxide_types::object::MAX_DENSE_PROPS {
            obj.set_array_len_override(final_len);
        } else {
            obj.clear_array_len_override();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use oxide_runtime_api::NativeResult;
    use oxide_types::object::NativeFnPtr;

    fn native_return_7(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
        NativeResult::Ok(JsValue::int(7))
    }

    fn native_get_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        NativeResult::Ok(vm.resolve_property(obj, marker_si).unwrap_or(JsValue::undefined()))
    }

    fn native_set_marker(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let this_val = vm.reg(args[0]);
        let value = vm.reg(args[1]);
        if !this_val.is_object() {
            return NativeResult::Ok(JsValue::undefined());
        }
        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, marker_si, value);
        NativeResult::Ok(JsValue::undefined())
    }

    fn native_function(vm: &mut Vm, f: crate::native::NativeFn) -> JsValue {
        let proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let mut obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.set_function(true);
        // SAFETY: f 是 NativeFn 函数项，可作为 NativeFnPtr 存储。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(f as *const ()) }));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    fn plain_object(vm: &mut Vm) -> JsValue {
        let proto = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let obj = JsObject::new_empty(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        JsValue::object(vm.alloc_object(obj) as *mut u8)
    }

    fn add_accessor(vm: &mut Vm, obj_val: JsValue, name: &str, get: JsValue, set: JsValue) {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        let shape_id = vm.kernel_core.shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape_id);
        let pos = obj.push_prop(JsValue::undefined());
        obj.set_accessor_meta(pos, get, set, PropAttributes::DEFAULT_DATA);
        obj.bump_generation();
    }

    fn set_data(vm: &mut Vm, obj_val: JsValue, name: &str, val: JsValue) {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.set_or_create_prop_value(obj, si, val);
    }

    #[test]
    fn ordinary_get_calls_own_native_getter() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        let getter = native_function(&mut vm, native_return_7);
        add_accessor(&mut vm, obj_val, "x", getter, JsValue::undefined());

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        let value = vm.ordinary_get(obj, x_si, obj_val).expect("getter");
        assert_eq!(value, JsValue::int(7));
    }

    #[test]
    fn ordinary_set_calls_own_native_setter() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, obj_val, "x", JsValue::undefined(), setter);

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        vm.ordinary_set(obj, x_si, JsValue::int(9), obj_val, true).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(obj, marker_si), Some(JsValue::int(9)));
    }

    #[test]
    fn inherited_getter_uses_original_receiver() {
        let mut vm = Vm::new();
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let getter = native_function(&mut vm, native_get_marker);
        add_accessor(&mut vm, proto_val, "x", getter, JsValue::undefined());
        set_data(&mut vm, child_val, "marker", JsValue::int(42));
        unsafe {
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        let value = vm.ordinary_get(child, x_si, child_val).expect("getter");
        assert_eq!(value, JsValue::int(42));
    }

    #[test]
    fn inherited_setter_uses_original_receiver() {
        let mut vm = Vm::new();
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, proto_val, "x", JsValue::undefined(), setter);
        unsafe {
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(12), child_val, true).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        let proto = unsafe { &*proto_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(12)));
        assert_eq!(vm.resolve_property(proto, marker_si), None);
    }

    #[test]
    fn deep_inherited_setter_uses_original_receiver() {
        let mut vm = Vm::new();
        let grand_proto_val = plain_object(&mut vm);
        let proto_val = plain_object(&mut vm);
        let child_val = plain_object(&mut vm);
        let setter = native_function(&mut vm, native_set_marker);
        add_accessor(&mut vm, grand_proto_val, "x", JsValue::undefined(), setter);
        unsafe {
            (*proto_val.as_js_object_ptr()).set_proto(grand_proto_val).expect("proto");
            (*child_val.as_js_object_ptr()).set_proto(proto_val).expect("proto");
        }

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let child = unsafe { &mut *child_val.as_js_object_ptr() };
        vm.ordinary_set(child, x_si, JsValue::int(15), child_val, true).expect("setter");

        let marker_si = vm.kernel_core.perm_interner().intern("marker").0;
        let child = unsafe { &*child_val.as_js_object_ptr() };
        assert_eq!(vm.resolve_property(child, marker_si), Some(JsValue::int(15)));
    }

    #[test]
    fn ordinary_data_property_still_reads_and_writes_without_meta() {
        let mut vm = Vm::new();
        let obj_val = plain_object(&mut vm);
        set_data(&mut vm, obj_val, "x", JsValue::int(1));

        let x_si = vm.kernel_core.perm_interner().intern("x").0;
        let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
        assert!(!obj.has_prop_meta());
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(1));
        vm.ordinary_set(obj, x_si, JsValue::int(2), obj_val, true).expect("set");
        assert_eq!(vm.ordinary_get(obj, x_si, obj_val).expect("get"), JsValue::int(2));
    }
}
