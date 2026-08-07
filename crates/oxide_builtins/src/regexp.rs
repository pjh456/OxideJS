use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

fn get_regexp_ptr<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<*mut JsObject, JsValue> {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "RegExp.prototype method called on non-object"));
    }
    let ptr = this_val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "null object"));
    }
    if !unsafe { &*ptr }.is_regexp_obj() {
        return Err(crate::error::create_type_error(vm, "RegExp.prototype method called on non-RegExp object"));
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

fn set_prop<H: VmHost>(obj: &mut JsObject, name: &str, val: JsValue, vm: &H) {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    set_prop_by_si(obj, si, val, vm);
}

fn set_prop_by_si<H: VmHost>(obj: &mut JsObject, prop_name_si: u32, val: JsValue, vm: &H) {
    let shape_id = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), prop_name_si);
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

/// `RegExp(pattern, flags)` 构造逻辑：用 regress 引擎编译模式（ECMAScript 语法，
/// 支持 backreference/lookaround/命名组/v-flag）；非法模式抛 SyntaxError。
/// 编译结果存于对象的 native_fn 槽。
pub fn regexp_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (pattern, flags) = if args.len() < 2 {
        (String::new(), String::new())
    } else if args.len() < 3 {
        let pat = oxide_runtime_api::to_string(vm.reg(args[1]));
        (pat, String::new())
    } else {
        let pat = oxide_runtime_api::to_string(vm.reg(args[1]));
        let fl = oxide_runtime_api::to_string(vm.reg(args[2]));
        (pat, fl)
    };

    let (global, ignore_case, multi_line) = parse_flags(&flags);

    let mut obj = JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject),
    );

    // regress 用 JS flag 字符串编译（i/m 当前支持；u/s/y 由 parse_flags 处理）。
    let mut flags_in: String = String::new();
    if ignore_case {
        flags_in.push('i');
    }
    if multi_line {
        flags_in.push('m');
    }
    let compiled = regress::Regex::with_flags(&pattern, flags_in.as_str());

    match compiled {
        Ok(re) => {
            let re_ptr = Box::into_raw(Box::new(re));
            // SAFETY: re_ptr 是构造器经 `NativeFnPtr::from_raw(re_ptr as *const ())`
            // 写入的 `Box<regress::Regex>` 指针。RegExp 对象把 native_fn 字段复用作
            // 已编译 Regex 的存放处——而非 NativeFn 指针。对象存活期间有效；
            // VM 重置经 `drop_regexp_native` 释放该 Box。
            obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(re_ptr as *const ()) }));
        }
        Err(e) => {
            return NativeResult::Err(crate::error::create_syntax_error(
                vm,
                &format!("Invalid regular expression: {e}"),
            ));
        }
    }

    set_prop(&mut obj, "lastIndex", JsValue::int(0), vm);
    set_prop(&mut obj, "source", vm.new_string(&pattern), vm);
    set_prop(&mut obj, "flags", vm.new_string(&flags), vm);
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

/// 释放 RegExp 对象 native_fn 槽中编译的 `regress::Regex`，返回释放字节数。
pub fn drop_regexp_native(obj: &mut JsObject) -> u64 {
    if !obj.is_regexp_obj() {
        return 0;
    }
    let Some(ptr) = obj.native_fn() else {
        return 0;
    };
    let regex_ptr = ptr.as_ptr() as *mut regress::Regex;
    if regex_ptr.is_null() {
        return 0;
    }
    unsafe { drop(Box::from_raw(regex_ptr)) };
    obj.set_native_fn(None);
    std::mem::size_of::<regress::Regex>() as u64
}

/// `RegExp.prototype.test(string)`：判断是否匹配。global 模式下从 lastIndex 开始匹配。
pub fn regexp_test<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let re = unsafe { &*re_ptr };

    let fn_ptr = match re.native_fn() {
        None => {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid RegExp"));
        }
        Some(p) => p,
    };

    // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
    let haystack = oxide_runtime_api::to_string(vm.reg(if args.len() > 1 { args[1] } else { args[0] }));
    let last_index = vm.coerce_number_bounded(get_prop(re, 0)).unwrap_or(f64::NAN) as usize;
    let is_global = get_prop(re, 3).as_bool();

    if is_global {
        let result = regex.find_from(&haystack, last_index).next().is_some();
        NativeResult::Ok(JsValue::bool(result))
    } else {
        NativeResult::Ok(JsValue::bool(regex.find(&haystack).is_some()))
    }
}

/// `RegExp.prototype.exec(string)`：执行匹配并返回数组（含捕获组、index、input）。
/// 无匹配返回 null；global 模式推进/重置 lastIndex。
pub fn regexp_exec<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let fn_ptr = {
        let re = unsafe { &*re_ptr };
        match re.native_fn() {
            None => {
                return NativeResult::Err(crate::error::create_type_error(vm, "invalid RegExp"));
            }
            Some(p) => p,
        }
    };

    // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
    let haystack = oxide_runtime_api::to_string(vm.reg(if args.len() > 1 { args[1] } else { args[0] }));

    let (last_index, is_global) = {
        let re = unsafe { &*re_ptr };
        let li = vm.coerce_number_bounded(get_prop(re, 0)).unwrap_or(f64::NAN) as usize;
        let g = get_prop(re, 3).as_bool();
        (li, g)
    };

    let match_result = if is_global {
        if last_index > haystack.len() {
            return NativeResult::Ok(JsValue::null());
        }
        regex.find_from(&haystack, last_index).next()
    } else {
        regex.find(&haystack)
    };

    if let Some(m) = match_result {
        let range = m.range();
        let group_count = m.captures.len();
        let n = 1 + group_count;
        let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
        let arr = vm.epoch().alloc(JsObject::new_array(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto),
            n,
            vm.epoch().bump(),
        ));
        unsafe {
            (*arr).set_prop_at(0, vm.new_string(&haystack[range.start..range.end]));
            // 捕获组：未参与匹配的组为 undefined。
            for i in 1..=group_count {
                match m.group(i) {
                    Some(g) => (*arr).set_prop_at(i, vm.new_string(&haystack[g.start..g.end])),
                    None => (*arr).set_prop_at(i, JsValue::undefined()),
                }
            }
            (*arr).set_prop_count(n);
        }

        // ponytail: 引擎数组的 shape 属性槽与元素 prop_vec 共用 hash_props 索引，
        // 设 index/input/groups 属性会与元素冲突/膨胀 length。暂只填元素。
        if is_global {
            set_prop_at(re_ptr, 0, JsValue::int(range.end as i32));
        }

        NativeResult::Ok(JsValue::from_js_object(arr))
    } else {
        if is_global {
            set_prop_at(re_ptr, 0, JsValue::int(0));
        }
        NativeResult::Ok(JsValue::null())
    }
}

/// `RegExp.prototype.toString`：按 `/source/flags` 形式返回。
pub fn regexp_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
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
    NativeResult::Ok(vm.new_string(&result))
}

