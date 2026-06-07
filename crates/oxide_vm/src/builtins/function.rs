use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::native::{NativeFn, NativeResult};
use crate::vm::Vm;

fn invoke_target(
    vm: &mut Vm,
    target_val: JsValue,
    this_val: JsValue,
    arg_regs: &[u8],
) -> NativeResult {
    if !target_val.is_object() {
        return Err(JsValue::undefined());
    }
    let tgt_ptr = target_val.as_js_object_ptr();
    if tgt_ptr.is_null() {
        return Err(JsValue::undefined());
    }
    let tgt = unsafe { &*tgt_ptr };
    if !tgt.is_function() || tgt.native_fn().is_none() {
        return Err(JsValue::undefined());
    }
    let func: NativeFn = unsafe { std::mem::transmute(tgt.native_fn().unwrap()) };

    let base = 230u8;
    let n = arg_regs.len();
    let mut args_buf = [0u8; 64];
    args_buf[0] = base;
    vm.set_reg(base, this_val);
    for (i, &reg) in arg_regs.iter().enumerate().take(n.min(63)) {
        vm.set_reg(base + 1 + i as u8, vm.reg(reg));
        args_buf[i + 1] = base + 1 + i as u8;
    }

    func(vm, &args_buf[..n + 1])
}

fn bind_dispatcher(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let wrapper_val = vm.reg(254);
    let wrapper = unsafe { &*wrapper_val.as_js_object_ptr() };
    let bound_target = wrapper.get_prop(0);
    let bound_this = wrapper.get_prop(1);

    let n = args.len().saturating_sub(1);
    let base = 230u8;
    let mut args_buf = [0u8; 64];
    args_buf[0] = base;
    vm.set_reg(base, bound_this);
    for (i, &reg) in args.iter().skip(1).enumerate().take(n.min(63)) {
        vm.set_reg(base + 1 + i as u8, vm.reg(reg));
        args_buf[i + 1] = base + 1 + i as u8;
    }

    if !bound_target.is_object() {
        return Err(JsValue::undefined());
    }
    let tgt_ptr = bound_target.as_js_object_ptr();
    if tgt_ptr.is_null() {
        return Err(JsValue::undefined());
    }
    let tgt = unsafe { &*tgt_ptr };
    if !tgt.is_function() || tgt.native_fn().is_none() {
        return Err(JsValue::undefined());
    }
    let func: NativeFn = unsafe { std::mem::transmute(tgt.native_fn().unwrap()) };
    func(vm, &args_buf[..n + 1])
}

pub fn function_call(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    let this_val = if args.len() > 1 {
        vm.reg(args[1])
    } else {
        JsValue::undefined()
    };
    let arg_regs: Vec<u8> = args.iter().skip(2).copied().collect();
    invoke_target(vm, target_val, this_val, &arg_regs)
}

pub fn function_apply(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    let this_val = if args.len() > 1 {
        vm.reg(args[1])
    } else {
        JsValue::undefined()
    };

    let arg_regs: Vec<u8>;
    if args.len() > 2 {
        let arr_val = vm.reg(args[2]);
        if arr_val.is_object() {
            let arr_ptr = arr_val.as_js_object_ptr();
            if !arr_ptr.is_null() {
                let arr = unsafe { &*arr_ptr };
                if arr.is_array() {
                    let n = arr.prop_count() as usize;
                    let base = 200u8;
                    arg_regs = (0..n).map(|i| base + i as u8).collect();
                    for i in 0..n {
                        vm.set_reg(base + i as u8, arr.get_prop(i as u8));
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
        return Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    if !target_val.is_object() {
        return Err(JsValue::undefined());
    }
    let bound_this = if args.len() > 1 {
        vm.reg(args[1])
    } else {
        JsValue::undefined()
    };

    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    unsafe {
        (*wrapper).set_function(true);
        (*wrapper).set_native_fn(Some(bind_dispatcher as *const ()));
        (*wrapper).set_native_arg_count(0);
        (*wrapper).set_prop_count(2);
        (*wrapper).set_inline_prop(0, target_val);
        (*wrapper).set_inline_prop(1, bound_this);
    }

    Ok(JsValue::from_js_object(wrapper))
}
