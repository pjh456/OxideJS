//! Array ES2023 不可变方法族（toReversed/with/toSorted/toSpliced）。

use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;

use super::common::{
    arraylike_get, arraylike_get_or_err, check_array_create_len, create_new_array, get_this_arraylike,
};
use super::from::from_engine_error;
use super::sort_iterator::{parse_sort_comparator, sort_values_inner};

/// Array.prototype.toReversed()：返回元素逆序的新数组，原数组不变。
pub fn array_to_reversed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.toReversed called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let new_arr = create_new_array(vm, n);
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    // 按倒序逐个读取原数组元素（规范 Get，访问器 getter 触发），写入新数组对应槽位：
    // 新数组即原数组元素的逆序。
    for i in (0..n).rev() {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        unsafe {
            (*new_arr).set_prop_at(n - 1 - i, elem);
        }
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}

/// Array.prototype.with(index, value)：返回把指定下标替换为新值的新数组，原数组不变。
pub fn array_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.with called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let index_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let idx = match vm.coerce_number_bounded(index_val) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    // ToIntegerOrInfinity：NaN/-0/+0 -> 0，其余截断。
    let rel = if idx.is_nan() || idx == 0.0 { 0.0 } else { idx.trunc() };
    let actual = if rel < 0.0 { n as f64 + rel } else { rel };
    if actual < 0.0 || actual >= n as f64 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid index for Array.prototype.with"));
    }
    let actual_idx = actual as usize;
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    let new_arr = create_new_array(vm, n);
    for i in 0..n {
        // 规范：被替换位置不执行 Get。
        let elem = if i == actual_idx { value } else { arraylike_get_or_err!(vm, arr_ptr, i) };
        unsafe {
            (*new_arr).set_prop_at(i, elem);
        }
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}
/// 返回按比较语义排序的新数组，原数组不变（`Array.prototype.toSorted`）。
///
/// # 边界与前提
/// - 先校验 compareFn 可调用（非 undefined 且不可调用则抛 TypeError），再读取
///   this/length；
/// - 未提供比较回调时按 ToString 结果的字符串字典序排序；底层 `sort_by` 为稳定
///   排序，比较结果相等的元素保持原有相对次序。
pub fn array_to_sorted<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.toSorted called with {} args", args.len());
    let comparator = if args.len() > 1 {
        match parse_sort_comparator(vm, vm.reg(args[1])) {
            Ok(c) => c,
            Err(err) => return NativeResult::Err(err),
        }
    } else {
        None
    };
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    let mut vals: Vec<JsValue> = match (0..n)
        .map(|i| arraylike_get(vm, arr_ptr, i))
        .collect::<Result<Vec<JsValue>, JsValue>>()
    {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    if let Err(err) = sort_values_inner(vm, &mut vals, comparator) {
        return NativeResult::Err(err);
    }
    let new_arr = create_new_array(vm, n);
    for (i, &v) in vals.iter().enumerate() {
        unsafe {
            (*new_arr).set_prop_at(i, v);
        }
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}
/// 按 start/deleteCount/插入值三元 splice 语义返回新数组，原数组不变
///（`Array.prototype.toSpliced`）。
///
/// # 边界与前提
/// - 全缺省（无 start 与 deleteCount）按拷贝处理；仅缺 deleteCount 时从 start
///   删到末尾；
/// - start 为负时按 len + start 折算并夹到 `[0, len]`；deleteCount 为负或 NaN
///   视为 0，上限为 len - start。
pub fn array_to_spliced<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.toSpliced called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let relative_start = if args.len() > 1 {
        match vm.coerce_number_bounded(vm.reg(args[1])) {
            Ok(v) => v,
            Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
        }
    } else {
        0.0
    };
    let relative_start = if relative_start.is_nan() { 0.0 } else { relative_start.trunc() };
    let actual_start = if relative_start < 0.0 {
        (n as f64 + relative_start).max(0.0) as usize
    } else {
        relative_start.min(n as f64) as usize
    };
    // 缺省规则：start 与 deleteCount 都缺省 -> 拷贝；仅缺省 deleteCount -> 删到末尾。
    let delete_count = if args.len() <= 1 {
        0
    } else if args.len() == 2 {
        n - actual_start
    } else {
        let dc = match vm.coerce_number_bounded(vm.reg(args[2])) {
            Ok(v) => v,
            Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
        };
        let dc = if dc.is_nan() || dc <= 0.0 { 0 } else { dc.trunc() as usize };
        dc.min(n - actual_start)
    };
    let insert_count = if args.len() > 3 { args.len() - 3 } else { 0 };
    // 新长度 newLen = len + insertCount - actualDeleteCount 超过 2^53-1 抛 TypeError；
    // 再经 check_array_create_len 检查，超过 2^32-1 抛 RangeError。
    let new_len = n.saturating_add(insert_count).saturating_sub(delete_count);
    if new_len > 9_007_199_254_740_991 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Invalid array length"));
    }
    if let Err(err) = check_array_create_len(vm, new_len) {
        return NativeResult::Err(err);
    }
    let mut out: Vec<JsValue> = Vec::with_capacity(new_len);
    out.extend(
        match (0..actual_start)
            .map(|i| arraylike_get(vm, arr_ptr, i))
            .collect::<Result<Vec<JsValue>, JsValue>>()
        {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(err),
        },
    );
    for k in 0..insert_count {
        out.push(vm.reg(args[3 + k]));
    }
    out.extend(
        match (actual_start + delete_count..n)
            .map(|i| arraylike_get(vm, arr_ptr, i))
            .collect::<Result<Vec<JsValue>, JsValue>>()
        {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(err),
        },
    );
    let new_arr = create_new_array(vm, out.len());
    for (i, &v) in out.iter().enumerate() {
        unsafe {
            (*new_arr).set_prop_at(i, v);
        }
    }
    NativeResult::Ok(JsValue::from_js_object(new_arr))
}
