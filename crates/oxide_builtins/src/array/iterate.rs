//! Array 高阶迭代方法（for_each/map/filter/reduce/find/some/every/flatMap 等）。

use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

use super::common::{
    array_ptr_len3, array_type_error, arraylike_get, arraylike_get_or_err, arraylike_index_present,
    check_array_create_len, create_new_array, get_this_arraylike, invoke_native_callback, js_array_index,
    require_callback, unexpected_tail_call_error,
};

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
        // 洞位跳过：回调不得在缺失索引上触发。
        if !arraylike_index_present(vm, arr_ptr, i) {
            continue;
        }
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
        // 洞位跳过回调，并在结果对应位留洞（结果数组预填 present-undefined，须显式置洞）。
        if !arraylike_index_present(vm, arr_ptr, i) {
            unsafe {
                (*new_arr).mark_hole_at(i);
            }
            continue;
        }
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
        // 洞位跳过：结果保持紧凑，不复制缺失索引。
        if !arraylike_index_present(vm, arr_ptr, i) {
            continue;
        }
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
        // FindFirstKey：无初值时取首个存在索引作累加器；全洞（含空数组）抛 TypeError。
        let mut found = None;
        for i in 0..n {
            if arraylike_index_present(vm, arr_ptr, i) {
                found = Some(i);
                break;
            }
        }
        let idx = match found {
            Some(idx) => idx,
            None => {
                builtins_error!("Array.prototype.reduce: invalid receiver");
                return NativeResult::Err(array_type_error(vm, "Reduce of empty array with no initial value"));
            }
        };
        accumulator = arraylike_get_or_err!(vm, arr_ptr, idx);
        start_idx = idx + 1;
    }
    let this_val = JsValue::undefined();
    for i in start_idx..n {
        // 洞位跳过：累加器保持上一 present 元素的结果。
        if !arraylike_index_present(vm, arr_ptr, i) {
            continue;
        }
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
        // 洞位跳过：回调不得在缺失索引上触发。
        if !arraylike_index_present(vm, arr_ptr, i) {
            continue;
        }
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
        // 洞位跳过：回调不得在缺失索引上触发。
        if !arraylike_index_present(vm, arr_ptr, i) {
            continue;
        }
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
        // 洞位跳过：回调不得在缺失索引上触发。
        if !arraylike_index_present(vm, arr_ptr, i) {
            continue;
        }
        let elem = arraylike_get_or_err!(vm, arr_ptr, i);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i), o_val]) {
            NativeResult::Ok(result) => {
                if result.is_object() {
                    let r_ptr = result.as_js_object_ptr();
                    if !r_ptr.is_null() {
                        let r = unsafe { &*r_ptr };
                        if r.is_array() {
                            // 嵌套数组展开丢弃洞位：结果为紧凑数组。
                            let rn = r.prop_count() as usize;
                            let dense = r.array_elements_meta_vec().is_none();
                            for j in 0..rn {
                                if dense || !r.prop_meta_at(j).is_some_and(|m| m.is_hole()) {
                                    flat.push(r.get_prop_at(j));
                                }
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
        let elem = arraylike_get_or_err!(vm, arr_ptr, i as usize);
        match invoke_native_callback(vm, callback_val, this_val, &[elem, js_array_index(i as usize), o_val]) {
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
        // FindFirstKey 自尾：无初值时取末尾首个存在索引作累加器；全洞抛 TypeError。
        let mut found = None;
        for i in (0..n).rev() {
            if arraylike_index_present(vm, arr_ptr, i) {
                found = Some(i);
                break;
            }
        }
        let idx = match found {
            Some(idx) => idx,
            None => {
                builtins_error!("Array.prototype.reduceRight: invalid receiver");
                return NativeResult::Err(array_type_error(vm, "Reduce of empty array with no initial value"));
            }
        };
        (arraylike_get_or_err!(vm, arr_ptr, idx), idx as i32 - 1)
    };
    for i in (0..=start_idx).rev() {
        // 洞位跳过：累加器保持上一 present 元素的结果。
        if !arraylike_index_present(vm, arr_ptr, i as usize) {
            continue;
        }
        let elem = arraylike_get_or_err!(vm, arr_ptr, i as usize);
        match invoke_native_callback(
            vm,
            callback_val,
            JsValue::undefined(),
            &[acc, elem, js_array_index(i as usize), o_val],
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
