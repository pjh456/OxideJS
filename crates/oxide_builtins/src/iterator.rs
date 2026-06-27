use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

const INNER_PROP: &str = "__inner__";
const INDEX_PROP: &str = "__index__";

pub fn iterator_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Iterator is not a constructor"))
}

pub fn iterator_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match make_iterator_for_value(vm, iterable) {
        Ok(iterator) => NativeResult::Ok(iterator),
        Err(err) => NativeResult::Err(err),
    }
}

pub fn make_iterator_for_value<H: VmHost>(vm: &mut H, value: JsValue) -> Result<JsValue, JsValue> {
    let inner = get_iterator(vm, value)?;
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));

    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));

    let next_fn = make_native_function(vm, "next", iterator_wrapper_next::<H> as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next_fn);

    // Forward IteratorClose to the inner iterator so for-of abrupt completion can clean up.
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
    let return_fn = make_native_function(vm, "return", iterator_wrapper_return::<H> as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, return_si, return_fn);

    Ok(JsValue::from_js_object(wrapper))
}

fn iterator_wrapper_return<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let wrapper = unsafe { &*this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if inner.is_object() => inner,
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    let inner_obj = unsafe { &*inner.as_js_object_ptr() };
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
    let return_fn = match vm.ordinary_get(inner_obj, return_si, inner) {
        Ok(f) if is_callable(f) => f,
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    match vm.call_function_sync(return_fn, inner, &[]) {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => match vm.take_uncaught_value() {
            Some(original) => NativeResult::Err(original),
            None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        },
    }
}

pub fn iterator_wrapper_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator wrapper next called on non-object"));
    }

    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "Iterator wrapper has no inner iterator")),
    };

    if let Some(result) = next_array_like(vm, wrapper, inner, index_si) {
        return NativeResult::Ok(result);
    }

    if inner.is_object() {
        let inner_obj = unsafe { &*inner.as_js_object_ptr() };
        let next_si = vm.kernel_core().perm_interner().intern("next").0;
        let next = match vm.ordinary_get(inner_obj, next_si, inner) {
            Ok(next) => next,
            Err(err) => return NativeResult::Err(crate::error::create_type_error(vm, &err)),
        };
        return match vm.call_function_sync(next, inner, &[]) {
            Ok(result) => NativeResult::Ok(result),
            // Forward the ORIGINAL thrown value (any type) instead of re-wrapping it as a
            // TypeError, so a surrounding try/catch sees the real error.
            Err(err) => match vm.take_uncaught_value() {
                Some(original) => NativeResult::Err(original),
                None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
            },
        };
    }

    NativeResult::Err(crate::error::create_type_error(vm, "value is not iterable"))
}

fn get_iterator<H: VmHost>(vm: &mut H, value: JsValue) -> Result<JsValue, JsValue> {
    if value.is_string() || is_array_value(value) || is_map_value(value) || is_set_value(value) {
        return Ok(value);
    }

    if value.is_object() {
        let obj = unsafe { &*value.as_js_object_ptr() };
        let next_si = vm.kernel_core().perm_interner().intern("next").0;
        if let Ok(next) = vm.ordinary_get(obj, next_si, value) {
            if is_callable(next) {
                return Ok(value);
            }
        }
    }

    Err(crate::error::create_type_error(vm, "value is not iterable"))
}

fn next_array_like<H: VmHost>(vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32) -> Option<JsValue> {
    if is_array_value(inner) {
        let index = current_index(vm, wrapper, index_si);
        let arr = unsafe { &*inner.as_js_object_ptr() };
        if index < arr.prop_count() as usize {
            let value = arr.get_prop_at(index);
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            return Some(make_iter_result(vm, value, false));
        }
        return Some(make_iter_result(vm, JsValue::undefined(), true));
    }

    if inner.is_string() {
        let index = current_index(vm, wrapper, index_si);
        let source = oxide_runtime_api::to_string(inner);
        let mut chars = source.chars();
        if let Some(ch) = chars.nth(index) {
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            let value = vm.new_string(&ch.to_string());
            return Some(make_iter_result(vm, value, false));
        }
        return Some(make_iter_result(vm, JsValue::undefined(), true));
    }

    // for-of loop default iterators: Map yields [key, value] entries, Set yields values.
    if is_map_value(inner) {
        return Some(map_set_step(vm, wrapper, inner, index_si, MapSetMode::MapEntries));
    }
    if is_set_value(inner) {
        return Some(map_set_step(vm, wrapper, inner, index_si, MapSetMode::SetValues));
    }

    None
}

fn current_index<H: VmHost>(vm: &mut H, wrapper: &JsObject, index_si: u32) -> usize {
    match vm.ordinary_get(wrapper, index_si, JsValue::undefined()) {
        Ok(value) if value.is_int() => value.as_int().max(0) as usize,
        Ok(value) if value.is_double() => value.as_double().max(0.0) as usize,
        _ => 0,
    }
}

fn make_iter_result<H: VmHost>(vm: &mut H, value: JsValue, done: bool) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let obj = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let obj_ref = unsafe { &mut *obj };
    vm.set_or_create_prop_value(obj_ref, value_si, value);
    vm.set_or_create_prop_value(obj_ref, done_si, JsValue::bool(done));
    JsValue::from_js_object(obj)
}

fn make_native_function<H: VmHost>(vm: &mut H, name: &str, native_fn: *const (), arg_count: u8) -> JsValue {
    let function_proto = vm.session().builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
    func.set_function(true);
    // SAFETY: native_fn is provided from a NativeFn fn item.
    func.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(native_fn) }));
    func.set_native_arg_count(arg_count);
    let func = vm.alloc_object(func);
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let value = vm.new_string(name);
    let func_ref = unsafe { &mut *func };
    vm.set_or_create_prop_value(func_ref, name_si, value);
    JsValue::from_js_object(func)
}

fn is_array_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_array()
}

fn is_callable(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_function()
}

fn is_map_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_map()
}

fn is_set_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_set()
}

fn make_map_set_pair<H: VmHost>(vm: &mut H, a: JsValue, b: JsValue) -> JsValue {
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let pair = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        2,
        vm.epoch().bump(),
    ));
    // SAFETY: pair is a freshly allocated array JsObject in the current epoch.
    unsafe {
        (*pair).set_prop_at(0, a);
        (*pair).set_prop_at(1, b);
        (*pair).set_prop_count(2);
    }
    JsValue::from_js_object(pair)
}

#[derive(Clone, Copy)]
enum MapSetMode {
    MapEntries,
    MapValues,
    MapKeys,
    SetEntries,
    SetValues,
}

/// Advance a Map/Set iterator wrapper one step, yielding the {value, done} result for
/// the requested mode. Entries live in the indexmap stored in the collection's
/// native-data slot; the (a, b) pair is copied out before any allocation so no borrow
/// of the native collection is held across a `vm` call.
fn map_set_step<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32, mode: MapSetMode,
) -> JsValue {
    let index = current_index(vm, wrapper, index_si);
    let is_map = matches!(mode, MapSetMode::MapEntries | MapSetMode::MapValues | MapSetMode::MapKeys);
    let entry: Option<(JsValue, JsValue)> = unsafe {
        let obj_ptr = inner.as_js_object_ptr();
        if obj_ptr.is_null() {
            None
        } else if is_map {
            let p = (*obj_ptr).native_data() as *const crate::map::MapInner;
            if p.is_null() {
                None
            } else {
                (*p).get_index(index).map(|(key, value)| (key.0, *value))
            }
        } else {
            let p = (*obj_ptr).native_data() as *const crate::set::SetInner;
            if p.is_null() {
                None
            } else {
                (*p).get_index(index).map(|elem| (elem.0, elem.0))
            }
        }
    };

    match entry {
        Some((a, b)) => {
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            let value = match mode {
                MapSetMode::MapEntries | MapSetMode::SetEntries => make_map_set_pair(vm, a, b),
                MapSetMode::MapValues => b,
                MapSetMode::MapKeys | MapSetMode::SetValues => a,
            };
            make_iter_result(vm, value, false)
        }
        None => make_iter_result(vm, JsValue::undefined(), true),
    }
}

fn map_set_next_dispatch<H: VmHost>(vm: &mut H, args: &[u8], mode: MapSetMode) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "iterator next called on non-object"));
    }
    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "iterator has no inner collection")),
    };
    NativeResult::Ok(map_set_step(vm, wrapper, inner, index_si, mode))
}

pub(crate) fn map_entries_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::MapEntries)
}

pub(crate) fn map_values_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::MapValues)
}

pub(crate) fn map_keys_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::MapKeys)
}

pub(crate) fn set_values_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::SetValues)
}

pub(crate) fn set_entries_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::SetEntries)
}

/// Build an iterator wrapper whose `next` delegates to a caller-supplied mode-specific
/// native function. Mirrors `make_iterator_for_value` but lets Map/Set prototype methods
/// pick the values/keys/entries variant.
pub(crate) fn make_mode_iterator<H: VmHost>(vm: &mut H, inner: JsValue, next_fn: *const ()) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    let next = make_native_function(vm, "next", next_fn, 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next);
    JsValue::from_js_object(wrapper)
}
