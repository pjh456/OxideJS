//! Array 排序方法与迭代协议（sort/values/entries/keys/iterator_next 及迭代器构造）。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

use super::common::{
    array_ptr, array_type_error, arraylike_get, arraylike_get_or_err, create_new_array, get_this_array_ref,
    invoke_native_callback, require_callback,
};

/// 对元素向量执行 `Array.prototype.sort` 的比较语义（原地排序）。
///
/// 默认按 ToString 结果的字符串字典序比较；提供比较回调时按其返回值
///（<0/=0/>0）决定次序，回调抛错则中止并把异常原样返回。
pub(crate) fn sort_values_inner<H: VmHost>(
    vm: &mut H, vals: &mut [JsValue], comparator: Option<JsValue>,
) -> Result<(), JsValue> {
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
pub(crate) fn parse_sort_comparator<H: VmHost>(vm: &mut H, candidate: JsValue) -> Result<Option<JsValue>, JsValue> {
    if candidate.is_undefined() {
        return Ok(None);
    }
    match require_callback(vm, candidate) {
        Ok(callback) => Ok(Some(callback)),
        Err(err) => Err(err),
    }
}

/// 原地排序数组元素并返回原数组（`Array.prototype.sort`）。
///
/// 未提供比较回调时按 ToString 结果的字符串字典序排序；底层 `sort_by` 为稳定
/// 排序，比较结果相等（含 NaN）的元素保持原有相对次序。洞位不参与比较：
/// present 值稳定排序后自索引 0 紧凑写回，尾部空位转为洞。
///
/// # 边界与前提
/// - 接收者不是数组时抛 TypeError；
/// - 比较回调抛错时中止排序并把异常原样返回。
pub fn array_sort<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Array.prototype.sort called with {} args", args.len());
    let arr_ptr = array_ptr!(vm, args);
    let len = unsafe { (*arr_ptr).prop_count() as usize };
    // 洞位不参与比较：只收集 present 元素（接收者恒为真数组，元数据表判洞免字符串键）。
    let arr_ref = unsafe { &*arr_ptr };
    let dense = arr_ref.array_elements_meta_vec().is_none();
    let mut vals: Vec<JsValue> = (0..len)
        .filter(|&i| dense || !arr_ref.prop_meta_at(i).is_some_and(|m| m.is_hole()))
        .map(|i| arr_ref.get_prop_at(i))
        .collect();
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
    // present 值稳定排序后从索引 0 紧凑写回，尾部空位转为洞。
    let arr = unsafe { &mut *arr_ptr };
    for (i, &v) in vals.iter().enumerate() {
        arr.set_prop_at(i, v);
    }
    for i in vals.len()..len {
        arr.mark_hole_at(i);
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
