//! Array 构造器与静态方法（constructor / isArray / of / from）及 species 构造辅助。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes, MAX_DENSE_PROPS};
use oxide_types::private_key::make_well_known_symbol_key;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use super::common::{
    array_length_arg, array_type_error, arraylike_get, arraylike_get_or_err, create_new_array, invoke_native_callback,
    is_constructor_value, js_array_index, require_callback, string_arraylike_units, unexpected_tail_call_error,
    unit_string_value,
};

/// JS `Array()` 构造逻辑：单个数字参数创建指定长度空数组，其余情况把参数作为元素。
pub fn array_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let proto_val = JsValue::from_js_object(proto);

    if args.len() == 2 {
        let val = vm.reg(args[1]);
        if val.is_int() || val.is_double() {
            let n = match array_length_arg(vm, val) {
                Ok(n) => n,
                Err(err) => return NativeResult::Err(err),
            };
            let arr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, proto_val, n, vm.epoch().bump()));
            // 数值长度分支建 n 个空洞而非 present-undefined：`in`、for-in、
            // Object.keys 按缺失处理，元素 Get 落原型链。
            for i in 0..n {
                unsafe {
                    (*arr).mark_hole_at(i);
                }
            }
            return NativeResult::Ok(JsValue::from_js_object(arr));
        }
    }

    let n_elems = if args.len() > 1 { args.len() - 1 } else { 0 };
    let arr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, proto_val, n_elems, vm.epoch().bump()));
    for i in 0..n_elems {
        unsafe {
            (*arr).set_prop_at(i, vm.reg(args[1 + i]));
        }
    }
    unsafe {
        (*arr).set_prop_count(n_elems);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

/// `Array.isArray(value)`：参数是否为真正的 Array 对象。
pub fn array_is_array<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    NativeResult::Ok(JsValue::bool(unsafe { &*ptr }.is_array()))
}

/// `Array.of(...items)`：以参数为元素构造新数组。
///
/// # 步骤
/// 1. 按 `this`（构造函数 C）以参数个数构造结果对象：C 可构造时 `new C(len)`，
///    否则新建普通数组。
/// 2. 元素逐个以 CreateDataProperty 语义写入结果对象。
/// 3. 收尾设置结果对象的 length（可触发继承的 length setter）。
pub fn array_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n_elems = if args.len() > 1 { args.len() - 1 } else { 0 };
    let c = vm.reg(args[0]);
    let (a_ptr, is_array) = match construct_array_from_result(vm, c, &[JsValue::int(n_elems as i32)]) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    for i in 0..n_elems {
        let val = vm.reg(args[1 + i]);
        if let Err(err) = array_from_set_prop(vm, a_ptr, is_array, i, val) {
            return NativeResult::Err(err);
        }
    }
    if let Err(err) = array_from_set_length(vm, a_ptr, n_elems) {
        return NativeResult::Err(err);
    }
    NativeResult::Ok(JsValue::from_js_object(a_ptr))
}

/// `Array.from(items, mapFn?, thisArg?)`：从可迭代对象或 array-like 构造新数组，
/// 可选 `mapFn` 逐元素映射（`thisArg` 作回调 this），返回新数组。
///
/// # 步骤
/// 1. `items` 为 null/undefined 抛 TypeError；`mapFn` 非 undefined 时须可调用。
/// 2. 读取 `@@iterator` 判定可迭代（不调用）：可迭代路径先按 `this`（构造函数 C）
///    `new C()` 构造结果对象，再经迭代协议逐个取元素；array-like 路径先 ToObject
///    读 `length`，再 `new C(len)` 构造结果对象，逐索引取值（缺失属性取 undefined）。
/// 3. 元素经 `mapFn` 映射后以 CreateDataProperty 语义写入结果对象，收尾设置 length。
/// 4. 迭代器或 `mapFn` 抛错时透传原异常值；迭代中途异常先执行 IteratorClose。
///
/// # 边界与前提
/// - C 不可构造（非函数/箭头/普通 native 方法）时回退到普通空数组。
/// - 写入元素遇不可扩展对象或不可配置属性冲突时抛 TypeError（CreateDataPropertyOrThrow）。
///
/// # 注意事项
/// - 迭代只读包装器 `next`/`done`/`value` 属性，与 spread 物化共用同一迭代协议。
/// - new.target 不向被构造的 C 传播（与 Reflect.construct 同一简化）。
pub fn array_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let items = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if items.is_null() || items.is_undefined() {
        return NativeResult::Err(array_type_error(vm, "Array.from requires an iterable or array-like object"));
    }
    let mapfn = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mapping = if mapfn.is_undefined() {
        None
    } else {
        match require_callback(vm, mapfn) {
            Ok(cb) => Some(cb),
            Err(err) => return NativeResult::Err(err),
        }
    };
    let this_arg = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let c = vm.reg(args[0]);

    // 先只读取 @@iterator 判定可迭代（不调用），保证"构造结果对象"先于"获取迭代器"，
    // 与规范的执行序一致。
    let iterable = match crate::iterator::peek_iterator_method(vm, items) {
        Ok(b) => b,
        Err(err) => return NativeResult::Err(err),
    };

    let a_ptr;
    let is_array;
    if iterable {
        // 可迭代路径：先 new C() 得到结果对象，再获取迭代器逐个取元素。
        a_ptr = match construct_array_from_result(vm, c, &[]) {
            Ok((ptr, is_arr)) => {
                is_array = is_arr;
                ptr
            }
            Err(err) => return NativeResult::Err(err),
        };

        let iterator = match crate::iterator::make_iterator_for_value(vm, items) {
            Ok(it) => it,
            Err(err) => return NativeResult::Err(err),
        };
        let next_si = vm.kernel_core().perm_interner().intern("next").0;
        let done_si = vm.kernel_core().perm_interner().intern("done").0;
        let value_si = vm.kernel_core().perm_interner().intern("value").0;
        let mut k = 0usize;
        let iter_result: Result<(), JsValue> = (|| {
            loop {
                let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
                let next_fn = match vm.ordinary_get(iter_obj, next_si, iterator) {
                    Ok(v) => v,
                    Err(err) => return Err(from_engine_error(vm, &err)),
                };
                let result = match vm.call_function_sync(next_fn, iterator, &[]) {
                    Ok(v) => v,
                    Err(err) => return Err(from_engine_error(vm, &err)),
                };
                if !result.is_object() {
                    return Err(array_type_error(vm, "iterator result is not an object"));
                }
                let result_obj = unsafe { &*result.as_js_object_ptr() };
                let done = match vm.ordinary_get(result_obj, done_si, result) {
                    Ok(v) => oxide_runtime_api::to_boolean(v),
                    Err(err) => return Err(from_engine_error(vm, &err)),
                };
                if done {
                    break;
                }
                let elem = match vm.ordinary_get(result_obj, value_si, result) {
                    Ok(v) => v,
                    Err(err) => return Err(from_engine_error(vm, &err)),
                };
                let mapped = match mapping {
                    Some(cb) => match invoke_native_callback(vm, cb, this_arg, &[elem, JsValue::int(k as i32)]) {
                        NativeResult::Ok(m) => m,
                        NativeResult::Err(err) => return Err(err),
                        NativeResult::TailCall { .. } => {
                            return Err(array_type_error(vm, "unexpected tail call in array callback"))
                        }
                    },
                    None => elem,
                };
                array_from_set_prop(vm, a_ptr, is_array, k, mapped)?;
                k += 1;
            }
            Ok(())
        })();
        if let Err(err) = iter_result {
            // IteratorClose：异常退出先关闭迭代器（return() 转发给内层），再抛原异常。
            close_iterator(vm, iterator);
            return NativeResult::Err(err);
        }
        if let Err(err) = array_from_set_length(vm, a_ptr, k) {
            return NativeResult::Err(err);
        }
    } else {
        // array-like 路径：ToObject 后读 length，构造结果对象，逐索引取值。
        // 字符串源（原始串/装箱串）的 length 与索引未物化，直接按 UTF-16 单元读。
        let string_units = string_arraylike_units(vm, items);
        let obj_val = match &string_units {
            Some(_) => JsValue::undefined(),
            None => match oxide_runtime_api::to_object(items, vm) {
                Ok(o) => o,
                Err(err) => return NativeResult::Err(crate::error::create_type_error(vm, &err)),
            },
        };
        let len = match &string_units {
            Some(units) => units.len().min(MAX_DENSE_PROPS),
            None => {
                let obj_ptr = obj_val.as_js_object_ptr();
                let obj = unsafe { &*obj_ptr };
                let length_key = vm.new_string("length");
                let length_si = vm.property_key_si(length_key);
                let len_val = match vm.ordinary_get(obj, length_si, obj_val) {
                    Ok(v) => v,
                    Err(err) => return NativeResult::Err(from_engine_error(vm, &err)),
                };
                let len_u64 = oxide_runtime_api::to_length(len_val);
                if len_u64 > u32::MAX as u64 {
                    return NativeResult::Err(crate::error::create_range_error(vm, "Invalid array length"));
                }
                len_u64.min(MAX_DENSE_PROPS as u64) as usize
            }
        };

        a_ptr = match construct_array_from_result(vm, c, &[JsValue::int(len as i32)]) {
            Ok((ptr, is_arr)) => {
                is_array = is_arr;
                ptr
            }
            Err(err) => return NativeResult::Err(err),
        };
        for i in 0..len {
            let elem = match &string_units {
                Some(units) => unit_string_value(vm, units[i]),
                None => arraylike_get_or_err!(vm, obj_val.as_js_object_ptr(), i),
            };
            let mapped = match mapping {
                Some(cb) => match invoke_native_callback(vm, cb, this_arg, &[elem, js_array_index(i)]) {
                    NativeResult::Ok(m) => m,
                    NativeResult::Err(err) => return NativeResult::Err(err),
                    NativeResult::TailCall { .. } => return unexpected_tail_call_error(vm),
                },
                None => elem,
            };
            if let Err(err) = array_from_set_prop(vm, a_ptr, is_array, i, mapped) {
                return NativeResult::Err(err);
            }
        }
        if let Err(err) = array_from_set_length(vm, a_ptr, len) {
            return NativeResult::Err(err);
        }
    }

    NativeResult::Ok(JsValue::from_js_object(a_ptr))
}

/// 引擎调用边界抛出的错误恢复为原始异常值（透传），否则按错误文本构造对应 kind 的错误。
pub(crate) fn from_engine_error<H: VmHost>(vm: &mut H, err: &str) -> JsValue {
    vm.take_uncaught_value()
        .unwrap_or_else(|| crate::error::create_from_text(vm, err))
}

/// 按构造函数 C 构造 Array.from 的结果对象：C 可构造时以 C.prototype 分配 `this`
/// 后调用 C（返回非对象时回退 `this`），否则返回普通空数组。
///
/// # 返回值
/// `(结果对象指针, 是否为真数组)`。
fn construct_array_from_result<H: VmHost>(
    vm: &mut H, c: JsValue, args: &[JsValue],
) -> Result<(*mut JsObject, bool), JsValue> {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let proto_val = JsValue::from_js_object(proto);
    let mut fallback_array = || {
        let arr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, proto_val, 0, vm.epoch().bump()));
        (arr, true)
    };
    if !c.is_object() {
        return Ok(fallback_array());
    }
    let c_obj = unsafe { &*c.as_js_object_ptr() };
    // 不可构造：箭头函数、非构造器标记的 native 方法。
    let constructible = c_obj.is_function()
        && !c_obj.is_arrow()
        && !(c_obj.native_fn().is_some() && c_obj.type_tag != JsObject::OBJ_TYPE_CONSTRUCTOR);
    if !constructible {
        return Ok(fallback_array());
    }

    // 仿 Reflect.construct：以 C.prototype 分配 this 后调用 C。
    let proto_si = vm.kernel_core().perm_interner().intern("prototype").0;
    let ctor_proto = match vm.resolve_property(c_obj, proto_si) {
        Some(p) if p.is_object() => p,
        _ => JsValue::from_js_object(vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject),
    };
    let this_ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, ctor_proto));
    let this_val = JsValue::from_js_object(this_ptr);
    match vm.call_function_sync(c, this_val, args) {
        Ok(ret) if ret.is_object() => {
            let ret_ptr = ret.as_js_object_ptr();
            let ret_tag = unsafe { &*ret_ptr }.type_tag;
            // 原始值包装对象（如 `Object(4)` 返回的 Number 包装）的索引属性存储不完整，
            // 回退到刚分配的对象；真数组/普通对象按规范使用构造返回对象。
            if matches!(
                ret_tag,
                JsObject::OBJ_TYPE_STRING_OBJ | JsObject::OBJ_TYPE_NUMBER_OBJ | JsObject::OBJ_TYPE_BOOLEAN_OBJ
            ) {
                return Ok((this_ptr, false));
            }
            Ok((ret_ptr, unsafe { &*ret_ptr }.is_array()))
        }
        Ok(_) => Ok((this_ptr, false)),
        Err(err) => Err(from_engine_error(vm, &err)),
    }
}

/// ArraySpeciesCreate(originalArray, length)：真数组读取 constructor / @@species
/// 并构造目标对象；array-like（非数组）直接 ArrayCreate(length)。
pub(crate) fn array_species_create<H: VmHost>(
    vm: &mut H, o_val: JsValue, is_array: bool, count: usize,
) -> Result<*mut JsObject, JsValue> {
    if !is_array {
        return Ok(create_new_array(vm, count));
    }
    let o_ptr = o_val.as_js_object_ptr();
    let o_obj = unsafe { &*o_ptr };
    let ctor_key = vm.kernel_core().perm_interner().intern("constructor").0;
    let mut c = match vm.ordinary_get(o_obj, ctor_key, o_val) {
        Ok(v) => v,
        Err(msg) => return Err(from_engine_error(vm, &msg)),
    };
    // C 为 undefined 时不查 species，直接回退内置 ArrayCreate。
    if c.is_undefined() {
        return Ok(create_new_array(vm, count));
    }
    if !is_constructor_value(c) {
        if !c.is_object() {
            return Err(crate::error::create_type_error(vm, "Species constructor not a constructor"));
        }
        let species_key = make_well_known_symbol_key(10);
        let s = match vm.ordinary_get(unsafe { &*c.as_js_object_ptr() }, species_key, c) {
            Ok(v) => v,
            Err(msg) => return Err(from_engine_error(vm, &msg)),
        };
        c = if s.is_null() { JsValue::undefined() } else { s };
    }
    if c.is_undefined() {
        return Ok(create_new_array(vm, count));
    }
    if !is_constructor_value(c) {
        return Err(crate::error::create_type_error(vm, "Species constructor not a constructor"));
    }
    match construct_array_from_result(vm, c, &[js_array_index(count)]) {
        Ok((ptr, _)) => Ok(ptr),
        Err(err) => Err(err),
    }
}

/// CreateDataPropertyOrThrow：以 `{writable, enumerable, configurable}=true` 写入属性，
/// 对象不可扩展或与既有不可配置属性冲突时抛 TypeError。
pub(crate) fn create_data_property_or_throw<H: VmHost>(
    vm: &mut H, obj: &mut JsObject, prop_name_si: u32, val: JsValue,
) -> Result<(), JsValue> {
    if vm.get_own_property_slot(obj, prop_name_si).is_none() && !obj.is_extensible() {
        return Err(array_type_error(vm, "Cannot define property: object is not extensible"));
    }
    match vm.define_data_property(obj, prop_name_si, val, PropAttributes::new(true, true, true)) {
        Ok(()) => Ok(()),
        Err(err) => Err(crate::error::create_type_error(vm, &err)),
    }
}

/// 把元素写入 Array.from 的结果对象（CreateDataPropertyOrThrow 语义）：真数组
/// 走密集元素区，普通对象属性名按十进制索引字符串。
///
/// # 边界与前提
/// - 真数组快路径（可扩展 + length 可写 + 槽位无元数据）与规范 define 同形，
///   裸写零漂移；完整性受限目标（frozen/sealed/preventExtensions）与携带元
///   数据的槽位（洞/只读/访问器）一律落慢路径，按规范抛 TypeError。
fn array_from_set_prop<H: VmHost>(
    vm: &mut H, a: *mut JsObject, is_array: bool, i: usize, val: JsValue,
) -> Result<(), JsValue> {
    if is_array {
        unsafe {
            let a_obj = &mut *a;
            let count = a_obj.array_prop_count as usize;
            // 越界增长不读元数据（越界读取会落命名属性区，语义错位）。
            let fast =
                a_obj.is_extensible() && a_obj.is_length_writable() && (i >= count || a_obj.prop_meta_at(i).is_none());
            if fast {
                a_obj.set_prop_at(i, val);
                return Ok(());
            }
        }
    }
    let key = vm.new_string(&i.to_string());
    let key_si = vm.property_key_si(key);
    create_data_property_or_throw(vm, unsafe { &mut *a }, key_si, val)
}

/// 收尾设置 Array.from 结果对象的 length：统一走普通 Set——真数组 length 写
/// 按 ArraySetLength 可写性判定（frozen / length 收窄不可写目标抛 TypeError），
/// 普通对象可触发继承的 length setter，其异常透传。
fn array_from_set_length<H: VmHost>(vm: &mut H, a: *mut JsObject, len: usize) -> Result<(), JsValue> {
    let a_obj = unsafe { &mut *a };
    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    let a_val = JsValue::from_js_object(a);
    match vm.ordinary_set(a_obj, length_si, JsValue::int(len as i32), a_val, true) {
        Ok(()) => Ok(()),
        Err(err) => Err(from_engine_error(vm, &err)),
    }
}

/// IteratorClose：迭代异常退出时调用迭代器包装器的 `return()`（转发给内层迭代器），
/// 丢弃其自身错误，保留在途异常。
fn close_iterator<H: VmHost>(vm: &mut H, iterator: JsValue) {
    if !iterator.is_object() {
        return;
    }
    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
    if let Ok(ret) = vm.ordinary_get(iter_obj, return_si, iterator) {
        if ret.is_object() && unsafe { &*ret.as_js_object_ptr() }.is_function() {
            // return() 的抛错被忽略，其值不得外泄进槽覆盖在途异常。
            let saved_uncaught = vm.take_uncaught_value();
            let _ = vm.call_function_sync(ret, iterator, &[]);
            vm.restore_uncaught_value(saved_uncaught);
        }
    }
}
