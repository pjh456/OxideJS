use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes, MAX_DENSE_PROPS};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

macro_rules! array_ptr {
    ($vm:expr, $args:expr) => {{
        match get_this_array_ref($vm, $vm.reg($args[0])) {
            Ok(ptr) => ptr,
            Err(err) => {
                builtins_error!("Array method: invalid receiver");
                return NativeResult::Err(err);
            }
        }
    }};
}

#[allow(unused_macros)]
macro_rules! array_ptr_len {
    ($vm:expr, $args:expr) => {{
        let this_val = $vm.reg($args[0]);
        let (arr_ptr, len, _is_arr) = match get_this_arraylike($vm, this_val) {
            Ok(v) => v,
            Err(err) => {
                builtins_error!("Array method: invalid receiver");
                return NativeResult::Err(err);
            }
        };
        (arr_ptr, len)
    }};
}

macro_rules! array_ptr_len3 {
    ($vm:expr, $args:expr) => {{
        let this_val = $vm.reg($args[0]);
        let (arr_ptr, len, is_arr) = match get_this_arraylike($vm, this_val) {
            Ok(v) => v,
            Err(err) => {
                builtins_error!("Array method: invalid receiver");
                return NativeResult::Err(err);
            }
        };
        (arr_ptr, len, is_arr)
    }};
}

fn array_type_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_type_error(vm, msg)
}

fn get_this_array_ref<H: VmHost>(vm: &mut H, val: JsValue) -> Result<*mut JsObject, JsValue> {
    if !val.is_object() {
        return Err(array_type_error(vm, "Array method called on incompatible receiver"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(array_type_error(vm, "Array method called on incompatible receiver"));
    }
    let obj = unsafe { &*ptr };
    if !obj.is_array() {
        return Err(array_type_error(vm, "Array method called on incompatible receiver"));
    }
    Ok(ptr)
}

/// 接受 array 或 ArrayLike (object with length property) — for read-only methods.
/// 返回 (object_ptr, length, is_real_array)
#[inline(always)]
fn get_this_arraylike<H: VmHost>(vm: &mut H, val: JsValue) -> Result<(*mut JsObject, usize, bool), JsValue> {
    if !val.is_object() {
        return Err(array_type_error(vm, "Array.prototype method called on null or undefined"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(array_type_error(vm, "Array.prototype method called on null or undefined"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_array() {
        return Ok((ptr, obj.prop_count() as usize, true));
    }
    // 读取 length 属性
    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    let len_val = vm.ordinary_get(unsafe { &*ptr }, length_si, val).unwrap_or(JsValue::int(0));
    let len_num = vm.coerce_number_bounded(len_val).unwrap_or(0.0);
    let len = if !len_num.is_finite() || len_num <= 0.0 {
        0
    } else {
        (len_num as usize).min(MAX_DENSE_PROPS)
    };
    Ok((ptr, len, false))
}

/// 从 arraylike 读取 index 位置元素
#[inline(always)]
fn arraylike_get<H: VmHost>(vm: &mut H, ptr: *mut JsObject, is_array: bool, i: usize) -> JsValue {
    if is_array {
        unsafe { (*ptr).get_prop_at(i) }
    } else {
        let key_str = vm.new_string(&i.to_string());
        let key_si = vm.property_key_si(key_str);
        let recv = JsValue::from_js_object(ptr);
        vm.ordinary_get(unsafe { &*ptr }, key_si, recv).unwrap_or(JsValue::undefined())
    }
}

pub(crate) fn require_callback<H: VmHost>(vm: &mut H, callback_val: JsValue) -> Result<JsValue, JsValue> {
    if !callback_val.is_object() {
        return Err(array_type_error(vm, "callback is not a function"));
    }
    let ptr = callback_val.as_js_object_ptr();
    if ptr.is_null() || !unsafe { &*ptr }.is_function() {
        return Err(array_type_error(vm, "callback is not a function"));
    }
    Ok(callback_val)
}

fn create_new_array<H: VmHost>(vm: &mut H, n: usize) -> *mut JsObject {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
        n.min(MAX_DENSE_PROPS),
        vm.epoch().bump(),
    ))
}

fn array_length_arg<H: VmHost>(vm: &mut H, value: JsValue) -> Result<usize, JsValue> {
    let n = vm.coerce_number_bounded(value).unwrap_or(f64::NAN);
    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > MAX_DENSE_PROPS as f64 {
        return Err(crate::error::create_range_error(vm, "Invalid array length"));
    }
    Ok(n as usize)
}

pub(crate) fn invoke_native_callback<H: VmHost>(
    vm: &mut H, callback_val: JsValue, this_val: JsValue, cb_args: &[JsValue],
) -> NativeResult {
    if !callback_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "callback is not a function"));
    }
    let cb_ptr = callback_val.as_js_object_ptr();
    if cb_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "callback is not a function"));
    }
    let cb = unsafe { &*cb_ptr };
    if !cb.is_function() {
        return NativeResult::Err(crate::error::create_type_error(vm, "callback is not a function"));
    }
    match vm.call_function_sync(callback_val, this_val, cb_args) {
        Ok(value) => NativeResult::Ok(value),
        Err(err) => match vm.take_uncaught_value() {
            Some(original) => NativeResult::Err(original),
            None => NativeResult::Err(callback_error_from_text(vm, &err)),
        },
    }
}

fn callback_error_from_text<H: VmHost>(vm: &mut H, err: &str) -> JsValue {
    let err = err.strip_prefix("uncaught ").unwrap_or(err);
    if let Some(msg) = err.strip_prefix("TypeError: ") {
        return crate::error::create_type_error(vm, msg);
    }
    if let Some(msg) = err.strip_prefix("ReferenceError: ") {
        return crate::error::create_reference_error(vm, msg);
    }
    if let Some(msg) = err.strip_prefix("RangeError: ") {
        return crate::error::create_range_error(vm, msg);
    }
    if let Some(msg) = err.strip_prefix("SyntaxError: ") {
        return crate::error::create_syntax_error(vm, msg);
    }
    if let Some(msg) = err.strip_prefix("Error: ") {
        return crate::error::create_error(vm, msg);
    }
    crate::error::create_error(vm, err)
}

fn unexpected_tail_call_error<H: VmHost>(vm: &mut H) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "unexpected tail call in array callback"))
}

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
    if let Err(err) = array_from_set_length(vm, a_ptr, is_array, n_elems) {
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
        if let Err(err) = array_from_set_length(vm, a_ptr, is_array, k) {
            return NativeResult::Err(err);
        }
    } else {
        // array-like 路径：ToObject 后读 length，构造结果对象，逐索引取值。
        let obj_val = match oxide_runtime_api::to_object(items, vm) {
            Ok(o) => o,
            Err(err) => return NativeResult::Err(crate::error::create_type_error(vm, &err)),
        };
        let obj_ptr = obj_val.as_js_object_ptr();
        let obj = unsafe { &*obj_ptr };
        let length_key = vm.new_string("length");
        let length_si = vm.property_key_si(length_key);
        let len_val = match vm.ordinary_get(obj, length_si, obj_val) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(from_engine_error(vm, &err)),
        };
        let len = oxide_runtime_api::to_length(len_val).min(MAX_DENSE_PROPS as u64) as usize;

        a_ptr = match construct_array_from_result(vm, c, &[JsValue::int(len as i32)]) {
            Ok((ptr, is_arr)) => {
                is_array = is_arr;
                ptr
            }
            Err(err) => return NativeResult::Err(err),
        };
        for i in 0..len {
            let elem = arraylike_get(vm, obj_ptr, false, i);
            let mapped = match mapping {
                Some(cb) => match invoke_native_callback(vm, cb, this_arg, &[elem, JsValue::int(i as i32)]) {
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
        if let Err(err) = array_from_set_length(vm, a_ptr, is_array, len) {
            return NativeResult::Err(err);
        }
    }

    NativeResult::Ok(JsValue::from_js_object(a_ptr))
}

/// 引擎调用边界抛出的错误恢复为原始异常值（透传），否则按错误文本构造普通错误。
fn from_engine_error<H: VmHost>(vm: &mut H, err: &str) -> JsValue {
    vm.take_uncaught_value().unwrap_or_else(|| callback_error_from_text(vm, err))
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

/// CreateDataPropertyOrThrow：以 `{writable, enumerable, configurable}=true` 写入属性，
/// 对象不可扩展或与既有不可配置属性冲突时抛 TypeError。
fn create_data_property_or_throw<H: VmHost>(
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

/// 把元素写入 Array.from 的结果对象：真数组走密集元素区，普通对象走
/// CreateDataProperty 语义（属性名按十进制索引字符串）。
fn array_from_set_prop<H: VmHost>(
    vm: &mut H, a: *mut JsObject, is_array: bool, i: usize, val: JsValue,
) -> Result<(), JsValue> {
    if is_array {
        unsafe {
            (*a).set_prop_at(i, val);
        }
        return Ok(());
    }
    let key = vm.new_string(&i.to_string());
    let key_si = vm.property_key_si(key);
    create_data_property_or_throw(vm, unsafe { &mut *a }, key_si, val)
}

/// 收尾设置 Array.from 结果对象的 length：真数组直接改元素计数，普通对象走
/// 普通 Set（可触发继承的 length setter 并透传其异常）。
fn array_from_set_length<H: VmHost>(vm: &mut H, a: *mut JsObject, is_array: bool, len: usize) -> Result<(), JsValue> {
    if is_array {
        unsafe {
            (*a).set_prop_count_fast(len);
        }
        return Ok(());
    }
    let a_obj = unsafe { &mut *a };
    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    let a_val = JsValue::from_js_object(a);
    match vm.ordinary_set(a_obj, length_si, JsValue::int(len as i32), a_val) {
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
            let _ = vm.call_function_sync(ret, iterator, &[]);
        }
    }
}

/// `Array.prototype.push(...items)`：追加元素到尾部，返回新长度。
pub fn array_push<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.push called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    for &arg_reg in args.iter().skip(1) {
        let val = vm.promote_if_needed_for_write_ptr(arr_ptr, vm.reg(arg_reg));
        // 元素写入须维护 array_prop_count（set_prop_at 对数组自动更新）。
        let arr = unsafe { &mut *arr_ptr };
        let idx = arr.prop_count();
        arr.set_prop_at(idx, val);
    }
    let len = unsafe { &*arr_ptr }.prop_count();
    NativeResult::Ok(JsValue::int(len as i32))
}

/// `Array.prototype.pop()`：移除并返回末位元素；空数组返回 undefined。
pub fn array_pop<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.pop called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count();
    if len == 0 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let last = arr.get_prop_at(len - 1);
    arr.set_prop_count_fast(len - 1);
    NativeResult::Ok(last)
}

/// `Array.prototype.slice(start, end)`：复制区间元素返回新数组（支持负索引）。
pub fn array_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.slice called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let n_isize = n as isize;
    let rel_start = if args.len() > 1 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1])) as isize
    } else {
        0
    };
    let rel_end = if args.len() > 2 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2])) as isize
    } else {
        n_isize
    };
    let start = if rel_start < 0 {
        (n_isize + rel_start).max(0)
    } else {
        rel_start.min(n_isize)
    } as usize;
    let end = if rel_end < 0 { (n_isize + rel_end).max(0) } else { rel_end.min(n_isize) } as usize;
    let count = end.saturating_sub(start);
    let new_arr = create_new_array(vm, count);
    unsafe {
        for i in 0..count {
            (*new_arr).set_prop_at(i, arraylike_get(vm, arr_ptr, is_array, start + i));
        }
        (*new_arr).set_prop_count(count);
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// `Array.prototype.splice(start, deleteCount, ...items)`：删除并/或插入元素，
/// 返回被删除元素组成的新数组。
pub fn array_splice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.splice called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let n = arr.prop_count() as usize;

    let start = if args.len() > 1 {
        let v = vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN);
        let s = v as i32;
        if s < 0 {
            (n as i32 + s).max(0) as usize
        } else {
            (s as usize).min(n)
        }
    } else {
        0
    };

    let delete_count = if args.len() > 2 {
        let v = vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN);
        (v as usize).min(n - start)
    } else {
        n - start
    };

    let insert_count = if args.len() > 3 { args.len() - 3 } else { 0 };

    let mut removed: Vec<JsValue> = Vec::new();
    for i in 0..delete_count {
        removed.push(arr.get_prop_at(start + i));
    }

    if insert_count > delete_count {
        let shift = insert_count - delete_count;
        for i in (start + delete_count..n).rev() {
            let val = arr.get_prop_at(i);
            arr.set_prop_at(i + shift, val);
        }
    } else if insert_count < delete_count {
        let shift = delete_count - insert_count;
        for i in start + delete_count..n {
            let val = arr.get_prop_at(i);
            arr.set_prop_at(i - shift, val);
        }
    }

    for i in 0..insert_count {
        arr.set_prop_at(start + i, vm.reg(args[3 + i]));
    }

    let new_len = n + insert_count - delete_count;
    arr.set_prop_count_fast(new_len);

    let removed_arr = create_new_array(vm, removed.len());
    unsafe {
        for (i, val) in removed.iter().enumerate() {
            (*removed_arr).set_prop_at(i, *val);
        }
        (*removed_arr).set_prop_count(removed.len());
    }
    NativeResult::Ok(JsValue::from_js_object(removed_arr))
}

/// `Array.prototype.concat(...items)`：连接 this 与参数（数组参数展开）返回新数组。
pub fn array_concat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.concat called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let mut all: Vec<JsValue> = Vec::new();
    for i in 0..n {
        all.push(arraylike_get(vm, arr_ptr, is_array, i));
    }
    for &arg_reg in args.iter().skip(1) {
        let val = vm.reg(arg_reg);
        if val.is_object() {
            let o_ptr = val.as_js_object_ptr();
            if !o_ptr.is_null() {
                let o = unsafe { &*o_ptr };
                let on = o.prop_count() as usize;
                // 真数组时展开元素。
                if o.is_array() && on > 0 {
                    for i in 0..on {
                        all.push(o.get_prop_at(i));
                    }
                    continue;
                }
            }
        }
        all.push(val);
    }
    let new_arr = create_new_array(vm, all.len());
    unsafe {
        for (i, val) in all.iter().enumerate() {
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(all.len());
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// `Array.prototype.join(separator)`：用分隔符连接元素字符串（null/undefined 视为空串）。
pub fn array_join<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.join called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let sep = if args.len() > 1 {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        ",".to_string()
    };
    let parts: Vec<String> = (0..n)
        .map(|i| {
            let v = arraylike_get(vm, arr_ptr, is_array, i);
            if v.is_undefined() || v.is_null() {
                String::new()
            } else {
                oxide_runtime_api::to_string(v)
            }
        })
        .collect();
    let joined = parts.join(&sep);
    NativeResult::Ok(vm.new_string(&joined))
}

/// `Array.prototype.toString`：委托给 join，默认用 `,` 分隔。
pub fn array_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    // Array.prototype.toString() 委托给 join，默认用 "," 分隔，
    // 按规范忽略自身参数。
    array_join(vm, &[args[0]])
}

/// `Array.prototype.indexOf(searchElement, fromIndex)`：用严格相等查找首个匹配索引，找不到返回 -1。
pub fn array_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.indexOf called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if n == 0 || args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let target = vm.reg(args[1]);
    // fromIndex：负索引从尾部折算，NaN 视为 0。
    let from_index = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = vm.coerce_number_bounded(v).unwrap_or(0.0);
        let f = if f.is_nan() { 0.0 } else { f.trunc() };
        if f >= 0.0 {
            (f as usize).min(n)
        } else {
            let from = n as f64 + f;
            if from < 0.0 {
                0
            } else {
                from as usize
            }
        }
    } else {
        0
    };
    for i in from_index..n {
        let elem = arraylike_get(vm, ptr, is_array, i);
        if oxide_runtime_api::strict_eq(elem, target) {
            return NativeResult::Ok(JsValue::int(i as i32));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `Array.prototype.includes(searchElement, fromIndex)`：用 SameValueZero 判断是否包含
/// （NaN 视为存在、+0/-0 视为相同）。
pub fn array_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.includes called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if n == 0 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let target = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let from_index = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = vm.coerce_number_bounded(v).unwrap_or(0.0);
        let f = if f.is_nan() { 0.0 } else { f.trunc() };
        if f >= 0.0 {
            (f as usize).min(n)
        } else {
            let from = n as f64 + f;
            if from < 0.0 {
                0
            } else {
                from as usize
            }
        }
    } else {
        0
    };
    for i in from_index..n {
        let elem = arraylike_get(vm, ptr, is_array, i);
        // SameValueZero：NaN 视为相等、+0 与 -0 视为相同。
        if same_value_zero(elem, target) {
            return NativeResult::Ok(JsValue::bool(true));
        }
    }
    NativeResult::Ok(JsValue::bool(false))
}

/// SameValueZero（ES2015 §7.2.10）：NaN 视为相等、+0 与 -0 视为相同。
fn same_value_zero(a: JsValue, b: JsValue) -> bool {
    // 双数字时特殊处理。
    let a_is_num = a.is_int() || a.is_double();
    let b_is_num = b.is_int() || b.is_double();
    if a_is_num && b_is_num {
        let av = if a.is_int() { a.as_int() as f64 } else { a.as_double() };
        let bv = if b.is_int() { b.as_int() as f64 } else { b.as_double() };
        if av.is_nan() && bv.is_nan() {
            return true;
        }
        return av == bv; // Rust f64 中 +0 == -0。
    }
    oxide_runtime_api::strict_equality(a, b)
}

/// `Array.prototype.reverse()`：原地反转元素顺序，返回 this。
pub fn array_reverse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.reverse called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let n = arr.prop_count() as usize;
    let mut i = 0;
    let mut j = n.saturating_sub(1);
    while i < j {
        let tmp = arr.get_prop_at(i);
        arr.set_prop_at(i, arr.get_prop_at(j));
        arr.set_prop_at(j, tmp);
        i += 1;
        j = j.saturating_sub(1);
    }
    NativeResult::Ok(vm.reg(args[0]))
}

/// `Array.prototype.flat(depth)`：按深度递归展开嵌套数组（含循环引用保护）返回新数组。
pub fn array_flat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.flat called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    let depth = if args.len() > 1 {
        let n = vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN);
        if !n.is_finite() {
            vm.kernel_core().config.max_call_depth
        } else {
            (n as i32).max(1) as usize
        }
    } else {
        1
    }
    .min(vm.kernel_core().config.max_call_depth);

    fn flatten(items: &[JsValue], remaining_depth: usize, seen: &mut Vec<*mut JsObject>) -> Vec<JsValue> {
        let mut out = Vec::new();
        for &v in items {
            if remaining_depth > 0 && v.is_object() {
                let ptr = v.as_js_object_ptr();
                if !ptr.is_null() {
                    if seen.iter().any(|seen_ptr| std::ptr::eq(*seen_ptr, ptr)) {
                        out.push(v);
                        continue;
                    }
                    let o = unsafe { &*ptr };
                    if o.is_array() {
                        seen.push(ptr);
                        let on = o.prop_count() as usize;
                        let sub: Vec<JsValue> = (0..on).map(|i| o.get_prop_at(i)).collect();
                        let flat = flatten(&sub, remaining_depth - 1, seen);
                        seen.pop();
                        out.extend(flat);
                        continue;
                    }
                }
            }
            out.push(v);
        }
        out
    }

    let all: Vec<JsValue> = (0..n).map(|i| arr.get_prop_at(i)).collect();
    let mut seen = vec![arr_ptr];
    let flat = flatten(&all, depth, &mut seen);
    let new_arr = create_new_array(vm, flat.len());
    unsafe {
        for (i, val) in flat.iter().enumerate() {
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(flat.len());
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// `Array.prototype.forEach(callback, thisArg)`：对每个元素调用 callback，返回 undefined。
pub fn array_for_each<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.forEach called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(_) => {}
            NativeResult::Err(err) => return NativeResult::Err(err),
            NativeResult::TailCall { .. } => return unexpected_tail_call_error(vm),
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Array.prototype.map(callback, thisArg)`：对每个元素调用 callback 生成新数组。
pub fn array_map<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.map called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let new_arr = create_new_array(vm, n);
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(mapped) => unsafe {
                (*new_arr).set_prop_at(i, mapped);
            },
            NativeResult::Err(err) => return NativeResult::Err(err),
            NativeResult::TailCall { .. } => return unexpected_tail_call_error(vm),
        }
    }
    unsafe {
        (*new_arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// `Array.prototype.filter(callback, thisArg)`：保留 callback 返回真值的元素形成新数组。
pub fn array_filter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.filter called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.filter: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.filter: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut kept: Vec<JsValue> = Vec::new();
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(result_val) => {
                if oxide_runtime_api::to_boolean(result_val) {
                    kept.push(elem);
                }
            }
            NativeResult::Err(err) => return NativeResult::Err(err),
            NativeResult::TailCall { .. } => return unexpected_tail_call_error(vm),
        }
    }
    let new_arr = create_new_array(vm, kept.len());
    unsafe {
        for (i, val) in kept.iter().enumerate() {
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(kept.len());
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// `Array.prototype.reduce(callback, initialValue)`：从左到右累计归约；
/// 空数组且无初始值抛 TypeError。
pub fn array_reduce<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.reduce called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if n == 0 && args.len() < 3 {
        builtins_error!("Array.prototype.reduce: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "Reduce of empty array with no initial value"));
    }
    if args.len() < 2 {
        builtins_error!("Array.prototype.reduce: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.reduce: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let has_initial = args.len() > 2;
    let mut accumulator;
    let start_idx;
    if has_initial {
        accumulator = vm.reg(args[2]);
        start_idx = 0;
    } else {
        accumulator = arraylike_get(vm, arr_ptr, is_array, 0);
        start_idx = 1;
    }
    let this_val = JsValue::undefined();
    for i in start_idx..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[accumulator, elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            NativeResult::Ok(result) => accumulator = result,
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.reduce: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.reduce: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(accumulator)
}

/// `Array.prototype.find(callback, thisArg)`：返回首个 callback 为真的元素，否则 undefined。
pub fn array_find<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.find called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.find: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.find: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(result_val) => {
                if oxide_runtime_api::to_boolean(result_val) {
                    return NativeResult::Ok(elem);
                }
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.find: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.find: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Array.prototype.some(callback, thisArg)`：任一元素满足 callback 返回 true。
pub fn array_some<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.some called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.some: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.some: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(result_val) => {
                if oxide_runtime_api::to_boolean(result_val) {
                    return NativeResult::Ok(JsValue::bool(true));
                }
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.some: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.some: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(JsValue::bool(false))
}

/// `Array.prototype.every(callback, thisArg)`：所有元素满足 callback 才返回 true。
pub fn array_every<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.every called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.every: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.every: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(result_val) => {
                if !oxide_runtime_api::to_boolean(result_val) {
                    return NativeResult::Ok(JsValue::bool(false));
                }
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.every: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.every: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `Array.prototype.flatMap(callback, thisArg)`：map 后把返回的数组展开一层。
pub fn array_flat_map<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.flatMap called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.flatMap: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.flatMap: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut flat: Vec<JsValue> = Vec::new();
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(result) => {
                if result.is_object() {
                    let r_ptr = result.as_js_object_ptr();
                    if !r_ptr.is_null() {
                        let r = unsafe { &*r_ptr };
                        if r.is_array() {
                            let rn = r.prop_count() as usize;
                            for j in 0..rn {
                                flat.push(r.get_prop_at(j));
                            }
                            continue;
                        }
                    }
                }
                flat.push(result);
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.flatMap: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.flatMap: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    let new_arr = create_new_array(vm, flat.len());
    unsafe {
        for (i, val) in flat.iter().enumerate() {
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(flat.len());
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// `Array.prototype.shift()`：移除并返回首元素，其余元素前移；空数组返回 undefined。
pub fn array_shift<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.shift called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count();
    if len == 0 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let first = arr.get_prop_at(0);
    for i in 1..len {
        let v = arr.get_prop_at(i);
        arr.set_prop_at(i - 1, v);
    }
    arr.set_prop_count_fast(len - 1);
    NativeResult::Ok(first)
}

/// `Array.prototype.unshift(...items)`：插入元素到头部，返回新长度。
pub fn array_unshift<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.unshift called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count();
    let n_items = args.len().saturating_sub(1);
    for i in (0..len as usize).rev() {
        let v = arr.get_prop_at(i);
        arr.set_prop_at(i + n_items, v);
    }
    for (j, &arg_reg) in args.iter().skip(1).enumerate() {
        arr.set_prop_at(j, vm.reg(arg_reg));
    }
    let new_len = len as usize + n_items;
    arr.set_prop_count_fast(new_len);
    NativeResult::Ok(JsValue::int(new_len as i32))
}

/// `Array.prototype.fill(value, start, end)`：用给定值填充区间，返回 this。
pub fn array_fill<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.fill called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count() as isize;
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let rel_start = if args.len() > 2 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2])) as isize
    } else {
        0
    };
    let rel_end = if args.len() > 3 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[3])) as isize
    } else {
        len
    };
    let start = (if rel_start < 0 { (len + rel_start).max(0) } else { rel_start.min(len) }) as usize;
    let end = (if rel_end < 0 { (len + rel_end).max(0) } else { rel_end.min(len) }) as usize;
    for i in start..end {
        arr.set_prop_at(i, value);
    }
    NativeResult::Ok(vm.reg(args[0]))
}

/// `Array.prototype.copyWithin(target, start, end)`：在数组内部复制元素区间，返回 this。
pub fn array_copy_within<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.copyWithin called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count() as isize;
    if len == 0 {
        return NativeResult::Ok(vm.reg(args[0]));
    }
    let rel_target = if args.len() > 1 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1])) as isize
    } else {
        0
    };
    let rel_start = if args.len() > 2 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2])) as isize
    } else {
        0
    };
    let rel_end = if args.len() > 3 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[3])) as isize
    } else {
        len
    };
    let target = (if rel_target < 0 { (len + rel_target).max(0) } else { rel_target.min(len) }) as usize;
    let start = (if rel_start < 0 { (len + rel_start).max(0) } else { rel_start.min(len) }) as usize;
    let end = (if rel_end < 0 { (len + rel_end).max(0) } else { rel_end.min(len) }) as usize;
    let len_usize = len as usize;
    for (to, from) in (target..).zip(start..end) {
        if to >= len_usize {
            break;
        }
        let v = arr.get_prop_at(from);
        arr.set_prop_at(to, v);
    }
    NativeResult::Ok(vm.reg(args[0]))
}

/// `Array.prototype.at(index)`：按索引取元素，支持负索引；越界返回 undefined。
pub fn array_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.at called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let len = arr.prop_count() as i32;
    let mut index = if args.len() > 1 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32
    } else {
        0
    };
    if index < 0 {
        index += len;
    }
    if index < 0 || index >= len {
        return NativeResult::Ok(JsValue::undefined());
    }
    NativeResult::Ok(arr.get_prop_at(index))
}

/// `Array.prototype.lastIndexOf(searchElement, fromIndex)`：从后往前查找首个匹配索引。
pub fn array_last_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.lastIndexOf called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if n == 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let search = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // fromIndex（缺省：n-1）。
    let from_index_isize: isize = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = vm.coerce_number_bounded(v).unwrap_or(0.0);
        if f.is_nan() {
            return NativeResult::Ok(JsValue::int(-1));
        }
        let f = f.trunc();
        if f >= 0.0 {
            (f as isize).min(n as isize - 1)
        } else {
            n as isize + f as isize
        }
    } else {
        n as isize - 1
    };
    if from_index_isize < 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    for i in (0..=from_index_isize as usize).rev() {
        let elem = arraylike_get(vm, ptr, is_array, i);
        if oxide_runtime_api::strict_eq(elem, search) {
            return NativeResult::Ok(JsValue::int(i as i32));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `Array.prototype.findIndex(callback, thisArg)`：返回首个 callback 为真的索引，否则 -1。
pub fn array_find_index<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.findIndex called with {} args", args.len());
    let (arr_ptr, n, is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.findIndex: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.findIndex: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..n {
        let elem = arraylike_get(vm, arr_ptr, is_array, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i as i32), vm.reg(args[0])]) {
            NativeResult::Ok(r) => {
                if oxide_runtime_api::to_boolean(r) {
                    return NativeResult::Ok(JsValue::int(i as i32));
                }
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.findIndex: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.findIndex: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `Array.prototype.findLast(callback, thisArg)`：从后往前返回首个 callback 为真的元素。
pub fn array_find_last<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.findLast called with {} args", args.len());
    let (arr_ptr, n) = {
        let (arr_ptr, len, _is_array) = array_ptr_len3!(vm, args);
        (arr_ptr, len as i32)
    };
    if args.len() < 2 {
        builtins_error!("Array.prototype.findLast: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.findLast: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in (0..n).rev() {
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i), vm.reg(args[0])]) {
            NativeResult::Ok(r) => {
                if oxide_runtime_api::to_boolean(r) {
                    return NativeResult::Ok(elem);
                }
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.findLast: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.findLast: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Array.prototype.reduceRight(callback, initialValue)`：从右到左累计归约。
pub fn array_reduce_right<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.reduceRight called with {} args", args.len());
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    if args.len() < 2 {
        builtins_error!("Array.prototype.reduceRight: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.reduceRight: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let (mut acc, start_idx): (JsValue, i32) = if args.len() > 2 {
        (vm.reg(args[2]), n as i32 - 1)
    } else {
        if n == 0 {
            builtins_error!("Array.prototype.reduceRight: invalid receiver");
            return NativeResult::Err(array_type_error(vm, "Reduce of empty array with no initial value"));
        }
        (unsafe { (*arr_ptr).get_prop_at(n - 1) }, n as i32 - 2)
    };
    for i in (0..=start_idx).rev() {
        let elem = unsafe { (*arr_ptr).get_prop_at(i as usize) };
        match invoke_native_callback(
            vm,
            callback_val,
            JsValue::undefined(),
            &[acc, elem, JsValue::int(i), vm.reg(args[0])],
        ) {
            NativeResult::Ok(r) => acc = r,
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.reduceRight: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.reduceRight: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(acc)
}

/// `Array.prototype.sort(compareFn)`：原地排序。默认按字符串字典序；
/// 提供比较函数时按其返回值（<0/=0/>0）排序，回调抛错则中止。
pub fn array_sort<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.sort called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let len = unsafe { (*arr_ptr).prop_count() as usize };
    let mut vals: Vec<JsValue> = (0..len).map(|i| unsafe { (*arr_ptr).get_prop_at(i) }).collect();
    let comparator = if args.len() > 1 {
        let candidate = vm.reg(args[1]);
        if candidate.is_undefined() {
            None
        } else {
            match require_callback(vm, candidate) {
                Ok(callback) => Some(callback),
                Err(err) => {
                    builtins_error!("Array.prototype.sort: invalid receiver");
                    return NativeResult::Err(err);
                }
            }
        }
    } else {
        None
    };
    let mut sort_error = None;
    vals.sort_by(|a, b| {
        if sort_error.is_some() {
            return std::cmp::Ordering::Equal;
        }
        if let Some(callback) = comparator {
            match invoke_native_callback(vm, callback, JsValue::undefined(), &[*a, *b]) {
                NativeResult::Ok(result) => {
                    let n = oxide_runtime_api::to_number(result);
                    if n.is_nan() || n == 0.0 {
                        std::cmp::Ordering::Equal
                    } else if n < 0.0 {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                NativeResult::Err(err) => {
                    sort_error = Some(err);
                    std::cmp::Ordering::Equal
                }
                NativeResult::TailCall { .. } => {
                    sort_error = Some(crate::error::create_type_error(vm, "unexpected tail call in array callback"));
                    std::cmp::Ordering::Equal
                }
            }
        } else {
            let sa = oxide_runtime_api::to_string(*a);
            let sb = oxide_runtime_api::to_string(*b);
            sa.cmp(&sb)
        }
    });
    if let Some(err) = sort_error {
        builtins_error!("Array.prototype.sort: invalid receiver");
        return NativeResult::Err(err);
    }
    let arr = unsafe { &mut *arr_ptr };
    for (i, &v) in vals.iter().enumerate() {
        arr.set_prop_at(i, v);
    }
    NativeResult::Ok(vm.reg(args[0]))
}

/// `Array.prototype.values()`：返回迭代数组元素的迭代器。
pub fn array_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.values called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    match crate::iterator::make_iterator_for_value(vm, this_val) {
        Ok(iterator) => NativeResult::Ok(iterator),
        Err(err) => {
            builtins_error!("Array.prototype.values: invalid receiver");
            NativeResult::Err(err)
        }
    }
}
