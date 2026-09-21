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

/// 取 this 为对象：入口门禁只要求 IsObject（规范对 exec 的 this 无内部槽要求，
/// 编译正则槽的可用性在调用方另行判定）；非对象（null/undefined/原始值）抛
/// TypeError。
///
/// 与 `get_regexp_ptr`（强校验 is_regexp_obj）并存：exec 走本放宽门禁，
/// test/toString 等入口保持强校验。
fn get_this_obj<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<*mut JsObject, JsValue> {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "RegExp.prototype method called on non-object"));
    }
    let ptr = this_val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "null object"));
    }
    Ok(ptr)
}

/// ToLength(Get) 读值的收尾口径：完整对象强转（对象经 ToPrimitive 触发
/// valueOf/toString，转换异常传播）后按 ToLength 收敛到 [0, 2^53-1]。
fn to_length_value<H: VmHost>(vm: &mut H, val: JsValue) -> Result<usize, JsValue> {
    let n = match oxide_runtime_api::to_number_full(val, vm) {
        Ok(n) => n,
        Err(e) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_from_text(vm, &e));
        }
    };
    let len = if n.is_nan() || n <= 0.0 { 0.0 } else { n.min(9_007_199_254_740_991.0) };
    Ok(len.trunc() as usize)
}

/// 以 Set 语义（strict）写 lastIndex：非可写属性抛 TypeError。
///
/// # 边界与前提
/// - 写入失败返回格式化错误文本，由调用方经 `create_from_text` 恢复错误种类。
fn set_last_index<H: VmHost>(
    vm: &mut H, re_ptr: *mut JsObject, this_val: JsValue, index: usize,
) -> Result<(), JsValue> {
    let li_si = vm.kernel_core().perm_interner().intern("lastIndex").0;
    match vm.ordinary_set(unsafe { &mut *re_ptr }, li_si, JsValue::int(index as i32), this_val, true) {
        Ok(()) => Ok(()),
        Err(err) => Err(crate::error::create_from_text(vm, &err)),
    }
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
    // [[Source]]/[[Flags]] 是 ToString 结果的**单元序列**：`to_units_full` 直取
    // （lossy `to_string` 会把孤立 surrogate 单元折成 FFFD，破坏 source 往返）。
    // regress 编译用 lossy 文本（其匹配语义对孤立 surrogate 本即既有缺口，
    // 不因本处改变）。
    let pattern_units = if args.len() < 2 {
        Vec::<u16>::new()
    } else {
        match oxide_runtime_api::to_units_full(vm.reg(args[1]), vm) {
            Ok(u) => u,
            Err(e) => return NativeResult::Err(crate::error::create_from_text(vm, &e)),
        }
    };
    let flags_units = if args.len() < 3 {
        Vec::<u16>::new()
    } else {
        match oxide_runtime_api::to_units_full(vm.reg(args[2]), vm) {
            Ok(u) => u,
            Err(e) => return NativeResult::Err(crate::error::create_from_text(vm, &e)),
        }
    };
    let pattern = String::from_utf16_lossy(&pattern_units);
    let flags = String::from_utf16_lossy(&flags_units);

    // u 与 v 互斥（regress 不校验此约束）：编译前置抛 SyntaxError。
    if flags.contains('u') && flags.contains('v') {
        return NativeResult::Err(crate::error::create_syntax_error(
            vm,
            "Invalid regular expression flags: 'u' and 'v' are mutually exclusive",
        ));
    }

    let mut obj = JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject),
    );

    // regress 用 JS flag 字符串编译：g/i/m/s/y/u/v 原样透传（vendored fork
    // 原生识别 v 标志，u/v 互斥已在上方前置校验）。
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
    // 单元口径物化 source/flags（孤立 surrogate 单元保持往返）。7 个 flag 布尔
    // 不再作实例数据属性：读侧走原型只读访问器，以 flags 串为唯一数据源。
    set_prop(&mut obj, "source", vm.new_string_units_owned(pattern_units), vm);
    set_prop(&mut obj, "flags", vm.new_string_units_owned(flags_units), vm);
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
    let is_global = regexp_has_flag(vm, re, 'g');

    // lastIndex 为码元口径；Str 臂的内部字节换算在 find_from_units 内完成。
    let text = haystack.as_match_text();
    let found = if is_global {
        text.find_from_units(regex, last_index).is_some()
    } else {
        text.find_from_units(regex, 0).is_some()
    };
    NativeResult::Ok(JsValue::bool(found))
}

/// `RegExp.prototype.exec(string)`：执行匹配并返回数组（含捕获组、index、input）；
/// 无匹配返回 null。
///
/// # 步骤
/// 1. this 仅需对象（IsObject 门禁）；编译正则槽必须存在，非 RegExp 对象无
///    可匹配模式，同抛 TypeError。
/// 2. S = ToString(string)；flags 串判 global/sticky（sticky 时 global 归 false
///    的效果体现为两者只影响同一分支）。
/// 3. lastIndex = ToLength(Get)：完整属性解析（accessor 与原型链均生效），
///    读异常传播原异常。
/// 4. 搜索起点：global/sticky 取 lastIndex，其余 0；起点越出串长时先 Set 0
///    （仅 global/sticky），再回 null（规范短路）。
/// 5. 匹配；sticky 加命中后置锚定过滤：匹配起点须恰在 lastIndex
///    （底层引擎对 y 不原生锚定，过滤为单点）。
/// 6. lastIndex 写回走 Set 语义且仅 global/sticky：失败置 0、成功置匹配末尾；
///    非可写属性抛 TypeError。
///
/// # 边界与前提
/// - lastIndex 负值/NaN/非数字经 ToLength 收敛为 0；值以码元口径计。
/// - 非对象 this 抛 TypeError（门禁），对象但无编译正则同样抛 TypeError。
///
/// # 副作用
/// - global/sticky 时按匹配结果 Set this.lastIndex。
/// - 命中时分配结果数组。
pub fn regexp_exec<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_this_obj(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = vm.reg(args[0]);

    // 编译正则槽：非 RegExp 对象无可匹配模式。
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
    let text = haystack.as_match_text();

    // 标志决定搜索起点与 lastIndex 写回条件。
    let (is_global, is_sticky) = {
        let re = unsafe { &*re_ptr };
        (regexp_has_flag(vm, re, 'g'), regexp_has_flag(vm, re, 'y'))
    };
    let tracks_last_index = is_global || is_sticky;

    // lastIndex 读走 Get + ToLength：完整属性解析，读异常传播原异常。
    let last_index = {
        let li_si = vm.kernel_core().perm_interner().intern("lastIndex").0;
        match vm.ordinary_get(unsafe { &*re_ptr }, li_si, this_val) {
            Ok(val) => match to_length_value(vm, val) {
                Ok(n) => n,
                Err(err) => return NativeResult::Err(err),
            },
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot read lastIndex"));
            }
        }
    };

    // 搜索起点：global/sticky 自 lastIndex 起，其余自 0。
    let start = if tracks_last_index { last_index } else { 0 };

    // 越界短路：lastIndex 超出串长必不匹配，规范先 Set 0 再空结果。
    if start > text.len_units() {
        if tracks_last_index {
            if let Err(err) = set_last_index(vm, re_ptr, this_val, 0) {
                return NativeResult::Err(err);
            }
        }
        return NativeResult::Ok(JsValue::null());
    }

    // 匹配范围取臂原生命径（Str 臂字节、Units 臂码元）；sticky 加后置锚定
    // 过滤——底层引擎对 y 不原生锚定，命中起点须恰在 lastIndex 才算有效。
    let mut match_result = text.find_from_units(regex, start);
    if is_sticky {
        if let Some(m) = &match_result {
            if text.unit_pos(m.range().start) != last_index {
                match_result = None;
            }
        }
    }

    let Some(m) = match_result else {
        // 失败：仅 global/sticky Set 0。
        if tracks_last_index {
            if let Err(err) = set_last_index(vm, re_ptr, this_val, 0) {
                return NativeResult::Err(err);
            }
        }
        return NativeResult::Ok(JsValue::null());
    };

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
                Some(g) => (*arr).set_prop_at(i, vm.new_string_units_owned(text.slice(g.start, g.end).into_owned())),
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

    // 成功：lastIndex 推进到匹配末尾（仅 global/sticky），Set 语义。
    if tracks_last_index {
        if let Err(err) = set_last_index(vm, re_ptr, this_val, text.unit_pos(range.end)) {
            return NativeResult::Err(err);
        }
    }

    NativeResult::Ok(JsValue::from_js_object(arr))
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

/// `RegExp.escape(string)`：返回语法字符、其它标点、空白/行终止符、孤立
/// surrogate、首字符数字/ASCII 字母均被转义的新字符串；参数非字符串抛 TypeError。
///
/// # 边界与前提
/// - 参数取 args[1]（args[0] 为 this，纯字符串变换不使用）；缺参按非字符串抛 TypeError。
/// - 变换按码点逐个进行（代理对是单码点，非 BMP 码点原样回编码）：控制字符
///   （0x09–0x0D）走单字母转义（\t \n \v \f \r）；语法字符反斜杠加字符本身；
///   其它标点 \xNN；空白/行终止符与孤立 surrogate ≤0xFF 用 \xNN、否则 \uNNNN
///   （十六进制小写）；仅首位码点为数字/ASCII 字母 \xNN；其余原样。
pub fn regexp_escape<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_string() {
        return NativeResult::Err(crate::error::create_type_error(vm, "RegExp.escape expects a string argument"));
    }

    let units = vm.string_units(val).into_owned();
    let mut out: Vec<u16> = Vec::with_capacity(units.len() + 4);
    let mut at_start = true;
    let mut i = 0;
    while i < units.len() {
        let (cp, width) = decode_code_point(&units, i);
        escape_code_point(&mut out, cp, at_start);
        at_start = false;
        i += width;
    }
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// 从单元序列下标处解出一个码点，返回（码点值，消耗单元数）；
/// 高 surrogate 后紧跟低 surrogate 时代理对合为单码点，孤立 surrogate 自成码点。
fn decode_code_point(units: &[u16], i: usize) -> (u32, usize) {
    let cu = units[i];
    if (0xD800..=0xDBFF).contains(&cu) && i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
        let cp = 0x10000 + (((cu - 0xD800) as u32) << 10) + (units[i + 1] as u32 - 0xDC00_u32);
        (cp, 2)
    } else {
        (cu as u32, 1)
    }
}

/// 单个码点的转义变换，结果追加到 out；at_start 仅约束首字符数字/字母规则。
fn escape_code_point(out: &mut Vec<u16>, cp: u32, at_start: bool) {
    if cp > 0xFFFF {
        // 非 BMP 码点原样回编码（代理对不按孤立 surrogate 转义）。
        out.push(0xD800 + ((cp - 0x10000) >> 10) as u16);
        out.push(0xDC00 + ((cp - 0x10000) & 0x3FF) as u16);
        return;
    }
    let cu = cp as u16;
    match cu {
        // 控制字符单字母转义形态。
        0x09 => push_escape_literal(out, b't'),
        0x0A => push_escape_literal(out, b'n'),
        0x0B => push_escape_literal(out, b'v'),
        0x0C => push_escape_literal(out, b'f'),
        0x0D => push_escape_literal(out, b'r'),
        // 语法字符：反斜杠加字符本身。
        0x24 | 0x28 | 0x29 | 0x2A | 0x2B | 0x2E | 0x2F | 0x3F | 0x5C | 0x5E | 0x5B | 0x5D | 0x7B | 0x7C | 0x7D => {
            out.push(0x5C);
            out.push(cu);
        }
        // 其它标点：\xNN。
        0x21 | 0x22 | 0x23 | 0x25 | 0x26 | 0x27 | 0x2C | 0x2D | 0x3A | 0x3B | 0x3C | 0x3D | 0x3E | 0x40 | 0x60
        | 0x7E => {
            push_hex_escape(out, b'x', cu, 2);
        }
        // 空白/行终止符与孤立 surrogate：\xNN（≤0xFF）或 \uNNNN。
        c if is_ws_or_line_terminator(c) || (0xD800..=0xDFFF).contains(&c) => {
            if c <= 0xFF {
                push_hex_escape(out, b'x', c, 2);
            } else {
                push_hex_escape(out, b'u', c, 4);
            }
        }
        // 仅首字符的数字/ASCII 字母：\xNN。
        c if at_start && ((0x30..=0x39).contains(&c) || (0x41..=0x5A).contains(&c) || (0x61..=0x7A).contains(&c)) => {
            push_hex_escape(out, b'x', c, 2)
        }
        _ => out.push(cu),
    }
}

/// 空白/行终止符单元全集（WhiteSpace 文法 + LineTerminator）；
/// 0x09–0x0D 已被 ControlEscape 分支先行拦截，不列于此。
fn is_ws_or_line_terminator(cu: u16) -> bool {
    matches!(
        cu,
        0x20 | 0xA0 | 0xFEFF | 0x1680 | 0x1681 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000
    )
}

/// 追加反斜杠加转义字母加字符本身（控制字符单字母转义形态）。
fn push_escape_literal(out: &mut Vec<u16>, letter: u8) {
    out.push(0x5C);
    out.push(letter as u16);
}

/// 追加反斜杠加转义字母加小写十六进制数字（digit_count = 2 为 \xNN，4 为 \uNNNN）。
fn push_hex_escape(out: &mut Vec<u16>, letter: u8, cu: u16, digit_count: u32) {
    out.push(0x5C);
    out.push(letter as u16);
    for shift in (0..digit_count).rev() {
        out.push(HEX_DIGITS[(cu >> (shift * 4)) as usize & 0xF]);
    }
}

const HEX_DIGITS: [u16; 16] = [
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66,
];

// RegExp 对象 hash 槽中属性的固定下标（构造顺序：lastIndex/source/flags）。
const PROP_LAST_INDEX: usize = 0;
const PROP_SOURCE: usize = 1;
const PROP_FLAGS: usize = 2;

/// 读实例 flags 串（[[OriginalFlags]] 的物化形态）判单个 flag 码元。
///
/// # 边界与前提
/// - `re` 须为持有编译正则的对象（RegExp 实例或 matchAll 载体）；flags 缺失
///   时各 flag 一律 false。
/// - 经自身+原型链解析 flags 键：flags 是实例数据属性，原型链回退仅覆盖
///   用户删改自身属性后的读法。
pub(crate) fn regexp_has_flag<H: VmHost>(vm: &H, re: &JsObject, unit: char) -> bool {
    let flags_si = vm.kernel_core().perm_interner().intern("flags").0;
    match vm.resolve_property(re, flags_si) {
        Some(val) => vm.lookup_str(val).unwrap_or_default().contains(unit),
        None => false,
    }
}

/// 8 个单 flag 只读访问器的公共核：this 为 RegExp.prototype 本身返回
/// undefined；this 非 RegExp 对象抛 TypeError；其余读实例 flags 串判码元。
fn regexp_flag_of<H: VmHost>(vm: &mut H, args: &[u8], unit: char) -> NativeResult {
    // proto 恒等判定先于 RegExp 校验：规范对 prototype 本身返回 undefined
    // 而非抛错，顺序不可互换。
    let this_val = vm.reg(args[0]);
    if this_val.is_object() {
        let this_ptr = this_val.as_js_object_ptr();
        let proto_ptr = vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject;
        if !this_ptr.is_null() && this_ptr == proto_ptr {
            return NativeResult::Ok(JsValue::undefined());
        }
    }
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let re = unsafe { &*re_ptr };
    NativeResult::Ok(JsValue::bool(regexp_has_flag(vm, re, unit)))
}

/// `RegExp.prototype.global` getter。
pub fn regexp_get_global<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'g')
}

/// `RegExp.prototype.ignoreCase` getter。
pub fn regexp_get_ignore_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'i')
}

/// `RegExp.prototype.multiline` getter。
pub fn regexp_get_multiline<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'm')
}

/// `RegExp.prototype.dotAll` getter。
pub fn regexp_get_dot_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 's')
}

/// `RegExp.prototype.sticky` getter。
pub fn regexp_get_sticky<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'y')
}

/// `RegExp.prototype.unicode` getter。
pub fn regexp_get_unicode<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'u')
}

/// `RegExp.prototype.hasIndices` getter。
pub fn regexp_get_has_indices<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'd')
}

/// `RegExp.prototype.unicodeSets` getter。
pub fn regexp_get_unicode_sets<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    regexp_flag_of(vm, args, 'v')
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
    let is_global = regexp_has_flag(vm, unsafe { &*re_ptr }, 'g');
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
    let is_global = regexp_has_flag(vm, unsafe { &*re_ptr }, 'g');
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
    let is_global = regexp_has_flag(vm, re, 'g');
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
