//! Array 族共享助手：`this`/类数组解析、长度与回调校验，以及 `array_ptr` 族宏。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, MAX_DENSE_PROPS};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use super::from::from_engine_error;

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
pub(crate) use array_ptr;

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
#[expect(unused_imports)] // 无调用点的预留宏，保留与其余助手宏一致的导出
pub(crate) use array_ptr_len;

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
pub(crate) use array_ptr_len3;

pub(crate) fn array_type_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_type_error(vm, msg)
}

pub(crate) fn get_this_array_ref<H: VmHost>(vm: &mut H, val: JsValue) -> Result<*mut JsObject, JsValue> {
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

/// 取 this 为真数组或类数组对象（带 length 属性的普通对象），按规范读取长度，
/// 返回 (对象指针, 长度, 是否为真数组)，供遍历查找族（forEach/map/filter/reduce/
/// some/every/find 族/flatMap）、切片拼接族（slice/concat/join/indexOf/includes/
/// lastIndexOf）与不可变复制族（toReversed/with/toSorted/toSpliced）共用。
///
/// # 步骤
/// 1. this 非对象（null/undefined/原始值）→ TypeError。
/// 2. 按规范 Get 语义读 length（getter 抛错向上传播，不走 prop_count 直读）。
/// 3. ToLength：ToIntegerOrInfinity 后负数与 NaN 归 0，上限 2^53-1。
#[inline(always)]
pub(crate) fn get_this_arraylike<H: VmHost>(vm: &mut H, val: JsValue) -> Result<(*mut JsObject, usize, bool), JsValue> {
    if !val.is_object() {
        return Err(array_type_error(vm, "Array.prototype method called on null or undefined"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(array_type_error(vm, "Array.prototype method called on null or undefined"));
    }
    let obj = unsafe { &*ptr };
    let is_array = obj.is_array();
    // 读取 length 属性（按规范 Get 语义，getter 抛错需向上传播）。
    // 真数组也走 Get：`arr.length = N` 已由 ordinary_set 的数组分支同步元素区，
    // 直接 prop_count() 会漏掉 length 赋值。
    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    let len_val = match vm.ordinary_get(unsafe { &*ptr }, length_si, val) {
        Ok(v) => v,
        Err(msg) => return Err(from_engine_error(vm, &msg)),
    };
    if len_val.is_symbol() {
        return Err(array_type_error(vm, "Cannot convert a Symbol value to a number"));
    }
    let len_num = match vm.coerce_number_bounded(len_val) {
        Ok(v) => v,
        Err(msg) => return Err(from_engine_error(vm, &msg)),
    };
    // ToLength：len 先按 ToIntegerOrInfinity 取整，负数与 NaN 归 0，上限 2^53-1。
    let len = if len_num.is_nan() || len_num <= 0.0 {
        0
    } else {
        len_num.min(9_007_199_254_740_991.0) as usize
    };
    Ok((ptr, len, is_array))
}

/// 从 arraylike 读取 index 位置元素（按规范 Get：访问器/原型链/异常完整传播）。
#[inline(always)]
pub(crate) fn arraylike_get<H: VmHost>(vm: &mut H, ptr: *mut JsObject, i: usize) -> Result<JsValue, JsValue> {
    // 真数组密集直读快路径：无元素元信息表（记录 hole/accessor 的可选侧表，
    // 空指针判定为 O(1)）且索引在元素区内时，直接读数据槽，免每元素构造数字串
    // + 属性键转换。语义与 ordinary_get 的数组分支一致：无元素元信息表保证该
    // 索引恒为数据属性，直读数据槽与规范 Get 结果相同。
    let obj = unsafe { &*ptr };
    if obj.is_array() && obj.array_elements_meta_vec().is_none() && i < obj.array_prop_count as usize {
        return Ok(obj.get_prop_at(i));
    }
    let key_str = vm.new_string(&i.to_string());
    let key_si = vm.property_key_si(key_str);
    let recv = JsValue::from_js_object(ptr);
    match vm.ordinary_get(obj, key_si, recv) {
        Ok(v) => Ok(v),
        Err(msg) => Err(from_engine_error(vm, &msg)),
    }
}

/// 字符串源的 array-like 单元序列；非字符串源返回 `None`。
///
/// String exotic 对象的 `length` 与索引属性在引擎内未物化，Array.from /
/// %TypedArray%.from 的 array-like 回退按 UTF-16 单元直接读：原始串取自身单元，
/// 装箱串取其 `[[StringData]]`（槽 0）。单元长度即 `length`，第 i 个索引为第 i 个码元。
///
/// # 边界
/// 装箱串槽 0 非字符串（异常形态）按非字符串源处理；指针为空返回 `None`。
pub(crate) fn string_arraylike_units<H: VmHost>(vm: &H, value: JsValue) -> Option<Vec<u16>> {
    let raw = if value.is_string() {
        value
    } else if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if ptr.is_null() {
            return None;
        }
        // SAFETY: ptr 来自对象 JsValue，指向存活的 JsObject。
        let obj = unsafe { &*ptr };
        if !obj.is_string_obj() {
            return None;
        }
        let data = obj.get_prop_at(0);
        if !data.is_string() {
            return None;
        }
        data
    } else {
        return None;
    };
    Some(vm.string_units(raw).into_owned())
}

/// 取单个 UTF-16 单元对应的字符串值：ASCII 单元命中共享 perm 串，其余按单元序列创建。
pub(crate) fn unit_string_value<H: VmHost>(vm: &mut H, unit: u16) -> JsValue {
    match vm.single_unit(unit) {
        Some(v) => v,
        None => vm.new_string_units(&[unit]),
    }
}

/// 索引位置存在性判定（规范 HasProperty：自身与原型链任一层命中即存在，
/// 数组元素区 hole 视同缺失，TypedArray 整数索引按视图长度判在界）。
pub(crate) fn arraylike_index_present<H: VmHost>(vm: &mut H, ptr: *mut JsObject, i: usize) -> bool {
    // SAFETY: ptr 来自 this/参数对象，指向存活对象。
    let key_si = vm.string_key_si(&i.to_string());
    vm.has_property(unsafe { &*ptr }, key_si)
}

/// 元素读取失败时直接把异常作为 NativeResult::Err 返回。
macro_rules! arraylike_get_or_err {
    ($vm:expr, $ptr:expr, $i:expr) => {
        match arraylike_get($vm, $ptr, $i) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(err),
        }
    };
}
pub(crate) use arraylike_get_or_err;

/// 数组下标转 JsValue：超出 i32 范围时用 double 精确表示。
#[inline(always)]
pub(crate) fn js_array_index(i: usize) -> JsValue {
    if i <= i32::MAX as usize {
        JsValue::int(i as i32)
    } else {
        JsValue::float(i as f64)
    }
}

/// ToIntegerOrInfinity 的 builtins 版本：对象经 ToPrimitive(number) 后取整，
/// NaN/±0 → 0，±Inf 保留；转换异常向上传播。
pub(crate) fn to_integer_or_infinity_bounded<H: VmHost>(vm: &mut H, val: JsValue) -> Result<f64, JsValue> {
    let n = match vm.coerce_number_bounded(val) {
        Ok(v) => v,
        Err(msg) => return Err(from_engine_error(vm, &msg)),
    };
    Ok(if n.is_nan() || n == 0.0 { 0.0 } else { n.trunc() })
}

/// 把 ToIntegerOrInfinity 结果夹到 [0, len]：负数按 len+rel，正无穷取 len。
#[inline(always)]
pub(crate) fn clamp_relative(rel: f64, n_f: f64, n: usize) -> usize {
    if rel.is_infinite() {
        if rel < 0.0 {
            0
        } else {
            n
        }
    } else if rel < 0.0 {
        (n_f + rel).max(0.0) as usize
    } else {
        rel.min(n_f) as usize
    }
}

/// IsConstructor 的近似判定（与 construct_array_from_result 保持一致）。
#[inline]
pub(crate) fn is_constructor_value(c: JsValue) -> bool {
    if !c.is_object() {
        return false;
    }
    let c_obj = unsafe { &*c.as_js_object_ptr() };
    c_obj.is_function()
        && !c_obj.is_arrow()
        && !(c_obj.native_fn().is_some() && c_obj.type_tag != JsObject::OBJ_TYPE_CONSTRUCTOR)
}

/// 校验回调参数为可调用函数对象（判空与 is_function 均须通过），不是则抛 TypeError。
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

pub(crate) fn create_new_array<H: VmHost>(vm: &mut H, n: usize) -> *mut JsObject {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
        n.min(MAX_DENSE_PROPS),
        vm.epoch().bump(),
    ))
}

/// ArrayCreate 前置检查：创建长度超过 2^32-1 时抛 RangeError
/// （密集数组长度按 u32 存储，超之不可容纳）。
pub(crate) fn check_array_create_len<H: VmHost>(vm: &mut H, n: usize) -> Result<(), JsValue> {
    if n > u32::MAX as usize {
        return Err(crate::error::create_range_error(vm, "Invalid array length"));
    }
    Ok(())
}

pub(crate) fn array_length_arg<H: VmHost>(vm: &mut H, value: JsValue) -> Result<usize, JsValue> {
    let n = vm.coerce_number_bounded(value).unwrap_or(f64::NAN);
    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > MAX_DENSE_PROPS as f64 {
        return Err(crate::error::create_range_error(vm, "Invalid array length"));
    }
    Ok(n as usize)
}

/// native 回调通用调用入口：先校验回调可调用（非对象、空指针、非函数均转
/// TypeError），再以 this_val 为接收者、cb_args 为实参同步调用用户回调；调用
/// 失败时优先取回未捕获的原始异常值，无原始值时按错误文本重建异常。
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
            None => NativeResult::Err(crate::error::create_from_text(vm, &err)),
        },
    }
}

pub(crate) fn unexpected_tail_call_error<H: VmHost>(vm: &mut H) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "unexpected tail call in array callback"))
}
