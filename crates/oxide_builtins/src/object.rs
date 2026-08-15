use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::PermInterner;
use oxide_types::object::{JsObject, PropAttributes, PropMetaEntry};
use oxide_types::private_key::{
    int_key_value, is_int_key, is_private_name_key, is_symbol_key, make_int_key, make_well_known_symbol_key,
    symbol_index_from_key, well_known_symbol_id_from_key,
};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

fn is_integer_index(key: &str) -> bool {
    if key.is_empty() || key.len() > 1 && key.as_bytes()[0] == b'0' {
        return false;
    }
    key.bytes().all(|b| b.is_ascii_digit()) && key.parse::<u64>().unwrap_or(u64::MAX) < (1u64 << 32) - 1
}

/// 收集对象全部自身属性（数组元素区 + shape 链），按规范顺序排列：整数索引在前
/// 升序，其余保持插入序。返回 `(属性键 si, 绝对存储索引)`：数组对象元素区索引即
/// 绝对下标，命名属性 = `array_prop_count + shape 槽位`；普通对象即 shape 槽位。
pub fn walk_own_keys<H: VmHost>(vm: &H, obj: &JsObject) -> Vec<(u32, u32)> {
    let mut keys: Vec<(u32, u32)> = Vec::new();
    // 数组元素区：整数下标是可枚举自身属性（hole 视为不存在），排在命名属性之前。
    if obj.is_array() {
        for i in 0..obj.array_prop_count {
            if obj.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                continue;
            }
            keys.push((make_int_key(i), i));
        }
    }
    let shape_id = obj.shape_id();
    let mut pos: u32 = 0;
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(id) {
            cursor = shape.parent;
            // Symbol 键/私有名键非字符串属性名，排除在字符串枚举之外（但仍占 shape
            // 槽位，pos 计数须含它们才能与物理存储对齐）。
            if shape.property_name != u32::MAX {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    for id in shape_ids.iter().rev() {
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(*id) {
            if shape.property_name != 0
                && !is_symbol_key(shape.property_name)
                && !is_private_name_key(shape.property_name)
            {
                // 绝对存储索引：数组命名属性位于元素区之后。
                let store = if obj.is_array() { obj.array_prop_count + pos } else { pos };
                keys.push((shape.property_name, store));
            }
        }
        pos += 1;
    }
    keys.sort_by(|(a_si, _), (b_si, _)| {
        // 整数键（含 INT 区间键）以数值升序排在普通字符串键之前。
        let a_idx = int_or_string_index(vm, *a_si);
        let b_idx = int_or_string_index(vm, *b_si);
        match (a_idx, b_idx) {
            (Some(ai), Some(bi)) => ai.cmp(&bi),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    keys
}

/// 键 si 的数组下标值：整数键直接反解，字符串键查 interner 后判是否规范数字串。
fn int_or_string_index<H: VmHost>(vm: &H, si: u32) -> Option<u32> {
    if is_int_key(si) {
        return Some(int_key_value(si));
    }
    let key = vm.kernel_core().perm_interner().lookup(si)?;
    is_integer_index(key).then(|| key.parse::<u32>().unwrap())
}

/// 键 si 物化为字符串文本：整数键反解数字串，其余查 interner。
pub fn key_si_to_string<H: VmHost>(vm: &H, si: u32) -> String {
    if is_int_key(si) {
        int_key_value(si).to_string()
    } else {
        vm.kernel_core().perm_interner().lookup(si).unwrap_or("").to_string()
    }
}

/// 收集对象自身全部 Symbol 键（shape 链），按键序排列（根→叶，即插入序）。
/// 与 [`walk_own_keys`] 互补：只返回 Symbol 键，供 `getOwnPropertySymbols` 使用。
fn walk_own_symbol_keys<H: VmHost>(vm: &H, obj: &JsObject) -> Vec<(u32, u32)> {
    let mut keys: Vec<(u32, u32)> = Vec::new();
    let shape_id = obj.shape_id();
    let mut pos: u32 = 0;
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(id) {
            cursor = shape.parent;
            if shape.property_name != u32::MAX && is_symbol_key(shape.property_name) {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    for id in shape_ids.iter().rev() {
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(*id) {
            if shape.property_name != 0 && is_symbol_key(shape.property_name) {
                keys.push((shape.property_name, pos));
            }
        }
        pos += 1;
    }
    keys
}

/// 把 Symbol 键反解为对应的 Symbol 值：well-known 键还原为内置 symbol 对象，
/// 用户 symbol 键还原为 `JsValue::symbol` 值。
fn decode_symbol_key<H: VmHost>(vm: &H, key: u32) -> JsValue {
    if let Some(id) = well_known_symbol_id_from_key(key) {
        let world = vm.session().builtin_world();
        let ptr = match id {
            0 => world.sym_iterator.as_ptr(),
            1 => world.sym_match.as_ptr(),
            2 => world.sym_replace.as_ptr(),
            3 => world.sym_search.as_ptr(),
            4 => world.sym_split.as_ptr(),
            5 => world.sym_to_primitive.as_ptr(),
            6 => world.sym_has_instance.as_ptr(),
            7 => world.sym_match_all.as_ptr(),
            8 => world.sym_async_iterator.as_ptr(),
            10 => world.sym_species.as_ptr(),
            _ => world.sym_to_string_tag.as_ptr(),
        };
        return JsValue::from_js_object(ptr as *mut JsObject);
    }
    JsValue::symbol(symbol_index_from_key(key))
}

/// `Object.getOwnPropertySymbols(obj)`：返回全部自身 Symbol 键数组。
pub fn object_get_own_property_symbols<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = match require_obj_arg(vm, args, "getOwnPropertySymbols") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let symbols: Vec<JsValue> = {
        let obj = unsafe { &*obj_ptr };
        walk_own_symbol_keys(vm, obj)
            .iter()
            .map(|(key, _)| decode_symbol_key(vm, *key))
            .collect()
    };

    let n = symbols.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, v) in symbols.iter().enumerate() {
        unsafe {
            (*arr).set_prop_at(i, *v);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

/// 字符串键是否为数组下标（"0"~"4294967294"，无前导零）。
fn array_index_of<H: VmHost>(vm: &H, key_si: u32) -> Option<u32> {
    if is_int_key(key_si) {
        return Some(int_key_value(key_si));
    }
    let key = vm.kernel_core().perm_interner().lookup(key_si)?;
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    key.parse::<u32>().ok()
}

/// 删除对象自身属性（字节码 delete 与 Reflect.deleteProperty 共用）。
///
/// 数组下标键在元素区（shape 链外），标记为 hole（值 undefined + hole meta），
/// length 不变；命名属性经 shape 链重建移除。属性不可配置时返回 false。
///
/// # 步骤
/// 1. 数组下标元素：检查 configurable，`mark_hole_at` 标记为 hole
/// 2. 命名属性：walk_own_keys 定位槽位，不可配置返回 false
/// 3. 数组先保存元素区（值 + meta），重建命名属性后恢复元素区
///
/// # 边界与前提
/// - 键不在对象自身（含原型链属性）返回 true
/// - 非 configurable 属性返回 false
/// - `key_si` 须已 intern
///
/// # 副作用
/// - 修改 obj 的 shape_id、属性表与 generation；数组元素区内容不变
///
/// # 注意事项
/// - 数组元素存在性以 `prop_meta_at` 的 hole 标记判定，删除后重新写入元素
///   会自动清除 hole 标记恢复存在
pub fn delete_own_property<H: VmHost>(vm: &mut H, obj: &mut JsObject, key_si: u32) -> bool {
    // 数组下标元素在元素区，不参与 shape 链，单独删除（保持 length 不变）。
    if obj.is_array() {
        if let Some(index) = array_index_of(vm, key_si) {
            if index < obj.array_prop_count {
                let meta = obj.prop_meta_at(index);
                // 已是 hole 视为不存在；非 configurable 不可删。
                if meta.is_some_and(|m| m.is_hole()) {
                    return true;
                }
                if meta.is_some_and(|m| !m.attributes.configurable()) {
                    return false;
                }
                obj.mark_hole_at(index);
                obj.bump_generation();
                return true;
            }
            return true;
        }
    }

    // walk_own_keys 只返回字符串键；Symbol 键（well-known/用户）需另行定位。
    let keys = walk_own_keys(vm, obj);
    let mut all_keys = keys;
    if is_symbol_key(key_si) {
        all_keys.extend(walk_own_symbol_keys(vm, obj));
    }
    let Some(delete_pos) = all_keys.iter().find(|(si, _)| *si == key_si).map(|(_, pos)| *pos) else {
        return true;
    };
    // walk_own_keys 已返回绝对存储索引（数组含元素区偏移），直接使用。
    if obj
        .prop_meta_at(delete_pos)
        .map(|meta| !meta.attributes.configurable())
        .unwrap_or(false)
    {
        return false;
    }

    // 数组重建前保存元素区（值 + meta），重建后恢复到命名属性之前。
    let saved_elements: Vec<JsValue> = if obj.is_array() {
        (0..obj.array_prop_count).map(|i| obj.get_prop_at(i)).collect()
    } else {
        Vec::new()
    };
    let saved_element_meta: Option<Vec<Option<PropMetaEntry>>> = if obj.is_array() {
        Some((0..obj.array_prop_count).map(|i| obj.prop_meta_at(i)).collect())
    } else {
        None
    };

    let retained: Vec<(u32, JsValue, Option<PropMetaEntry>)> = all_keys
        .into_iter()
        // 数组元素区由下方独立保存/恢复，此处只重建命名属性（元素键绝对下标 < 元素数）。
        .filter(|(_, pos)| *pos != delete_pos && !(obj.is_array() && *pos < obj.array_prop_count))
        .map(|(si, pos)| (si, obj.get_prop_at(pos), obj.prop_meta_at(pos)))
        .collect();

    // 重建 shape 链与属性表（数组先清空，含元素区，随后恢复）。
    obj.set_shape_id(EMPTY_SHAPE_ID);
    obj.clear_props();
    for (si, value, meta) in retained {
        let shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape);
        let pos = obj.push_prop(value);
        if let Some(meta) = meta {
            if meta.is_accessor {
                obj.set_accessor_meta(pos, meta.get, meta.set, meta.attributes);
            } else {
                obj.set_data_meta(pos, meta.attributes);
            }
        }
    }
    if obj.is_array() {
        let n = saved_elements.len();
        obj.set_prop_count(n);
        for (i, val) in saved_elements.into_iter().enumerate() {
            obj.set_prop_at(i, val);
        }
        if let Some(saved_meta) = saved_element_meta {
            let meta = obj.ensure_array_elements_meta();
            for (i, entry) in saved_meta.into_iter().enumerate() {
                if let Some(entry) = entry {
                    while meta.len() <= i {
                        meta.push(None);
                    }
                    meta[i] = Some(entry);
                }
            }
        }
    }
    obj.bump_generation();
    true
}

/// JS `Object()` 构造逻辑：创建空对象（prototype 为 null，由 VM 补装内置原型）。
pub fn object_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    // 对象参数原样返回；原始值参数创建对应 boxed 对象（规范 ToObject）。
    if val.is_object() {
        return NativeResult::Ok(val);
    }
    if val.is_null() || val.is_undefined() {
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_int() || val.is_double() {
        let proto = vm.session().builtin_world().number_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_NUMBER_OBJ;
        obj_ref.set_prop_at(0, val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_string() {
        let proto = vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_STRING_OBJ;
        obj_ref.set_prop_at(0, val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_bool() {
        let proto = vm.session().builtin_world().boolean_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_BOOLEAN_OBJ;
        obj_ref.set_prop_at(0, val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_symbol() {
        let proto = vm.session().builtin_world().symbol_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_SYMBOL_OBJ;
        obj_ref.set_prop_at(0, val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_bigint() {
        let proto = vm.session().builtin_world().bigint_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.set_prop_at(0, val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// `Object.keys(obj)`：返回可枚举自身属性的字符串名数组。
pub fn object_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = match require_obj_arg(vm, args, "keys") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let key_names: Vec<String>;
    {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        let owned_keys: Vec<(u32, u32)> = keys
            .into_iter()
            .filter(|(_si, offset)| {
                obj.prop_meta_at(*offset)
                    .map(|m| m.attributes.enumerable())
                    .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
            })
            .collect();
        key_names = owned_keys.iter().map(|(si, _offset)| key_si_to_string(vm, *si)).collect();
    }
    let n = key_names.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, k) in key_names.iter().enumerate() {
        let str_val = vm.new_string(k);
        unsafe {
            (*arr).set_prop_at(i, str_val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

/// `Object.create(proto, properties)`：以指定 prototype 创建新对象，可选地按
/// 属性描述符集合定义自身属性。
pub fn object_create<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.create: at least 1 argument required"));
    }
    let proto_val = vm.reg(args[1]);
    if proto_val.is_null() {
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        if args.len() >= 3 && !vm.reg(args[2]).is_undefined() {
            if let Err(msg) = define_all_from_properties(vm, obj, vm.reg(args[2])) {
                return NativeResult::Err(crate::error::create_type_error(vm, &msg));
            }
        }
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if !proto_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.create: prototype must be an object or null",
        ));
    }
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    if args.len() >= 3 && !vm.reg(args[2]).is_undefined() {
        if let Err(msg) = define_all_from_properties(vm, obj, vm.reg(args[2])) {
            return NativeResult::Err(crate::error::create_type_error(vm, &msg));
        }
    }
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// `Object.assign(target, ...sources)`：拷贝各源对象的可枚举自身属性到目标，返回目标。
pub fn object_assign<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.assign requires a target"));
    }
    let target_val = vm.reg(args[1]);
    let target_val = match oxide_runtime_api::to_object(target_val, vm) {
        Ok(val) => val,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let target_ptr = target_val.as_js_object_ptr();
    if target_ptr.is_null() {
        return NativeResult::Ok(target_val);
    }

    let mut all_assignments: Vec<(u32, JsValue)> = Vec::new();
    for &arg_reg in args.iter().skip(2) {
        let source_val = vm.reg(arg_reg);
        if !source_val.is_object() {
            continue;
        }
        let source_ptr = source_val.as_js_object_ptr();
        if source_ptr.is_null() {
            continue;
        }
        let source_keys: Vec<(u32, u32)> = {
            let source = unsafe { &*source_ptr };
            walk_own_keys(vm, source)
                .into_iter()
                // CopyDataProperties：只拷贝可枚举自身属性（无显式 meta 视为可枚举）。
                .filter(|(_si, offset)| {
                    source
                        .prop_meta_at(*offset)
                        .map(|m| m.attributes.enumerable())
                        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
                })
                .collect()
        };
        for (si, _offset) in source_keys {
            let source = unsafe { &*source_ptr };
            // ordinary_get 取值（receiver = 源对象）：数据属性等价直读，访问器属性
            // 触发 getter；getter 抛错按规范中断整个 assign。
            let val = match vm.ordinary_get(source, si, source_val) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::error::create_type_error(vm, &err)),
            };
            all_assignments.push((si, val));
        }
    }

    let target = unsafe { &mut *target_ptr };
    for (si, val) in all_assignments {
        let promoted = vm.promote_if_needed_for_write_ptr(target_ptr, val);
        // Set(to, key, value, true)：receiver 为目标对象（目标同名 setter 的 this 指向 target）。
        if let Err(err) = vm.ordinary_set(target, si, promoted, target_val) {
            return NativeResult::Err(crate::error::create_type_error(vm, &err));
        }
    }
    NativeResult::Ok(target_val)
}

/// `Object.is(a, b)`：按 SameValue 语义比较（NaN 相等、+0/-0 不等）。
pub fn object_is<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.is called with insufficient arguments"));
    }
    let lhs = vm.reg(args[1]);
    let rhs = vm.reg(args[2]);
    NativeResult::Ok(JsValue::bool(oxide_runtime_api::same_value(lhs, rhs)))
}

/// 按 ToPropertyDescriptor 语义把描述符定义/修改到对象的自身属性上。
///
/// 处理数据（value/writable）与访问器（get/set）两类描述符；描述符字段沿原型
/// 链解析（ordinary_get，触发 accessor getter）；字段缺失时按已有属性回填，
/// 新属性缺省字段为 false。
///
/// # 步骤
/// 1. 解析 value/get/set/writable/enumerable/configurable 字段
/// 2. 校验 data 与 accessor 字段互斥、getter/setter 可调用
/// 3. 按访问器/数据/无字段三种形态调用对应 define 操作
///
/// # 边界与前提
/// - `desc_val` 必须是对象；`key_si` 须已 intern
/// - 修改已有属性时缺省字段回填现有值；新属性缺省为 false
///
/// # 副作用
/// - 修改 obj 的 shape 链、属性表与 generation
fn define_from_descriptor<H: VmHost>(
    vm: &mut H, obj_ptr: *mut JsObject, key_si: u32, desc_val: JsValue,
) -> Result<(), String> {
    if !desc_val.is_object() {
        return Err("Property description must be an object".to_string());
    }

    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let get_si = vm.kernel_core().perm_interner().intern("get").0;
    let set_si = vm.kernel_core().perm_interner().intern("set").0;
    let writable_si = vm.kernel_core().perm_interner().intern("writable").0;
    let enumerable_si = vm.kernel_core().perm_interner().intern("enumerable").0;
    let configurable_si = vm.kernel_core().perm_interner().intern("configurable").0;

    let value_field = own_field(vm, desc_val, value_si);
    let get_field = own_field(vm, desc_val, get_si);
    let set_field = own_field(vm, desc_val, set_si);
    let writable_field = own_field(vm, desc_val, writable_si);

    let existing_pos = {
        let obj = unsafe { &*obj_ptr };
        vm.get_own_property_slot(obj, key_si)
    };
    let existing_meta = existing_pos.map(|pos| {
        let obj = unsafe { &*obj_ptr };
        // 已有属性无显式 meta（普通数据属性/数组元素）时按默认属性回填：
        // 描述符缺省字段保持现有值（writable/enumerable/configurable 均 true），
        // 而非按新属性处理为 false（Object.defineProperty 省略字段不改已有属性）。
        obj.prop_meta_at(pos)
            .unwrap_or_else(|| oxide_types::object::PropMetaEntry::data(PropAttributes::DEFAULT_DATA))
    });

    // 修改已有属性时缺省字段回填现有值，仅定义新属性时缺省才为 false。
    let enumerable = own_field(vm, desc_val, enumerable_si)
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or_else(|| existing_meta.map(|m| m.attributes.enumerable()).unwrap_or(false));
    let configurable = own_field(vm, desc_val, configurable_si)
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or_else(|| existing_meta.map(|m| m.attributes.configurable()).unwrap_or(false));

    let has_data = value_field.is_some() || writable_field.is_some();
    let has_accessor = get_field.is_some() || set_field.is_some();
    if has_data && has_accessor {
        return Err("Invalid property descriptor: cannot mix data and accessor fields".to_string());
    }

    let existing_value = existing_pos.map_or(JsValue::undefined(), |pos| {
        let obj = unsafe { &*obj_ptr };
        obj.get_prop_at(pos)
    });

    let obj = unsafe { &mut *obj_ptr };
    if has_accessor {
        let get = get_field.unwrap_or_else(|| {
            existing_meta
                .filter(|m| m.is_accessor)
                .map(|m| m.get)
                .unwrap_or(JsValue::undefined())
        });
        let set = set_field.unwrap_or_else(|| {
            existing_meta
                .filter(|m| m.is_accessor)
                .map(|m| m.set)
                .unwrap_or(JsValue::undefined())
        });
        if (!get.is_undefined() && !is_callable(get)) || (!set.is_undefined() && !is_callable(set)) {
            return Err("accessor descriptor get/set must be callable or undefined".to_string());
        }
        vm.define_accessor_property(obj, key_si, get, set, PropAttributes::new(false, enumerable, configurable))?;
    } else if has_data {
        let value = if existing_pos.is_some() {
            value_field.unwrap_or(existing_value)
        } else {
            value_field.unwrap_or(JsValue::undefined())
        };
        let writable = if existing_pos.is_some() {
            writable_field
                .map(oxide_runtime_api::to_boolean)
                .unwrap_or_else(|| existing_meta.map(|m| m.attributes.writable()).unwrap_or(false))
        } else {
            writable_field.map(oxide_runtime_api::to_boolean).unwrap_or(false)
        };
        vm.define_data_property(obj, key_si, value, PropAttributes::new(writable, enumerable, configurable))?;
    } else {
        if existing_pos.is_none() {
            vm.define_data_property(
                obj,
                key_si,
                JsValue::undefined(),
                PropAttributes::new(false, enumerable, configurable),
            )?;
            return Ok(());
        }
        let is_accessor = existing_meta.map(|m| m.is_accessor).unwrap_or(false);
        let writable = existing_meta.map(|m| m.attributes.writable()).unwrap_or(true);
        let attrs = PropAttributes::new(writable, enumerable, configurable);
        if is_accessor {
            let get = existing_meta.map(|m| m.get).unwrap_or(JsValue::undefined());
            let set = existing_meta.map(|m| m.set).unwrap_or(JsValue::undefined());
            vm.define_accessor_property(obj, key_si, get, set, attrs)?;
        } else {
            vm.define_data_property(obj, key_si, existing_value, attrs)?;
        }
    }
    Ok(())
}

/// 遍历对象自身的可枚举属性，把每个值当作描述符依次定义到目标对象上。
/// `Object.defineProperties` 与 `Object.create(proto, properties)` 共用。
///
/// # 边界与前提
/// - `props_val` 必须是对象，否则返回错误（null 触 ToObject 抛 TypeError）
/// - 仅处理可枚举自身属性；描述符值经 ordinary_get 读取（触发访问器 getter）
///
/// # 副作用
/// - 修改 target 的 shape 链、属性表与 generation
fn define_all_from_properties<H: VmHost>(
    vm: &mut H, target_ptr: *mut JsObject, props_val: JsValue,
) -> Result<(), String> {
    if !props_val.is_object() {
        return Err("Property description must be an object".to_string());
    }
    let props_ptr = props_val.as_js_object_ptr();
    if props_ptr.is_null() {
        return Err("Property description must be an object".to_string());
    }
    let prop_keys: Vec<(u32, u32)> = {
        let props = unsafe { &*props_ptr };
        walk_own_keys(vm, props)
    };
    for (key_si, offset) in prop_keys {
        let props = unsafe { &*props_ptr };
        // 非可枚举自身属性跳过；无显式 meta 视为可枚举（普通字面量默认）。
        if props.prop_meta_at(offset).map(|m| !m.attributes.enumerable()).unwrap_or(false) {
            continue;
        }
        let desc_val = vm.ordinary_get(props, key_si, props_val)?;
        define_from_descriptor(vm, target_ptr, key_si, desc_val)?;
    }
    Ok(())
}

/// `Object.defineProperty(obj, key, descriptor)`：按 descriptor 定义/修改属性，
/// 支持数据与访问器描述符，兼容已有属性的默认回填。
pub fn object_define_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.defineProperty: expected at least 3 arguments",
        ));
    }
    let obj_val = match oxide_runtime_api::to_object(vm.reg(args[1]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj_ptr = obj_val.as_js_object_ptr();
    // well-known symbol 等特殊键统一走 property_key_si（映射到各自的 Symbol 键），
    // 保证与计算属性访问、Reflect.defineProperty 等读键路径一致。
    let si = vm.property_key_si(vm.reg(args[2]));

    if let Err(msg) = define_from_descriptor(vm, obj_ptr, si, vm.reg(args[3])) {
        return NativeResult::Err(crate::error::create_type_error(vm, &msg));
    }
    NativeResult::Ok(obj_val)
}

/// `Object.getOwnPropertyDescriptor(obj, key)`：返回自身属性的描述符对象；
/// 不存在返回 undefined。
pub fn object_get_own_property_descriptor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.getOwnPropertyDescriptor called on non-object",
        ));
    }
    let obj_val = match oxide_runtime_api::to_object(vm.reg(args[1]), vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj_ptr = obj_val.as_js_object_ptr();
    let key = vm.property_key_si(vm.reg(args[2]));
    let obj = unsafe { &*obj_ptr };
    let Some(offset) = vm.get_own_property_slot(obj, key) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let found_value = obj.get_prop_at(offset);
    let found_meta = obj.prop_meta_at(offset);

    let sf_ptr = vm.kernel_core().perm_interner().as_ref() as *const PermInterner;
    let sh_ptr = vm.kernel_core().shape_forge().as_ref() as *const ShapeForge;
    let desc_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let desc = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(desc_proto)));
    let sf = unsafe { &*sf_ptr };
    let sh = unsafe { &*sh_ptr };

    let d: &mut JsObject = unsafe { &mut *desc };
    let meta = found_meta.unwrap_or_else(|| oxide_types::object::PropMetaEntry::data(PropAttributes::DEFAULT_DATA));
    if meta.is_accessor {
        push_desc_prop(d, sh, sf.intern("get").0, meta.get);
        push_desc_prop(d, sh, sf.intern("set").0, meta.set);
        push_desc_prop(d, sh, sf.intern("enumerable").0, JsValue::bool(meta.attributes.enumerable()));
        push_desc_prop(d, sh, sf.intern("configurable").0, JsValue::bool(meta.attributes.configurable()));
    } else {
        push_desc_prop(d, sh, sf.intern("value").0, found_value);
        push_desc_prop(d, sh, sf.intern("writable").0, JsValue::bool(meta.attributes.writable()));
        push_desc_prop(d, sh, sf.intern("enumerable").0, JsValue::bool(meta.attributes.enumerable()));
        push_desc_prop(d, sh, sf.intern("configurable").0, JsValue::bool(meta.attributes.configurable()));
    }

    NativeResult::Ok(JsValue::from_js_object(desc))
}

/// 按 ToPropertyDescriptor 语义取描述符字段：沿原型链判存在性，存在时经
/// ordinary_get 取值（触发 accessor getter，receiver 为描述符对象本身）。
fn own_field<H: VmHost>(vm: &mut H, desc: JsValue, prop_si: u32) -> Option<JsValue> {
    // ordinary_get 对缺失属性返回 undefined，无法区分"不存在"与"值为
    // undefined"，故先用 resolve_property 沿原型链判 HasProperty。
    let obj = unsafe { &*desc.as_js_object_ptr() };
    vm.resolve_property(obj, prop_si)?;
    vm.ordinary_get(obj, prop_si, desc).ok()
}

fn is_callable(value: JsValue) -> bool {
    value.is_object() && unsafe { &*value.as_js_object_ptr() }.is_function()
}

fn push_desc_prop(obj: &mut JsObject, shape_forge: &ShapeForge, prop_si: u32, val: JsValue) {
    let shape_id = shape_forge.make_shape(obj.shape_id(), prop_si);
    obj.set_shape_id(shape_id);
    obj.push_prop(val);
}

fn require_obj_arg<H: VmHost>(vm: &mut H, args: &[u8], fn_name: &str) -> Result<*mut JsObject, JsValue> {
    if args.len() < 2 {
        return Err(crate::error::create_type_error(vm, &format!("Object.{fn_name} called on non-object")));
    }
    let val = vm.reg(args[1]);
    // ToObject：原始值装箱后返回其对象指针；null/undefined 抛 TypeError。
    let obj_val = match oxide_runtime_api::to_object(val, vm) {
        Ok(v) => v,
        Err(msg) => return Err(crate::error::create_type_error(vm, &msg)),
    };
    Ok(obj_val.as_js_object_ptr())
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

/// 把指定存储下标的 own 属性 meta 改写为冻结形态：数据属性 writable=false +
/// configurable=false，访问器属性 configurable=false（writable 不适用），
/// enumerable 保留原值。
fn freeze_own_prop_meta(obj: &mut JsObject, store: u32) {
    let meta = obj
        .prop_meta_at(store)
        .unwrap_or_else(|| PropMetaEntry::data(PropAttributes::DEFAULT_DATA));
    let attrs = PropAttributes::new(false, meta.attributes.enumerable(), false);
    if meta.is_accessor {
        obj.set_accessor_meta(store, meta.get, meta.set, attrs);
    } else {
        obj.set_data_meta(store, attrs);
    }
}

/// 把指定存储下标的 own 属性 meta 改写为密封形态：configurable=false，
/// writable/enumerable 与访问器形态保留原值。
fn seal_own_prop_meta(obj: &mut JsObject, store: u32) {
    let meta = obj
        .prop_meta_at(store)
        .unwrap_or_else(|| PropMetaEntry::data(PropAttributes::DEFAULT_DATA));
    let attrs = PropAttributes::new(meta.attributes.writable(), meta.attributes.enumerable(), false);
    if meta.is_accessor {
        obj.set_accessor_meta(store, meta.get, meta.set, attrs);
    } else {
        obj.set_data_meta(store, attrs);
    }
}

/// `Object.freeze(obj)`：冻结对象（不可扩展 + 全部属性不可配置/不可写），返回原对象。
///
/// # 副作用
/// - 逐属性写 meta 使 `has_prop_meta()` 恒 true，IC 直写路径自动失效，后续
///   写/define 一律回落 ordinary_set / define 检查（writable/configurable 判定）
pub fn object_freeze<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.freeze called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    let obj_ptr = val.as_js_object_ptr();
    {
        let obj = unsafe { &mut *obj_ptr };
        // 命名属性：walk_own_keys 返回绝对存储索引（数组含元素区偏移）。
        let keys = walk_own_keys(vm, obj);
        for (_si, pos) in keys {
            freeze_own_prop_meta(obj, pos);
        }
        // 数组元素区独立于 shape 链（hole 非 own 属性，跳过）。
        if obj.is_array() {
            for i in 0..obj.array_prop_count {
                if obj.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                    continue;
                }
                freeze_own_prop_meta(obj, i);
            }
        }
        obj.set_frozen(true);
        obj.set_extensible(false);
    }
    NativeResult::Ok(val)
}

/// `Object.seal(obj)`：密封对象（不可扩展 + 全部属性不可配置），返回原对象。
///
/// # 副作用
/// - 逐属性写 meta 使 `has_prop_meta()` 恒 true，IC 直写路径自动失效，后续
///   define 回落 non-configurable 检查（已有属性写仍可正常进行）
pub fn object_seal<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.seal called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    let obj_ptr = val.as_js_object_ptr();
    {
        let obj = unsafe { &mut *obj_ptr };
        let keys = walk_own_keys(vm, obj);
        for (_si, pos) in keys {
            seal_own_prop_meta(obj, pos);
        }
        if obj.is_array() {
            for i in 0..obj.array_prop_count {
                if obj.prop_meta_at(i).is_some_and(|m| m.is_hole()) {
                    continue;
                }
                seal_own_prop_meta(obj, i);
            }
        }
        obj.set_sealed(true);
        obj.set_extensible(false);
    }
    NativeResult::Ok(val)
}

/// `Object.preventExtensions(obj)`：禁止添加新属性，返回原对象。
pub fn object_prevent_extensions<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.preventExtensions called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    let obj_ptr = val.as_js_object_ptr();
    unsafe {
        (*obj_ptr).set_extensible(false);
    }
    NativeResult::Ok(val)
}

/// `Object.isFrozen(obj)`：对象是否冻结（检查 extensible 及全部属性 writable/configurable）。
pub fn object_is_frozen<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.isFrozen called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj = unsafe { &*obj_ptr };
    if obj.is_frozen() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    if obj.is_extensible() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let keys = walk_own_keys(vm, obj);
    for (_si, offset) in keys {
        if let Some(meta) = obj.prop_meta_at(offset) {
            if meta.attributes.configurable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
            if !meta.is_accessor && meta.attributes.writable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
        } else {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `Object.isSealed(obj)`：对象是否密封（检查 extensible 及全部属性 configurable）。
pub fn object_is_sealed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.isSealed called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj = unsafe { &*obj_ptr };
    if obj.is_sealed() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    if obj.is_extensible() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let keys = walk_own_keys(vm, obj);
    for (_si, offset) in keys {
        if let Some(meta) = obj.prop_meta_at(offset) {
            if meta.attributes.configurable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
        } else {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `Object.isExtensible(obj)`：对象是否可扩展。
pub fn object_is_extensible<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.isExtensible called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj = unsafe { &*obj_ptr };
    NativeResult::Ok(JsValue::bool(obj.is_extensible()))
}

/// `Object.getOwnPropertyNames(obj)`：返回全部自身属性名（含不可枚举）。
pub fn object_get_own_property_names<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "getOwnPropertyNames"));

    let key_names: Vec<String> = {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        keys.iter().map(|(si, _)| key_si_to_string(vm, *si)).collect()
    };

    let n = key_names.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, k) in key_names.iter().enumerate() {
        let str_val = vm.new_string(k);
        unsafe {
            (*arr).set_prop_at(i, str_val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

/// `Object.defineProperties(obj, descriptors)`：批量按描述符定义属性，返回目标对象。
pub fn object_define_properties<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.defineProperties: expected at least 2 arguments",
        ));
    }
    let target_val = vm.reg(args[1]);
    let target_ptr = match require_obj_arg(vm, args, "defineProperties") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    if let Err(msg) = define_all_from_properties(vm, target_ptr, vm.reg(args[2])) {
        return NativeResult::Err(crate::error::create_type_error(vm, &msg));
    }
    NativeResult::Ok(target_val)
}

/// `Object.fromEntries(entries)`：由 `[key, value]` 对数组构建对象。
pub fn object_from_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.fromEntries: expected 1 argument"));
    }
    let entries = vm.reg(args[1]);
    if !entries.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.fromEntries: argument must be iterable"));
    }
    let entries_ptr = entries.as_js_object_ptr();
    if entries_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.fromEntries: argument must be iterable"));
    }
    let obj = vm.alloc_object(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject),
    ));
    let target_val = JsValue::from_js_object(obj);
    // entry 数组的元素存元素区（元素区之外才是命名属性区），按元素区长度迭代。
    let n: usize = unsafe { (*entries_ptr).array_prop_count } as usize;
    for i in 0..n {
        let pair_val = unsafe { (*entries_ptr).get_prop_at(i) };
        if !pair_val.is_object() {
            continue;
        }
        let pair_ptr = pair_val.as_js_object_ptr();
        if pair_ptr.is_null() {
            continue;
        }
        let pair = unsafe { &*pair_ptr };
        let key_val = pair.get_prop_at(0);
        let value_val = pair.get_prop_at(1);
        // ToPropertyKey 语义建键：int/规范数字串/symbol 统一映射，避免数字键分裂。
        let si = vm.property_key_si(key_val);
        let promoted = vm.promote_if_needed_for_write_ptr(obj, value_val);
        let _ = vm.ordinary_set(unsafe { &mut *obj }, si, promoted, target_val);
    }
    NativeResult::Ok(target_val)
}

/// `Object.getPrototypeOf(obj)`：返回对象的 prototype。
pub fn object_get_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "getPrototypeOf"));
    NativeResult::Ok(unsafe { (*obj_ptr).proto() })
}

/// `Object.hasOwn(obj, key)`：对象是否有指定自身属性。
pub fn object_has_own<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj_ptr = native_try!(require_obj_arg(vm, args, "hasOwn"));
    let key_si = vm.property_key_si(vm.reg(args[2]));
    let obj = unsafe { &*obj_ptr };
    NativeResult::Ok(JsValue::bool(vm.get_own_property_slot(obj, key_si).is_some()))
}

/// `Object.prototype.valueOf`：返回 this 本身（配合 OrdinaryToPrimitive 的兜底）。
pub fn object_proto_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    // Object.prototype.valueOf 原样返回 this 对象；OrdinaryToPrimitive
    // 因结果非原始值而继续走 toString。
    NativeResult::Ok(vm.reg(args[0]))
}

/// `Object.prototype.toString`：返回 `[object Tag]`。对象路径先按内置类型判定标签，
/// 再读 `@@toStringTag`——为字符串时覆盖内置标签（TypedArray 依赖它区分具体类型）。
pub fn object_proto_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let tag = if !this_val.is_object() {
        if this_val.is_string() {
            "String"
        } else if this_val.is_int() || this_val.is_double() {
            "Number"
        } else if this_val.is_bool() {
            "Boolean"
        } else if this_val.is_symbol() {
            "Symbol"
        } else {
            "Object"
        }
    } else {
        let ptr = this_val.as_js_object_ptr();
        if ptr.is_null() {
            "Object"
        } else {
            let obj = unsafe { &*ptr };
            let builtin = if obj.is_array() {
                "Array"
            } else if obj.is_function() {
                "Function"
            } else if obj.is_string_obj() {
                "String"
            } else if obj.is_number_obj() {
                "Number"
            } else if obj.is_boolean_obj() {
                "Boolean"
            } else if obj.is_regexp_obj() {
                "RegExp"
            } else if obj.is_date_obj() {
                "Date"
            } else if obj.is_typed_array_obj() {
                "TypedArray"
            } else if obj.is_array_buffer_obj() {
                "ArrayBuffer"
            } else if obj.is_arguments_obj() {
                "Arguments"
            } else if std::ptr::eq(ptr, vm.session().builtin_world().math_object.as_ptr()) {
                "Math"
            } else if std::ptr::eq(ptr, vm.session().builtin_world().json_object.as_ptr()) {
                "JSON"
            } else {
                "Object"
            };
            // @@toStringTag 为字符串时覆盖内置标签；getter 抛错透传原异常。
            let tag_key = make_well_known_symbol_key(9);
            let tag_value = match vm.ordinary_get(obj, tag_key, this_val) {
                Ok(value) if !value.is_undefined() => Ok(value),
                Ok(_) => {
                    let legacy_key = vm.kernel_core().perm_interner().intern("@@toStringTag").0;
                    vm.ordinary_get(obj, legacy_key, this_val)
                }
                Err(err) => Err(err),
            };
            match tag_value {
                Ok(v) => match vm.lookup_str(v) {
                    Some(s) => return NativeResult::Ok(vm.new_string(&format!("[object {s}]"))),
                    None => builtin,
                },
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            }
        }
    };
    NativeResult::Ok(vm.new_string(&format!("[object {tag}]")))
}

/// `Object.prototype.hasOwnProperty(key)`：this 是否有指定自身属性。
///
/// # 步骤
/// 1. 先 ToPropertyKey 求键（spec 顺序：键先于 ToObject）。
/// 2. ToObject 装箱 this（null/undefined 抛 TypeError，原始值装箱后查 own 槽）。
pub fn object_proto_has_own_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let key_si = vm.property_key_si(vm.reg(args[1]));
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    NativeResult::Ok(JsValue::bool(vm.get_own_property_slot(obj, key_si).is_some()))
}

/// `Object.prototype.propertyIsEnumerable(key)`：指定自身属性是否可枚举。
///
/// # 步骤
/// 1. 先 ToPropertyKey 求键。
/// 2. ToObject 装箱 this（null/undefined 抛 TypeError）。
pub fn object_proto_property_is_enumerable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let key_si = vm.property_key_si(vm.reg(args[1]));
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    let Some(pos) = vm.get_own_property_slot(obj, key_si) else {
        return NativeResult::Ok(JsValue::bool(false));
    };
    let enumerable = obj
        .prop_meta_at(pos)
        .map(|meta| meta.attributes.enumerable())
        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
    NativeResult::Ok(JsValue::bool(enumerable))
}

/// `Object.entries(obj)`：返回可枚举自身属性的 `[key, value]` 对数组。
pub fn object_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "entries"));
    let obj = unsafe { &*obj_ptr };
    let keys = walk_own_keys(vm, obj);
    let owned_keys: Vec<(u32, u32)> = keys
        .into_iter()
        .filter(|(_si, offset)| {
            obj.prop_meta_at(*offset)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
        })
        .collect();
    let n = owned_keys.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (si, offset)) in owned_keys.iter().enumerate() {
        let key_str = key_si_to_string(vm, *si);
        let key_val = vm.new_string(&key_str);
        let val = obj.get_prop_at(*offset);
        let pair = vm.alloc_object(JsObject::new_array(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(array_proto),
            2,
            vm.epoch().bump(),
        ));
        unsafe {
            (*pair).set_prop_at(0, key_val);
            (*pair).set_prop_at(1, val);
            (*pair).set_prop_count(2);
            (*arr).set_prop_at(i, JsValue::from_js_object(pair));
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

/// `Object.values(obj)`：返回可枚举自身属性的值数组。
pub fn object_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "values"));
    let obj = unsafe { &*obj_ptr };
    let keys = walk_own_keys(vm, obj);
    let owned_keys: Vec<(u32, u32)> = keys
        .into_iter()
        .filter(|(_si, offset)| {
            obj.prop_meta_at(*offset)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
        })
        .collect();
    let n = owned_keys.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (_si, offset)) in owned_keys.iter().enumerate() {
        let val = obj.get_prop_at(*offset);
        unsafe {
            (*arr).set_prop_at(i, val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}
