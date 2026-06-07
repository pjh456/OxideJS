use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::{NativeFn, NativeResult};
use crate::vm::Vm;

fn get_this_array_ref(val: JsValue) -> Result<*mut JsObject, JsValue> {
    if !val.is_object() {
        return Err(JsValue::undefined());
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(JsValue::undefined());
    }
    let obj = unsafe { &*ptr };
    if !obj.is_array() {
        return Err(JsValue::undefined());
    }
    Ok(ptr)
}

fn create_new_array(vm: &mut Vm, n: usize) -> *mut JsObject {
    let proto = vm.kernel().builtin_world().array_proto.as_ptr() as *mut JsObject;
    vm.epoch().alloc(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
        n,
        vm.epoch().bump(),
    ))
}

fn invoke_native_callback(
    vm: &mut Vm,
    callback_val: JsValue,
    this_val: JsValue,
    cb_args: &[JsValue],
) -> NativeResult {
    if !callback_val.is_object() {
        return Err(JsValue::undefined());
    }
    let cb_ptr = callback_val.as_js_object_ptr();
    if cb_ptr.is_null() {
        return Err(JsValue::undefined());
    }
    let cb = unsafe { &*cb_ptr };
    if !cb.is_function() || cb.native_fn().is_none() {
        return Err(JsValue::undefined());
    }
    let func: NativeFn = unsafe { std::mem::transmute(cb.native_fn().unwrap()) };

    let base = 240u8;
    let n = cb_args.len().min(15);
    let mut args_buf = [0u8; 17];
    vm.set_reg(base, this_val);
    args_buf[0] = base;
    for i in 0..n {
        vm.set_reg(base + 1 + i as u8, cb_args[i]);
        args_buf[i + 1] = base + 1 + i as u8;
    }

    func(vm, &args_buf[..n + 1])
}

pub fn array_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let proto = vm.kernel().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let proto_val = JsValue::from_js_object(proto);

    if args.len() == 2 {
        let val = vm.reg(args[1]);
        if val.is_int() {
            let n = val.as_int().max(0) as usize;
            let arr = vm.epoch().alloc(JsObject::new_array(
                EMPTY_SHAPE_ID,
                proto_val,
                n,
                vm.epoch().bump(),
            ));
            return Ok(JsValue::from_js_object(arr));
        }
        if val.is_double() {
            let n = val.as_double() as usize;
            let arr = vm.epoch().alloc(JsObject::new_array(
                EMPTY_SHAPE_ID,
                proto_val,
                n,
                vm.epoch().bump(),
            ));
            return Ok(JsValue::from_js_object(arr));
        }
    }

    let n_elems = if args.len() > 1 { args.len() - 1 } else { 0 };
    let arr = vm.epoch().alloc(JsObject::new_array(
        EMPTY_SHAPE_ID,
        proto_val,
        n_elems,
        vm.epoch().bump(),
    ));
    for i in 0..n_elems {
        unsafe {
            (*arr).set_prop(i as u8, vm.reg(args[1 + i]));
        }
    }
    unsafe {
        (*arr).set_prop_count(n_elems as u8);
    }
    Ok(JsValue::from_js_object(arr))
}

pub fn array_push(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &mut *arr_ptr };
    let mut len = arr.prop_count() as usize;
    let bump = vm.epoch().bump();
    for &arg_reg in args.iter().skip(1) {
        let val = vm.reg(arg_reg);
        arr.set_prop_expand(len as u8, val, bump);
        len += 1;
    }
    let new_len = (len as u8).min(31);
    arr.set_prop_count(new_len);
    Ok(JsValue::int(new_len as i32))
}

pub fn array_pop(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count();
    if len == 0 {
        return Ok(JsValue::undefined());
    }
    let last = arr.get_prop(len - 1);
    arr.set_prop_count(len - 1);
    Ok(last)
}

pub fn array_slice(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    let start = if args.len() > 1 {
        (coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32) as usize
    } else {
        0
    };
    let end = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as i32) as usize
    } else {
        n
    };
    let start = start.min(n);
    let end = end.min(n);
    let count = end.saturating_sub(start);

    let new_arr = create_new_array(vm, count);
    unsafe {
        for i in 0..count {
            (*new_arr).set_prop(i as u8, arr.get_prop((start + i) as u8));
        }
        (*new_arr).set_prop_count(count.min(31) as u8);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_splice(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &mut *arr_ptr };
    let n = arr.prop_count() as usize;

    let start = if args.len() > 1 {
        let v = coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref());
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
        let v = coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref());
        (v as usize).min(n - start)
    } else {
        n - start
    };

    let insert_count = if args.len() > 3 { args.len() - 3 } else { 0 };

    let mut removed: Vec<JsValue> = Vec::new();
    for i in 0..delete_count {
        removed.push(arr.get_prop((start + i) as u8));
    }

    let bump = vm.epoch().bump();
    if insert_count > delete_count {
        let shift = insert_count - delete_count;
        for i in (start + delete_count..n).rev() {
            let val = arr.get_prop(i as u8);
            arr.set_prop_expand((i + shift) as u8, val, bump);
        }
    } else if insert_count < delete_count {
        let shift = delete_count - insert_count;
        for i in start + delete_count..n {
            let val = arr.get_prop(i as u8);
            arr.set_prop((i - shift) as u8, val);
        }
    }

    for i in 0..insert_count {
        arr.set_prop((start + i) as u8, vm.reg(args[3 + i]));
    }

    let new_len = n + insert_count - delete_count;
    arr.set_prop_count(new_len.min(31) as u8);

    let removed_arr = create_new_array(vm, removed.len());
    unsafe {
        for (i, val) in removed.iter().enumerate() {
            (*removed_arr).set_prop(i as u8, *val);
        }
        (*removed_arr).set_prop_count(removed.len().min(31) as u8);
    }
    Ok(JsValue::from_js_object(removed_arr))
}

pub fn array_concat(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    let mut all: Vec<JsValue> = Vec::new();
    for i in 0..n {
        all.push(arr.get_prop(i as u8));
    }
    for &arg_reg in args.iter().skip(1) {
        let val = vm.reg(arg_reg);
        if val.is_object() {
            let o_ptr = val.as_js_object_ptr();
            if !o_ptr.is_null() {
                let o = unsafe { &*o_ptr };
                if o.is_array() {
                    let on = o.prop_count() as usize;
                    for i in 0..on {
                        all.push(o.get_prop(i as u8));
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
            (*new_arr).set_prop(i as u8, *val);
        }
        (*new_arr).set_prop_count(all.len().min(31) as u8);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_join(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    let sep = if args.len() > 1 {
        coercion::to_string(vm.kernel().string_forge().as_ref(), vm.reg(args[1]))
    } else {
        ",".to_string()
    };
    let sf = vm.kernel().string_forge().as_ref();
    let parts: Vec<String> = (0..n)
        .map(|i| {
            let v = arr.get_prop(i as u8);
            if v.is_undefined() || v.is_null() {
                String::new()
            } else {
                coercion::to_string(sf, v)
            }
        })
        .collect();
    let joined = parts.join(&sep);
    let result_val = vm.intern(&joined);
    Ok(result_val)
}

pub fn array_index_of(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    if args.len() < 2 {
        return Ok(JsValue::int(-1));
    }
    let target = vm.reg(args[1]);
    for i in 0..n {
        if coercion::strict_equality(arr.get_prop(i as u8), target) {
            return Ok(JsValue::int(i as i32));
        }
    }
    Ok(JsValue::int(-1))
}

pub fn array_includes(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let target = vm.reg(args[1]);
    for i in 0..n {
        let elem = arr.get_prop(i as u8);
        if elem.is_double() && target.is_double() {
            let ea = elem.as_double();
            let ta = target.as_double();
            if ea.is_nan() && ta.is_nan() {
                return Ok(JsValue::bool(true));
            }
        }
        if coercion::strict_equality(elem, target) {
            return Ok(JsValue::bool(true));
        }
    }
    Ok(JsValue::bool(false))
}

pub fn array_reverse(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &mut *arr_ptr };
    let n = arr.prop_count() as usize;
    let mut i = 0;
    let mut j = n.saturating_sub(1);
    while i < j {
        let tmp = arr.get_prop(i as u8);
        arr.set_prop(i as u8, arr.get_prop(j as u8));
        arr.set_prop(j as u8, tmp);
        i += 1;
        j = j.saturating_sub(1);
    }
    Ok(vm.reg(args[0]))
}

pub fn array_flat(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    let depth = if args.len() > 1 {
        (coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32).max(1)
            as usize
    } else {
        1
    };

    fn flatten(items: &[JsValue], remaining_depth: usize) -> Vec<JsValue> {
        let mut out = Vec::new();
        for &v in items {
            if remaining_depth > 0 && v.is_object() {
                let ptr = v.as_js_object_ptr();
                if !ptr.is_null() {
                    let o = unsafe { &*ptr };
                    if o.is_array() {
                        let on = o.prop_count() as usize;
                        let sub: Vec<JsValue> = (0..on).map(|i| o.get_prop(i as u8)).collect();
                        let flat = flatten(&sub, remaining_depth - 1);
                        out.extend(flat);
                        continue;
                    }
                }
            }
            out.push(v);
        }
        out
    }

    let all: Vec<JsValue> = (0..n).map(|i| arr.get_prop(i as u8)).collect();
    let flat = flatten(&all, depth);
    let new_arr = create_new_array(vm, flat.len());
    unsafe {
        for (i, val) in flat.iter().enumerate() {
            (*new_arr).set_prop(i as u8, *val);
        }
        (*new_arr).set_prop_count(flat.len().min(31) as u8);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_for_each(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Err(JsValue::undefined());
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        let _ = invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        );
    }
    Ok(JsValue::undefined())
}

pub fn array_map(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Err(JsValue::undefined());
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    let new_arr = create_new_array(vm, n);
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(mapped) => unsafe {
                (*new_arr).set_prop(i as u8, mapped);
            },
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    unsafe {
        (*new_arr).set_prop_count(n.min(31) as u8);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_filter(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Err(JsValue::undefined());
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    let mut kept: Vec<JsValue> = Vec::new();
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(result_val) => {
                let sf = vm.kernel().string_forge().as_ref();
                if coercion::to_boolean(result_val, sf) {
                    kept.push(elem);
                }
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    let new_arr = create_new_array(vm, kept.len());
    unsafe {
        for (i, val) in kept.iter().enumerate() {
            (*new_arr).set_prop(i as u8, *val);
        }
        (*new_arr).set_prop_count(kept.len().min(31) as u8);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_reduce(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if n == 0 && args.len() < 3 {
        return Err(JsValue::undefined());
    }
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Err(JsValue::undefined());
    }
    let has_initial = args.len() > 2;
    let mut accumulator;
    let start_idx;
    if has_initial {
        accumulator = vm.reg(args[2]);
        start_idx = 0;
    } else {
        accumulator = unsafe { (*arr_ptr).get_prop(0) };
        start_idx = 1;
    }
    let this_val = JsValue::undefined();
    for i in start_idx..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[accumulator, elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(result) => accumulator = result,
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(accumulator)
}

pub fn array_find(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Ok(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Ok(JsValue::undefined());
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(result_val) => {
                let sf = vm.kernel().string_forge().as_ref();
                if coercion::to_boolean(result_val, sf) {
                    return Ok(elem);
                }
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(JsValue::undefined())
}

pub fn array_some(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Ok(JsValue::bool(false));
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(result_val) => {
                let sf = vm.kernel().string_forge().as_ref();
                if coercion::to_boolean(result_val, sf) {
                    return Ok(JsValue::bool(true));
                }
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(JsValue::bool(false))
}

pub fn array_every(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Ok(JsValue::bool(false));
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(result_val) => {
                let sf = vm.kernel().string_forge().as_ref();
                if !coercion::to_boolean(result_val, sf) {
                    return Ok(JsValue::bool(false));
                }
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(JsValue::bool(true))
}

pub fn array_flat_map(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = get_this_array_ref(vm.reg(args[0]))?;
    let n = unsafe { (*arr_ptr).prop_count() } as usize;
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    if !callback_val.is_object() {
        return Err(JsValue::undefined());
    }
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    let mut flat: Vec<JsValue> = Vec::new();
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop(i as u8) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(result) => {
                if result.is_object() {
                    let r_ptr = result.as_js_object_ptr();
                    if !r_ptr.is_null() {
                        let r = unsafe { &*r_ptr };
                        if r.is_array() {
                            let rn = r.prop_count() as usize;
                            for j in 0..rn {
                                flat.push(r.get_prop(j as u8));
                            }
                            continue;
                        }
                    }
                }
                flat.push(result);
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    let new_arr = create_new_array(vm, flat.len());
    unsafe {
        for (i, val) in flat.iter().enumerate() {
            (*new_arr).set_prop(i as u8, *val);
        }
        (*new_arr).set_prop_count(flat.len().min(31) as u8);
    }
    Ok(JsValue::from_js_object(new_arr))
}
