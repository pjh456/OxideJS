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

mod element;
pub use element::*;

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
