use crate::vm::{ForInIter, FrameArgs, FrameContinuation, Vm, MAX_PROTO_CHAIN_DEPTH};
use crate::vm_trace;
use oxide_runtime_api::{push_units_to, to_boolean, to_units_full};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{
    int_key_value, is_int_key, is_private_name_key, is_symbol_key, make_int_key, make_well_known_symbol_key,
    INT_KEY_COUNT, WELL_KNOWN_SYMBOL_HAS_INSTANCE,
};
use oxide_types::value::JsValue;

impl Vm {
    pub(crate) fn dispatch_new_expression(&mut self, rd: usize, a: usize, b: usize) -> Result<bool, String> {
        let constructor_reg = a;
        let first_arg_reg = b as u8;
        vm_trace!("NEW_EXPRESSION rd={}", rd);

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
        // native 方法（非构造器）不可 new；bound 包装除外（其构造语义转发到 target）。
        if ctor_obj.native_fn().is_some()
            && ctor_obj.type_tag != oxide_types::object::JsObject::OBJ_TYPE_CONSTRUCTOR
            && ctor_obj.type_tag != oxide_types::object::JsObject::OBJ_TYPE_BOUND
        {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }

        let ext = self.bytecode[self.pc];
        self.pc += 1;
        let arg_count = (ext & 0xFF) as usize;
        // ext 高 8 位 = 调用点存活上界（0 = 未编码/全量），压帧窗口按此截断。
        let call_window = (ext >> 8) as u8;

        // bound 包装：解包链后转发到最内层 target（[[Construct]] 语义）。
        if ctor_obj.type_tag == oxide_types::object::JsObject::OBJ_TYPE_BOUND {
            let mut args = Vec::with_capacity(arg_count);
            for i in 0..arg_count.min(256) {
                args.push(self.regs[first_arg_reg.wrapping_add(i as u8) as usize]);
            }
            let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
            let new_obj = self.alloc_object(JsObject::new_empty(
                oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
                JsValue::from_js_object(proto_ptr),
            ));
            return self.dispatch_new_bound(rd, constructor, new_obj, args, call_window);
        }

        let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
        let new_obj = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));

        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(ctor_obj, proto_si) {
            if proto_val.is_object() {
                let new_obj_mut = unsafe { &mut *new_obj };
                let proto_obj_ptr = proto_val.as_js_object_ptr();
                let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
            }
        }

        if ctor_obj.native_fn().is_some() {
            let new_obj_val = JsValue::object(new_obj as *mut u8);
            let mut args = Vec::with_capacity(arg_count);
            for i in 0..arg_count.min(256) {
                args.push(self.regs[first_arg_reg.wrapping_add(i as u8) as usize]);
            }
            // native 构造器：收口到 call_function_sync（与 spread / bound 变体一致）。
            // 该入口统一保存/恢复调用窗口与 253/254 槽且不触碰 255，调用方
            // new.target 与 this 跨 native 构造不被污染；receiver 经 arg0 打包，
            // native 侧经 reg(args[0]) 读取，构造语义不变。newTarget 快照后置入
            // reg(255) 暴露给 native 构造器（调用后恢复原值）；构造形态标记同窗
            // 夹持（调用后恢复）。
            let saved_new_target = self.regs[255];
            let saved_constructing = self.constructing_native;
            self.regs[255] = constructor;
            self.constructing_native = true;
            let result = self.call_function_sync(constructor, new_obj_val, &args);
            self.constructing_native = saved_constructing;
            self.regs[255] = saved_new_target;
            match result {
                Ok(v) => {
                    self.regs[rd] = if v.is_object() { v } else { new_obj_val };
                    Ok(false)
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, "constructor call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    self.unwind().map(|_| true)
                }
            }
        } else if ctor_obj.sub_module_index() > 0 {
            let sub_idx = ctor_obj.sub_module_index() as usize;
            let sub = match self.callee_module(ctor_obj) {
                Some(m) => m,
                None => {
                    // 上界取构造器自身代际平表长度（跨 run 调用时可异于当前代际）。
                    return Err(format!(
                        "NEW_EXPRESSION: sub_module_index {} out of bounds (max {})",
                        sub_idx,
                        self.tables.get(&ctor_obj.table_gen()).map(|t| t.modules.len()).unwrap_or(0)
                    ));
                }
            };
            // 生成器函数不是构造器：`new g()` 抛 TypeError。
            if sub.is_generator {
                return self.raise_type_error("g is not a constructor").map(|_| true);
            }
            // 异步函数不是构造器：`new f()` 抛 TypeError。
            if sub.is_async {
                return self.raise_type_error("g is not a constructor").map(|_| true);
            }

            let new_obj_val = JsValue::object(new_obj as *mut u8);
            // 收敛到统一压帧入口：derived 构造器在 super() 前 this 为 undefined，
            // 基类构造器 this = 新对象；new.target = 构造器本身。
            let this_value = if ctor_obj.is_derived_constructor() {
                JsValue::undefined()
            } else {
                new_obj_val
            };
            self.push_bytecode_frame(
                constructor,
                this_value,
                FrameArgs::RegRange {
                    first: first_arg_reg,
                    count: arg_count,
                },
                Some(rd as u8),
                Some(new_obj_val),
                constructor,
                FrameContinuation::None,
                call_window,
            )?;
            Ok(true)
        } else {
            let error = oxide_builtins::error::create_from_text(
                self,
                "NEW_EXPRESSION: bytecode constructors not yet supported",
            );
            self.exception_value = Some(error);
            self.pending_error_kind = Some(self.thrown_error_kind(error));
            self.unwind().map(|_| true)
        }
    }

    /// bound 函数构造分支：按 bound [[Construct]] 语义解包链后转发到最内层 target。
    ///
    /// # 步骤
    /// 1. 逐层解包 [[BoundTargetFunction]]：每层绑定实参（状态对象存储槽 2+）
    ///    拼到调用实参之前（外层先解包 → 最终顺序为内层绑定实参先、外层后、
    ///    调用实参尾）
    /// 2. 校验最内层 target 可构造（native 须 CONSTRUCTOR 标记；字节码须非
    ///    arrow/async/generator），否则抛 TypeError
    /// 3. 新对象原型取 target.prototype（new 表达式路径 newTarget 恒等于构造器，
    ///    规范 SameValue 替换后 newTarget = target）
    /// 4. 构造调用：native target 值传递（receiver = 新对象）；字节码 target 压
    ///    构造帧（this = 新对象，派生 target 为 undefined，new.target = target）
    ///
    /// # 边界与前提
    /// - `call_args` 为调用点实参（寄存器连续区或 spread 物化），绑定实参前置拼接
    /// - 原型属性非对象时保持 object_proto 默认（OrdinaryCreateFromConstructor 回退）
    pub(crate) fn dispatch_new_bound(
        &mut self, rd: usize, wrapper_val: JsValue, new_obj: *mut JsObject, mut call_args: Vec<JsValue>,
        call_window: u8,
    ) -> Result<bool, String> {
        // 逐层解包 bound 链，绑定实参前置拼接。
        let mut target_val = wrapper_val;
        loop {
            let wrapper_obj = unsafe { &*target_val.as_js_object_ptr() };
            // 绑定状态对象 [target, thisArg, ...boundArgs]（oxide_builtins::function
            // 固定第 4 号 shape 属性）：target 取存储槽 0，绑定实参取存储槽 2+。
            let state = oxide_builtins::function::bound_state_values(wrapper_obj);
            let target = state.first().copied().unwrap_or(JsValue::undefined());
            let mut combined: Vec<JsValue> = state.iter().skip(2).copied().collect();
            combined.extend_from_slice(&call_args);
            call_args = combined;
            let is_bound = target.is_object()
                && !target.as_js_object_ptr().is_null()
                && unsafe { &*target.as_js_object_ptr() }.type_tag == JsObject::OBJ_TYPE_BOUND;
            target_val = target;
            if !is_bound {
                break;
            }
        }

        // 校验最内层 target 可构造。
        if !target_val.is_object() || target_val.as_js_object_ptr().is_null() {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }
        let target_obj = unsafe { &*target_val.as_js_object_ptr() };
        if !target_obj.is_function() || target_obj.is_arrow() {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }
        if target_obj.native_fn().is_some() && target_obj.type_tag != JsObject::OBJ_TYPE_CONSTRUCTOR {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }
        // 按 target 自身记录的表代际解析：生成器/异步函数不是构造器。
        if matches!(self.callee_module(target_obj), Some(sub) if sub.is_generator || sub.is_async) {
            return self.raise_type_error("object is not a constructor").map(|_| true);
        }

        // 新对象原型取最内层 target 的 prototype（bound 包装自身无 prototype）。
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        if let Some(proto_val) = self.resolve_property(target_obj, proto_si) {
            if proto_val.is_object() {
                let new_obj_mut = unsafe { &mut *new_obj };
                let proto_obj_ptr = proto_val.as_js_object_ptr();
                let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
            }
        }
        let new_obj_val = JsValue::object(new_obj as *mut u8);

        if target_obj.native_fn().is_some() {
            // native 构造器：receiver = 新对象，值传递调用（错误原值恢复后展开）。
            // new.target 按 bound [[Construct]] 语义取最外层 bound 包装（wrapper_val），
            // 快照后置入 reg(255) 暴露给 native 构造器（调用后恢复原值）；构造
            // 形态标记同窗夹持（调用后恢复）。
            let saved_new_target = self.regs[255];
            let saved_constructing = self.constructing_native;
            self.regs[255] = wrapper_val;
            self.constructing_native = true;
            let result = self.call_function_sync(target_val, new_obj_val, &call_args);
            self.constructing_native = saved_constructing;
            self.regs[255] = saved_new_target;
            match result {
                Ok(v) => {
                    self.regs[rd] = if v.is_object() { v } else { new_obj_val };
                    Ok(false)
                }
                Err(_) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, "constructor call failed"));
                    self.exception_value = Some(exc);
                    self.pending_error_kind = Some(self.thrown_error_kind(exc));
                    self.unwind().map(|_| true)
                }
            }
        } else if target_obj.sub_module_index() > 0 {
            // 字节码构造器：this = 新对象（派生 target 为 undefined，super() 装配），
            // new.target = 最内层 target。
            let this_value = if target_obj.is_derived_constructor() {
                JsValue::undefined()
            } else {
                new_obj_val
            };
            self.push_bytecode_frame(
                target_val,
                this_value,
                FrameArgs::Slice(&call_args),
                Some(rd as u8),
                Some(new_obj_val),
                target_val,
                FrameContinuation::None,
                call_window,
            )?;
            Ok(true)
        } else {
            self.raise_type_error("object is not a constructor").map(|_| true)
        }
    }

    pub(crate) fn dispatch_template_str(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("TEMPLATE_STR rd={}", rd);
        let header = self.bytecode[self.pc];
        self.pc += 1;
        let segment_count = (header >> 16) as usize;
        let len_hint = (header & 0xFFFF) as usize;

        // 单元缓冲：字符串操作数按单元序列展开（lone surrogate 保真），
        // len_hint 为单元数口径（emit 侧同口径）。
        let mut units: Vec<u16> = Vec::with_capacity(len_hint.max(16));
        for _ in 0..segment_count {
            let seg = self.bytecode[self.pc];
            self.pc += 1;
            if (seg >> 31) == 1 {
                let reg = (seg & 0x7FFF_FFFF) as usize;
                let val = self.regs[reg];
                if val.is_string() {
                    // SAFETY: val 是字符串值；借用仅在本次展开内消费，不跨分配点。
                    let s = unsafe { &*val.as_string_ptr() };
                    units.extend_from_slice(s.units().as_ref());
                } else if val.is_object() || val.is_symbol() {
                    // 对象走完整 ToString（ToPrimitive 副作用顺序，结果保单元）；
                    // Symbol 抛 TypeError（to_units_full 无 symbol 直写分支）。
                    units.extend(to_units_full(val, self)?);
                } else {
                    // 原始值直写结果缓冲，免中间 String。
                    push_units_to(val, &mut units);
                }
            } else {
                let const_idx = (seg & 0x7FFF_FFFF) as usize;
                let imm = self.immutables();
                if const_idx < imm.len() {
                    let val = imm[const_idx];
                    if val.is_string() {
                        // SAFETY: val 是字符串值；借用仅在本次展开内消费，不跨分配点。
                        let s = unsafe { &*val.as_string_ptr() };
                        units.extend_from_slice(s.units().as_ref());
                    }
                }
            }
        }
        self.regs[rd] = self.new_string_units_owned(units);
        Ok(())
    }

    pub(crate) fn dispatch_instanceof(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("INSTANCEOF rd={}", rd);
        let lhs_val = self.regs[a];
        let rhs_val = self.regs[b];

        if !rhs_val.is_object() {
            return self.raise_type_error("INSTANCEOF right-hand side is not callable");
        }

        let has_instance_si = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_HAS_INSTANCE);

        let ctor_obj = unsafe { &*rhs_val.as_js_object_ptr() };
        let has_instance_val = self.ordinary_get(ctor_obj, has_instance_si, rhs_val)?;
        if !has_instance_val.is_undefined() && !has_instance_val.is_null() {
            let fn_ptr = has_instance_val.as_js_object_ptr();
            if !fn_ptr.is_null() {
                let fn_obj = unsafe { &*fn_ptr };
                if fn_obj.is_function() {
                    let result = self.call_function_sync(has_instance_val, rhs_val, &[lhs_val])?;
                    self.regs[rd] = JsValue::bool(to_boolean(result));
                    return Ok(());
                }
            }
        }

        if !lhs_val.is_object() {
            self.regs[rd] = JsValue::bool(false);
            return Ok(());
        }

        let rhs_obj = unsafe { &*rhs_val.as_js_object_ptr() };
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        let ctor_proto = self.resolve_property(rhs_obj, proto_si);

        let ctor_proto_ptr = match ctor_proto {
            Some(v) if v.is_object() => v.as_js_object_ptr(),
            _ => {
                self.regs[rd] = JsValue::bool(false);
                return Ok(());
            }
        };

        let mut proto = unsafe { &*lhs_val.as_js_object_ptr() }.proto();
        let mut depth = 0usize;
        loop {
            if !proto.is_object() {
                self.regs[rd] = JsValue::bool(false);
                break;
            }
            if depth >= MAX_PROTO_CHAIN_DEPTH {
                self.regs[rd] = JsValue::bool(false);
                break;
            }
            depth += 1;
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
        vm_trace!("IN rd={}", rd);
        let key_val = self.regs[a];
        let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            return self.raise_type_error("IN right-hand side is not an object");
        }
        let obj = unsafe { &*obj_ptr };
        let prop_name_si = self.property_key_si(key_val)?;
        // `in` 即 HasProperty：原型链各层都要看数组元素区与 TypedArray 整数索引，
        // 读值解析不覆盖这两处。
        let found = self.has_property(obj, prop_name_si);
        self.regs[rd] = JsValue::bool(found);
        Ok(())
    }

    pub(crate) fn dispatch_for_in_init(&mut self, a: usize) -> Result<(), String> {
        vm_trace!("FOR_IN_INIT r{}={:?}", a, self.regs[a]);
        let obj_val = self.regs[a];
        if obj_val.is_null() || obj_val.is_undefined() {
            // null/undefined 枚举不到任何键——是空 for-in，而非 TypeError。
            let keys_vec: bumpalo::collections::Vec<(JsValue, u32)> =
                bumpalo::collections::Vec::new_in(self.epoch.bump());
            let iter = self.epoch.alloc(ForInIter { keys: keys_vec, index: 0 });
            self.iters.push_for_in(iter.cast::<ForInIter<'static>>());
            return Ok(());
        }
        if !obj_val.is_object() {
            // 未支持：其它基本类型的 ToObject 强转尚未实现，在此之前抛 TypeError 是正确行为。
            return self.raise_type_error("for-in right-hand side is not an object");
        }

        let mut keys_vec: Vec<(JsValue, u32)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = obj_val;

        // 数组把整型下标元素存在 prop_vec，而 prop_vec 不属于 shape 链。
        // 单独枚举它们（ES：数组下标是可枚举字符串键，按升序排在其它自有键之前）。
        // hole（删除标记）跳过——for-in 不枚举数组稀疏空洞。
        if current.is_object() {
            let arr = unsafe { &*current.as_js_object_ptr() };
            if arr.is_array() {
                for i in 0..arr.array_prop_count {
                    if arr.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                        continue;
                    }
                    let is_enum = arr
                        .prop_meta_at(i)
                        .map(|m| m.attributes.enumerable())
                        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                    if is_enum {
                        // 整数下标编码为整数键（免 intern + 防永久泄漏）；枚举值
                        // 物化为数字字符串，排序时由 array_index_from_property_key
                        // 直接反解。
                        let idx = make_int_key(i);
                        keys_vec.push((self.new_string(&i.to_string()), idx));
                    }
                }
            }
            // TA 元素键：整数下标 0..live 长升序排前（尾稳定排序负责序）；
            // 越界（含 detach）live 长 0 零枚；元素住 buffer，不在形状链。
            if arr.is_typed_array_obj() {
                let len = oxide_builtins::typed_array::ta_view_length(self, arr) as u32;
                for i in 0..len.min(INT_KEY_COUNT) {
                    keys_vec.push((self.new_string(&i.to_string()), make_int_key(i)));
                }
            }
        }

        let mut depth = 0usize;

        loop {
            if !current.is_object() {
                break;
            }
            if depth >= MAX_PROTO_CHAIN_DEPTH {
                break;
            }
            depth += 1;
            let cur = unsafe { &*current.as_js_object_ptr() };
            let obj_start = keys_vec.len();
            let mut cursor = Some(cur.shape_id());
            while let Some(id) = cursor {
                if id == oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
                    break;
                }
                if let Some(shape) = self.kernel_core.shape_forge().get_shape(id) {
                    // Symbol 键/私有名键不参与 for-in 枚举。
                    if shape.property_name != u32::MAX
                        && !is_symbol_key(shape.property_name)
                        && !is_private_name_key(shape.property_name)
                        && seen.insert(shape.property_name)
                    {
                        let enumerable = self
                            .kernel_core
                            .shape_forge()
                            .lookup_position(cur.shape_id(), shape.property_name)
                            .and_then(|pos| cur.prop_meta_at(pos))
                            .map(|meta| meta.attributes.enumerable())
                            .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
                        if enumerable {
                            // 整数键不在 interner：物化为数字字符串；其余键取永久串。
                            let key_val = if is_int_key(shape.property_name) {
                                self.new_string(&int_key_value(shape.property_name).to_string())
                            } else {
                                JsValue::perm_string(self.kernel_core.perm_interner().string_ptr(shape.property_name))
                            };
                            keys_vec.push((key_val, shape.property_name));
                        }
                    }
                    cursor = shape.parent;
                } else {
                    break;
                }
            }
            // shape 链按叶→根遍历（与插入序相反）；在接上原型前把本对象的切片
            // 翻转为插入序。
            keys_vec[obj_start..].reverse();
            current = cur.proto();
        }

        // 模块命名空间 exotic：EnumerateObjectProperties 构建键表时逐键走
        // `? [[GetOwnProperty]]`，未初始化导出抛 ReferenceError。校验须在排序与
        // 迭代器建立之前（INIT 期），不遗留 for-in 迭代器；非 live ns 无条目表，
        // `module_ns_export` 返回 None，落普通路径零行为变化。
        if obj_val.is_object() {
            let ns_obj = unsafe { &*obj_val.as_js_object_ptr() };
            if ns_obj.is_module_namespace() {
                for (_key_val, si) in &keys_vec {
                    if is_symbol_key(*si) {
                        continue;
                    }
                    if let Some(oxide_builtins::module::ModuleNsQuery::Uninitialized) =
                        oxide_builtins::module::module_ns_export(ns_obj, *si)
                    {
                        return self
                            .raise_error_kind("ReferenceError", oxide_builtins::module::NS_UNINITIALIZED_MESSAGE);
                    }
                }
            }
        }

        // ES 枚举序：整型下标键升序，其余按插入序。稳定排序保持非下标键间的插入序。
        keys_vec.sort_by(|(_, a_si), (_, b_si)| {
            match (self.array_index_from_property_key(*a_si), self.array_index_from_property_key(*b_si)) {
                (Some(ai), Some(bi)) => ai.cmp(&bi),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        // std Vec 建完后迁入 bump 区，避免枚举循环期间对 self 的 &mut 借用
        // 与 bump 借用冲突。
        let keys_bump: bumpalo::collections::Vec<(JsValue, u32)> =
            bumpalo::collections::Vec::from_iter_in(keys_vec, self.epoch.bump());
        let iter = self.epoch.alloc(ForInIter { keys: keys_bump, index: 0 });
        self.iters.push_for_in(iter.cast::<ForInIter<'static>>());
        Ok(())
    }

    pub(crate) fn dispatch_for_in_next(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_IN_NEXT rd={}", rd);
        let iter_ptr = self.iters.last_for_in();
        if iter_ptr.is_null() {
            return Err("FOR_IN_NEXT without active iterator".into());
        }
        let iter = unsafe { &mut *iter_ptr };
        if iter.index < iter.keys.len() {
            self.regs[rd] = iter.keys[iter.index].0;
            iter.index += 1;
        } else {
            self.regs[rd] = JsValue::undefined();
        }
        Ok(())
    }

    pub(crate) fn dispatch_for_in_done(&mut self, rd: usize) {
        vm_trace!("FOR_IN_DONE rd={}", rd);
        let iter_ptr = self.iters.last_for_in();
        if iter_ptr.is_null() {
            self.regs[rd] = JsValue::bool(true);
        } else {
            let iter = unsafe { &*iter_ptr };
            self.regs[rd] = JsValue::bool(iter.index >= iter.keys.len());
        }
    }

    pub(crate) fn dispatch_for_in_cleanup(&mut self) {
        vm_trace!("FOR_IN_CLEANUP");
        self.iters.pop_for_in();
    }

    pub(crate) fn dispatch_for_of_init(&mut self, a: usize) -> Result<(), String> {
        vm_trace!("FOR_OF_INIT r{}={:?}", a, self.regs[a]);
        let iterable = self.regs[a];
        match oxide_builtins::iterator::make_iterator_for_value(self, iterable) {
            Ok(iterator) => {
                self.iters.push_for_of(iterator, false);
                Ok(())
            }
            Err(err) => {
                self.exception_value = Some(err);
                self.pending_error_kind = Some(self.thrown_error_kind(err));
                self.unwind()
            }
        }
    }

    /// for-await-of 初始化：按 GetAsyncIterator 协议取异步迭代器（缺失 `@@asyncIterator`
    /// 时回退同步迭代器并包 AsyncFromSyncIterator），压入 for-of 迭代器栈。
    pub(crate) fn dispatch_for_await_of_init(&mut self, a: usize) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_INIT r{}={:?}", a, self.regs[a]);
        let iterable = self.regs[a];
        match crate::async_from_sync::make_async_iterator(self, iterable) {
            Ok(iterator) => {
                self.iters.push_for_of(iterator, true);
                Ok(())
            }
            Err(err) => {
                self.exception_value = Some(err);
                self.pending_error_kind = Some(self.thrown_error_kind(err));
                self.unwind()
            }
        }
    }

    /// for-await-of 步进：调用迭代器 `next()`，把返回的 promise 写入 rd。
    /// 随后的 `AWAIT` 负责挂起等待；next() 抛出时弹出迭代器并透传（与同步 for-of 一致）。
    pub(crate) fn dispatch_for_await_of_next(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_NEXT rd={}", rd);
        self.last_uncaught_value = None;
        let Some(iterator) = self.iters.last_for_of() else {
            return Err("FOR_AWAIT_OF_NEXT without active iterator".into());
        };
        if !iterator.is_object() {
            return Err("FOR_AWAIT_OF_NEXT iterator is not an object".into());
        }
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        let result = match self.call_function_sync(next_fn, iterator, &[]) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        self.regs[rd] = result;
        Ok(())
    }

    /// for-await-of 步进完成检查：读取 `AWAIT` 恢复值（a 槽，即迭代器结果对象）的
    /// `done`，写入本迭代器条目结果并把 `!done` 写 rd（供 JMP_IF_FALSE 分支）。
    pub(crate) fn dispatch_for_await_of_done(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_DONE rd={} r{}={:?}", rd, a, self.regs[a]);
        let result = self.regs[a];
        if !result.is_object() {
            return self.raise_type_error("iterator result is not an object");
        }
        self.iters.set_last_result(result);
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let done_val = match self.ordinary_get(result_obj, done_si, result) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        self.regs[rd] = JsValue::bool(!to_boolean(done_val));
        Ok(())
    }

    /// for-await-of 收尾：迭代器自然 done 时跳过；否则执行异步 IteratorClose——
    /// 调用 `return()`，其返回的 promise 经 await 挂起，恢复后继续循环后的指令。
    pub(crate) fn dispatch_for_await_of_close(&mut self) -> Result<(), String> {
        vm_trace!("FOR_AWAIT_OF_CLOSE");
        let Some(entry) = self.iters.pop_for_of() else {
            return Ok(());
        };
        let iterator = entry.iterator;
        let result = entry.last_result;
        if result.is_object() {
            let result_obj = unsafe { &*result.as_js_object_ptr() };
            let done_si = self.kernel_core.perm_interner().intern("done").0;
            if let Ok(done_val) = self.ordinary_get(result_obj, done_si, result) {
                if to_boolean(done_val) {
                    return Ok(());
                }
            }
        }
        if !iterator.is_object() {
            return Ok(());
        }
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let return_si = self.kernel_core.perm_interner().intern("return").0;
        let return_fn = self.ordinary_get(iter_obj, return_si, iterator)?;
        if !return_fn.is_object() {
            return Ok(());
        }
        if !unsafe { &*return_fn.as_js_object_ptr() }.is_function() {
            return Ok(());
        }
        let inner = self.call_function_sync(return_fn, iterator, &[])?;
        if !inner.is_object() {
            return self.raise_type_error("iterator return() result is not an object");
        }
        self.perform_await(inner)
    }

    pub(crate) fn dispatch_for_of_done(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_OF_DONE rd={}", rd);
        let Some(iterator) = self.iters.last_for_of() else {
            return Err("FOR_OF_DONE without active iterator".into());
        };
        if !iterator.is_object() {
            return Err("FOR_OF_DONE iterator is not an object".into());
        }

        // 调用 next()/读 done 前清空异常值槽：跨调用残留不得污染本指令的取槽。
        self.last_uncaught_value = None;
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
            Ok(v) => v,
            // 错误路径只经 throw_for_of_error 传播，不写 rd（rd 保持循环决策位，
            // 异常展开后指令流离开循环，残留值无效）。
            Err(e) => return self.throw_for_of_error(e),
        };
        let result = match self.call_function_sync(next_fn, iterator, &[]) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        if !result.is_object() {
            return self.raise_type_error("iterator result is not an object");
        }
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let done_val = match self.ordinary_get(result_obj, done_si, result) {
            Ok(v) => v,
            Err(e) => return self.throw_for_of_error(e),
        };
        let done = to_boolean(done_val);
        self.iters.set_last_result(result);
        self.regs[rd] = JsValue::bool(!done);
        Ok(())
    }

    pub(crate) fn dispatch_for_of_next(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("FOR_OF_NEXT rd={}", rd);
        self.last_uncaught_value = None;
        let result = self.iters.last_result();
        if !result.is_object() {
            self.regs[rd] = JsValue::undefined();
            return Ok(());
        }
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let value_si = self.kernel_core.perm_interner().intern("value").0;
        self.regs[rd] = match self.ordinary_get(result_obj, value_si, result) {
            Ok(v) => v,
            // 错误路径只经 throw_for_of_error 传播，不写 rd（同上）。
            Err(e) => return self.throw_for_of_error(e),
        };
        Ok(())
    }

    /// next()/value 访问抛出：经 unwind 走异常展开，使外围 try/catch 能捕获，并尽可能
    /// 重新抛出原始值。按 ECMA-262，next() 抛出时不会经 return() 关闭迭代器——
    /// 先弹出它，使展开时的 IteratorClose 遍历跳过该迭代器。
    fn throw_for_of_error(&mut self, msg: String) -> Result<(), String> {
        self.iters.pop_for_of();
        let exc = match self.last_uncaught_value.take() {
            Some(v) => v,
            None => oxide_builtins::error::create_from_text(self, &msg),
        };
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(self.thrown_error_kind(exc));
        // 无论取到原值还是重建错误对象，异常必须展开传播，不得静默返回 Ok——
        // 否则 for-of 循环继续推进形成不终止。
        self.unwind()
    }

    pub(crate) fn dispatch_for_of_close(&mut self) -> Result<(), String> {
        vm_trace!("FOR_OF_CLOSE");
        let Some(entry) = self.iters.pop_for_of() else {
            return Ok(());
        };
        let iterator = entry.iterator;
        let result = entry.last_result;
        // 迭代已自然结束（最后一次 next 返回 done:true）时不调 return()；
        // 仅当元素耗尽但迭代器未 done（提前退出）才执行 IteratorClose。
        if result.is_object() {
            let result_obj = unsafe { &*result.as_js_object_ptr() };
            let done_si = self.kernel_core.perm_interner().intern("done").0;
            if let Ok(done_val) = self.ordinary_get(result_obj, done_si, result) {
                if to_boolean(done_val) {
                    return Ok(());
                }
            }
        }
        // 正常 / break / return 退出：此前无进行中的突然完成，return() 自身的抛出直接传播。
        self.close_for_of_iterator(iterator, false)
    }

    /// 调用 `iterator.return()`（IteratorClose）。
    ///
    /// # 边界与前提
    /// - `suppress_return_error` 为 true 时表示正在展开某个外围突然完成：return() 的
    ///   自身结果被丢弃，并保留在途异常跨调用存活。
    /// - 为 false 时（正常 for-of 退出）return() 的错误向外传播。
    pub(crate) fn close_for_of_iterator(
        &mut self, iterator: JsValue, suppress_return_error: bool,
    ) -> Result<(), String> {
        if !iterator.is_object() {
            return Ok(());
        }
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let return_si = self.kernel_core.perm_interner().intern("return").0;
        let return_fn = match self.ordinary_get(iter_obj, return_si, iterator) {
            Ok(v) => v,
            Err(e) => {
                if suppress_return_error {
                    return Ok(());
                }
                return Err(e);
            }
        };
        if return_fn.is_object() {
            let return_obj = unsafe { &*return_fn.as_js_object_ptr() };
            if return_obj.is_function() {
                if suppress_return_error {
                    let saved_exc = self.exception_value;
                    let saved_kind = self.pending_error_kind;
                    // 忽略调用期间的原始异常值一并暂存：return() 自身抛错按契约被在途
                    // 异常替代，其值不得外泄进槽供后续 take 误取。
                    let saved_uncaught = self.last_uncaught_value.take();
                    let _ = self.call_function_sync(return_fn, iterator, &[]);
                    self.exception_value = saved_exc;
                    self.pending_error_kind = saved_kind;
                    self.last_uncaught_value = saved_uncaught;
                } else {
                    let inner = match self.call_function_sync(return_fn, iterator, &[]) {
                        Ok(v) => v,
                        // return() 抛出：恢复原始异常值并展开，使外围 try/catch 可捕获。
                        Err(e) => return self.raise_call_error(&e).map(|_| ()),
                    };
                    // IteratorClose 要求 return() 返回值是对象，否则抛 TypeError。
                    if !inner.is_object() {
                        self.raise_type_error("iterator return() result is not an object")?;
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// 对 `depth` 以上的所有活跃 for-of 迭代器执行 IteratorClose，供 `unwind()` 关闭
    /// 被异常中断的循环。先弹出再关闭，防止重入调用重复关闭，同时跳过 next()-throw
    /// 路径（它已弹出自己的迭代器）。
    pub(crate) fn close_for_of_above(&mut self, depth: usize) {
        while self.iters.for_of_iters.len() > depth {
            let Some(entry) = self.iters.pop_for_of() else {
                break;
            };
            let _ = self.close_for_of_iterator(entry.iterator, true);
        }
    }

    pub(crate) fn dispatch_rest_object(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("REST_OBJECT rd={}", rd);
        // ext 字（excluded 常量下标）在任何路径都先消费，防止 ToObject 早返回后
        // pc 错位把 ext 字当下一条指令解码（曾误读成 UNSPILL slot）。
        let excluded_idx = self.bytecode[self.pc] as usize;
        self.pc += 1;
        let src = self.regs[a];
        if !src.is_object() {
            // ToObject：null/undefined 抛；number/boolean/symbol 无自有属性 → 空对象；
            // string 的索引字符（UTF-16 code unit）是可枚举自有属性。
            if src.is_null() || src.is_undefined() {
                return self.raise_type_error("Cannot destructure property of null/undefined");
            }
            let proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
            let rest_ptr = self.alloc_object(JsObject::new_empty(
                oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
                JsValue::from_js_object(proto_ptr),
            ));
            if src.is_string() {
                // SAFETY: src 是字符串值；单元序列展开（lone surrogate 保真）。
                let code_units: Vec<u16> = unsafe { (*src.as_string_ptr()).units().into_owned() };
                for (i, unit) in code_units.iter().enumerate() {
                    let si = make_int_key(i as u32);
                    let ch_val = self.unit_char_value(*unit);
                    let rest = unsafe { &mut *rest_ptr };
                    self.set_or_create_prop_value(rest, si, ch_val);
                }
            }
            self.regs[rd] = JsValue::from_js_object(rest_ptr);
            return Ok(());
        }
        // 排除名单常量按单元序列展开（lone surrogate 名保真），0x0000 分隔。
        let excluded_const = self
            .immutables()
            .get(excluded_idx)
            .and_then(|v| {
                if v.is_string() {
                    // SAFETY: v 是字符串常量值。
                    Some(unsafe { (*v.as_string_ptr()).units().into_owned() })
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let mut excluded: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut seg: Vec<u16> = Vec::new();
        for &u in excluded_const.iter() {
            if u == 0x0000 {
                if !seg.is_empty() {
                    excluded.insert(self.string_key_units(&seg));
                    seg.clear();
                }
                continue;
            }
            seg.push(u);
        }
        if !seg.is_empty() {
            excluded.insert(self.string_key_units(&seg));
        }
        // 运行时 excluded：b 槽数组（computed key 求值结果）的元素 ToPropertyKey 后排除。
        if b != 0 {
            let arr_val = self.regs[b];
            if arr_val.is_object() {
                let arr_obj = unsafe { &*arr_val.as_js_object_ptr() };
                if arr_obj.is_array() {
                    for i in 0..arr_obj.array_prop_count {
                        let v = arr_obj.get_prop_at(i);
                        let si = self.property_key_si(v)?;
                        excluded.insert(si);
                    }
                }
            }
        }

        let src_obj = unsafe { &*src.as_js_object_ptr() };

        // 收集-提交：先取齐 (键, 值)，取值阶段触发 getter（可能抛异常），再统一写
        // 目标对象，避免提交写与取值互相交错。只复制可枚举自有属性（CopyDataProperties）。
        // 字符串包装对象的索引字符是构造期物化的可枚举自有属性，走同一 walk 路径。
        let mut assignments: Vec<(u32, JsValue)> = Vec::new();

        // 自身属性：walk_own_keys 已合并数组元素区（hole 跳过）并返回绝对存储索引，
        // 仅可枚举，跳过 pattern 已绑定的键。
        let keys = oxide_builtins::object::walk_own_keys(self, src_obj);
        for (si, pos) in keys {
            let enumerable = src_obj
                .prop_meta_at(pos)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
            if !enumerable {
                continue;
            }
            if excluded.contains(&si) {
                continue;
            }
            let val = match self.ordinary_get(src_obj, si, src) {
                Ok(v) => v,
                Err(e) => return self.raise_call_error(&e).map(|_| ()),
            };
            assignments.push((si, val));
        }

        // 提交到 rest 对象。
        let proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let rest_ptr = self.alloc_object(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ));
        let rest = unsafe { &mut *rest_ptr };
        for (si, val) in assignments {
            let promoted = self.promote_if_needed_for_write_ptr(rest_ptr, val);
            self.set_or_create_prop_value(rest, si, promoted);
        }
        self.regs[rd] = JsValue::from_js_object(rest_ptr);
        Ok(())
    }

    /// SPREAD_OBJECT：对象字面量 `...` 展开，把源的可枚举自有属性写入目标对象（原地改）。
    ///
    /// 语义 = CopyDataProperties 的"非 null/undefined 源"分支：`{...null}` / `{...undefined}`
    /// 合法（产出空展开），与 REST_OBJECT 对 null/undefined 抛 TypeError 不同。
    ///
    /// # 步骤
    /// 1. null/undefined 源直接返回（空展开）
    /// 2. 字符串源按 UTF-16 code unit 下标复制为可枚举索引属性
    /// 3. 对象源先收集可枚举自有属性（数组元素区 + shape 链），取值经 ordinary_get
    ///    触发访问器 getter，再统一写入目标
    ///
    /// # 边界与前提
    /// - 其它原始值（number/boolean/symbol）无自有可枚举属性 → 空展开
    /// - 目标已有同名属性被覆盖（后定义/后展开者胜）
    ///
    /// # 副作用
    /// - 原地修改 rd 指向的对象；取值可能触发 getter 并抛异常（异常传播）
    pub(crate) fn dispatch_spread_object(&mut self, rd: usize, a: usize) -> Result<(), String> {
        vm_trace!("SPREAD_OBJECT rd={}", rd);
        let target_val = self.regs[rd];
        let src = self.regs[a];

        // null/undefined 源合法：展开为空，直接返回。
        if src.is_null() || src.is_undefined() {
            return Ok(());
        }

        // 字符串源：按单元下标复制（可枚举索引属性，lone surrogate 保真）。
        if src.is_string() {
            let target = unsafe { &mut *target_val.as_js_object_ptr() };
            // SAFETY: src 是字符串值。
            let code_units: Vec<u16> = unsafe { (*src.as_string_ptr()).units().into_owned() };
            for (i, unit) in code_units.iter().enumerate() {
                let si = make_int_key(i as u32);
                let ch_val = self.unit_char_value(*unit);
                self.set_or_create_prop_value(target, si, ch_val);
            }
            return Ok(());
        }

        // 其它原始值无自有可枚举属性 → 空展开。
        if !src.is_object() {
            return Ok(());
        }

        let src_obj = unsafe { &*src.as_js_object_ptr() };

        // 收集-提交：先取齐 (键, 值)，取值阶段触发 getter（可能抛异常），
        // 再统一写目标，避免提交写与取值互相交错。
        let mut assignments: Vec<(u32, JsValue)> = Vec::new();

        // 自身属性：walk_own_keys 已合并数组元素区（hole 跳过）并返回绝对存储索引，
        // 仅可枚举；取值经 ordinary_get 触发访问器 getter。
        let keys = oxide_builtins::object::walk_own_keys(self, src_obj);
        for (si, pos) in keys {
            let enumerable = src_obj
                .prop_meta_at(pos)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
            if !enumerable {
                continue;
            }
            let val = self.ordinary_get(src_obj, si, src)?;
            assignments.push((si, val));
        }

        // 提交：覆盖目标已有同名属性（从左到右求值，后展开者胜）。
        let target = unsafe { &mut *target_val.as_js_object_ptr() };
        for (si, val) in assignments {
            let promoted = self.promote_if_needed_for_write_ptr(target, val);
            self.set_or_create_prop_value(target, si, promoted);
        }
        Ok(())
    }

    /// GET_TEMPLATE_OBJECT：GetTemplateObject 语义——按 (模块 flat_id, site 序号)
    /// 查缓存，未命中则构建模板对象（cooked/raw 数组 + raw 属性）并冻结。
    ///
    /// # ext 布局（紧跟指令后的扩展字）
    /// - `n`：quasis 段数；
    /// - 每段两个字：cooked 字（高位 `0x8000_0000` 标记非法转义 → undefined，
    ///   低 31 位为常量池下标）、raw 字（常量池下标）；
    /// - 末尾：site 序号。
    ///
    /// # 副作用
    /// - 首次构建时分配 cooked/raw 数组并写入 `template_objects` 缓存（GC 根）；
    ///   缓存命中时零分配。
    pub(crate) fn dispatch_get_template_object(&mut self, rd: usize) -> Result<(), String> {
        vm_trace!("GET_TEMPLATE_OBJECT rd={}", rd);
        let n = self.bytecode[self.pc] as usize;
        self.pc += 1;
        let mut cooked_words = Vec::with_capacity(n);
        let mut raw_idxs = Vec::with_capacity(n);
        for _ in 0..n {
            cooked_words.push(self.bytecode[self.pc]);
            self.pc += 1;
            raw_idxs.push(self.bytecode[self.pc]);
            self.pc += 1;
        }
        let site_no = self.bytecode[self.pc];
        self.pc += 1;

        // 键含表代际：跨 run 调用的旧代模块与当前 run 模块 flat_id 重编号，
        // 无代际维度会误命中他代同 (flat_id, site) 的模板对象。
        let key = (self.active_table_gen, self.active_flat_id, site_no);
        if let Some(&cached) = self.template_objects.get(&key) {
            self.regs[rd] = cached;
            return Ok(());
        }

        let proto_ptr = self.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let proto_val = JsValue::from_js_object(proto_ptr);
        // 模板对象直接分配为 session 对象：跨 epoch 写入（存进 global/数组）时
        // 免 promote 搬移——若按 epoch 分配，首次写入 session 根会把对象搬到新
        // 地址，缓存中的旧指针与新实例分叉，同一 site 两次取值将返回不同对象。
        let mut alloc_session_array = |n: usize| {
            let mut clone =
                JsObject::new_array(oxide_kernel::shape_forge::EMPTY_SHAPE_ID, proto_val, n, self.epoch.bump());
            // 标记 session 归属：session_epoch 分配的对象须显式置位，GC/释放路径
            // 据 SESSION_EPOCH_BIT 判定归属（与 promote_object 的 clone 路径一致）。
            clone.set_session_epoch(true);
            let ptr = self.gc_state.session_epoch.alloc(clone) as *mut JsObject;
            self.gc_state.session_object_ptrs.push(ptr);
            // 直 session 分配计入堆账目（与 promote 同式：对象头 + 对象堆数据）。
            self.gc_state.session_bytes_allocated += std::mem::size_of::<JsObject>()
                + crate::session_gc::SessionGc::object_heap_data_bytes(unsafe { &*ptr }) as usize;
            ptr
        };
        let cooked = alloc_session_array(n);
        let raw = alloc_session_array(n);
        let cooked_val = JsValue::from_js_object(cooked);
        let raw_val = JsValue::from_js_object(raw);

        // 元素：可枚举、不可写、不可配置（模板对象属性恒只读，禁止改写元素）。
        let elem_attrs = PropAttributes::new(false, true, false);
        for i in 0..n {
            let (c_val, r_val) = {
                // immutables 借用限于本块：promote 需 &mut self，先取值再放行借用。
                let imm = self.immutables();
                let c_val = if cooked_words[i] & 0x8000_0000 != 0 {
                    JsValue::undefined()
                } else {
                    imm.get(cooked_words[i] as usize).copied().unwrap_or(JsValue::undefined())
                };
                let r_val = imm.get(raw_idxs[i] as usize).copied().unwrap_or(JsValue::undefined());
                (c_val, r_val)
            };
            let c_promoted = self.promote_if_needed_for_write_ptr(cooked, c_val);
            let r_promoted = self.promote_if_needed_for_write_ptr(raw, r_val);
            // SAFETY: cooked/raw 为本函数刚分配的 epoch 对象，借用仅在本循环内消费。
            unsafe {
                (*cooked).set_prop_at(i, c_promoted);
                (*cooked).set_data_meta(i, elem_attrs);
                (*raw).set_prop_at(i, r_promoted);
                (*raw).set_data_meta(i, elem_attrs);
            }
        }

        // raw 属性：不可枚举、不可写、不可配置（规范 desc 无 writable/enumerable
        // 键，DefinePropertyOrThrow 默认 false）。先于冻结定义（冻结后不可扩展，
        // 新增属性会被拒）。
        let raw_si = self.kernel_core.perm_interner().intern("raw").0;
        // SAFETY: cooked 为本函数分配的 epoch 对象，借用仅在本次 define 内消费。
        self.define_data_property(unsafe { &mut *cooked }, raw_si, raw_val, PropAttributes::new(false, false, false))?;

        // 冻结两个数组：不可扩展 + length writable=false（is_frozen 使 length
        // 赋值与元素写入经 ordinary_set 拒绝）；length {e:false,w:false,c:false}
        // 由 getOwnPropertyDescriptor 的数组 length 特判表达。
        // SAFETY: cooked/raw 为本函数分配的 session 对象。
        unsafe {
            (*cooked).set_frozen(true);
            (*cooked).set_extensible(false);
            (*raw).set_frozen(true);
            (*raw).set_extensible(false);
        }
        self.template_objects.insert(key, cooked_val);
        self.regs[rd] = cooked_val;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use oxide_bytecode::module::CompiledModule;
    use oxide_bytecode::opcode;

    use crate::vm_state::ForOfEntry;

    #[test]
    fn for_of_close_pops_iterator_stack() {
        let module = CompiledModule {
            bytecode: Arc::from(vec![
                opcode::encode(opcode::OpCode::FOR_OF_CLOSE, 0, 0, 0),
                opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
            ]),
            n_registers: 1,
            ..CompiledModule::new()
        };
        let mut vm = Vm::new();
        vm.iters.for_of_iters.push(ForOfEntry {
            iterator: JsValue::undefined(),
            last_result: JsValue::undefined(),
            is_async: false,
        });

        vm.run(&Arc::new(module))
            .expect("FOR_OF_CLOSE should tolerate non-object sentinel");

        assert!(vm.iters.for_of_iters.is_empty());
    }

    #[test]
    fn dispatch_new_expression_param_overlap_reads_spill_first() {
        // NEW 收敛路径同款重叠几何（实参源 regs[1..3) 与 callee 形参写入区 regs[2..4)
        // 重叠，first < param_base）：经 NEW_EXPRESSION 入口压帧，形参与 spill 实参区
        // （arguments 对象源）必须取原始实参值，不得先写后读串值。
        let mut vm = Vm::new();
        // sub_modules[1] = 构造器：2 个形参，param_base=2（与调用方实参槽 2 重叠）
        let mut ctor_mod = CompiledModule::new();
        ctor_mod.n_args = 2;
        ctor_mod.param_base = 2;
        ctor_mod.n_registers = 5;
        ctor_mod.bytecode = Arc::from(vec![opcode::encode(opcode::OpCode::RETURN, 0, 0, 0)]);
        vm.install_module_table_for_test(Arc::new(vec![Arc::new(CompiledModule::new()), Arc::new(ctor_mod)]));
        vm.active_reg_limit = 8;
        // NEW_EXPRESSION 指令：ext 低 8 位 = 实参个数 2，高 8 位 = 窗口 0（全量）
        vm.bytecode = Arc::from(vec![opcode::encode(opcode::OpCode::NEW_EXPRESSION, 0, 0, 0), 2]);
        vm.pc = 0;
        // 调用方实参区 regs[1..3)：arg0=10, arg1=20；regs[2] 同时是 callee 形参槽（param_base=2）
        vm.regs[1] = JsValue::int(10);
        vm.regs[2] = JsValue::int(20);
        vm.regs[5] = vm.create_function_object(1, vm.current_gen, false, false, false, false);

        vm.dispatch_new_expression(0, 5, 1).expect("NEW 压帧成功");

        let frame = vm.frames.last().expect("压帧后应有帧");
        // 形参从 spill 实参区取源：regs[2]=arg0=10，regs[3]=arg1=20（不得被先写覆盖）
        assert_eq!(vm.regs[2], JsValue::int(10), "形参 a 应为实参 arg0");
        assert_eq!(vm.regs[3], JsValue::int(20), "形参 b 应为实参 arg1（不受先写覆盖）");
        // spill 实参区（arguments 对象源）保持原实参值
        let base = frame.arguments_base as usize;
        assert_eq!(vm.spill_stack[base], JsValue::int(10), "spill 实参区 arg0");
        assert_eq!(vm.spill_stack[base + 1], JsValue::int(20), "spill 实参区 arg1");
        // NEW 帧契约：构造结果寄存器与 constructed_this 随帧携带
        assert_eq!(frame.construct_result_reg, Some(0), "构造结果写回 regs[0]");
        assert!(
            frame.constructed_this.is_some_and(|v| v.is_object()),
            "基类构造路径 constructed_this 应为新对象"
        );
        assert!(!frame.is_derived_constructor, "普通函数非 derived 构造器");
    }
}
