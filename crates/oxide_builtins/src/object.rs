use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::PermInterner;
use oxide_types::object::{JsObject, PropAttributes, PropMetaEntry};
use oxide_types::private_key::{
    int_key_value, is_int_key, is_private_name_key, is_symbol_key, make_int_key, make_well_known_symbol_key,
    symbol_index_from_key, well_known_symbol_id_from_key, INT_KEY_COUNT,
};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;

/// 判断字符串是否为规范整数索引：非空、无前导零、全数字且值 < 2^32-1。
fn is_integer_index(key: &str) -> bool {
    if key.is_empty() || key.len() > 1 && key.as_bytes()[0] == b'0' {
        return false;
    }
    key.bytes().all(|b| b.is_ascii_digit()) && key.parse::<u64>().unwrap_or(u64::MAX) < (1u64 << 32) - 1
}

/// 收集对象全部自身属性（数组元素区 + shape 链），按规范顺序排列：整数索引在前
/// 升序，其余保持插入序。si 指 interned 字符串键编号（PermInterner 分配的 u32 id）。
/// 返回 `(属性键 si, 绝对存储索引)`：数组对象元素区索引即绝对下标，命名属性 =
/// `array_prop_count + shape 槽位`；普通对象即 shape 槽位。TA 元素键（0..live
/// 长）以哨兵存储索引 `u32::MAX` 推入（无槽位），越界（含 detach）live 长 0 键集空。
pub fn walk_own_keys<H: VmHost>(vm: &H, obj: &JsObject) -> Vec<(u32, u32)> {
    let mut keys: Vec<(u32, u32)> = Vec::new();
    // TA 元素键：整数下标 0..live 长是可枚举自身属性（元素住 buffer，不在形状
    // 链）；越界（含 detach）live 长 0，键集自然空。哨兵存储下标 u32::MAX =
    // 无槽位（meta 读一律 None，取值消费方按键路由元素读）。
    if obj.is_typed_array_obj() {
        let len = crate::typed_array::ta_view_length(vm, obj) as u32;
        for i in 0..len.min(INT_KEY_COUNT) {
            keys.push((make_int_key(i), u32::MAX));
        }
    }
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
            if !is_symbol_key(shape.property_name) && !is_private_name_key(shape.property_name) {
                // 绝对存储索引：数组命名属性位于元素区之后。
                let store = if obj.is_array() { obj.array_prop_count + pos } else { pos };
                keys.push((shape.property_name, store));
            }
        }
        pos += 1;
    }
    // 类构造器属性序修正：shape 链构建序为 prototype→length→name（叶→根），
    // 规范要求的插入序为 length→name→prototype。按规范重排字符串键。
    if obj.is_class_constructor() {
        let perm = vm.kernel_core().perm_interner();
        let canonical = ["length", "name", "prototype"];
        let mut canonical_map: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, &name) in canonical.iter().enumerate() {
            canonical_map.insert(name, i);
        }
        // 按规范序重新排列字符串键，整数键保持原位。
        keys.sort_by(|(a_si, _), (b_si, _)| {
            let a_idx = int_or_string_index(vm, *a_si);
            let b_idx = int_or_string_index(vm, *b_si);
            match (a_idx, b_idx) {
                (Some(ai), Some(bi)) => ai.cmp(&bi),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => {
                    // 两者均为字符串键：查规范序。
                    let a_name = perm.lookup(*a_si).unwrap_or("");
                    let b_name = perm.lookup(*b_si).unwrap_or("");
                    let a_rank = canonical_map.get(a_name).copied().unwrap_or(usize::MAX);
                    let b_rank = canonical_map.get(b_name).copied().unwrap_or(usize::MAX);
                    // 规范键按序排，其余保持原序。
                    a_rank.cmp(&b_rank)
                }
            }
        });
    } else {
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
    }
    keys
}

/// 取键 si（interned 字符串键编号）对应的数组下标值：整数键直接反解，字符串键查
/// interner 后判是否规范数字串。
fn int_or_string_index<H: VmHost>(vm: &H, si: u32) -> Option<u32> {
    if is_int_key(si) {
        return Some(int_key_value(si));
    }
    let key = vm.kernel_core().perm_interner().lookup(si)?;
    is_integer_index(key).then(|| key.parse::<u32>().ok()).flatten()
}

/// 键 si 构造为 JS 可见字符串值（Object.keys / ownKeys / entries 族）：
/// 整数键反解数字串，字符串键经 `decode_key` 还原单元序列（孤立 surrogate
/// 构造为 FlatU16；良形键的输出为良形 UTF-8 字符串，与直接 `to_string` 结果一致）。
pub fn key_si_to_js_value<H: VmHost>(vm: &mut H, si: u32) -> JsValue {
    if is_int_key(si) {
        return vm.new_string(&int_key_value(si).to_string());
    }
    let units = vm
        .kernel_core()
        .perm_interner()
        .lookup(si)
        .map(oxide_kernel::string_forge::decode_key)
        .unwrap_or_default();
    vm.new_string_units_owned(units)
}

/// 收集对象自身全部 Symbol 键（shape 链），按键序排列（根→叶，即插入序）。
/// 与 [`walk_own_keys`] 互补：只返回 Symbol 键。返回 `(属性键 si, 绝对存储索引)`：
/// 槽位计数须含全部非空节点（含字符串键）并对数组加元素区偏移，与物理存储及
/// [`walk_own_keys`] 的口径一致，消费方（delete 重建）才能按槽位取回正确值。
pub(crate) fn walk_own_symbol_keys<H: VmHost>(vm: &H, obj: &JsObject) -> Vec<(u32, u32)> {
    let mut keys: Vec<(u32, u32)> = Vec::new();
    let shape_id = obj.shape_id();
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(id) {
            cursor = shape.parent;
            if shape.property_name != u32::MAX {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    for (pos, id) in (0_u32..).zip(shape_ids.iter().rev()) {
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(*id) {
            // Symbol 键节点按绝对槽位回传（数组命名属性位于元素区之后）。
            let store = if obj.is_array() { obj.array_prop_count + pos } else { pos };
            if is_symbol_key(shape.property_name) {
                keys.push((shape.property_name, store));
            }
        }
    }
    keys
}

/// 把 Symbol 键反解为对应的 Symbol 值：well-known 键还原为对应下标的符号原语，
/// 用户 symbol 键按偏移还原为 `JsValue::symbol` 值。
fn decode_symbol_key(key: u32) -> JsValue {
    if let Some(id) = well_known_symbol_id_from_key(key) {
        return JsValue::symbol(id);
    }
    JsValue::symbol(symbol_index_from_key(key))
}

/// 收集对象自身全部 Symbol 键并按插入序物化为 Symbol 值。
///
/// 供 `Object.getOwnPropertySymbols` 与 `Reflect.ownKeys` 的 Symbol 段共用；
/// 与 [`walk_own_keys`] 互补，后者只返回字符串键与整数键。
///
/// # 边界与前提
/// - 无自身 Symbol 键时返回空向量；数组元素区不含 Symbol 键。
///
/// # 注意事项
/// - 只读 shape 链，不分配对象；well-known symbol 物化为符号原语。
pub fn own_symbol_key_values<H: VmHost>(vm: &H, obj: &JsObject) -> Vec<JsValue> {
    walk_own_symbol_keys(vm, obj)
        .iter()
        .map(|(key, _)| decode_symbol_key(*key))
        .collect()
}

/// 按规范 [[OwnPropertyKeys]] 重排命名空间导出：字符串键按 UTF-16 单元序升序，
/// Symbol 键（`@@toStringTag`）保持在其后。
///
/// 命名空间导出在模块 body 执行期按语句序写入，而规范要求导出键有序。重建 shape
/// 链使插入序即枚举序，字符串枚举消费端（getOwnPropertyNames / Reflect.ownKeys /
/// for-in / Object.keys / JSON）无需各自特判。
///
/// # 边界与前提
/// - 仅对模块命名空间对象调用；无自身字符串键时只保留 Symbol 键。
///
/// # 副作用
/// - 重写 obj 的 shape 链与属性表并 bump generation；对象 identity、属性值与
///   描述符均不变。
pub fn sort_namespace_exports<H: VmHost>(vm: &mut H, obj: &mut JsObject) {
    // 收集字符串导出与其值/描述符；Symbol 键不参与字符串排序，收集后原序追加。
    let mut entries: Vec<(u32, JsValue, Option<PropMetaEntry>)> = walk_own_keys(vm, obj)
        .into_iter()
        .map(|(si, pos)| (si, obj.get_prop_at(pos), obj.prop_meta_at(pos)))
        .collect();
    let symbols: Vec<(u32, JsValue, Option<PropMetaEntry>)> = walk_own_symbol_keys(vm, obj)
        .into_iter()
        .map(|(si, pos)| (si, obj.get_prop_at(pos), obj.prop_meta_at(pos)))
        .collect();

    // 排序口径 = UTF-16 单元序（BMP 外字符的单元序与码点序不同）。
    entries.sort_by_cached_key(|(si, _, _)| namespace_key_units(vm, *si));

    // 重建 shape 链与属性表：插入序 = 枚举序。
    obj.set_shape_id(EMPTY_SHAPE_ID);
    obj.clear_props();
    for (si, value, meta) in entries.into_iter().chain(symbols) {
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
    obj.bump_generation();
}

/// 命名空间导出键的排序单元序列：字符串键取 UTF-16 单元，整数键取十进制文本。
fn namespace_key_units<H: VmHost>(vm: &H, si: u32) -> Vec<u16> {
    if is_int_key(si) {
        return int_key_value(si).to_string().encode_utf16().collect();
    }
    vm.kernel_core()
        .perm_interner()
        .lookup(si)
        .map(|key| key.encode_utf16().collect())
        .unwrap_or_default()
}

/// `Object.getOwnPropertySymbols(obj)`：返回全部自身 Symbol 键数组。
pub fn object_get_own_property_symbols<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = match require_obj_arg(vm, args, "getOwnPropertySymbols") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let symbols: Vec<JsValue> = {
        let obj = unsafe { &*obj_ptr };
        own_symbol_key_values(vm, obj)
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

/// 自身属性删除结果三态：区分「属性缺失 / 已删除 / 不可配置」，让共享实现同时
/// 服务 delete 运算符（不可配置在严格模式须抛 TypeError）与
/// `Reflect.deleteProperty`（恒投影为布尔、不抛错）两个消费者。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeleteOutcome {
    /// 属性不在对象自身（含原型链属性）：删除按规范成功。
    Missing,
    /// 属性存在且已删除。
    Deleted,
    /// 属性存在但不可配置：删除失败。
    NonConfigurable,
}

/// 删除对象自身属性并返回三态结果（字节码 delete 与 Reflect.deleteProperty 共用）。
///
/// 数组下标键在元素区（shape 链外），标记为 hole（值 undefined + hole meta），
/// length 不变；数组 length 虚拟属性与命名属性经 shape 链处理。仅对象自身属性
/// 参与删除判定，原型链属性不影响结果。
///
/// # 步骤
/// 1. TA 数值键：界内索引返回 `NonConfigurable`（元素零修改）；数字无效键
///    返回 `Missing`；非数字串 / symbol 键落下方普通路径
/// 2. 数组 length 虚拟属性：不可配置，返回 `NonConfigurable`
/// 3. 数组下标元素：检查 configurable，`mark_hole_at` 标记为 hole
/// 4. 命名属性：walk_own_keys 定位槽位，不可配置返回 `NonConfigurable`
/// 5. 数组先保存元素区（值 + meta），重建命名属性后恢复元素区
///
/// # 边界与前提
/// - 键不在对象自身（含原型链属性）返回 `Missing`
/// - 非 configurable 属性返回 `NonConfigurable`
/// - TA 键 live 长 0（detach / 收缩越界）时全数值键归数字无效，删除成功
/// - `key_si` 须已 intern
///
/// # 副作用
/// - 修改 obj 的 shape_id、属性表与 generation；数组元素区内容不变
/// - TA 数字无效臂同步全局内置镜像槽为 undefined（非全局内置 TA 上为 no-op）
///
/// # 注意事项
/// - 数组元素存在性以 `prop_meta_at` 的 hole 标记判定，删除后重新写入元素
///   会自动清除 hole 标记恢复存在
pub fn delete_own_property_outcome<H: VmHost>(vm: &mut H, obj: &mut JsObject, key_si: u32) -> DeleteOutcome {
    // TA 数值键：界内索引不可配置（删除失败，元素零修改、镜像槽不同步）；
    // 数字无效键视为缺失（删除成功，镜像槽同步 undefined，对非全局内置 TA
    // 为 no-op）；非数字串 / symbol 键落下方普通属性路径。live 长 0
    // （detach / 收缩越界）时全数值键自然归数字无效，与读 / 写 / 定义面同口径。
    if obj.is_typed_array_obj() {
        match crate::typed_array::ta_index_gate(vm, obj, key_si) {
            crate::typed_array::TaIndexGate::NumericValid(_) => return DeleteOutcome::NonConfigurable,
            crate::typed_array::TaIndexGate::NumericInvalid => {
                vm.sync_global_builtin_mirror(obj, key_si, JsValue::undefined());
                return DeleteOutcome::Missing;
            }
            crate::typed_array::TaIndexGate::Ordinary => {}
        }
    }

    // 数组下标元素在元素区，不参与 shape 链，单独删除（保持 length 不变）。
    if obj.is_array() {
        if let Some(index) = array_index_of(vm, key_si) {
            if index < obj.array_prop_count {
                let meta = obj.prop_meta_at(index);
                // 已是 hole 视为不存在；非 configurable 不可删。
                if meta.is_some_and(|m| m.is_hole()) {
                    return DeleteOutcome::Missing;
                }
                if meta.is_some_and(|m| !m.attributes.configurable()) {
                    return DeleteOutcome::NonConfigurable;
                }
                obj.mark_hole_at(index);
                obj.bump_generation();
                return DeleteOutcome::Deleted;
            }
            return DeleteOutcome::Missing;
        }
        // 非下标键：length 是虚拟属性（无 shape 槽、不在元素区），但描述符声明
        // configurable:false，删除恒失败。
        if key_si == vm.kernel_core().perm_interner().intern("length").0 {
            return DeleteOutcome::NonConfigurable;
        }
    }

    // walk_own_keys 只返回字符串键；Symbol 键（well-known/用户）需另行定位。
    let keys = walk_own_keys(vm, obj);
    let mut all_keys = keys;
    if is_symbol_key(key_si) {
        all_keys.extend(walk_own_symbol_keys(vm, obj));
    }
    let Some(delete_pos) = all_keys.iter().find(|(si, _)| *si == key_si).map(|(_, pos)| *pos) else {
        // 属性缺失：delete 按规范返 true，镜像槽同步 undefined 保持
        // "槽 = A 侧原始存储"不变式（与入口预载缺位语义幂等）。
        vm.sync_global_builtin_mirror(obj, key_si, JsValue::undefined());
        return DeleteOutcome::Missing;
    };
    // walk_own_keys 已返回绝对存储索引（数组含元素区偏移），直接使用。
    if obj
        .prop_meta_at(delete_pos)
        .map(|meta| !meta.attributes.configurable())
        .unwrap_or(false)
    {
        return DeleteOutcome::NonConfigurable;
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
    // 删除成功：镜像槽同步 undefined（成员形删除 / Reflect.deleteProperty /
    // 0x9C 清槽同源收口，后两者对此幂等）。
    vm.sync_global_builtin_mirror(obj, key_si, JsValue::undefined());
    DeleteOutcome::Deleted
}

/// 删除对象自身属性并投影为布尔（`Reflect.deleteProperty` 语义）：成功返回 true，
/// 属性不可配置返回 false，恒不抛错。
pub fn delete_own_property<H: VmHost>(vm: &mut H, obj: &mut JsObject, key_si: u32) -> bool {
    delete_own_property_outcome(vm, obj, key_si) != DeleteOutcome::NonConfigurable
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
        obj_ref.set_boxed_value(val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_string() {
        let proto = vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_STRING_OBJ;
        // 构造期物化：boxed_value 载荷与字符索引/length 固有属性同批落地。
        oxide_runtime_api::materialize_string_box(vm, obj_ref, val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_bool() {
        let proto = vm.session().builtin_world().boolean_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_BOOLEAN_OBJ;
        obj_ref.set_boxed_value(val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_symbol() {
        let proto = vm.session().builtin_world().symbol_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_SYMBOL_OBJ;
        obj_ref.set_boxed_value(val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if val.is_bigint() {
        let proto = vm.session().builtin_world().bigint_proto.as_ptr() as *mut JsObject;
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.set_boxed_value(val);
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// 模块命名空间字符串导出按 exotic `[[GetOwnProperty]]` 读取活值状态：
/// - `Ok(Some(v))`：该键是已初始化导出，`v` 为条目活值；
/// - `Ok(None)`：非 module namespace / 非导出名 / symbol 键，调用方落普通属性路径；
/// - `Err(msg)`：导出名已预注册但未初始化，调用方抛 `msg` 对应的 ReferenceError。
///
/// # 注意事项
/// - 只查条目表、不读真实属性槽：条目表是 live 命名空间的权威状态，真实槽可能
///   只是初始化前的 `undefined` 占位。
fn namespace_export_get(obj: &JsObject, key_si: u32) -> Result<Option<JsValue>, &'static str> {
    if is_symbol_key(key_si) {
        return Ok(None);
    }
    match crate::module::module_ns_export(obj, key_si) {
        None => Ok(None),
        Some(crate::module::ModuleNsQuery::Initialized(v)) => Ok(Some(v)),
        Some(crate::module::ModuleNsQuery::Uninitialized) => Err(crate::module::NS_UNINITIALIZED_MESSAGE),
    }
}

/// values/entries 族单自身属性值读（EnumerableOwnProperties 的 Get）：模块命名空间
/// 导出走活值查询（未初始化抛 ReferenceError）；访问器属性触发 getter（this = 对象
/// 自身），异常传播原始抛出值；数据属性直读存储槽。TA 元素键无存储槽：live 界判
/// （含 detach/收缩）直读元素值，访问器语义不适用。
fn own_property_value<H: VmHost>(
    vm: &mut H, obj: &JsObject, obj_val: JsValue, si: u32, offset: u32,
) -> Result<JsValue, JsValue> {
    // TA 元素键无存储槽（元素住 buffer）：live 界判含 detach/收缩，直读元素值。
    if obj.is_typed_array_obj() && is_int_key(si) {
        return Ok(
            crate::typed_array::typed_array_element_get(vm, obj, int_key_value(si)).unwrap_or(JsValue::undefined())
        );
    }
    match namespace_export_get(obj, si) {
        Ok(Some(value)) => return Ok(value),
        Ok(None) => {}
        Err(msg) => return Err(crate::error::create_reference_error(vm, msg)),
    }
    if obj.is_accessor_meta(offset) {
        match vm.ordinary_get(obj, si, obj_val) {
            Ok(value) => return Ok(value),
            Err(msg) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                return Err(exc);
            }
        }
    }
    Ok(obj.get_prop_at(offset))
}

/// `Object.keys(obj)`：返回可枚举自身属性的字符串名数组。
pub fn object_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = match require_obj_arg(vm, args, "keys") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let obj = unsafe { &*obj_ptr };
    let owned_keys: Vec<(u32, u32)> = walk_own_keys(vm, obj)
        .into_iter()
        .filter(|(_si, offset)| {
            obj.prop_meta_at(*offset)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
        })
        .collect();
    // EnumerableOwnProperties 对每个字符串键走 `? [[GetOwnProperty]]`：未初始化导出抛错。
    for (si, _offset) in &owned_keys {
        if let Err(msg) = namespace_export_get(obj, *si) {
            return NativeResult::Err(crate::error::create_reference_error(vm, msg));
        }
    }
    let n = owned_keys.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (si, _offset)) in owned_keys.iter().enumerate() {
        let key_val = key_si_to_js_value(vm, *si);
        unsafe {
            (*arr).set_prop_at(i, key_val);
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
                // 强转期用户异常原值重抛；define 失败文本按 kind 前缀恢复原类型。
                if let Some(exc) = vm.take_pending_length_exception() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_define_failure(vm, &msg));
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
            // 强转期用户异常原值重抛；define 失败文本按 kind 前缀恢复原类型。
            if let Some(exc) = vm.take_pending_length_exception() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_define_failure(vm, &msg));
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
        // null/undefined 源静默跳过（语料口径，非规范 ToObject 抛错）；其余
        // 基元经 ToObject 装箱后走同一 walk（string 源自此获字符属性拷贝）。
        let source_val = match oxide_runtime_api::to_object(vm.reg(arg_reg), vm) {
            Ok(v) => v,
            Err(_) => continue,
        };
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
                // ReferenceError 等错误文本经 create_from_text 恢复类型，不得降级为 TypeError。
                Err(err) => return NativeResult::Err(crate::error::create_from_text(vm, &err)),
            };
            all_assignments.push((si, val));
        }
    }

    let target = unsafe { &mut *target_ptr };
    for (si, val) in all_assignments {
        let promoted = vm.promote_if_needed_for_write_ptr(target_ptr, val);
        // Set(to, key, value, true)：receiver 为目标对象（目标同名 setter 的 this 指向 target）。
        if let Err(err) = vm.ordinary_set(target, si, promoted, target_val, true) {
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
pub(crate) fn define_from_descriptor<H: VmHost>(
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
    let enumerable_field = own_field(vm, desc_val, enumerable_si);
    let configurable_field = own_field(vm, desc_val, configurable_si);

    // 数组 length 是无 shape 槽的虚拟数据属性：需与普通已有属性一样参与描述符
    // 缺省回填（writable 保持当前值、value 保持当前长度），其当前描述符由元素
    // 计数与独立可写位虚拟构造；enumerable/configurable 恒为 false。
    let length_si = vm.kernel_core().perm_interner().intern("length").0;
    let is_array_length = unsafe { &*obj_ptr }.is_array() && key_si == length_si;

    let existing_pos = if is_array_length {
        None
    } else {
        let obj = unsafe { &*obj_ptr };
        vm.get_own_property_slot(obj, key_si)
    };
    let existing_meta = if is_array_length {
        let writable = unsafe { &*obj_ptr }.is_length_writable();
        Some(oxide_types::object::PropMetaEntry::data(PropAttributes::new(writable, false, false)))
    } else {
        existing_pos.map(|pos| {
            let obj = unsafe { &*obj_ptr };
            // 已有属性无显式 meta（普通数据属性/数组元素）时按默认属性回填：
            // 描述符缺省字段保持现有值（writable/enumerable/configurable 均 true），
            // 而非按新属性处理为 false（Object.defineProperty 省略字段不改已有属性）。
            obj.prop_meta_at(pos)
                .unwrap_or_else(|| oxide_types::object::PropMetaEntry::data(PropAttributes::DEFAULT_DATA))
        })
    };
    let has_existing = existing_pos.is_some() || is_array_length;

    // 模块命名空间 exotic [[DefineOwnProperty]]（规范 10.4.6.5）：Symbol 键委托
    // 普通语义；导出键仅接受无变更、值不变或 writable:true，值变与 writable:false
    // 拒绝；非导出键由对象不可扩展在后续 define 处拒绝。enumerable/configurable
    // 与 accessor 的收窄由 define_* 的不可配置守卫承担。
    if unsafe { &*obj_ptr }.is_module_namespace() && !is_symbol_key(key_si) {
        let Some(pos) = existing_pos else {
            return Err("Cannot define property on module namespace: not an export".to_string());
        };
        if writable_field.is_some_and(|w| !oxide_runtime_api::to_boolean(w)) {
            return Err("Cannot redefine module namespace export".to_string());
        }
        if let Some(v) = value_field {
            let obj = unsafe { &*obj_ptr };
            if !oxide_runtime_api::same_value(v, obj.get_prop_at(pos)) {
                return Err("Cannot redefine module namespace export".to_string());
            }
        }
    }

    // 修改已有属性时缺省字段回填现有值，仅定义新属性时缺省才为 false。
    let enumerable = enumerable_field
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or_else(|| existing_meta.map(|m| m.attributes.enumerable()).unwrap_or(false));
    let configurable = configurable_field
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or_else(|| existing_meta.map(|m| m.attributes.configurable()).unwrap_or(false));

    let has_data = value_field.is_some() || writable_field.is_some();
    let has_accessor = get_field.is_some() || set_field.is_some();
    if has_data && has_accessor {
        return Err("Invalid property descriptor: cannot mix data and accessor fields".to_string());
    }

    let existing_value = if is_array_length {
        unsafe { &*obj_ptr }.logical_len_value()
    } else {
        existing_pos.map_or(JsValue::undefined(), |pos| {
            let obj = unsafe { &*obj_ptr };
            obj.get_prop_at(pos)
        })
    };

    // 访问器描述符：解析 get/set（缺失字段回填现有属性）并做可调用性校验——
    // 规范步序先于索引/约束检查。
    let (get, set) = if has_accessor {
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
        (get, set)
    } else {
        (JsValue::undefined(), JsValue::undefined())
    };

    // TA 数值索引臂：置于混入/可调用检查之后、define 路由之前。字段在场四检查
    // 取原始描述符字段（非缺省回填后的属性位）；有效索引值臂缺省读当前元素
    // （通用描述符）；数字无效索引零副作用直接失败；非规范数字串 / symbol 键
    // 落真实自有属性路径。
    if unsafe { &*obj_ptr }.is_typed_array_obj() {
        match crate::typed_array::ta_index_gate(vm, unsafe { &*obj_ptr }, key_si) {
            crate::typed_array::TaIndexGate::NumericValid(index) => {
                if has_accessor
                    || writable_field.is_some_and(|w| !oxide_runtime_api::to_boolean(w))
                    || enumerable_field.is_some_and(|e| !oxide_runtime_api::to_boolean(e))
                    || configurable_field.is_some_and(|c| !oxide_runtime_api::to_boolean(c))
                {
                    return Err(
                        "cannot define property: TypedArray index only accepts a writable, enumerable, configurable data descriptor"
                            .to_string(),
                    );
                }
                let value = value_field.unwrap_or_else(|| {
                    crate::typed_array::typed_array_element_get(vm, unsafe { &*obj_ptr }, index)
                        .unwrap_or(JsValue::undefined())
                });
                return crate::typed_array::typed_array_element_define(vm, unsafe { &mut *obj_ptr }, index, value);
            }
            crate::typed_array::TaIndexGate::NumericInvalid => {
                return Err("cannot define property: TypedArray index out of range".to_string());
            }
            crate::typed_array::TaIndexGate::Ordinary => {}
        }
    }

    let obj = unsafe { &mut *obj_ptr };
    if has_accessor {
        vm.define_accessor_property(obj, key_si, get, set, PropAttributes::new(false, enumerable, configurable))?;
    } else if has_data {
        let value = if has_existing {
            value_field.unwrap_or(existing_value)
        } else {
            value_field.unwrap_or(JsValue::undefined())
        };
        let writable = if has_existing {
            writable_field
                .map(oxide_runtime_api::to_boolean)
                .unwrap_or_else(|| existing_meta.map(|m| m.attributes.writable()).unwrap_or(false))
        } else {
            writable_field.map(oxide_runtime_api::to_boolean).unwrap_or(false)
        };
        vm.define_data_property(obj, key_si, value, PropAttributes::new(writable, enumerable, configurable))?;
    } else {
        if !has_existing {
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

/// 把 properties 实参按 ToObject 装箱后遍历其可枚举自身属性，把每个值当作
/// 描述符定义到目标对象上。`Object.defineProperties` 与
/// `Object.create(proto, properties)` 共用。
///
/// # 步骤
/// 1. ToObject 装箱 properties（null/undefined 抛 TypeError，包装盒无键则零定义）
/// 2. 先对全部可枚举键完成描述符值收集（触发全部 getter），再逐键定义——
///    先取后写，避免前键定义影响后键读取
///
/// # 边界与前提
/// - 仅处理可枚举自身属性；描述符值经 ordinary_get 读取（触发访问器 getter）
/// - 描述符值的非对象校验由 define_from_descriptor 承担（ToPropertyDescriptor）
/// - String 盒装箱后自身键为物化的字符下标（值为对应字符），走同一
///   walk 路径；字符值非对象，ToPropertyDescriptor 按规范抛 TypeError
///
/// # 副作用
/// - 修改 target 的 shape 链、属性表与 generation
fn define_all_from_properties<H: VmHost>(
    vm: &mut H, target_ptr: *mut JsObject, props_val: JsValue,
) -> Result<(), String> {
    // ToObject：原始值装箱；null/undefined 的 TypeError 文本经调用方 kind
    // 前缀恢复后原类型抛出。
    let props_obj = oxide_runtime_api::to_object(props_val, vm)?;
    let props_ptr = props_obj.as_js_object_ptr();
    let descriptors: Vec<(u32, JsValue)> = {
        let props = unsafe { &*props_ptr };
        walk_own_keys(vm, props)
            .into_iter()
            // 非可枚举自身属性跳过；无显式 meta 视为可枚举（普通字面量默认）。
            .filter(|(_si, offset)| !props.prop_meta_at(*offset).map(|m| !m.attributes.enumerable()).unwrap_or(false))
            .map(|(key_si, _offset)| {
                let props = unsafe { &*props_ptr };
                vm.ordinary_get(props, key_si, props_obj).map(|desc_val| (key_si, desc_val))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    for (key_si, desc_val) in descriptors {
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
    // target 非对象直接抛 TypeError（不做 ToObject 装箱）。
    let obj_val = vm.reg(args[1]);
    if !obj_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.defineProperty called on non-object"));
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    // well-known symbol 等特殊键统一走 property_key_si（映射到各自的 Symbol 键），
    // 保证与计算属性访问、Reflect.defineProperty 等读键路径一致。
    let si = vm.property_key_si(vm.reg(args[2]));

    if let Err(msg) = define_from_descriptor(vm, obj_ptr, si, vm.reg(args[3])) {
        // 强转期用户代码（valueOf / Symbol.toPrimitive）抛出的异常值转存专用槽，
        // 须原值重抛而非改写成引擎 TypeError。
        if let Some(exc) = vm.take_pending_length_exception() {
            return NativeResult::Err(exc);
        }
        return NativeResult::Err(crate::error::create_define_failure(vm, &msg));
    }
    NativeResult::Ok(obj_val)
}

/// 取单个自身属性的描述符对象（gOPD / gOPDs 共享核）：按 ToPropertyDescriptor
/// 形态构造描述符对象，proto = %Object.prototype%。
///
/// # 边界与前提
/// - 键不在对象自身返回 `Ok(None)`
/// - 模块命名空间未初始化导出返回 `Err`（ReferenceError，原值传播）
/// - TypedArray 界内整数键返回常量四字段描述符（writable/enumerable/
///   configurable 恒真，不读 buffer 状态）；数字无效与非数字串键落槽位路径
///
/// # 副作用
/// - 在 epoch 分配一个描述符对象
fn own_descriptor_of<H: VmHost>(vm: &mut H, obj: &JsObject, key_si: u32) -> Result<Option<JsValue>, JsValue> {
    // TA 数值键：界内索引返回元素值 + 常量四字段描述符（writable/enumerable/
    // configurable 恒真）；数字无效与非数字串键落下方槽位路径（真实 own 属性
    // 原样返回，无属性则 None）。
    if obj.is_typed_array_obj() {
        if let crate::typed_array::TaIndexGate::NumericValid(index) = crate::typed_array::ta_index_gate(vm, obj, key_si)
        {
            let value = crate::typed_array::typed_array_element_get(vm, obj, index).unwrap_or(JsValue::undefined());
            let desc = alloc_desc_object(vm);
            let sh_ptr = vm.kernel_core().shape_forge().as_ref() as *const ShapeForge;
            let sf_ptr = vm.kernel_core().perm_interner().as_ref() as *const PermInterner;
            // SAFETY: desc 为本函数刚分配的 epoch 对象；sh/sf 为 kernel 永久引用。
            let d: &mut JsObject = unsafe { &mut *desc };
            let sh = unsafe { &*sh_ptr };
            let sf = unsafe { &*sf_ptr };
            push_desc_prop(d, sh, sf.intern("value").0, value);
            push_desc_prop(d, sh, sf.intern("writable").0, JsValue::bool(true));
            push_desc_prop(d, sh, sf.intern("enumerable").0, JsValue::bool(true));
            push_desc_prop(d, sh, sf.intern("configurable").0, JsValue::bool(true));
            return Ok(Some(JsValue::from_js_object(desc)));
        }
    }

    // 数组 length 是虚拟属性（无 shape 槽，ordinary_get 直接返回逻辑长度）：
    // 描述符 {value: len, writable: !frozen && 非显式收窄, enumerable: false,
    // configurable: false}。冻结数组与经 defineProperty 收窄的数组 writable=false。
    let length_si = vm.kernel_core().perm_interner().intern("length").0;
    if obj.is_array() && key_si == length_si {
        let desc = alloc_desc_object(vm);
        let sh_ptr = vm.kernel_core().shape_forge().as_ref() as *const ShapeForge;
        let sf_ptr = vm.kernel_core().perm_interner().as_ref() as *const PermInterner;
        let d: &mut JsObject = unsafe { &mut *desc };
        let sh = unsafe { &*sh_ptr };
        let sf = unsafe { &*sf_ptr };
        push_desc_prop(d, sh, sf.intern("value").0, obj.logical_len_value());
        push_desc_prop(d, sh, sf.intern("writable").0, JsValue::bool(obj.is_length_writable()));
        push_desc_prop(d, sh, sf.intern("enumerable").0, JsValue::bool(false));
        push_desc_prop(d, sh, sf.intern("configurable").0, JsValue::bool(false));
        return Ok(Some(JsValue::from_js_object(desc)));
    }

    let Some(offset) = vm.get_own_property_slot(obj, key_si) else {
        return Ok(None);
    };
    // 模块命名空间字符串导出：`? [[Get]]` 读条目活值，未初始化抛 ReferenceError。
    let found_value = match namespace_export_get(obj, key_si) {
        Ok(Some(value)) => value,
        Ok(None) => obj.get_prop_at(offset),
        Err(msg) => return Err(crate::error::create_reference_error(vm, msg)),
    };
    let found_meta = obj.prop_meta_at(offset);

    let desc = alloc_desc_object(vm);
    let sh_ptr = vm.kernel_core().shape_forge().as_ref() as *const ShapeForge;
    let sf_ptr = vm.kernel_core().perm_interner().as_ref() as *const PermInterner;
    let d: &mut JsObject = unsafe { &mut *desc };
    let sh = unsafe { &*sh_ptr };
    let sf = unsafe { &*sf_ptr };
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
    Ok(Some(JsValue::from_js_object(desc)))
}

/// 分配空描述符对象（proto = %Object.prototype%）。
fn alloc_desc_object<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let desc_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(desc_proto)))
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
    match own_descriptor_of(vm, obj, key) {
        Ok(Some(desc)) => NativeResult::Ok(desc),
        Ok(None) => NativeResult::Ok(JsValue::undefined()),
        Err(exc) => NativeResult::Err(exc),
    }
}

/// `Object.getOwnPropertyDescriptors(obj)`：返回全部自身属性描述符集合对象
/// （键 = 自身属性名，值 = 对应描述符对象）。
///
/// # 步骤
/// 1. ToObject 装箱接收者（null/undefined 抛 TypeError）
/// 2. 按规范三段序枚举自身键（walk_own_keys + walk_own_symbol_keys）；
///    String 盒装箱后自身键为物化的字符下标 + length，走同一路径
/// 3. 逐键取描述符（缺失键跳过），以数据属性定义到结果对象
///
/// # 边界与前提
/// - 结果对象 proto = %Object.prototype%，各键属性恒 writable/enumerable/
///   configurable 全真（fresh 普通对象的 CreateDataPropertyOrThrow 形态）
/// - 描述符取值与 gOPD 同核；模块命名空间未初始化导出抛 ReferenceError 原值
///
/// # 副作用
/// - 分配结果对象与每个描述符对象
pub fn object_get_own_property_descriptors<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = match require_obj_arg(vm, args, "getOwnPropertyDescriptors") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let obj = unsafe { &*obj_ptr };

    // 三段自身键序：字符串/整数键（整键升序在前）后接 Symbol 键插入序。
    let mut keys = walk_own_keys(vm, obj);
    keys.extend(walk_own_symbol_keys(vm, obj));

    // 结果对象：proto = %Object.prototype%。
    let desc_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let result = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(desc_proto)));
    let result_val = JsValue::from_js_object(result);

    for (si, _pos) in keys {
        // 描述符核：缺失键跳过；未初始化导出原值传播。
        let desc = match own_descriptor_of(vm, obj, si) {
            Ok(Some(desc)) => desc,
            Ok(None) => continue,
            Err(exc) => return NativeResult::Err(exc),
        };
        // CreateDataPropertyOrThrow：结果对象为 fresh 普通对象，属性恒全真。
        if let Err(err) = vm.define_data_property(unsafe { &mut *result }, si, desc, PropAttributes::DEFAULT_DATA) {
            return NativeResult::Err(crate::error::create_type_error(vm, &err));
        }
    }
    NativeResult::Ok(result_val)
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

/// 校验并取出 Object 类方法的目标对象：`args[0]` 为接收者，`args[1]` 为待转换值，
/// 缺参或值经 ToObject 失败（null/undefined）时抛 TypeError。
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
/// - TA 元素键无 meta 槽，循环跳键（元素零修改）
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
        let obj = unsafe { &*obj_ptr };
        // 模块命名空间导出保持 writable:true，冻结须失败（规范 SetIntegrityLevel）。
        // 无字符串导出的空命名空间没有可写属性，允许冻结成功。
        if obj.is_module_namespace() && !walk_own_keys(vm, obj).is_empty() {
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot freeze module namespace"));
        }
    }
    {
        let obj = unsafe { &mut *obj_ptr };
        // 命名属性：walk_own_keys 返回绝对存储索引（数组含元素区偏移）。
        let keys = walk_own_keys(vm, obj);
        for (si, pos) in keys {
            // TA 元素键无 meta 槽（元素住 buffer）：完整性处理不触元素，跳键免
            // set_meta_at 越界扩属性表。
            if obj.is_typed_array_obj() && is_int_key(si) {
                continue;
            }
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
/// - TA 元素键无 meta 槽，循环跳键（元素零修改）
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
        for (si, pos) in keys {
            // TA 元素键无 meta 槽（元素住 buffer）：完整性处理不触元素，跳键免
            // set_meta_at 越界扩属性表。
            if obj.is_typed_array_obj() && is_int_key(si) {
                continue;
            }
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
    for (si, offset) in keys {
        // 同款跳键：元素键描述符 configurable:true，PE/sealed 的 TA 判 sealed
        // （V8 同口径）。
        if obj.is_typed_array_obj() && is_int_key(si) {
            continue;
        }
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

    let obj = unsafe { &*obj_ptr };
    let keys = walk_own_keys(vm, obj);
    let n = keys.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (si, _)) in keys.iter().enumerate() {
        let key_val = key_si_to_js_value(vm, *si);
        unsafe {
            (*arr).set_prop_at(i, key_val);
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
    // target 非对象直接抛 TypeError（不做 ToObject 装箱）。
    let target_val = vm.reg(args[1]);
    if !target_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.defineProperties called on non-object"));
    }
    let target_ptr = target_val.as_js_object_ptr();
    if let Err(msg) = define_all_from_properties(vm, target_ptr, vm.reg(args[2])) {
        // 与单属性入口一致：强转期用户异常原值重抛，非法 length 以 RangeError
        // kind 穿透，其余 define 失败投影为 TypeError。
        if let Some(exc) = vm.take_pending_length_exception() {
            return NativeResult::Err(exc);
        }
        return NativeResult::Err(crate::error::create_define_failure(vm, &msg));
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
        // 写入失败按 builtin 边界传播（目标为 fresh 对象实际不可达，不得静默吞）。
        if let Err(err) = vm.ordinary_set(unsafe { &mut *obj }, si, promoted, target_val, true) {
            return NativeResult::Err(crate::array::from_engine_error(vm, &err));
        }
    }
    NativeResult::Ok(target_val)
}

/// `Object.getPrototypeOf(obj)`：返回对象的 prototype。
pub fn object_get_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "getPrototypeOf"));
    NativeResult::Ok(unsafe { (*obj_ptr).proto() })
}

/// `Object.setPrototypeOf(obj, proto)`：设置对象的 prototype，返回原值。
///
/// # 边界与前提
/// - `obj` 为 null/undefined 抛 TypeError（RequireObjectCoercible）；原始值原样返回，
///   不装箱。
/// - `proto` 既非对象也非 null 时抛 TypeError。
/// - 目标不可设置（原型环，或不可扩展且新旧原型不同）时抛 TypeError；与 Reflect
///   版按布尔返回的语义不同。
///
/// # 副作用
/// - 目标为对象且设置成功时改写其 proto 槽并递增 generation；新旧原型相同
///   （SameValue）时按规范直接成功。
pub fn object_set_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = args.get(1).map(|reg| vm.reg(*reg)).unwrap_or_else(JsValue::undefined);
    if target_val.is_null() || target_val.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.setPrototypeOf called on null or undefined",
        ));
    }
    let proto = args.get(2).map(|reg| vm.reg(*reg)).unwrap_or_else(JsValue::undefined);
    if !proto.is_object() && !proto.is_null() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.setPrototypeOf: prototype must be an object or null",
        ));
    }
    if !target_val.is_object() {
        return NativeResult::Ok(target_val);
    }
    let target_ptr = target_val.as_js_object_ptr();
    let target = unsafe { &mut *target_ptr };
    // OrdinarySetPrototypeOf 步 2-4：新旧原型 SameValue 时直接成功，可扩展标志
    // 不参与判断；不可扩展且新旧不同才失败。set_proto 是裸写槽操作，不含该判定。
    if !target.is_extensible() && target.proto() != proto {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.setPrototypeOf: cannot set prototype of non-extensible object",
        ));
    }
    if target.set_proto(proto).is_err() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.setPrototypeOf: cannot set prototype"));
    }
    NativeResult::Ok(target_val)
}

/// `Object.hasOwn(obj, key)`：对象是否有指定自身属性。
///
/// # 边界与前提
/// - TypedArray 界内整数键经统一数值键门预支恒 true，不查形状槽。
pub fn object_has_own<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj_ptr = native_try!(require_obj_arg(vm, args, "hasOwn"));
    let key_si = vm.property_key_si(vm.reg(args[2]));
    let obj = unsafe { &*obj_ptr };
    // 统一数值键门（exotic [[HasOwnProperty]]）：界内整数键恒存在；数字无效
    // 与非数字串键落下方槽位判定。
    if obj.is_typed_array_obj()
        && matches!(
            crate::typed_array::ta_index_gate(vm, obj, key_si),
            crate::typed_array::TaIndexGate::NumericValid(_)
        )
    {
        return NativeResult::Ok(JsValue::bool(true));
    }
    NativeResult::Ok(JsValue::bool(vm.get_own_property_slot(obj, key_si).is_some()))
}

/// `Object.prototype.valueOf`：返回 this 的对象形式（配合 OrdinaryToPrimitive 的兜底）。
///
/// # 步骤
/// 1. ToObject 装箱 this（null/undefined 抛 TypeError）。
///
/// # 副作用
/// 无；返回装箱后的对象值，OrdinaryToPrimitive 因结果非原始值而继续走 toString。
pub fn object_proto_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.prototype.valueOf called on non-object"));
    }
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    NativeResult::Ok(obj_val)
}

/// `Object.prototype.isPrototypeOf(arg)`：this 是否在 arg 的原型链上。
///
/// # 步骤
/// 1. arg 非对象 → false（this 的转换不做，与 V8 行为一致）。
/// 2. ToObject 装箱 this（null/undefined 抛 TypeError）。
/// 3. 沿 arg 原型链逐节点比较指针恒等，命中 true，链尽 false。
pub fn object_proto_is_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let this_val = vm.reg(args[0]);
    let arg = vm.reg(if args.len() < 2 { 0 } else { args[1] });
    if !arg.is_object() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let target = obj_val.as_js_object_ptr();
    let mut cur = arg.as_js_object_ptr();
    while !cur.is_null() {
        if cur == target {
            return NativeResult::Ok(JsValue::bool(true));
        }
        // SAFETY: cur 沿原型链遍历，链上每个节点都是合法 JsObject（proto 非对象时为空指针终止）。
        let o = unsafe { &*cur };
        cur = o.proto().as_js_object_ptr();
    }
    NativeResult::Ok(JsValue::bool(false))
}

/// `Object.prototype.toLocaleString`：调用 this 的 `toString` 并以字符串返回。
///
/// # 步骤
/// 1. ToObject 装箱 this（null/undefined 抛 TypeError）。
/// 2. 以原始 this 值作接收者读 `toString`：accessor getter 的 this 保持原始值
///    （装箱副本不外泄），getter 抛错透传原异常。
/// 3. `toString` 非 callable → TypeError；调用结果经 ToString 返回。
pub fn object_proto_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.toLocaleString called on non-object",
        ));
    }
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    let si_to_string = vm.kernel_core().perm_interner().intern("toString").0;
    let to_str = match vm.ordinary_get(obj, si_to_string, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    if !is_callable(to_str) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.toLocaleString: toString is not callable",
        ));
    }
    let result = match vm.call_function_sync(to_str, this_val, &[]) {
        Ok(v) => v,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    match oxide_runtime_api::to_string_value_full(result, vm) {
        Ok(s) => NativeResult::Ok(s),
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// `Object.prototype.__defineSetter__(key, setter)`：把 key 定义为访问器属性，
/// setter 作 `[[Set]]`，描述符 `{ enumerable: true, configurable: true }`。
///
/// # 步骤
/// 1. ToObject 装箱 this（null/undefined 抛 TypeError）
/// 2. setter 非 callable → TypeError
/// 3. ToPropertyKey 求键（转换异常原样传播）
/// 4. DefinePropertyOrThrow（复用 `define_accessor_property` 的拒绝语义：
///    不可扩展对象 / 不可配置属性覆盖均抛 TypeError）
///
/// # 副作用
/// - 修改 this 的 shape 链与属性表；返回 undefined
pub fn object_proto_define_setter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.__defineSetter__ requires 2 arguments",
        ));
    }
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let setter = vm.reg(args[2]);
    if !is_callable(setter) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.__defineSetter__: setter must be callable",
        ));
    }
    let key_si = match vm.to_property_key_si(vm.reg(args[1])) {
        Ok(si) => si,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
    let attrs = PropAttributes::new(false, true, true);
    // DefinePropertyOrThrow 的 desc 无 [[Get]] 键：覆盖既有访问器时保留其 getter
    // （define_accessor_property 是全量替换，get 缺省须显式传现有值）。
    let existing_get = vm
        .get_own_property_slot(obj, key_si)
        .and_then(|pos| obj.prop_meta_at(pos))
        .filter(|m| m.is_accessor)
        .map(|m| m.get)
        .unwrap_or(JsValue::undefined());
    if let Err(e) = vm.define_accessor_property(obj, key_si, existing_get, setter, attrs) {
        return NativeResult::Err(crate::error::create_type_error(vm, &e));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// 沿 own+proto 链查找指定键的访问器字段（getter 或 setter）：数据属性与
/// 缺侧（get/set 为 undefined）继续上爬，链尽返回 undefined。
///
/// # 步骤
/// 1. ToObject 装箱 this（null/undefined 抛 TypeError）
/// 2. ToPropertyKey 求键（转换异常原样传播）
/// 3. 逐节点 GetOwnProperty：命名空间未初始化导出抛 ReferenceError；
///    accessor 命中且目标侧非 undefined 即返回
fn lookup_accessor_field<H: VmHost>(vm: &mut H, args: &[u8], want_getter: bool) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            if want_getter {
                "Object.prototype.__lookupGetter__ requires 1 argument"
            } else {
                "Object.prototype.__lookupSetter__ requires 1 argument"
            },
        ));
    }
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let key_si = match vm.to_property_key_si(vm.reg(args[1])) {
        Ok(si) => si,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    let mut cur = obj_val.as_js_object_ptr();
    while !cur.is_null() {
        // SAFETY: cur 沿原型链遍历，链上每个节点都是合法 JsObject。
        let o = unsafe { &*cur };
        if let Some(pos) = vm.get_own_property_slot(o, key_si) {
            if let Err(msg) = namespace_export_get(o, key_si) {
                return NativeResult::Err(crate::error::create_reference_error(vm, msg));
            }
            if let Some(meta) = o.prop_meta_at(pos) {
                if meta.is_accessor {
                    let field = if want_getter { meta.get } else { meta.set };
                    if !field.is_undefined() {
                        return NativeResult::Ok(field);
                    }
                }
            }
        }
        cur = o.proto().as_js_object_ptr();
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Object.prototype.__lookupGetter__(key)`：沿原型链取 key 的 `[[Get]]`。
pub fn object_proto_lookup_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    lookup_accessor_field(vm, args, true)
}

/// `Object.prototype.__lookupSetter__(key)`：沿原型链取 key 的 `[[Set]]`。
pub fn object_proto_lookup_setter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    lookup_accessor_field(vm, args, false)
}

/// 沿原型链判断是否为 Error 家族对象：对象自身带 `OBJ_TYPE_ERROR` 标签，或原型链上
/// 含 Error.prototype（等价 [[ErrorData]] 内部槽语义，覆盖 Error 用户子类）。
fn is_error_family<H: VmHost>(vm: &H, ptr: *mut JsObject) -> bool {
    let error_proto_ptr = vm.session().builtin_world().error_proto.as_ptr() as *mut JsObject;
    let mut cur = ptr;
    while !cur.is_null() {
        if std::ptr::eq(cur, error_proto_ptr) {
            return true;
        }
        // SAFETY: cur 沿原型链遍历，链上每个节点都是合法 JsObject（proto 非对象时为空指针终止）。
        let o = unsafe { &*cur };
        if o.is_error_obj() {
            return true;
        }
        cur = o.proto().as_js_object_ptr();
    }
    false
}

/// `Object.prototype.toString`：返回 `[object Tag]`。
///
/// # 步骤
/// 1. undefined/null 走快路径（`[object Undefined]` / `[object Null]`）。
/// 2. ToObject 装箱 this（原始值装箱后按对象品牌判定）。
/// 3. 品牌表定内置名（String/Number/Boolean/Symbol/Array/ArrayBuffer/DataView/
///    Function/RegExp/Date/TypedArray/Arguments/Error 族），其余一律 "Object"
///    （Math/JSON/Promise 等靠自身 `@@toStringTag` 覆盖）。
/// 4. 读 `Symbol.toStringTag`：字符串覆盖品牌名，非字符串/缺失回退品牌名，
///    getter 抛错透传原异常。
pub fn object_proto_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if this_val.is_undefined() {
        return NativeResult::Ok(vm.new_string("[object Undefined]"));
    }
    if this_val.is_null() {
        return NativeResult::Ok(vm.new_string("[object Null]"));
    }
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    // 装箱 Symbol 无内置品牌（原型链 tag 缺失时回退 "Object"，与 V8 一致），
    // 故品牌表不含 symbol。
    let builtin = if obj.is_string_obj() {
        "String"
    } else if obj.is_number_obj() {
        "Number"
    } else if obj.is_boolean_obj() {
        "Boolean"
    } else if obj.is_array() {
        "Array"
    } else if obj.is_array_buffer_obj() {
        "ArrayBuffer"
    } else if obj.is_data_view_obj() {
        "DataView"
    } else if obj.is_function() {
        "Function"
    } else if obj.is_regexp_obj() {
        "RegExp"
    } else if obj.is_date_obj() {
        "Date"
    } else if obj.is_typed_array_obj() {
        "TypedArray"
    } else if obj.is_arguments_obj() {
        "Arguments"
    } else if is_error_family(vm, obj_val.as_js_object_ptr()) {
        "Error"
    } else {
        "Object"
    };
    // @@toStringTag 为字符串时覆盖品牌名；非字符串（含缺失）回退品牌名，
    // getter 抛错透传原异常。
    let tag_key = make_well_known_symbol_key(9);
    match vm.ordinary_get(obj, tag_key, obj_val) {
        Ok(v) => {
            // 先把 tag 字符串取出（结束对 vm 的不可变借用），再拼结果串。
            let tag = vm.lookup_str(v);
            let name = tag.as_deref().unwrap_or(builtin);
            let text = format!("[object {name}]");
            NativeResult::Ok(vm.new_string(&text))
        }
        Err(err) => NativeResult::Err(crate::iterator::engine_error(vm, &err)),
    }
}

/// `Object.prototype.hasOwnProperty(key)`：this 是否有指定自身属性。
///
/// # 步骤
/// 1. 先 ToPropertyKey 求键（spec 顺序：键先于 ToObject）。
/// 2. ToObject 装箱 this（null/undefined 抛 TypeError，原始值装箱后查 own 槽）。
///
/// # 边界与前提
/// - TypedArray 界内整数键经统一数值键门预支恒 true，不查形状槽。
pub fn object_proto_has_own_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let key_si = match vm.to_property_key_si(vm.reg(args[1])) {
        Ok(si) => si,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    // 统一数值键门（exotic [[HasOwnProperty]]）：界内整数键恒存在；数字无效
    // 与非数字串键落下方槽位判定（命名空间检查在门臂之后，TA 非命名空间
    // 不触达）。
    if obj.is_typed_array_obj()
        && matches!(
            crate::typed_array::ta_index_gate(vm, obj, key_si),
            crate::typed_array::TaIndexGate::NumericValid(_)
        )
    {
        return NativeResult::Ok(JsValue::bool(true));
    }
    if vm.get_own_property_slot(obj, key_si).is_none() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    // HasOwnProperty 经 `? [[GetOwnProperty]]`：未初始化导出抛 ReferenceError。
    if let Err(msg) = namespace_export_get(obj, key_si) {
        return NativeResult::Err(crate::error::create_reference_error(vm, msg));
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `Object.prototype.propertyIsEnumerable(key)`：指定自身属性是否可枚举。
///
/// # 步骤
/// 1. 先 ToPropertyKey 求键。
/// 2. ToObject 装箱 this（null/undefined 抛 TypeError）。
///
/// # 边界与前提
/// - TypedArray 界内整数键经统一数值键门预支恒 true（enumerable 恒真），
///   不读 meta。
pub fn object_proto_property_is_enumerable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let key_si = match vm.to_property_key_si(vm.reg(args[1])) {
        Ok(si) => si,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    // 统一数值键门（exotic PropertyIsEnumerable）：界内整数键恒可枚举；
    // 数字无效与非数字串键落下方 meta 读取。
    if obj.is_typed_array_obj()
        && matches!(
            crate::typed_array::ta_index_gate(vm, obj, key_si),
            crate::typed_array::TaIndexGate::NumericValid(_)
        )
    {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let Some(pos) = vm.get_own_property_slot(obj, key_si) else {
        return NativeResult::Ok(JsValue::bool(false));
    };
    // PropertyIsEnumerable 同样先经 `? [[GetOwnProperty]]` 再取 enumerable 位。
    if let Err(msg) = namespace_export_get(obj, key_si) {
        return NativeResult::Err(crate::error::create_reference_error(vm, msg));
    }
    let enumerable = obj
        .prop_meta_at(pos)
        .map(|meta| meta.attributes.enumerable())
        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
    NativeResult::Ok(JsValue::bool(enumerable))
}

/// `Object.prototype.__defineGetter__(key, getter)`：把 key 定义为访问器属性，
/// getter 作 `[[Get]]`，描述符 `{ enumerable: true, configurable: true }`。
///
/// # 步骤
/// 1. ToObject 装箱 this（null/undefined 抛 TypeError）
/// 2. getter 非 callable → TypeError
/// 3. ToPropertyKey 求键
/// 4. DefinePropertyOrThrow（复用 `define_accessor_property` 的拒绝语义：
///    不可扩展对象 / 不可配置属性覆盖均抛 TypeError）
///
/// # 副作用
/// - 修改 this 的 shape 链与属性表；返回 undefined
pub fn object_proto_define_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.__defineGetter__ requires 2 arguments",
        ));
    }
    let this_val = vm.reg(args[0]);
    let obj_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let getter = vm.reg(args[2]);
    if !is_callable(getter) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.__defineGetter__: getter must be callable",
        ));
    }
    let key_si = match vm.to_property_key_si(vm.reg(args[1])) {
        Ok(si) => si,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    let obj = unsafe { &mut *obj_val.as_js_object_ptr() };
    let attrs = PropAttributes::new(false, true, true);
    // DefinePropertyOrThrow 的 desc 无 [[Set]] 键：覆盖既有访问器时保留其 setter
    // （define_accessor_property 是全量替换，set 缺省须显式传现有值）。
    let existing_set = vm
        .get_own_property_slot(obj, key_si)
        .and_then(|pos| obj.prop_meta_at(pos))
        .filter(|m| m.is_accessor)
        .map(|m| m.set)
        .unwrap_or(JsValue::undefined());
    if let Err(e) = vm.define_accessor_property(obj, key_si, getter, existing_set, attrs) {
        return NativeResult::Err(crate::error::create_type_error(vm, &e));
    }
    NativeResult::Ok(JsValue::undefined())
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
    let obj_val = JsValue::from_js_object(obj_ptr);
    for (i, (si, offset)) in owned_keys.iter().enumerate() {
        let key_val = key_si_to_js_value(vm, *si);
        // EnumerableOwnProperties 的 "key+value"：accessor 触发 getter，异常传播原值。
        let val = match own_property_value(vm, obj, obj_val, *si, *offset) {
            Ok(value) => value,
            Err(exc) => return NativeResult::Err(exc),
        };
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

/// `Object.groupBy(items, callbackFn)`：按回调返回的键分组迭代元素到新对象。
/// 回调以 `(element, key)` 调用（对数组 items，key 为下标），返回值经
/// ToPropertyKey 转换后作为分组键；结果对象原型为 null。
pub fn object_group_by<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Object.groupBy called with {} args", args.len());
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.groupBy requires 2 arguments"));
    }
    let callback_val = vm.reg(args[2]);
    if !is_callable(callback_val) {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.groupBy: callbackFn is not callable"));
    }
    let items_val = vm.reg(args[1]);
    // 取 @@iterator 方法（同步）。
    let items_obj = match oxide_runtime_api::to_object(items_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let items_ptr = items_obj.as_js_object_ptr();
    let sym_iter_si = make_well_known_symbol_key(0);
    let iter_method = match unsafe { vm.ordinary_get(&*items_ptr, sym_iter_si, items_val) } {
        Ok(m) => m,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    if !is_callable(iter_method) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.groupBy: items[Symbol.iterator] is not callable",
        ));
    }
    let iter_val = match vm.call_function_sync(iter_method, items_val, &[]) {
        Ok(v) => v,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    if !iter_val.is_object() || iter_val.as_js_object_ptr().is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.groupBy: iterator is not an object"));
    }
    let iter_obj = unsafe { &*iter_val.as_js_object_ptr() };
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let next_fn = match vm.ordinary_get(iter_obj, next_si, iter_val) {
        Ok(f) => f,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    // 创建结果对象（OrdinaryObjectCreate(null)，与 Object.create(null) 同款）。
    let result = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    let result_val = JsValue::from_js_object(result);
    let mut counter: i32 = 0;
    loop {
        let next_result = match vm.call_function_sync(next_fn, iter_val, &[]) {
            Ok(v) => v,
            Err(e) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        if !next_result.is_object() || next_result.as_js_object_ptr().is_null() {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "Object.groupBy: iterator next() returned non-object",
            ));
        }
        let nr = unsafe { &*next_result.as_js_object_ptr() };
        let done_si = vm.kernel_core().perm_interner().intern("done").0;
        let done = match vm.ordinary_get(nr, done_si, next_result) {
            Ok(v) => oxide_runtime_api::to_boolean(v),
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        if done {
            break;
        }
        let value_si = vm.kernel_core().perm_interner().intern("value").0;
        let element = match vm.ordinary_get(nr, value_si, next_result) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        // 调用 callbackFn(element, counter)：counter 对数组 items 即元素下标 k。
        let key = match vm.call_function_sync(callback_val, JsValue::undefined(), &[element, JsValue::int(counter)]) {
            Ok(v) => v,
            Err(e) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        // ToPropertyKey：undefined 归 "undefined" 分组，不跳过；对象转换异常原样传播。
        let key_si = match vm.to_property_key_si(key) {
            Ok(si) => si,
            Err(e) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        // 检查 result 上是否已有该键的数组分组。
        let result_ref = unsafe { &*result };
        let existing = match vm.ordinary_get(result_ref, key_si, result_val) {
            Ok(v) if v.is_object() && !v.as_js_object_ptr().is_null() => {
                let arr = unsafe { &*v.as_js_object_ptr() };
                if arr.is_array() {
                    Some(v)
                } else {
                    None
                }
            }
            _ => None,
        };
        let arr_val = if let Some(existing) = existing {
            existing
        } else {
            let arr_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
            let arr = vm.alloc_object(JsObject::new_array(
                EMPTY_SHAPE_ID,
                JsValue::from_js_object(arr_proto),
                0,
                vm.epoch().bump(),
            ));
            let new_arr_val = JsValue::from_js_object(arr);
            let result_ref_mut = unsafe { &mut *result };
            let promoted = vm.promote_if_needed_for_write_ptr(result, new_arr_val);
            // 写入失败按 builtin 边界传播（result 为 fresh 对象实际不可达，不得静默吞）。
            if let Err(err) = vm.ordinary_set(result_ref_mut, key_si, promoted, result_val, true) {
                return NativeResult::Err(crate::array::from_engine_error(vm, &err));
            }
            new_arr_val
        };
        // push element 到分组数组（写入前 promote，与 Array.prototype.push 同款）。
        let arr_obj = unsafe { &mut *arr_val.as_js_object_ptr() };
        let idx = arr_obj.prop_count();
        let promoted_elem = vm.promote_if_needed_for_write_ptr(arr_val.as_js_object_ptr(), element);
        arr_obj.set_prop_at(idx, promoted_elem);
        counter += 1;
    }
    NativeResult::Ok(result_val)
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
    let obj_val = JsValue::from_js_object(obj_ptr);
    for (i, (si, offset)) in owned_keys.iter().enumerate() {
        // EnumerableOwnProperties 的 "value"：accessor 触发 getter，异常传播原值。
        let val = match own_property_value(vm, obj, obj_val, *si, *offset) {
            Ok(value) => value,
            Err(exc) => return NativeResult::Err(exc),
        };
        unsafe {
            (*arr).set_prop_at(i, val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}
