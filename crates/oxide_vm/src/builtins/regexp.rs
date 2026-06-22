use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

fn get_regexp_ptr(vm: &Vm, args: &[u8]) -> Result<*mut JsObject, JsValue> {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return Err(JsValue::string(
            vm.kernel_core()
                .string_forge()
                .intern("TypeError: RegExp.prototype method called on non-object")
                .0,
            0,
        ));
    }
    let ptr = this_val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(JsValue::string(vm.kernel_core().string_forge().intern("TypeError: null object").0, 0));
    }
    if !unsafe { &*ptr }.is_regexp_obj() {
        return Err(JsValue::string(
            vm.kernel_core()
                .string_forge()
                .intern("TypeError: RegExp.prototype method called on non-RegExp object")
                .0,
            0,
        ));
    }
    Ok(ptr)
}

fn parse_flags(flags: &str) -> (bool, bool, bool) {
    let mut global = false;
    let mut ignore_case = false;
    let mut multi_line = false;
    for c in flags.chars() {
        match c {
            'g' => global = true,
            'i' => ignore_case = true,
            'm' => multi_line = true,
            _ => {}
        }
    }
    (global, ignore_case, multi_line)
}

fn set_prop(obj: &mut JsObject, name: &str, val: JsValue, vm: &Vm) {
    let si = vm.kernel_core().string_forge().intern(name).0;
    let shape_id = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), si);
    obj.set_shape_id(shape_id);
    obj.ensure_hash_props().push(val);
}

fn get_prop(obj: &JsObject, idx: usize) -> JsValue {
    obj.hash_props_vec()
        .and_then(|v| v.get(idx))
        .copied()
        .unwrap_or(JsValue::undefined())
}

fn set_prop_at(obj: *mut JsObject, idx: usize, val: JsValue) {
    unsafe {
        let re = &mut *obj;
        if let Some(vec) = re.hash_props_vec() {
            let ptr = vec.as_ptr() as *mut JsValue;
            let v = &mut *ptr.add(idx);
            *v = val;
        }
    }
}

pub fn regexp_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let (pattern, flags) = if args.len() < 2 {
        (String::new(), String::new())
    } else if args.len() < 3 {
        let pat = coercion::to_string(vm.kernel_core().string_forge().as_ref(), vm.reg(args[1]));
        (pat, String::new())
    } else {
        let pat = coercion::to_string(vm.kernel_core().string_forge().as_ref(), vm.reg(args[1]));
        let fl = coercion::to_string(vm.kernel_core().string_forge().as_ref(), vm.reg(args[2]));
        (pat, fl)
    };

    let (global, ignore_case, multi_line) = parse_flags(&flags);

    let mut obj = JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject),
    );

    let compiled = regex::RegexBuilder::new(&pattern)
        .case_insensitive(ignore_case)
        .multi_line(multi_line)
        .build();

    match compiled {
        Ok(re) => {
            let re_ptr = Box::into_raw(Box::new(re));
            // SAFETY: re_ptr is a Box<regex::Regex> pointer stored by the constructor via
            // NativeFnPtr::from_raw(re_ptr as *const ()). RegExp objects repurpose the native_fn
            // field to hold the compiled Regex — not a NativeFn pointer. Valid for the object's
            // lifetime; VM reset drops the Box through `drop_regexp_native`.
            obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(re_ptr as *const ()) }));
        }
        Err(e) => {
            let msg = format!("SyntaxError: Invalid regular expression: {}", e);
            return NativeResult::Err(JsValue::string(vm.kernel_core().string_forge().intern(&msg).0, 0));
        }
    }

    set_prop(&mut obj, "lastIndex", JsValue::int(0), vm);
    set_prop(&mut obj, "source", vm.intern(&pattern), vm);
    set_prop(&mut obj, "flags", vm.intern(&flags), vm);
    set_prop(&mut obj, "global", JsValue::bool(global), vm);
    set_prop(&mut obj, "ignoreCase", JsValue::bool(ignore_case), vm);
    set_prop(&mut obj, "multiline", JsValue::bool(multi_line), vm);
    set_prop(&mut obj, "dotAll", JsValue::bool(false), vm);
    set_prop(&mut obj, "sticky", JsValue::bool(false), vm);
    set_prop(&mut obj, "unicode", JsValue::bool(false), vm);
    obj.type_tag = JsObject::OBJ_TYPE_REGEXP;

    let obj_ptr = vm.alloc_object(obj);
    NativeResult::Ok(JsValue::from_js_object(obj_ptr))
}

pub(crate) fn drop_regexp_native(obj: &mut JsObject) -> u64 {
    if !obj.is_regexp_obj() {
        return 0;
    }
    let Some(ptr) = obj.native_fn() else {
        return 0;
    };
    let regex_ptr = ptr.as_ptr() as *mut regex::Regex;
    if regex_ptr.is_null() {
        return 0;
    }
    unsafe { drop(Box::from_raw(regex_ptr)) };
    obj.set_native_fn(None);
    std::mem::size_of::<regex::Regex>() as u64
}

pub fn regexp_test(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let re = unsafe { &*re_ptr };

    let fn_ptr = match re.native_fn() {
        None => {
            return NativeResult::Err(JsValue::string(
                vm.kernel_core().string_forge().intern("TypeError: invalid RegExp").0,
                0,
            ));
        }
        Some(p) => p,
    };

    // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
    let haystack = coercion::to_string(
        vm.kernel_core().string_forge().as_ref(),
        vm.reg(if args.len() > 1 { args[1] } else { args[0] }),
    );
    let last_index = coercion::to_number(get_prop(re, 0), vm.kernel_core().string_forge().as_ref()) as usize;
    let is_global = get_prop(re, 3).as_bool();

    if is_global {
        let result = regex.find_at(&haystack, last_index).is_some();
        NativeResult::Ok(JsValue::bool(result))
    } else {
        NativeResult::Ok(JsValue::bool(regex.is_match(&haystack)))
    }
}

pub fn regexp_exec(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let fn_ptr = {
        let re = unsafe { &*re_ptr };
        match re.native_fn() {
            None => {
                return NativeResult::Err(JsValue::string(
                    vm.kernel_core().string_forge().intern("TypeError: invalid RegExp").0,
                    0,
                ));
            }
            Some(p) => p,
        }
    };

    // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
    let haystack = coercion::to_string(
        vm.kernel_core().string_forge().as_ref(),
        vm.reg(if args.len() > 1 { args[1] } else { args[0] }),
    );

    let (last_index, is_global) = {
        let re = unsafe { &*re_ptr };
        let li = coercion::to_number(get_prop(re, 0), vm.kernel_core().string_forge().as_ref()) as usize;
        let g = get_prop(re, 3).as_bool();
        (li, g)
    };

    let match_result = if is_global {
        if last_index > haystack.len() {
            return NativeResult::Ok(JsValue::null());
        }
        regex.find_at(&haystack, last_index)
    } else {
        regex.find(&haystack)
    };

    if let Some(m) = match_result {
        let mut match_obj = JsObject::new_empty(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject),
        );

        let full_match = &haystack[m.start()..m.end()];
        let full_si = vm.kernel_core().string_forge().intern(full_match).0;
        match_obj.ensure_hash_props().push(JsValue::string(full_si, 0));

        let captures = regex.captures(&haystack[m.start()..m.end()]);
        if let Some(caps) = &captures {
            for i in 1..caps.len() {
                let cap_str = caps.get(i).map(|cm| cm.as_str()).unwrap_or("");
                let si = vm.kernel_core().string_forge().intern(cap_str).0;
                match_obj.ensure_hash_props().push(JsValue::string(si, 0));
            }
        }

        set_prop(&mut match_obj, "index", JsValue::int(m.start() as i32), vm);
        let haystack_val = vm.intern(&haystack);
        set_prop(&mut match_obj, "input", haystack_val, vm);
        set_prop(&mut match_obj, "groups", JsValue::undefined(), vm);

        if is_global {
            set_prop_at(re_ptr, 0, JsValue::int(m.end() as i32));
        }

        let obj_ptr = vm.alloc_object(match_obj);
        NativeResult::Ok(JsValue::from_js_object(obj_ptr))
    } else {
        if is_global {
            set_prop_at(re_ptr, 0, JsValue::int(0));
        }
        NativeResult::Ok(JsValue::null())
    }
}

pub fn regexp_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let re = unsafe { &*re_ptr };
    let source = {
        let val = get_prop(re, 1);
        vm.lookup_str(val).unwrap_or_default()
    };
    let flags = {
        let val = get_prop(re, 2);
        vm.lookup_str(val).unwrap_or_default()
    };
    let result = format!("/{}/{}", source, flags);
    let si = vm.kernel_core().string_forge().intern(&result).0;
    NativeResult::Ok(JsValue::string(si, 0))
}
