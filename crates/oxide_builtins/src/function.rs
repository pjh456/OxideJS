use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use oxide_runtime_api::{to_integer_or_infinity, to_string_full, NativeResult, VmHost};

fn invoke_target<H: VmHost>(vm: &mut H, target_val: JsValue, this_val: JsValue, arg_regs: &[u8]) -> NativeResult {
    let args: Vec<JsValue> = arg_regs.iter().map(|&r| vm.reg(r)).collect();
    NativeResult::TailCall {
        callee: target_val,
        this: this_val,
        args,
    }
}

/// `Function(...)` / `new Function(...)` 构造器：动态编译一个匿名函数。
///
/// 除最后一个实参为函数体外，其余实参为形参名；无实参时函数体为空串。
/// 编译成功返回函数对象，语法错误抛 SyntaxError，实参 ToString 失败抛 TypeError。
pub fn function_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let arg_regs = &args[1..];
    let (param_regs, body_reg) = if arg_regs.is_empty() {
        (vec![], None)
    } else {
        let (head, tail) = arg_regs.split_at(arg_regs.len() - 1);
        (head.to_vec(), Some(tail[0]))
    };

    let mut params = Vec::with_capacity(param_regs.len());
    for &r in &param_regs {
        match to_string_full(vm.reg(r), vm) {
            Ok(s) => params.push(s),
            Err(e) => return NativeResult::Err(to_string_error_value(vm, &e)),
        }
    }
    let body = match body_reg {
        Some(r) => match to_string_full(vm.reg(r), vm) {
            Ok(s) => s,
            Err(e) => return NativeResult::Err(to_string_error_value(vm, &e)),
        },
        None => String::new(),
    };

    match vm.create_dynamic_function(&params, &body) {
        Ok(func) => NativeResult::Ok(func),
        Err(msg) => NativeResult::Err(crate::error::create_syntax_error(vm, &msg)),
    }
}

/// 把 ToString 强制转换错误转成可抛异常值：用户回调抛出的原始值优先原样传播
/// （经 `last_uncaught_value` 侧通道），否则按普通 TypeError 处理。
fn to_string_error_value<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    vm.take_uncaught_value()
        .unwrap_or_else(|| crate::error::create_type_error(vm, msg))
}

fn bind_dispatcher<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let wrapper_val = vm.reg(254);
    let wrapper = unsafe { &*wrapper_val.as_js_object_ptr() };
    // dense 布局：[length, name, caller, arguments, target, thisArg, ...boundArgs]——
    // 前 4 槽是 shape 属性（length/name 数据 + caller/arguments 访问器占位），
    // 绑定状态从槽位 4 起（shape 槽位与 dense 下标对齐）。
    let props = wrapper.hash_props_vec().cloned().unwrap_or_default();
    let bound_target = props.get(4).copied().unwrap_or(JsValue::undefined());
    let bound_this = props.get(5).copied().unwrap_or(JsValue::undefined());

    // 拼接调用实参：绑定实参（props[6..]）在前，本次调用实参（跳过 args[0]
    // 即绑定包装器的 receiver）在后，与规范的"绑定实参先于调用实参"一致。
    let mut call_args: Vec<JsValue> = props.iter().skip(6).copied().collect();
    for &r in args.iter().skip(1) {
        call_args.push(vm.reg(r));
    }
    NativeResult::TailCall {
        callee: bound_target,
        this: bound_this,
        args: call_args,
    }
}

/// `Function.prototype.call(thisArg, ...args)`：以指定 this 调用目标函数。
/// 返回 TailCall 让 VM 继续执行目标函数体。
pub fn function_call<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[0]);
    let this_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let arg_regs: Vec<u8> = args.iter().skip(2).copied().collect();
    invoke_target(vm, target_val, this_val, &arg_regs)
}

/// `Function.prototype.apply(thisArg, argsArray)`：以指定 this 和参数数组调用目标函数。
/// 数组元素拷入寄存器（上限受寄存器空间限制，最多 55 个参数）。
pub fn function_apply<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
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
                    let max_args = 55usize; // base=200，255 号寄存器之前的安全上限。
                    let n = arr.prop_count() as usize;
                    let n = n.min(max_args);
                    let base = 200u8;
                    arg_regs = (0..n).map(|i| base + i as u8).collect();
                    for i in 0..n {
                        vm.set_reg(base + i as u8, arr.get_prop_at(i));
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

/// `Function.prototype.bind(thisArg, ...args)`：返回绑定 this 与前置实参的新包装函数，
/// 调用时通过 `bind_dispatcher` 把绑定实参拼到调用实参前转发到原目标。非函数目标抛 TypeError。
pub fn function_bind<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.is_empty() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Function.prototype.bind called on null or undefined",
        ));
    }
    let target_val = vm.reg(args[0]);
    if !target_val.is_object()
        || target_val.as_js_object_ptr().is_null()
        || !unsafe { &*target_val.as_js_object_ptr() }.is_function()
    {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Function.prototype.bind called on non-function",
        ));
    }
    let bound_this = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let bound_arg_count = args.len().saturating_sub(2);

    // bound 函数 [[Prototype]] 为 Function.prototype（BoundFunctionCreate 语义）。
    let fn_proto_val = JsValue::from_js_object(vm.session().builtin_world().function_proto.as_ptr() as *mut JsObject);
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    unsafe {
        (*wrapper).set_function(true);
        (*wrapper).set_native_fn(Some(NativeFnPtr::from_raw(bind_dispatcher::<H> as *const ())));
        // 绑定实参个数记入 native_arg_count，与 Function.length 语义一致。
        (*wrapper).set_native_arg_count(bound_arg_count as u8);
    }

    // length/name 先定义占 dense 槽位 0/1（shape 属性），绑定状态 [target, thisArg,
    // ...boundArgs] 随后从槽位 2 起存放，保证 shape 槽位与 dense 下标对齐。
    let length_si = vm.kernel_core().perm_interner().intern("length").0;
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let caller_si = vm.kernel_core().perm_interner().intern("caller").0;
    let arguments_si = vm.kernel_core().perm_interner().intern("arguments").0;
    let attrs = PropAttributes::new(false, false, true);

    // bound 函数的 caller/arguments 是受限访问器：读写一律抛 TypeError（poisoned）。
    let thrower = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    unsafe {
        (*thrower).set_function(true);
        (*thrower).set_native_fn(Some(NativeFnPtr::from_raw(bound_restricted_thrower::<H> as *const ())));
    }
    let thrower_val = JsValue::from_js_object(thrower);

    // 全部 shape 属性（length/name/caller/arguments）须先于状态数据定义完成，
    // 保证 shape 槽位与 dense 下标对齐，再 push [target, thisArg, ...boundArgs]。
    unsafe {
        let wrapper_ref = &mut *wrapper;
        let target_obj = &*target_val.as_js_object_ptr();
        let target_length = vm
            .resolve_property(target_obj, length_si)
            .map(to_integer_or_infinity)
            .unwrap_or(0.0);
        // length = max(0, target.length - boundArgs)；target.length 为 +∞ 时保持 +∞。
        let length = if target_length == f64::INFINITY {
            f64::INFINITY
        } else {
            (target_length - bound_arg_count as f64).max(0.0)
        };
        // 规范 length 无上界：i32 装得下用 int，否则（如 2^31、MAX_SAFE_INTEGER）用 float。
        let length_val = if length <= i32::MAX as f64 {
            JsValue::int(length as i32)
        } else {
            JsValue::float(length)
        };
        if let Err(e) = vm.define_data_property(wrapper_ref, length_si, length_val, attrs) {
            return NativeResult::Err(crate::error::create_type_error(vm, &e));
        }
        let target_name = vm
            .resolve_property(target_obj, name_si)
            .and_then(|v| vm.lookup_str(v))
            .unwrap_or_default();
        let name_val = vm.new_string(&format!("bound {target_name}"));
        if let Err(e) = vm.define_data_property(wrapper_ref, name_si, name_val, attrs) {
            return NativeResult::Err(crate::error::create_type_error(vm, &e));
        }
        if let Err(e) = vm.define_accessor_property(wrapper_ref, caller_si, thrower_val, thrower_val, attrs) {
            return NativeResult::Err(crate::error::create_type_error(vm, &e));
        }
        if let Err(e) = vm.define_accessor_property(wrapper_ref, arguments_si, thrower_val, thrower_val, attrs) {
            return NativeResult::Err(crate::error::create_type_error(vm, &e));
        }
    }
    unsafe {
        let props = (*wrapper).ensure_hash_props();
        props.push(target_val);
        props.push(bound_this);
        for &r in args.iter().skip(2) {
            props.push(vm.reg(r));
        }
    }

    NativeResult::Ok(JsValue::from_js_object(wrapper))
}

/// bound 函数 caller/arguments 的受限访问器：任何访问（get/set）都抛 TypeError。
fn bound_restricted_thrower<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "'caller' and 'arguments' are restricted on bound functions",
    ))
}

/// `Function.prototype.toString`：返回 `function name() { [native code] }`
/// 或 `[bytecode]` 形式；函数名取自 `name` 属性或字节码子模块名。
pub fn function_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Function.prototype.toString called on non-function",
        ));
    }

    let func = unsafe { &*this_val.as_js_object_ptr() };
    if !func.is_function() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Function.prototype.toString called on non-function",
        ));
    }

    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let name = vm
        .resolve_property(func, name_si)
        .and_then(|v| vm.lookup_str(v))
        .unwrap_or_else(|| {
            let sub_idx = func.sub_module_index();
            if sub_idx > 0 {
                vm.sub_module_function_name((sub_idx - 1) as u16)
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
    NativeResult::Ok(vm.new_string(&result))
}
