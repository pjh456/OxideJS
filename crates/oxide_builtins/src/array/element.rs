//! Array 元素变更与查找方法（push/pop/slice/splice/concat/join/indexOf 等）。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

use super::common::{
    array_ptr, array_type_error, arraylike_get, arraylike_get_or_err, check_array_create_len, clamp_relative,
    create_new_array, get_this_array_ref, get_this_arraylike, js_array_index, to_integer_or_infinity_bounded,
};
use super::from::{array_species_create, create_data_property_or_throw, from_engine_error};

/// `Array.prototype.push(...items)`：追加元素到尾部，返回新长度。
///
/// # 步骤
/// 1. 读 `LengthOfArrayLike(this)` 作起始索引。
/// 2. 每个实参按当前索引生成属性键，以严格模式 `[[Set]]` 写入（继承 setter /
///    不可写元素按规范抛 TypeError），写入成功后才递增索引。
/// 3. 以 `Set(this, "length", len, true)` 收尾，length 不可写时抛 TypeError。
///
/// # 副作用
/// - 修改元素区与 length；元素写入可经原型链触发用户 setter。
pub fn array_push<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.push called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let recv = JsValue::from_js_object(arr_ptr);
    let mut len = unsafe { &*arr_ptr }.logical_len() as usize;

    for &arg_reg in args.iter().skip(1) {
        let key = vm.string_key_si(&len.to_string());
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, key, vm.reg(arg_reg), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
        len += 1;
    }

    // length 收尾同样走 Set：不可写 length 在此抛 TypeError。
    let length_si = vm.string_key_si("length");
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(len), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(js_array_index(len))
}

/// `Array.prototype.pop()`：移除并返回末位元素；空数组返回 undefined。
///
/// # 步骤
/// 1. 读 `LengthOfArrayLike(this)`；为 0 时以 `Set(this, "length", +0, true)` 收尾。
/// 2. 否则按 Get / DeletePropertyOrThrow / Set length 的顺序处理末位元素。
///
/// # 副作用
/// - 删除末位元素并收缩 length；Get 可触发原型链 getter，length 不可写时抛 TypeError。
pub fn array_pop<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.pop called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let recv = JsValue::from_js_object(arr_ptr);
    let len = unsafe { &*arr_ptr }.logical_len() as usize;
    let length_si = vm.string_key_si("length");
    if len == 0 {
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, JsValue::int(0), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
        return NativeResult::Ok(JsValue::undefined());
    }
    let index = len - 1;
    let key = vm.string_key_si(&index.to_string());
    let last = match vm.ordinary_get(unsafe { &*arr_ptr }, key, recv) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };

    // DeletePropertyOrThrow：不可配置元素删除失败抛 TypeError，否则标记为 hole。
    match unsafe { &*arr_ptr }.prop_meta_at(index) {
        Some(meta) if !meta.attributes.configurable() => {
            return NativeResult::Err(array_type_error(vm, "Cannot delete property"));
        }
        _ => unsafe { (*arr_ptr).mark_hole_at(index) },
    }

    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(index), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
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
    let n_f = n as f64;
    // ToIntegerOrInfinity(start)，缺省 0；对象经 ToPrimitive(number)。
    let rel_start = if args.len() > 1 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        0.0
    };
    // 规范：end 为 undefined 时使用 len；其余走 ToIntegerOrInfinity。
    let rel_end = if args.len() > 2 && !vm.reg(args[2]).is_undefined() {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        n_f
    };
    let start = clamp_relative(rel_start, n_f, n);
    let end = clamp_relative(rel_end, n_f, n);
    let count = end.saturating_sub(start);
    if let Err(err) = check_array_create_len(vm, count) {
        return NativeResult::Err(err);
    }
    // ArraySpeciesCreate(O, count)：真数组按 constructor/@@species 构造目标。
    let a_ptr = match array_species_create(vm, this_val, is_array, count) {
        Ok(p) => p,
        Err(err) => return NativeResult::Err(err),
    };
    // HasProperty + Get + CreateDataPropertyOrThrow 逐元素拷贝。
    let mut k = start;
    let mut out_idx = 0usize;
    while k < end {
        let key_str = vm.new_string(&k.to_string());
        let key_si = vm.property_key_si(key_str);
        if vm.resolve_property(unsafe { &*arr_ptr }, key_si).is_some() {
            let elem = arraylike_get_or_err!(vm, arr_ptr, k);
            let out_key = vm.new_string(&out_idx.to_string());
            let out_key_si = vm.property_key_si(out_key);
            if let Err(err) = create_data_property_or_throw(vm, unsafe { &mut *a_ptr }, out_key_si, elem) {
                return NativeResult::Err(err);
            }
            out_idx += 1;
        }
        k += 1;
    }
    // 收尾 Set(A, "length", final)：普通 Set 语义（可触发继承 setter/异常）。
    let a_val = JsValue::from_js_object(a_ptr);
    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    if let Err(err) = vm.ordinary_set(unsafe { &mut *a_ptr }, length_si, js_array_index(out_idx), a_val, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(JsValue::from_js_object(a_ptr))
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
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let mut all: Vec<JsValue> = Vec::new();
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    for i in 0..n {
        all.push(arraylike_get_or_err!(vm, arr_ptr, i));
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
    let (arr_ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let sep = if args.len() > 1 {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        ",".to_string()
    };
    let parts: Vec<String> = match (0..n)
        .map(|i| {
            let v = arraylike_get(vm, arr_ptr, i)?;
            if v.is_undefined() || v.is_null() {
                Ok(String::new())
            } else {
                Ok(oxide_runtime_api::to_string(v))
            }
        })
        .collect::<Result<Vec<String>, JsValue>>()
    {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
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
    let (ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
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
        let elem = arraylike_get_or_err!(vm, ptr, i);
        if oxide_runtime_api::strict_equality(elem, target) {
            return NativeResult::Ok(js_array_index(i));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `Array.prototype.includes(searchElement, fromIndex)`：用 SameValueZero 判断是否包含
/// （NaN 视为存在、+0/-0 视为相同）。
pub fn array_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.includes called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    let (ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
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
        let elem = arraylike_get_or_err!(vm, ptr, i);
        // SameValueZero：NaN 视为相等、+0 与 -0 视为相同。
        if oxide_runtime_api::same_value_zero(elem, target) {
            return NativeResult::Ok(JsValue::bool(true));
        }
    }
    NativeResult::Ok(JsValue::bool(false))
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

/// `Array.prototype.shift()`：移除并返回首元素，其余元素前移；空数组返回 undefined。
///
/// # 步骤
/// 1. 读 `LengthOfArrayLike(this)`；为 0 时以 `Set(this, "length", +0, true)` 收尾。
/// 2. Get 首元素后，把 `1..len` 逐位前移：源存在则 Get + 严格 Set 到前一格，
///    源缺失（hole）则 DeletePropertyOrThrow 目标格。
/// 3. 以 `Set(this, "length", len-1, true)` 收尾。
///
/// # 副作用
/// - 元素整体前移并收缩 length；Get/Set 可经原型链触发用户代码。
pub fn array_shift<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.shift called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let recv = JsValue::from_js_object(arr_ptr);
    let len = unsafe { &*arr_ptr }.logical_len() as usize;
    let length_si = vm.string_key_si("length");
    if len == 0 {
        if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, JsValue::int(0), recv, true) {
            return NativeResult::Err(from_engine_error(vm, &err));
        }
        return NativeResult::Ok(JsValue::undefined());
    }
    let first_key = vm.string_key_si("0");
    let first = match vm.ordinary_get(unsafe { &*arr_ptr }, first_key, recv) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };

    for k in 1..len {
        let from = vm.string_key_si(&k.to_string());
        let to = vm.string_key_si(&(k - 1).to_string());
        // HasProperty 走原型链存在性判定（hole 视同缺失），命中才 Get + Set。
        if vm.resolve_property(unsafe { &*arr_ptr }, from).is_some() {
            let val = match vm.ordinary_get(unsafe { &*arr_ptr }, from, recv) {
                Ok(v) => v,
                Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
            };
            if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to, val, recv, true) {
                return NativeResult::Err(from_engine_error(vm, &err));
            }
        } else {
            // DeletePropertyOrThrow(O, to)：不可配置元素删除失败抛 TypeError。
            match unsafe { &*arr_ptr }.prop_meta_at(k - 1) {
                Some(meta) if !meta.attributes.configurable() => {
                    return NativeResult::Err(array_type_error(vm, "Cannot delete property"));
                }
                _ => unsafe { (*arr_ptr).mark_hole_at(k - 1) },
            }
        }
    }

    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(len - 1), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(first)
}

/// `Array.prototype.unshift(...items)`：插入元素到头部，返回新长度。
///
/// # 步骤
/// 1. 读 `LengthOfArrayLike(this)`；`len + 参数数` 超出 2^53-1 时抛 TypeError。
/// 2. 参数数大于 0 时自高到低搬移已有元素（源存在则 Get + 严格 Set，缺失则
///    DeletePropertyOrThrow），再把实参逐个严格 Set 到头部。
/// 3. 以 `Set(this, "length", len+参数数, true)` 收尾。
///
/// # 副作用
/// - 元素整体后移并扩容 length；Get/Set 可经原型链触发用户代码。
pub fn array_unshift<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.unshift called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let recv = JsValue::from_js_object(arr_ptr);
    let len = unsafe { &*arr_ptr }.logical_len() as usize;
    let n_items = args.len().saturating_sub(1);
    let length_si = vm.string_key_si("length");
    if n_items > 0 {
        if len as f64 + n_items as f64 > 9_007_199_254_740_991.0 {
            return NativeResult::Err(array_type_error(vm, "Invalid array length"));
        }
        for k in (1..=len).rev() {
            let from = vm.string_key_si(&(k - 1).to_string());
            let to_idx = k - 1 + n_items;
            let to = vm.string_key_si(&to_idx.to_string());
            if vm.resolve_property(unsafe { &*arr_ptr }, from).is_some() {
                let val = match vm.ordinary_get(unsafe { &*arr_ptr }, from, recv) {
                    Ok(v) => v,
                    Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
                };
                if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, to, val, recv, true) {
                    return NativeResult::Err(from_engine_error(vm, &err));
                }
            } else {
                // DeletePropertyOrThrow(O, to)：不可配置元素删除失败抛 TypeError。
                match unsafe { &*arr_ptr }.prop_meta_at(to_idx) {
                    Some(meta) if !meta.attributes.configurable() => {
                        return NativeResult::Err(array_type_error(vm, "Cannot delete property"));
                    }
                    _ => unsafe { (*arr_ptr).mark_hole_at(to_idx) },
                }
            }
        }
        for (j, &arg_reg) in args.iter().skip(1).enumerate() {
            let key = vm.string_key_si(&j.to_string());
            if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, key, vm.reg(arg_reg), recv, true) {
                return NativeResult::Err(from_engine_error(vm, &err));
            }
        }
    }

    let new_len = len + n_items;
    if let Err(err) = vm.ordinary_set(unsafe { &mut *arr_ptr }, length_si, js_array_index(new_len), recv, true) {
        return NativeResult::Err(from_engine_error(vm, &err));
    }
    NativeResult::Ok(js_array_index(new_len))
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
    let (ptr, n, _is_array) = match get_this_arraylike(vm, this_val) {
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
        let elem = arraylike_get_or_err!(vm, ptr, i);
        if oxide_runtime_api::strict_equality(elem, search) {
            return NativeResult::Ok(js_array_index(i));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}
