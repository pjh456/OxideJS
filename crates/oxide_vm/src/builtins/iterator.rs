use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::vm::Vm;
use oxide_runtime_api::NativeResult;

const INNER_PROP: &str = "__inner__";
const INDEX_PROP: &str = "__index__";

pub fn iterator_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::builtins::error::create_type_error(vm, "Iterator is not a constructor"))
}

pub fn iterator_from(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match make_iterator_for_value(vm, iterable) {
        Ok(iterator) => NativeResult::Ok(iterator),
        Err(err) => NativeResult::Err(err),
    }
}

pub(crate) fn make_iterator_for_value(vm: &mut Vm, value: JsValue) -> Result<JsValue, JsValue> {
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

    let next_fn = make_native_function(vm, "next", iterator_wrapper_next as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next_fn);

    Ok(JsValue::from_js_object(wrapper))
}

pub fn iterator_wrapper_next(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::builtins::error::create_type_error(
            vm,
            "Iterator wrapper next called on non-object",
        ));
    }

    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => {
            return NativeResult::Err(crate::builtins::error::create_type_error(
                vm,
                "Iterator wrapper has no inner iterator",
            ))
        }
    };

    if let Some(result) = next_array_like(vm, wrapper, inner, index_si) {
        return NativeResult::Ok(result);
    }

    if inner.is_object() {
        let inner_obj = unsafe { &*inner.as_js_object_ptr() };
        let next_si = vm.kernel_core().perm_interner().intern("next").0;
        let next = match vm.ordinary_get(inner_obj, next_si, inner) {
            Ok(next) => next,
            Err(err) => return NativeResult::Err(crate::builtins::error::create_type_error(vm, &err)),
        };
        return match vm.call_function_sync(next, inner, &[]) {
            Ok(result) => NativeResult::Ok(result),
            Err(err) => NativeResult::Err(crate::builtins::error::create_type_error(vm, &err)),
        };
    }

    NativeResult::Err(crate::builtins::error::create_type_error(vm, "value is not iterable"))
}

fn get_iterator(vm: &mut Vm, value: JsValue) -> Result<JsValue, JsValue> {
    if value.is_string() || is_array_value(value) {
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

    Err(crate::builtins::error::create_type_error(vm, "value is not iterable"))
}

fn next_array_like(vm: &mut Vm, wrapper: &mut JsObject, inner: JsValue, index_si: u32) -> Option<JsValue> {
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
        let source = coercion::to_string(inner);
        let mut chars = source.chars();
        if let Some(ch) = chars.nth(index) {
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            let value = vm.new_string(&ch.to_string());
            return Some(make_iter_result(vm, value, false));
        }
        return Some(make_iter_result(vm, JsValue::undefined(), true));
    }

    None
}

fn current_index(vm: &mut Vm, wrapper: &JsObject, index_si: u32) -> usize {
    match vm.ordinary_get(wrapper, index_si, JsValue::undefined()) {
        Ok(value) if value.is_int() => value.as_int().max(0) as usize,
        Ok(value) if value.is_double() => value.as_double().max(0.0) as usize,
        _ => 0,
    }
}

fn make_iter_result(vm: &mut Vm, value: JsValue, done: bool) -> JsValue {
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

fn make_native_function(vm: &mut Vm, name: &str, native_fn: *const (), arg_count: u8) -> JsValue {
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
