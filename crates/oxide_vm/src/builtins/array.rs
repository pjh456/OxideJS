use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::{NativeFn, NativeResult};
use crate::vm::Vm;

macro_rules! array_ptr {
    ($vm:expr, $args:expr) => {
        get_this_array_ref($vm.reg($args[0]))?
    };
}

macro_rules! array_ptr_len {
    ($vm:expr, $args:expr) => {{
        let arr_ptr = array_ptr!($vm, $args);
        let len = unsafe { (*arr_ptr).prop_count() } as usize;
        (arr_ptr, len)
    }};
}

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
            (*arr).set_prop_at(i, vm.reg(args[1 + i]));
        }
    }
    unsafe {
        (*arr).set_prop_count(n_elems);
    }
    Ok(JsValue::from_js_object(arr))
}

pub fn array_push(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let mut len = arr.prop_count();
    for &arg_reg in args.iter().skip(1) {
        let val = vm.reg(arg_reg);
        arr.set_prop_at(len, val);
        len += 1;
    }
    arr.set_prop_count(len);
    Ok(JsValue::int(len as i32))
}

pub fn array_pop(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count();
    if len == 0 {
        return Ok(JsValue::undefined());
    }
    let last = arr.get_prop_at(len - 1);
    arr.set_prop_count(len - 1);
    Ok(last)
}

pub fn array_slice(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
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
            (*new_arr).set_prop_at(i, arr.get_prop_at(start + i));
        }
        (*new_arr).set_prop_count(count);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_splice(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
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
    arr.set_prop_count(new_len);

    let removed_arr = create_new_array(vm, removed.len());
    unsafe {
        for (i, val) in removed.iter().enumerate() {
            (*removed_arr).set_prop_at(i, *val);
        }
        (*removed_arr).set_prop_count(removed.len());
    }
    Ok(JsValue::from_js_object(removed_arr))
}

pub fn array_concat(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    let mut all: Vec<JsValue> = Vec::new();
    for i in 0..n {
        all.push(arr.get_prop_at(i));
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
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_join(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
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
            let v = arr.get_prop_at(i);
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
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    if args.len() < 2 {
        return Ok(JsValue::int(-1));
    }
    let target = vm.reg(args[1]);
    for i in 0..n {
        if coercion::strict_equality(arr.get_prop_at(i), target) {
            return Ok(JsValue::int(i as i32));
        }
    }
    Ok(JsValue::int(-1))
}

pub fn array_includes(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let n = arr.prop_count() as usize;
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let target = vm.reg(args[1]);
    for i in 0..n {
        let elem = arr.get_prop_at(i);
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
    Ok(vm.reg(args[0]))
}

pub fn array_flat(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
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
                        let sub: Vec<JsValue> = (0..on).map(|i| o.get_prop_at(i)).collect();
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

    let all: Vec<JsValue> = (0..n).map(|i| arr.get_prop_at(i)).collect();
    let flat = flatten(&all, depth);
    let new_arr = create_new_array(vm, flat.len());
    unsafe {
        for (i, val) in flat.iter().enumerate() {
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(flat.len());
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_for_each(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(mapped) => unsafe {
                (*new_arr).set_prop_at(i, mapped);
            },
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    unsafe {
        (*new_arr).set_prop_count(n);
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_filter(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(kept.len());
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_reduce(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        accumulator = unsafe { (*arr_ptr).get_prop_at(0) };
        start_idx = 1;
    }
    let this_val = JsValue::undefined();
    for i in start_idx..n {
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
    let (arr_ptr, n) = array_ptr_len!(vm, args);
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
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
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
                                flat.push(r.get_prop_at(j));
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
            (*new_arr).set_prop_at(i, *val);
        }
        (*new_arr).set_prop_count(flat.len());
    }
    Ok(JsValue::from_js_object(new_arr))
}

pub fn array_shift(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count();
    if len == 0 {
        return Ok(JsValue::undefined());
    }
    let first = arr.get_prop_at(0);
    for i in 1..len {
        let v = arr.get_prop_at(i);
        arr.set_prop_at(i - 1, v);
    }
    arr.set_prop_count(len - 1);
    Ok(first)
}

pub fn array_unshift(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
    arr.set_prop_count(new_len);
    Ok(JsValue::int(new_len as i32))
}

pub fn array_fill(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count() as usize;
    let value = if args.len() > 1 {
        vm.reg(args[1])
    } else {
        JsValue::undefined()
    };
    let start = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as i32).max(0)
            as usize
    } else {
        0
    };
    let end = if args.len() > 3 {
        let e = coercion::to_number(vm.reg(args[3]), vm.kernel().string_forge().as_ref()) as i32;
        (e as usize).min(len)
    } else {
        len
    };
    for i in start..end {
        arr.set_prop_at(i, value);
    }
    Ok(vm.reg(args[0]))
}

pub fn array_copy_within(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count() as usize;
    if len == 0 {
        return Ok(vm.reg(args[0]));
    }
    let target = if args.len() > 1 {
        (coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32) as usize
    } else {
        0
    };
    let start = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as i32) as usize
    } else {
        0
    };
    let end = if args.len() > 3 {
        (coercion::to_number(vm.reg(args[3]), vm.kernel().string_forge().as_ref()) as i32 as usize)
            .min(len)
    } else {
        len
    };
    let mut to = target;
    for from in start..end {
        if to >= len {
            break;
        }
        let v = arr.get_prop_at(from);
        arr.set_prop_at(to, v);
        to += 1;
    }
    Ok(vm.reg(args[0]))
}

pub fn array_at(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let len = arr.prop_count() as i32;
    let mut index = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32
    } else {
        0
    };
    if index < 0 {
        index += len;
    }
    if index < 0 || index >= len {
        return Ok(JsValue::undefined());
    }
    Ok(arr.get_prop_at(index))
}

pub fn array_last_index_of(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, args);
    let arr = unsafe { &*arr_ptr };
    let len = arr.prop_count() as i32;
    let search = if args.len() > 1 {
        vm.reg(args[1])
    } else {
        JsValue::undefined()
    };
    for i in (0..len).rev() {
        if coercion::strict_equality(arr.get_prop_at(i), search) {
            return Ok(JsValue::int(i));
        }
    }
    Ok(JsValue::int(-1))
}

pub fn array_find_index(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (arr_ptr, n) = array_ptr_len!(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::int(-1));
    }
    let callback_val = vm.reg(args[1]);
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    for i in 0..n {
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i as i32), vm.reg(args[0])],
        ) {
            Ok(r) => {
                if coercion::to_boolean(r, vm.kernel().string_forge().as_ref()) {
                    return Ok(JsValue::int(i as i32));
                }
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(JsValue::int(-1))
}

pub fn array_find_last(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (arr_ptr, n) = {
        let (arr_ptr, len) = array_ptr_len!(vm, args);
        (arr_ptr, len as i32)
    };
    if args.len() < 2 {
        return Ok(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    let this_val = if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    };
    for i in (0..n).rev() {
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
        match invoke_native_callback(
            vm,
            callback_val,
            this_val,
            &[elem, JsValue::int(i), vm.reg(args[0])],
        ) {
            Ok(r) => {
                if coercion::to_boolean(r, vm.kernel().string_forge().as_ref()) {
                    return Ok(elem);
                }
            }
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(JsValue::undefined())
}

pub fn array_reduce_right(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (arr_ptr, n) = array_ptr_len!(vm, args);
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let callback_val = vm.reg(args[1]);
    let (mut acc, start_idx): (JsValue, i32) = if args.len() > 2 {
        (vm.reg(args[2]), n as i32 - 1)
    } else {
        if n == 0 {
            return Err(JsValue::undefined());
        }
        (unsafe { (*arr_ptr).get_prop_at(n - 1) }, n as i32 - 2)
    };
    for i in (0..=start_idx).rev() {
        let elem = unsafe { (*arr_ptr).get_prop_at(i) };
        match invoke_native_callback(
            vm,
            callback_val,
            JsValue::undefined(),
            &[acc, elem, JsValue::int(i), vm.reg(args[0])],
        ) {
            Ok(r) => acc = r,
            Err(_) => return Err(JsValue::undefined()),
        }
    }
    Ok(acc)
}

pub fn array_sort(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let arr_ptr = array_ptr!(vm, _args);
    let arr = unsafe { &mut *arr_ptr };
    let len = arr.prop_count() as usize;
    let mut vals: Vec<JsValue> = (0..len).map(|i| arr.get_prop_at(i)).collect();
    vals.sort_by(|a, b| {
        let sa = coercion::to_string(vm.kernel().string_forge().as_ref(), *a);
        let sb = coercion::to_string(vm.kernel().string_forge().as_ref(), *b);
        sa.cmp(&sb)
    });
    for (i, &v) in vals.iter().enumerate() {
        arr.set_prop_at(i, v);
    }
    Ok(vm.reg(_args[0]))
}
