use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::native::NativeResult;
use crate::vm::Vm;

fn invoke_target(vm: &mut Vm, target_val: JsValue, this_val: JsValue, arg_regs: &[u8]) -> NativeResult {
    let args: Vec<JsValue> = arg_regs.iter().map(|&r| vm.reg(r)).collect();
    NativeResult::TailCall {
        callee: target_val,
        this: this_val,
        args,
    }
}

fn bind_dispatcher(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let wrapper_val = vm.reg(254);
    let wrapper = unsafe { &*wrapper_val.as_js_object_ptr() };
    let bound_target = wrapper
        .hash_props_vec()
        .and_then(|v| v.first().copied())
        .unwrap_or(JsValue::undefined());
    let bound_this = wrapper
        .hash_props_vec()
        .and_then(|v| v.get(1).copied())
        .unwrap_or(JsValue::undefined());

    // Forward bound call arguments (skip args[0], the bound-wrapper receiver).
    let arg_regs: Vec<u8> = args.iter().skip(1).copied().collect();
    invoke_target(vm, bound_target, bound_this, &arg_regs)
}

pub fn function_call(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    let this_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let arg_regs: Vec<u8> = args.iter().skip(2).copied().collect();
    invoke_target(vm, target_val, this_val, &arg_regs)
}

pub fn function_apply(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    let this_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };

    let arg_regs: Vec<u8>;
    if args.len() > 2 {
        let arr_val = vm.reg(args[2]);
        if arr_val.is_object() {
            let arr_ptr = arr_val.as_js_object_ptr();
            if !arr_ptr.is_null() {
                let arr = unsafe { &*arr_ptr };
                if arr.is_array() {
                    let max_args = 55usize; // base=200, max safe register is 255.
                    let n = arr.hash_props_vec().map_or(0, |v| v.len()).min(max_args);
                    let base = 200u8;
                    arg_regs = (0..n).map(|i| base + i as u8).collect();
                    for i in 0..n {
                        let vec = arr.hash_props_vec();
                        if let Some(v) = vec.and_then(|v| v.get(i)) {
                            vm.set_reg(base + i as u8, *v);
                        }
                    }
                } else {
                    arg_regs = Vec::new();
                }
            } else {
                arg_regs = Vec::new();
            }
        } else {
            arg_regs = Vec::new();
        }
    } else {
        arg_regs = Vec::new();
    }
    invoke_target(vm, target_val, this_val, &arg_regs)
}

pub fn function_bind(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    if !target_val.is_object() {
        return NativeResult::Err(JsValue::undefined());
    }
    let bound_this = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };

    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    unsafe {
        (*wrapper).set_function(true);
        // SAFETY: bind_dispatcher is a NativeFn fn-item; valid to store as NativeFnPtr.
        (*wrapper).set_native_fn(Some(NativeFnPtr::from_raw(bind_dispatcher as *const ())));
        (*wrapper).set_native_arg_count(0);
        (*wrapper).ensure_hash_props().push(target_val);
        (*wrapper).ensure_hash_props().push(bound_this);
    }

    NativeResult::Ok(JsValue::from_js_object(wrapper))
}

pub fn function_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::builtins::error::create_type_error(
            vm,
            "Function.prototype.toString called on non-function",
        ));
    }

    let func = unsafe { &*this_val.as_js_object_ptr() };
    if !func.is_function() {
        return NativeResult::Err(crate::builtins::error::create_type_error(
            vm,
            "Function.prototype.toString called on non-function",
        ));
    }

    let name_si = vm.kernel_core().string_forge().intern("name").0;
    let name = vm
        .resolve_property(func, name_si)
        .and_then(|v| {
            if v.is_string() {
                vm.kernel_core().string_forge().lookup(v.as_string_index())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            let sub_idx = func.sub_module_index();
            if sub_idx > 0 && (sub_idx as usize) <= vm.sub_modules.len() {
                vm.sub_modules[sub_idx as usize - 1].function_name.clone().unwrap_or_default()
            } else {
                String::new()
            }
        });

    let body = if func.native_fn().is_some() { "[native code]" } else { "[bytecode]" };
    let result = if name.is_empty() {
        format!("function () {{ {body} }}")
    } else {
        format!("function {name}() {{ {body} }}")
    };
    let si = vm.kernel_core().string_forge().intern(&result).0;
    NativeResult::Ok(JsValue::string(si, 0))
}
