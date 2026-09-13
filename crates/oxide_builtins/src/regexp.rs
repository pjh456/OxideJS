use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::string::{make_units_array, regex_replace_fn, regex_replace_manual_units, OwnedText};

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

fn parse_flags(flags: &str) -> (bool, bool, bool, bool, bool, bool, bool) {
    let mut global = false;
    let mut ignore_case = false;
    let mut multi_line = false;
    let mut dot_all = false;
    let mut sticky = false;
    let mut unicode = false;
    let mut has_indices = false;
    for c in flags.chars() {
        match c {
            'g' => global = true,
            'i' => ignore_case = true,
            'm' => multi_line = true,
            's' => dot_all = true,
            'y' => sticky = true,
            'u' => unicode = true,
            'd' => has_indices = true,
            _ => {}
        }
    }
    (global, ignore_case, multi_line, dot_all, sticky, unicode, has_indices)
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

    let (global, ignore_case, multi_line, dot_all, sticky, unicode, has_indices) = parse_flags(&flags);

    let mut obj = JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject),
    );

    // regress 用 JS flag 字符串编译：g/i/m/s/u/y 原样透传（v 由调用方映射为 u+set 语义）。
    let compiled = regress::Regex::with_flags(&pattern, flags.as_str());

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
    set_prop(&mut obj, "dotAll", JsValue::bool(dot_all), vm);
    set_prop(&mut obj, "sticky", JsValue::bool(sticky), vm);
    set_prop(&mut obj, "unicode", JsValue::bool(unicode), vm);
    set_prop(&mut obj, "hasIndices", JsValue::bool(has_indices), vm);
    obj.type_tag = JsObject::OBJ_TYPE_REGEXP;

    let obj_ptr = vm.alloc_object(obj);
    NativeResult::Ok(JsValue::from_js_object(obj_ptr))
}

/// 只读核算持有编译正则对象（RegExp / matchAll 载体）的正则字节（不释放）。
pub fn regexp_native_size(obj: &JsObject) -> u64 {
    if !obj.holds_compiled_regex() {
        return 0;
    }
    let Some(ptr) = obj.native_fn() else {
        return 0;
    };
    let regex_ptr = ptr.as_ptr() as *mut regress::Regex;
    if regex_ptr.is_null() {
        return 0;
    }
    std::mem::size_of::<regress::Regex>() as u64
}

/// 释放持有编译正则对象（RegExp / matchAll 载体）native_fn 槽中的
/// `regress::Regex`，返回释放字节数。
pub fn drop_regexp_native(obj: &mut JsObject) -> u64 {
    let bytes = regexp_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = obj.native_fn() else {
        return 0;
    };
    let regex_ptr = ptr.as_ptr() as *mut regress::Regex;
    // SAFETY: regex_ptr 非空（regexp_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(regex_ptr)) };
    obj.set_native_fn(None);
    bytes
}

/// 深拷贝已编译正则到新对象（GC 搬移 / 晋升流程用）。
///
/// 与 drop 配对：新对象获得独立的 `Box<regress::Regex>`，源对象保留自己的指针，
/// 两侧各自释放恰好一次，杜绝跨 arena 克隆后的指针别名双释放。
/// 作用于全部持编译正则的对象形态（RegExp / matchAll 载体）。
pub fn clone_regexp_native(old_obj: &JsObject, new_obj: &mut JsObject) {
    if !old_obj.holds_compiled_regex() {
        return;
    }
    let Some(ptr) = old_obj.native_fn() else {
        return;
    };
    let regex_ptr = ptr.as_ptr() as *const regress::Regex;
    if regex_ptr.is_null() {
        return;
    }
    // SAFETY: old_obj 的 native_fn 槽存 `Box<regress::Regex>` 指针，对象存活期间有效。
    let cloned_ptr = Box::into_raw(Box::new(unsafe { (&*regex_ptr).clone() }));
    new_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(cloned_ptr as *const ()) }));
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
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };
    let last_index = vm.coerce_number_bounded(get_prop(re, 0)).unwrap_or(f64::NAN) as usize;
    let is_global = is_global(re);

    // lastIndex 为码元口径；Str 臂的内部字节换算在 find_from_units 内完成。
    let text = haystack.as_match_text();
    let found = if is_global {
        text.find_from_units(regex, last_index).is_some()
    } else {
        text.find_from_units(regex, 0).is_some()
    };
    NativeResult::Ok(JsValue::bool(found))
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
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };

    let (last_index, is_global) = {
        let re = unsafe { &*re_ptr };
        let li = vm.coerce_number_bounded(get_prop(re, 0)).unwrap_or(f64::NAN) as usize;
        let g = is_global(re);
        (li, g)
    };

    // 匹配范围取臂原生命径（Str 臂字节、Units 臂码元）；index/lastIndex 落盘
    // 前统一换算到码元口径（规格口径）。
    let text = haystack.as_match_text();
    let match_result = if is_global {
        if last_index > text.len_units() {
            return NativeResult::Ok(JsValue::null());
        }
        text.find_from_units(regex, last_index)
    } else {
        text.find_from_units(regex, 0)
    };

    if let Some(m) = match_result {
        let range = m.range();
        let group_count = m.captures.len();
        let n = 1 + group_count;
        let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
        let arr =
            vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto), n, vm.epoch().bump()));
        unsafe {
            (*arr).set_prop_at(0, vm.new_string_units_owned(text.slice(range.start, range.end).into_owned()));
            // 捕获组：未参与匹配的组为 undefined。
            for i in 1..=group_count {
                match m.group(i) {
                    Some(g) => {
                        (*arr).set_prop_at(i, vm.new_string_units_owned(text.slice(g.start, g.end).into_owned()))
                    }
                    None => (*arr).set_prop_at(i, JsValue::undefined()),
                }
            }
            (*arr).set_prop_count(n);
        }

        let index_si = vm.kernel_core().perm_interner().intern("index").0;
        vm.set_or_create_prop_value(unsafe { &mut *arr }, index_si, JsValue::int(text.unit_pos(range.start) as i32));
        let input_val = haystack.to_value(vm);
        let input_si = vm.kernel_core().perm_interner().intern("input").0;
        vm.set_or_create_prop_value(unsafe { &mut *arr }, input_si, input_val);
        let groups_si = vm.kernel_core().perm_interner().intern("groups").0;
        vm.set_or_create_prop_value(unsafe { &mut *arr }, groups_si, JsValue::undefined());
        if is_global {
            set_prop_at(re_ptr, 0, JsValue::int(text.unit_pos(range.end) as i32));
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
    NativeResult::Ok(vm.new_string_owned(result))
}

// RegExp 对象 hash 槽中属性的固定下标（构造顺序：lastIndex/source/flags/global/...）。
const PROP_LAST_INDEX: usize = 0;
const PROP_SOURCE: usize = 1;
const PROP_FLAGS: usize = 2;
const PROP_GLOBAL: usize = 3;

/// 读 global 标志；槽值被 accessor 重定义后不再保证是 bool，非 bool 一律视为 false。
fn is_global(re: &JsObject) -> bool {
    match get_prop(re, PROP_GLOBAL) {
        v if v.is_bool() => v.as_bool(),
        _ => false,
    }
}

/// 取匹配文本参数，按 ToString 语义完整转换（对象经 toString/valueOf）；
/// 结果按载荷形态 owned：良形文本走 Str 臂，含孤立 surrogate 等走 Units 臂。
/// 对象 ToString 抛出的原生异常原样传播。
fn regexp_text_arg<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<OwnedText, JsValue> {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    match oxide_runtime_api::to_string_value_full(val, vm) {
        Ok(v) => {
            // SAFETY: v 为字符串值，借用即时消费。
            let sp = unsafe { &*v.as_string_ptr() };
            if sp.is_flat() {
                Ok(OwnedText::Str(sp.as_str().to_string()))
            } else {
                Ok(OwnedText::Units(sp.units().into_owned()))
            }
        }
        Err(_) => {
            // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播。
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert value to a string"))
        }
    }
}

/// 取 this 的已编译 regress 正则与匹配文本；this 非 RegExp 或缺失编译结果时返回 Err。
fn regexp_regex_and_text<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(&'static regress::Regex, OwnedText), JsValue> {
    let re_ptr = get_regexp_ptr(vm, args)?;
    let fn_ptr = {
        let re = unsafe { &*re_ptr };
        match re.native_fn() {
            None => return Err(crate::error::create_type_error(vm, "invalid RegExp")),
            Some(p) => p,
        }
    };
    // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
    let haystack = regexp_text_arg(vm, args)?;
    Ok((regex, haystack))
}

/// 替换参数转单元序列：字符串值按载荷形态原样借出，其余经完整 ToString；
/// Symbol 按规范抛 TypeError；对象 ToString 抛出的原生异常原样传播。
fn replacement_units<H: VmHost>(vm: &mut H, val: JsValue) -> Result<Vec<u16>, JsValue> {
    match oxide_runtime_api::to_string_value_full(val, vm) {
        Ok(v) => Ok(vm.string_units(v).into_owned()),
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"))
        }
    }
}

/// `RegExp.prototype[Symbol.match](string)`：global 正则收集全部匹配串，
/// 非 global 返回单个 exec 匹配数组；无匹配返回 null。
///
/// # 步骤
/// 1. 非 global：直接复用 exec 结果（数组含 index/input/groups）。
/// 2. global：lastIndex 归零后逐匹配收集完整匹配串，末次匹配后推进 lastIndex。
pub fn regexp_symbol_match<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let is_global = is_global(unsafe { &*re_ptr });
    if !is_global {
        return regexp_exec(vm, args);
    }

    let (regex, haystack) = match regexp_regex_and_text(vm, args) {
        Ok(pair) => pair,
        Err(err) => return NativeResult::Err(err),
    };
    set_prop_at(re_ptr, PROP_LAST_INDEX, JsValue::int(0));
    let text = haystack.as_match_text();
    let mut matches: Vec<Vec<u16>> = Vec::new();
    let mut last_end = 0usize;
    text.for_each_match(regex, |m| {
        let range = m.range();
        matches.push(text.slice(range.start, range.end).into_owned());
        last_end = text.unit_pos(range.end);
    });
    set_prop_at(re_ptr, PROP_LAST_INDEX, JsValue::int(last_end as i32));
    if matches.is_empty() {
        return NativeResult::Ok(JsValue::null());
    }
    NativeResult::Ok(make_units_array(vm, matches))
}

/// `RegExp.prototype[Symbol.replace](string, replacement)`：按匹配替换。
/// 字符串 replacement 展开 `$` 引用（`$$`/`$&`/`` $` ``/`$'`/`$n`），函数 replacement
/// 逐匹配调用；global 全替换，否则替换首个。
///
/// # 步骤
/// 1. global 时 lastIndex 归零，复用 string 模块的替换逻辑。
/// 2. 结束后把 lastIndex 推进到末次匹配末尾（无匹配保持 0）。
pub fn regexp_symbol_replace<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let is_global = is_global(unsafe { &*re_ptr });
    let (regex, haystack) = match regexp_regex_and_text(vm, args) {
        Ok(pair) => pair,
        Err(err) => return NativeResult::Err(err),
    };
    if is_global {
        set_prop_at(re_ptr, PROP_LAST_INDEX, JsValue::int(0));
    }

    // 替换文本统一走单元口径（$ 展开/回调参数/拼接全在单元序列上进行）。
    let text = haystack.as_match_text();
    let units = text.units();

    let result = if args.len() > 2 {
        let replacer_val = vm.reg(args[2]);
        if replacer_val.is_object() {
            let o = unsafe { &*replacer_val.as_js_object_ptr() };
            if o.is_function() {
                // 回调第 4 参（原字符串）预构一次：原始字符串参数直接复用零拷贝，
                // 对象参数复用已转换的 haystack 建单个会话串（原每匹配整串复制）。
                let text_val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
                let text_arg = if text_val.is_string() { text_val } else { haystack.to_value(vm) };
                regex_replace_fn(vm, regex, &units, replacer_val, is_global, text_arg)
            } else {
                let replacement = match replacement_units(vm, replacer_val) {
                    Ok(u) => u,
                    Err(err) => return NativeResult::Err(err),
                };
                NativeResult::Ok(vm.new_string_units_owned(regex_replace_manual_units(
                    regex,
                    &units,
                    &replacement,
                    is_global,
                )))
            }
        } else {
            let replacement = match replacement_units(vm, replacer_val) {
                Ok(u) => u,
                Err(err) => return NativeResult::Err(err),
            };
            NativeResult::Ok(vm.new_string_units_owned(regex_replace_manual_units(
                regex,
                &units,
                &replacement,
                is_global,
            )))
        }
    } else {
        NativeResult::Ok(vm.new_string_units_owned(regex_replace_manual_units(regex, &units, &[], is_global)))
    };

    if is_global {
        let mut last_end = 0usize;
        text.for_each_match(regex, |m| {
            last_end = text.unit_pos(m.range().end);
        });
        set_prop_at(re_ptr, PROP_LAST_INDEX, JsValue::int(last_end as i32));
    }
    result
}

/// `RegExp.prototype[Symbol.search](string)`：返回首个匹配位置（码元索引），
/// 无匹配返回 -1。global 标志不影响 search。
pub fn regexp_symbol_search<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (regex, haystack) = match regexp_regex_and_text(vm, args) {
        Ok(pair) => pair,
        Err(err) => return NativeResult::Err(err),
    };
    let text = haystack.as_match_text();
    if let Some(m) = text.find_from_units(regex, 0) {
        return NativeResult::Ok(JsValue::int(text.unit_pos(m.range().start) as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `RegExp.prototype[Symbol.split](string, limit)`：按匹配切分，捕获组作为
/// 分隔元素插入结果；limit 限制结果长度，缺省切分全部。
pub fn regexp_symbol_split<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (regex, haystack) = match regexp_regex_and_text(vm, args) {
        Ok(pair) => pair,
        Err(err) => return NativeResult::Err(err),
    };
    let limit = if args.len() > 2 {
        let l = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if l.is_infinite() {
            u32::MAX as usize
        } else {
            (l.max(0.0).trunc() as u64).min(u32::MAX as u64) as usize
        }
    } else {
        u32::MAX as usize
    };

    // 片段切片取臂原生命径（Str 臂字节、Units 臂码元）；limit 触顶后提前停推
    // 但保持游标语义一致（尾段仅在未触顶时追加）。
    let text = haystack.as_match_text();
    let mut parts: Vec<Vec<u16>> = Vec::new();
    let mut last_end = 0usize;
    text.for_each_match(regex, |m| {
        if parts.len() >= limit {
            return;
        }
        let range = m.range();
        parts.push(text.slice(last_end, range.start).into_owned());
        if parts.len() >= limit {
            return;
        }
        for i in 1..=m.captures.len() {
            if parts.len() >= limit {
                break;
            }
            match m.group(i) {
                Some(g) => parts.push(text.slice(g.start, g.end).into_owned()),
                None => parts.push(Vec::new()),
            }
        }
        last_end = range.end;
    });
    if parts.len() < limit {
        parts.push(text.slice(last_end, text.raw_len()).into_owned());
    }
    NativeResult::Ok(make_units_array(vm, parts))
}

/// `RegExp.prototype[Symbol.matchAll](string)`：返回按 global 语义逐个产出
/// 匹配数组的迭代器。global 正则直接复用（初始下标取 lastIndex）；
/// 非 global 正则复制一份加 `g` 标志后迭代。
pub fn regexp_symbol_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let re = unsafe { &*re_ptr };
    let is_global = is_global(re);
    let last_index = vm.coerce_number_bounded(get_prop(re, PROP_LAST_INDEX)).unwrap_or(f64::NAN) as usize;
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };

    // 迭代目标：global 用原正则，非 global 复制并补 g 标志。
    let iter_re = if is_global {
        vm.reg(args[0])
    } else {
        let source = {
            let val = get_prop(re, PROP_SOURCE);
            vm.lookup_str(val).unwrap_or_default()
        };
        let mut flags = {
            let val = get_prop(re, PROP_FLAGS);
            vm.lookup_str(val).unwrap_or_default()
        };
        if !flags.contains('g') {
            flags.push('g');
        }
        let compiled = match regress::Regex::with_flags(&source, flags.as_str()) {
            Ok(rx) => rx,
            Err(e) => {
                return NativeResult::Err(crate::error::create_syntax_error(
                    vm,
                    &format!("Invalid regular expression: {e}"),
                ));
            }
        };
        let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
        let mut stub = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
        // 载体专型标签：native_fn 槽的 Box 经 RegExp 同一守卫释放/深拷贝，
        // 避免每次 matchAll 泄漏一个已编译正则。
        stub.type_tag = JsObject::OBJ_TYPE_REGEX_STUB;
        let raw = Box::into_raw(Box::new(compiled)) as *const u8;
        stub.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(raw as *const ()) }));
        JsValue::from_js_object(vm.alloc_object(stub))
    };

    // 复用 String.prototype.matchAll 的迭代器包装（next 经 string_match_all_next 推进，
    // 挂 %RegExpStringIteratorPrototype% 原型，不设实例 own next）。
    let regexp_iter_proto = vm.session().builtin_world().regexp_string_iterator_proto.as_ptr() as *mut JsObject;
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(regexp_iter_proto)));
    let wrapper_obj = unsafe { &mut *wrapper };
    let input_si = vm.kernel_core().perm_interner().intern(crate::string::MALL_INPUT).0;
    let index_si = vm.kernel_core().perm_interner().intern(crate::string::MALL_INDEX).0;
    let re_si = vm.kernel_core().perm_interner().intern(crate::string::MALL_RE).0;
    // input 属性存完整转换后的字符串值（单元保真）；index 游标为码元口径，
    // next 按 input 载荷形态各自消费。
    let input_val = haystack.to_value(vm);
    vm.set_or_create_prop_value(wrapper_obj, input_si, input_val);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(last_index as i32));
    vm.set_or_create_prop_value(wrapper_obj, re_si, iter_re);
    NativeResult::Ok(JsValue::from_js_object(wrapper))
}
