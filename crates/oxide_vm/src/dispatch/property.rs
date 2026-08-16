use crate::{ic_debug, ic_trace, vm_trace};
use oxide_bytecode::opcode::{OpCode, IC_EXT_WORDS};
use oxide_runtime_api as coercion;
use oxide_types::object::JsObject;
use oxide_types::private_key::{int_key_value, is_int_key, make_int_key, make_private_name_id};
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
            OpCode::SET_PROP_BATCH => self.dispatch_set_prop_batch(rd, a, b),
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
            let length_si = self.length_si;
            if prop_name_si == length_si {
                // SAFETY: val 是字符串值。utf16_len 构造时缓存，O(1) 读取。
                let len = unsafe { (*val.as_string_ptr()).utf16_len() };
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
        if val.is_bigint() {
            let proto_ptr = self.session.builtin_world().bigint_proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &*proto_ptr };
            return self.ordinary_get(proto, prop_name_si, val).map(Some);
        }
        if val.is_symbol() {
            let proto_ptr = self.session.builtin_world().symbol_proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &*proto_ptr };
            return self.ordinary_get(proto, prop_name_si, val).map(Some);
        }
        Ok(None)
    }

    fn private_slot(&self, obj: &JsObject, private_key: u32) -> Option<u32> {
        self.kernel_core.shape_forge().lookup_position(obj.shape_id(), private_key)
    }

    /// 沿实例→proto 链查找私有方法/访问器槽（存于类的 home=proto）。字段槽不跨原型
    /// （PrivateFieldFind 只查 own）——链上遇到字段槽（无 hole meta）即拒绝，防
    /// `Object.create(instance)` 穿透读取实例字段。
    fn find_private_method_slot(&self, obj: &JsObject, private_key: u32) -> Option<*mut JsObject> {
        let mut proto = obj.proto();
        let mut depth = 0usize;
        while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
            depth += 1;
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self.private_slot(proto_obj, private_key) {
                // 方法槽有 hole 标记（set_private_method_meta）；字段槽（存实例）沿链出现
                // 说明接收者并非本类实例 → 拒绝穿透。
                if proto_obj.prop_meta_at(pos).is_some_and(|m| m.is_hole()) {
                    return Some(proto_obj as *const JsObject as *mut JsObject);
                }
                return None;
            }
            proto = proto_obj.proto();
        }
        None
    }

    /// 私有方法/访问器/静态字段访问的 brand 检查：接收者 own 的 brand 槽（构造时写入
    /// 的类 brand 对象）必须与当前类 brand 对象同一。类每次求值创建新 brand，同字节码
    /// 多次实例化（eval/factory）可借此区分。
    fn private_brand_check(&mut self, obj: &JsObject, brand_reg: usize, brand_key: u32) -> Result<(), String> {
        let Some(pos) = self.private_slot(obj, brand_key) else {
            return self.raise_type_error("private field brand check failed");
        };
        let inst_brand = obj.get_prop_at(pos);
        if !coercion::strict_equality(inst_brand, self.regs[brand_reg]) {
            return self.raise_type_error("private field brand check failed");
        }
        Ok(())
    }

    fn dispatch_get_private(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GET_PRIVATE rd={} a={} b={}", rd, a, b);
        let obj_val = self.regs[a];
        let Some(obj_ptr) = self.checked_object_ptr(obj_val, "private field access on non-object")? else {
            return Ok(());
        };
        let private_key = self.private_key_from_reg(b);
        let brand_reg = self.bytecode[self.pc] as usize;
        let brand_key = make_private_name_id(self.bytecode[self.pc + 1]);
        self.pc += 2;
        let obj = unsafe { &*obj_ptr };
        // 字段路径：own 有该私有槽（字段存实例 own）→ 直接读，不跨原型、不做 brand 值比较
        // （PrivateFieldFind 只查 own；接收者带槽即构造器产物）。
        if let Some(pos) = self.private_slot(obj, private_key) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    if meta.get.is_undefined() {
                        return self.raise_type_error("private field has no getter");
                    }
                    let getter = meta.get;
                    self.push_bytecode_getter_frame(getter, obj_val, rd as u8)?;
                    self.accessor_frame_target_reg.take();
                    return Ok(());
                }
            }
            self.regs[rd] = obj.get_prop_at(pos);
            return Ok(());
        }
        // 方法路径：own 无槽（私有方法/访问器存于 home=proto）→ 沿链找方法槽 + brand 检查。
        let Some(home_ptr) = self.find_private_method_slot(obj, private_key) else {
            return self.raise_type_error("private field brand check failed");
        };
        if brand_reg != 0 {
            self.private_brand_check(obj, brand_reg, brand_key)?;
        }
        let home = unsafe { &*home_ptr };
        let pos = self.private_slot(home, private_key).expect("slot just found");
        if let Some(meta) = home.prop_meta_at(pos) {
            if meta.is_accessor {
                if meta.get.is_undefined() {
                    return self.raise_type_error("private field has no getter");
                }
                let getter = meta.get;
                self.push_bytecode_getter_frame(getter, obj_val, rd as u8)?;
                // 与 dispatch_get_prop 一致：getter 帧入栈后消费 accessor_frame_target_reg，
                // 避免残留状态污染后续属性访问（getter 返回经 continuation 写 rd）。
                self.accessor_frame_target_reg.take();
                return Ok(());
            }
        }
        self.regs[rd] = home.get_prop_at(pos);
        Ok(())
    }

    fn dispatch_set_private(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SET_PRIVATE rd={} a={} b={}", rd, a, b);
        let obj_val = self.regs[rd];
        let Some(obj_ptr) = self.checked_object_ptr(obj_val, "private field assignment on non-object")? else {
            return Ok(());
        };
        let private_key = self.private_key_from_reg(b);
        let brand_reg = self.bytecode[self.pc] as usize;
        let brand_key = make_private_name_id(self.bytecode[self.pc + 1]);
        self.pc += 2;
        let obj = unsafe { &*obj_ptr };
        // 字段路径：own 有槽（字段存实例 own）→ 直接写，不跨原型、不做 brand 值比较。
        if let Some(pos) = self.private_slot(obj, private_key) {
            if let Some(meta) = obj.prop_meta_at(pos) {
                if meta.is_accessor {
                    if meta.set.is_undefined() {
                        return self.raise_type_error("private field has no setter");
                    }
                    let setter = meta.set;
                    let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
                    return self.call_or_push_setter(setter, obj_val, value, true);
                }
                // 私有方法槽不可写（init 时打标记）。
                if meta.is_hole() {
                    return self
                        .raise_type_error("Cannot write private member to an object whose class did not declare it");
                }
            }
            let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
            let obj = unsafe { &mut *obj_ptr };
            obj.set_prop_shape(pos, value);
            return Ok(());
        }
        // 方法路径：own 无槽（私有方法/访问器存于 home=proto）→ 沿链找方法槽 + brand 检查。
        let Some(home_ptr) = self.find_private_method_slot(obj, private_key) else {
            return self.raise_type_error("private field brand check failed");
        };
        if brand_reg != 0 {
            self.private_brand_check(obj, brand_reg, brand_key)?;
        }
        let home = unsafe { &*home_ptr };
        let pos = self.private_slot(home, private_key).expect("slot just found");
        if let Some(meta) = home.prop_meta_at(pos) {
            if meta.is_accessor {
                if meta.set.is_undefined() {
                    return self.raise_type_error("private field has no setter");
                }
                let setter = meta.set;
                let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
                return self.call_or_push_setter(setter, obj_val, value, true);
            }
            // 私有方法槽不可写（init 时打标记）。
            if meta.is_hole() {
                return self
                    .raise_type_error("Cannot write private member to an object whose class did not declare it");
            }
        }
        let value = self.promote_if_needed_for_write_ptr(home_ptr, self.regs[a]);
        let home = unsafe { &mut *home_ptr };
        home.set_prop_shape(pos, value);
        Ok(())
    }

    fn dispatch_init_private(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("INIT_PRIVATE rd={} a={} b={}", rd, a, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "private field initialization on non-object")?
        else {
            return Ok(());
        };
        let private_key = self.private_key_from_reg(b);
        let is_method = self.bytecode[self.pc] != 0;
        self.pc += 1;
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let obj = unsafe { &mut *obj_ptr };
        if self
            .kernel_core
            .shape_forge()
            .lookup_position(obj.shape_id(), private_key)
            .is_some()
        {
            // 私有元素重复初始化（构造器返回外部对象、同一对象再次初始化）：
            // 规范 PrivateFieldAdd 对已存在的私有名抛 TypeError。
            return self.raise_type_error("Cannot initialize private element twice");
        }
        self.set_or_create_prop_value(obj, private_key, value);
        if is_method {
            let pos = self
                .kernel_core
                .shape_forge()
                .lookup_position(obj.shape_id(), private_key)
                .expect("private slot just created");
            obj.set_private_method_meta(pos);
        }
        Ok(())
    }

    fn dispatch_private_brand_in(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("PRIVATE_BRAND_IN rd={} a={} b={}", rd, a, b);
        let obj_val = self.regs[a];
        if !obj_val.is_object() {
            // `#x in rval`：rval 非对象抛 TypeError（规范 PrivateFieldIn 步骤 4）。
            return self.raise_type_error("private field `in` requires an object on the right-hand side");
        }
        let obj = unsafe { &*obj_val.as_js_object_ptr() };
        let private_key = self.private_key_from_reg(b);
        let brand_reg = self.bytecode[self.pc] as usize;
        let brand_key = make_private_name_id(self.bytecode[self.pc + 1]);
        self.pc += 2;
        // 只查 own 私有元素（PrivateFieldIn 不跨原型链）：own 有该私有槽（字段）→ true；
        // 否则接收者 own 有当前类 brand 槽（方法/访问器，槽在 home）→ true；否则 false。
        // brand_key 是类级私有名 id（每类独立），own 存在该槽即本类实例——存在性即可
        // 判定，不做 cell 值比较（规避 upvalue cell 生命周期缺陷）。
        if self.private_slot(obj, private_key).is_some() {
            self.regs[rd] = JsValue::bool(true);
            return Ok(());
        }
        let has_brand = brand_reg != 0 && self.private_slot(obj, brand_key).is_some();
        self.regs[rd] = JsValue::bool(has_brand);
        Ok(())
    }

    fn dispatch_ic_get_prop(&mut self, a: usize, b: usize) -> Result<(), String> {
        let val = self.regs[a];
        let obj_ptr = val.as_object_ptr() as *mut JsObject;
        if obj_ptr.is_null() {
            let prop_name_si = self.property_key_si(self.regs[b])?;
            if let Some(resolved) = self.primitive_property_get(val, prop_name_si)? {
                self.regs[a] = resolved;
                self.pc += IC_EXT_WORDS;
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
        let (cached_shape_id, cached_slot, cached_depth) = ic_helper::read_ic_slot0(&self.bytecode, &mut self.pc);
        let ic_pc = self.pc;
        if obj.has_prop_meta() {
            // meta 对象整对象走慢路径，此时才解析键。
            let prop_name_si = self.property_key_si(self.regs[b])?;
            let val = self.ordinary_get_with_target(obj, prop_name_si, val, a as u8)?;
            if self.accessor_frame_target_reg.take().is_none() {
                self.regs[a] = val;
            }
            return Ok(());
        }

        if let Some(value) = ic_get_hit(obj, cached_shape_id, cached_slot, cached_depth) {
            // 槽 0 单态命中：只比对 shape_id，不解析键。
            self.regs[a] = value;
            self.profiling.record_ic_hit();
            ic_trace!("IC_GET hit shape={} slot={} depth={}", cached_shape_id, cached_slot, cached_depth);
        } else if let Some(value) = ic_helper::ic_get_hit_poly(obj, &self.bytecode, ic_pc - IC_EXT_WORDS) {
            // 多态槽（1..3）命中。
            self.regs[a] = value;
            self.profiling.record_ic_hit();
            ic_trace!("IC_GET poly hit pc={} shape={}", ic_pc, obj.shape_id());
        } else {
            // miss 分支才解析键（IC 站点键恒为编译期字符串常量，延后解析无 ToPrimitive 副作用）。
            let prop_name_si = self.property_key_si(self.regs[b])?;
            self.profiling.record_ic_miss();
            prop_cache_miss();
            // 模板快路径：prop_forge 只缓存每 shape 最后新增的属性，恰好覆盖该属性时直取槽位。
            if let Some(template) = self.kernel_core.prop_forge().get_template(obj.shape_id()) {
                if template.prop_name == prop_name_si && template.position < obj.prop_vec_len() as u32 {
                    ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, obj.shape_id(), template.position, 0);
                    ic_debug!(
                        "IC_GET propforge hit shape={} prop={} slot={}",
                        obj.shape_id(),
                        prop_name_si,
                        template.position
                    );
                    self.regs[a] = obj.get_prop_shape(template.position);
                } else {
                    // 模板不覆盖该属性（读非末位属性）：走通用 miss 路径并写回，
                    // 否则该 shape 的缓存条目永远学不到。
                    ic_debug!("IC_GET miss shape={} prop={}", obj.shape_id(), prop_name_si);
                    self.regs[a] = self.proto_chain_ic_get(obj, prop_name_si, val)?;
                }
            } else {
                ic_debug!("IC_GET miss shape={} prop={}", obj.shape_id(), prop_name_si);
                let resolved = self.ordinary_get(obj, prop_name_si, val)?;
                // 先试自身属性（快路径，depth=0）。
                if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
                    if !obj.is_accessor_meta(pos) {
                        ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, obj.shape_id(), pos, 0);
                        ic_debug!("IC_GET write-back own shape={} slot={}", obj.shape_id(), pos);
                    }
                } else {
                    // 沿原型链查找真正拥有该属性的对象。
                    let mut cursor = obj.proto().as_js_object_ptr();
                    let mut depth = 1u8;
                    while !cursor.is_null() {
                        let co_ref = unsafe { &*cursor };
                        if let Some(pos) =
                            self.kernel_core.shape_forge().lookup_position(co_ref.shape_id(), prop_name_si)
                        {
                            if !co_ref.is_accessor_meta(pos) {
                                ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, co_ref.shape_id(), pos, depth);
                                ic_debug!(
                                    "IC_GET write-back proto shape={} slot={} depth={}",
                                    co_ref.shape_id(),
                                    pos,
                                    depth
                                );
                            }
                            break;
                        }
                        if !co_ref.proto().is_object() {
                            break;
                        }
                        cursor = co_ref.proto().as_js_object_ptr();
                        depth += 1;
                    }
                }
                self.regs[a] = resolved;
            }
        }

        Ok(())
    }

    fn dispatch_ic_set_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "Cannot create property on non-object")? else {
            return Ok(());
        };

        // read_ic_slot0 先消费全部扩展字，命中路径无需解析键。
        let (cached_shape_id, cached_slot, cached_depth) = ic_helper::read_ic_slot0(&self.bytecode, &mut self.pc);
        let ic_pc = self.pc;
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let receiver = self.regs[rd];
        let obj = unsafe { &mut *obj_ptr };

        // 命中路径零键零拦截：__proto__ 赋值从不写 IC，缓存槽必为数据槽。
        if !obj.has_prop_meta()
            && (ic_set_hit(obj, cached_shape_id, cached_slot, cached_depth, value)
                || ic_helper::ic_set_hit_poly(obj, &self.bytecode, ic_pc - IC_EXT_WORDS, value))
        {
            self.profiling.record_ic_hit();
            ic_trace!("IC_SET hit shape={} slot={} depth={}", cached_shape_id, cached_slot, cached_depth);
            return Ok(());
        }

        // 慢路径（has_prop_meta / miss）才解析键并拦截 __proto__ 赋值，须先于写槽，
        // 防 __proto__ 落入数据槽。
        let prop_name_si = self.property_key_si(self.regs[b])?;
        if self.kernel_core.perm_interner().lookup(prop_name_si) == Some("__proto__") {
            if self.is_object_prototype(obj_ptr) && !value.is_null() {
                self.raise_type_error("Object.prototype.__proto__ is immutable")?;
                return Ok(());
            }
            obj.set_proto(value).map_err(|e| e.to_string())?;
            return Ok(());
        }

        if obj.has_prop_meta() {
            self.ordinary_set_dispatch(obj, prop_name_si, value, receiver)?;
            return Ok(());
        }

        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(obj.shape_id(), prop_name_si) {
            self.profiling.record_ic_miss();
            prop_cache_miss();
            obj.set_prop_shape(pos, value);
            ic_helper::write_ic_back(self.bytecode_mut(), ic_pc, obj.shape_id(), pos, 0);
            ic_debug!("IC_SET write-back shape={} slot={}", obj.shape_id(), pos);
        } else {
            self.profiling.record_ic_miss();
            prop_cache_miss();
            // 写新属性（shape 转换）：数组 length / 整数索引键仍走完整
            // ordinary_set_dispatch 语义，其余走 CreateDataProperty 快路径。
            if self.named_prop_create_needs_ordinary_set(obj, prop_name_si) {
                self.ordinary_set_dispatch(obj, prop_name_si, value, receiver)?;
                return Ok(());
            }
            self.create_named_prop_fast(obj, prop_name_si, value, receiver, ic_pc, true)?;
        }

        Ok(())
    }

    fn dispatch_get_prop(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GET_PROP rd={} a={} b={}", rd, a, b);
        let prop_name_si = self.property_key_si(self.regs[b])?;
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
        let prop_name_si = self.property_key_si(self.regs[b])?;
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

    /// 对象字面量批量构造的纯槽写：把值写入键序预建 shape 的 `slot` 槽。
    /// 不做键解析/`__proto__` 拦截/promote 之外的任何形状变更——键与槽位由
    /// NEW_OBJECT 的键常量表一次性建好。
    ///
    /// # 边界与前提
    /// - `slot` 必须 < NEW_OBJECT 预分配的槽数（emit 保证 ≤255）。
    /// - 值仍需 promote（对象值可能来自 session epoch 且目标是逃逸根）。
    fn dispatch_set_prop_batch(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("SET_PROP_BATCH rd={} slot={}", rd, b);
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "Cannot create property on non-object")? else {
            return Ok(());
        };
        let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[a]);
        let obj = unsafe { &mut *obj_ptr };
        obj.set_prop_storage(b, value);
        Ok(())
    }

    fn dispatch_get_prop_dynamic(&mut self, rd: usize, a: usize, b: usize) -> Result<(), String> {
        vm_trace!("GET_PROP_DYNAMIC rd={} a={} b={}", rd, a, b);
        let prop_name_si = self.property_key_si(self.regs[a])?;
        let receiver = self.regs[rd];
        // 数组元素 fast path：整数键 + 真数组 + 无元素 meta（无 hole/accessor/描述符）
        // + 界内 → 直读元素区，免 primitive_property_get / checked_object_ptr /
        // ordinary_get 调用链。判定条件与 ordinary_get_inner 数组分支严格等价。
        // 非对象 receiver（如 `"abc"[0]`）须先 is_object 快检再 checked_object_ptr，
        // 否则该检查会误抛 TypeError（原路径对原始值走 primitive_property_get 不抛）。
        if is_int_key(prop_name_si) && receiver.is_object() {
            if let Some(obj_ptr) = self.checked_object_ptr(receiver, "GET_PROP_DYNAMIC on non-object")? {
                let obj = unsafe { &*obj_ptr };
                if obj.is_array() && obj.array_elements_meta_vec().is_none() {
                    let idx = int_key_value(prop_name_si);
                    if idx < obj.array_prop_count {
                        self.regs[b] = obj.get_prop_at(idx);
                        return Ok(());
                    }
                }
            }
        }
        if let Some(value) = self.primitive_property_get(receiver, prop_name_si)? {
            self.regs[b] = value;
            return Ok(());
        }
        let Some(obj_ptr) = self.checked_object_ptr(receiver, "GET_PROP_DYNAMIC on non-object")? else {
            return Ok(());
        };
        let obj = unsafe { &*obj_ptr };
        let val = self.ordinary_get_with_target(obj, prop_name_si, receiver, b as u8)?;
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
        let prop_name_si = self.property_key_si(self.regs[a])?;
        // 数组元素 fast path：整数键 + 真数组 + 无 meta + 界内 → 直写元素区，
        // 免 __proto__ 查询 / ordinary_set_dispatch / get_own_property_slot 调用链。
        // 与 ordinary_set_inner 数组分支等价（界内写不触发 length 扩展、无 shadow
        // 同步）。越界写必须走原路径（CreateDataProperty + length 扩展），绝不可
        // 直写——set_prop_storage 对越界下标会落到 hash_props 属性区。
        if is_int_key(prop_name_si) {
            let obj = unsafe { &*obj_ptr };
            if obj.is_array() && obj.array_elements_meta_vec().is_none() {
                let idx = int_key_value(prop_name_si);
                if idx < obj.array_prop_count {
                    // promote 必须保留：obj 是 session 根、写入值可能是 epoch 对象，
                    // 不 promote 下轮 GC 会悬垂。
                    let value = self.promote_if_needed_for_write_ptr(obj_ptr, self.regs[b]);
                    let obj = unsafe { &mut *obj_ptr };
                    obj.set_prop_storage(idx as usize, value);
                    return Ok(());
                }
            }
        }
        // 整数键恒不可能等于 "__proto__"（字符串 intern id 远小于整数键区间），
        // 短路免去每次 set 的 RwLock 查询。
        if !is_int_key(prop_name_si) && self.kernel_core.perm_interner().lookup(prop_name_si) == Some("__proto__") {
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
        // 有属性 meta（freeze/seal 逐属性写 meta 后恒 true，含 accessor 元素/描述符
        // 元素）：回落 ordinary_set 走完整 writable/accessor/extensible 检查，防止
        // 直写绕过冻结语义。
        if obj.has_prop_meta() {
            let si = make_int_key(idx);
            return self.ordinary_set_dispatch(obj, si, value, self.regs[rd]);
        }
        // 无 meta 快路径：不可扩展对象禁止新增元素（越界下标）；已有元素直写不受限
        // （无 meta 即默认可写）。
        if !obj.is_extensible() && idx >= obj.array_prop_count {
            self.raise_type_error("object is not extensible")?;
            return Ok(());
        }
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
        let prop_name_si = self.property_key_si(key_val)?;
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "delete on non-object")? else {
            self.regs[rd] = JsValue::bool(true);
            return Ok(false);
        };
        let obj = unsafe { &mut *obj_ptr };
        // 统一走共享删除逻辑（与 Reflect.deleteProperty 一致）；不可配置返回 false。
        let deleted = oxide_builtins::object::delete_own_property(self, obj, prop_name_si);
        self.regs[rd] = JsValue::bool(deleted);
        Ok(false)
    }

    pub(crate) fn dispatch_delete_prop_dynamic(&mut self, rd: usize, b: usize) -> Result<bool, String> {
        vm_trace!("DELETE_PROP_DYNAMIC rd={}", rd);
        let prop_name_si = self.property_key_si(self.regs[b])?;
        let Some(obj_ptr) = self.checked_object_ptr(self.regs[rd], "delete on non-object")? else {
            self.regs[rd] = JsValue::bool(true);
            return Ok(false);
        };
        let obj = unsafe { &mut *obj_ptr };
        // 统一走共享删除逻辑（与 Reflect.deleteProperty 一致）；不可配置返回 false。
        let deleted = oxide_builtins::object::delete_own_property(self, obj, prop_name_si);
        self.regs[rd] = JsValue::bool(deleted);
        Ok(false)
    }
}
