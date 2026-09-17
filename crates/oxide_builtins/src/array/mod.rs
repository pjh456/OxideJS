use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

mod common;
pub(crate) use common::*;

mod from;
pub use from::*;

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

/// `Array.prototype.forEach(callback, thisArg)`：对每个元素调用 callback，返回 undefined。
pub fn array_for_each<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.forEach called with {} args", args.len());
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
    if args.len() < 2 {
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..n {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
    if args.len() < 2 {
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let new_arr = create_new_array(vm, n);
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    for i in 0..n {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    for i in 0..n {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
        accumulator = arraylike_get_or_err!(vm, arr_ptr, 0);
        start_idx = 1;
    }
    let this_val = JsValue::undefined();
    for i in start_idx..n {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[accumulator, elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
    if let Err(err) = check_array_create_len(vm, n) {
        return NativeResult::Err(err);
    }
    for i in 0..n {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
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

/// `Array.prototype.findIndex(callback, thisArg)`：返回首个 callback 为真的索引，否则 -1。
pub fn array_find_index<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.findIndex called with {} args", args.len());
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
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
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
            NativeResult::Ok(r) => {
                if oxide_runtime_api::to_boolean(r) {
                    return NativeResult::Ok(js_array_index(i));
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
    let o_val = vm.reg(args[0]);
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
        match invoke_native_callback(vm, callback_val, this_val, &[elem, JsValue::int(i), o_val]) {
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
    let o_val = vm.reg(args[0]);
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
        match invoke_native_callback(vm, callback_val, JsValue::undefined(), &[acc, elem, JsValue::int(i), o_val]) {
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
/// 对元素向量执行 Array.prototype.sort 的比较语义（原地）。
/// 比较回调抛错时中止并把异常返回；默认按字符串字典序。
fn sort_values_inner<H: VmHost>(vm: &mut H, vals: &mut [JsValue], comparator: Option<JsValue>) -> Result<(), JsValue> {
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
        return Err(err);
    }
    Ok(())
}

/// 解析 sort/toSorted 的比较回调参数（undefined 视为默认排序）。
fn parse_sort_comparator<H: VmHost>(vm: &mut H, candidate: JsValue) -> Result<Option<JsValue>, JsValue> {
    if candidate.is_undefined() {
        return Ok(None);
    }
    match require_callback(vm, candidate) {
        Ok(callback) => Ok(Some(callback)),
        Err(err) => Err(err),
    }
}

pub fn array_sort<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.sort called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let len = unsafe { (*arr_ptr).prop_count() as usize };
    let mut vals: Vec<JsValue> = (0..len).map(|i| unsafe { (*arr_ptr).get_prop_at(i) }).collect();
    let comparator = if args.len() > 1 {
        match parse_sort_comparator(vm, vm.reg(args[1])) {
            Ok(c) => c,
            Err(err) => {
                builtins_error!("Array.prototype.sort: invalid receiver");
                return NativeResult::Err(err);
            }
        }
    } else {
        None
    };
    if let Err(err) = sort_values_inner(vm, &mut vals, comparator) {
        builtins_error!("Array.prototype.sort: invalid receiver");
        return NativeResult::Err(err);
    }
    let arr = unsafe { &mut *arr_ptr };
    for (i, &v) in vals.iter().enumerate() {
        arr.set_prop_at(i, v);
    }
    NativeResult::Ok(vm.reg(args[0]))
}

/// `Array.prototype.values()`：返回迭代数组元素的迭代器（%ArrayIteratorPrototype%）。
pub fn array_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.values called with {} args", args.len());
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    match make_array_iterator(vm, this_val, ARRAY_ITER_KIND_VALUES) {
        Ok(iterator) => NativeResult::Ok(iterator),
        Err(err) => {
            builtins_error!("Array.prototype.values: invalid receiver");
            NativeResult::Err(err)
        }
    }
}

/// Array Iterator 的内部 kind 编码：0=values，1=keys，2=entries。
/// TypedArray 迭代器复用同一编码与 %ArrayIteratorPrototype%（规范同族）。
pub(crate) const ARRAY_ITER_KIND_VALUES: i32 = 0;
pub(crate) const ARRAY_ITER_KIND_KEYS: i32 = 1;
pub(crate) const ARRAY_ITER_KIND_ENTRIES: i32 = 2;

const ARRAY_ITER_TARGET_PROP: &str = "__target__";
const ARRAY_ITER_INDEX_PROP: &str = "__index__";
const ARRAY_ITER_KIND_PROP: &str = "__kind__";

/// 创建 Array/TA Iterator 对象：记录目标、当前下标与迭代种类，原型挂
/// `%ArrayIteratorPrototype%`（链到 `%IteratorPrototype%`，Array 与 TA 共享）。
/// `next` 不挂实例 own——由原型提供。
pub(crate) fn make_array_iterator<H: VmHost>(vm: &mut H, this_val: JsValue, kind: i32) -> Result<JsValue, JsValue> {
    let target = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(v) => v,
        Err(msg) => return Err(array_type_error(vm, &msg)),
    };
    let array_iter_proto = vm.session().builtin_world().array_iterator_proto.as_ptr() as *mut JsObject;
    let iter = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(array_iter_proto)));
    let target_si = vm.kernel_core().perm_interner().intern(ARRAY_ITER_TARGET_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(ARRAY_ITER_INDEX_PROP).0;
    let kind_si = vm.kernel_core().perm_interner().intern(ARRAY_ITER_KIND_PROP).0;
    let iter_ref = unsafe { &mut *iter };
    vm.set_or_create_prop_value(iter_ref, target_si, target);
    vm.set_or_create_prop_value(iter_ref, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(iter_ref, kind_si, JsValue::int(kind));
    Ok(JsValue::from_js_object(iter))
}

/// Array.prototype.entries()：返回按索引产出 [index, value] 对的迭代器。
pub fn array_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.entries called with {} args", args.len());
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    match make_array_iterator(vm, this_val, ARRAY_ITER_KIND_ENTRIES) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// Array.prototype.keys()：返回按索引产出下标的迭代器。
pub fn array_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.keys called with {} args", args.len());
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    match make_array_iterator(vm, this_val, ARRAY_ITER_KIND_KEYS) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// Array Iterator 的 next：读取目标当前下标处的值，按种类产出后推进下标。
pub fn array_iterator_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(array_type_error(vm, "Array Iterator next called on non-object"));
    }
    let iter = unsafe { &mut *this_val.as_js_object_ptr() };
    let target_si = vm.kernel_core().perm_interner().intern(ARRAY_ITER_TARGET_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(ARRAY_ITER_INDEX_PROP).0;
    let kind_si = vm.kernel_core().perm_interner().intern(ARRAY_ITER_KIND_PROP).0;
    let target = match vm.ordinary_get(iter, target_si, this_val) {
        Ok(v) if v.is_undefined() => {
            return NativeResult::Ok(crate::iterator::make_iter_result(vm, JsValue::undefined(), true));
        }
        Ok(v) if v.is_object() => v,
        _ => return NativeResult::Err(array_type_error(vm, "Array Iterator has no iterated object")),
    };
    let index = vm
        .ordinary_get(iter, index_si, this_val)
        .ok()
        .and_then(|v| if v.is_int() { Some(v.as_int()) } else { None })
        .unwrap_or(0);
    let kind = vm
        .ordinary_get(iter, kind_si, this_val)
        .ok()
        .and_then(|v| if v.is_int() { Some(v.as_int()) } else { None })
        .unwrap_or(ARRAY_ITER_KIND_VALUES);

    let target_obj = unsafe { &*target.as_js_object_ptr() };
    // TypedArray 目标走底层 buffer 读取（元素非普通属性，length 语义不同）：
    // 与 Array 共享 %ArrayIteratorPrototype%，须在此分支区分两种取数路径。
    if target_obj.is_typed_array_obj() {
        let view = match crate::typed_array::get_typed_array_data(vm, target) {
            Ok(view) => view,
            Err(err) => return NativeResult::Err(err),
        };
        if (index as usize) >= view.length {
            vm.set_or_create_prop_value(iter, target_si, JsValue::undefined());
            return NativeResult::Ok(crate::iterator::make_iter_result(vm, JsValue::undefined(), true));
        }
        let element = match crate::typed_array::typed_array_element_get(vm, target_obj, index as u32) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(crate::error::create_type_error(vm, &e)),
        };
        let value = match kind {
            ARRAY_ITER_KIND_KEYS => JsValue::int(index),
            ARRAY_ITER_KIND_ENTRIES => {
                let pair = create_new_array(vm, 2);
                let pair_ref = unsafe { &mut *pair };
                pair_ref.set_prop_at(0, JsValue::int(index));
                pair_ref.set_prop_at(1, element);
                JsValue::from_js_object(pair)
            }
            _ => element,
        };
        if let Err(msg) = vm.ordinary_set(iter, index_si, JsValue::int(index + 1), this_val, true) {
            return NativeResult::Err(crate::error::create_from_text(vm, &msg));
        }
        return NativeResult::Ok(crate::iterator::make_iter_result(vm, value, false));
    }

    let length_key = vm.new_string("length");
    let length_si = vm.property_key_si(length_key);
    let len_val = match vm.ordinary_get(target_obj, length_si, target) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(crate::error::create_from_text(vm, &err)),
    };
    let len_num = vm.coerce_number_bounded(len_val).unwrap_or(0.0);
    let len = if !len_num.is_finite() || len_num <= 0.0 { 0 } else { len_num as usize };

    if (index as usize) >= len {
        vm.set_or_create_prop_value(iter, target_si, JsValue::undefined());
        return NativeResult::Ok(crate::iterator::make_iter_result(vm, JsValue::undefined(), true));
    }

    let _ = target_obj.is_array();
    let element = arraylike_get_or_err!(vm, target.as_js_object_ptr(), index as usize);
    let value = match kind {
        ARRAY_ITER_KIND_KEYS => JsValue::int(index),
        ARRAY_ITER_KIND_ENTRIES => {
            let pair = create_new_array(vm, 2);
            let pair_ref = unsafe { &mut *pair };
            pair_ref.set_prop_at(0, JsValue::int(index));
            pair_ref.set_prop_at(1, element);
            JsValue::from_js_object(pair)
        }
        _ => element,
    };
    if let Err(msg) = vm.ordinary_set(iter, index_si, JsValue::int(index + 1), this_val, true) {
        return NativeResult::Err(crate::error::create_from_text(vm, &msg));
    }
    NativeResult::Ok(crate::iterator::make_iter_result(vm, value, false))
}

/// Array.prototype.findLastIndex(callback, thisArg)：从后往前返回首个 callback 为真的下标。
pub fn array_find_last_index<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.findLastIndex called with {} args", args.len());
    let (arr_ptr, n, _is_array) = array_ptr_len3!(vm, args);
    let o_val = vm.reg(args[0]);
    if args.len() < 2 {
        builtins_error!("Array.prototype.findLastIndex: invalid receiver");
        return NativeResult::Err(array_type_error(vm, "callback is not a function"));
    }
    let callback_val = match require_callback(vm, vm.reg(args[1])) {
        Ok(callback) => callback,
        Err(err) => {
            builtins_error!("Array.prototype.findLastIndex: invalid receiver");
            return NativeResult::Err(err);
        }
    };
    let this_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in (0..n).rev() {
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
            NativeResult::Ok(r) => {
                if oxide_runtime_api::to_boolean(r) {
                    return NativeResult::Ok(js_array_index(i));
                }
            }
            NativeResult::Err(err) => {
                builtins_error!("Array.prototype.findLastIndex: invalid receiver");
                return NativeResult::Err(err);
            }
            NativeResult::TailCall { .. } => {
                builtins_error!("Array.prototype.findLastIndex: invalid receiver");
                return unexpected_tail_call_error(vm);
            }
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

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
    // ????????????????getter ???????????
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
pub fn array_to_sorted<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.toSorted called with {} args", args.len());
    // 规范先校验 compareFn 可调用，再读取 this/length。
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
    // ???newLen = len + insertCount - actualDeleteCount?> 2**53-1 ? TypeError?
    // ArrayCreate(newLen) ? 2**32-1 ? RangeError??????/???????
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
