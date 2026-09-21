use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::string::{make_units_array, MatchText, OwnedText};

/// 构建 RegExp match result 的 `groups` 对象：键为命名捕获组名，值为匹配字符串或 undefined。
pub(crate) fn build_groups_object<H: VmHost>(vm: &mut H, m: &regress::Match, text: &MatchText) -> JsValue {
    // 收集所有命名组（迭代器惰性，先 collect 判断是否有命名组）。
    let named: Vec<_> = m.named_groups().collect();
    if named.is_empty() {
        return JsValue::undefined();
    }
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let groups_obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let groups_ptr = unsafe { &mut *groups_obj };

    // 遍历所有命名捕获组，按名称设置属性（重复名称按规范取最后一次匹配）。
    for (name, range) in named {
        let name_si = vm.kernel_core().perm_interner().intern(name).0;
        let value = match range {
            Some(r) => vm.new_string_units_owned(text.slice(r.start, r.end).into_owned()),
            None => JsValue::undefined(),
        };
        vm.set_or_create_prop_value(groups_ptr, name_si, value);
    }

    JsValue::from_js_object(groups_obj)
}

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
    // setter 抛错优先恢复 uncaught 原异常值（与 rx_set_prop 同口径）。
    match vm.ordinary_set(unsafe { &mut *re_ptr }, li_si, JsValue::int(index as i32), this_val, true) {
        Ok(()) => Ok(()),
        Err(err) => Err(crate::iterator::engine_error(vm, &err)),
    }
}

/// 以 Get 语义按名读属性：完整属性解析（自身数据/自身 accessor/原型链），
/// getter 抛错时恢复原异常值。
fn rx_get_prop<H: VmHost>(vm: &mut H, obj: *mut JsObject, name: &str, this_val: JsValue) -> Result<JsValue, JsValue> {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    match vm.ordinary_get(unsafe { &*obj }, si, this_val) {
        Ok(v) => Ok(v),
        Err(_) => Err(crate::iterator::engine_error(vm, "cannot read property")),
    }
}

/// 以 Get 语义读结果数组的数值下标属性：下标经规范整数键换算（与字符串
/// "0" 形态同键），读异常恢复原异常值。
fn rx_get_index<H: VmHost>(vm: &mut H, obj: *mut JsObject, idx: i32, receiver: JsValue) -> Result<JsValue, JsValue> {
    let si = vm.property_key_si(JsValue::int(idx));
    match vm.ordinary_get(unsafe { &*obj }, si, receiver) {
        Ok(v) => Ok(v),
        Err(_) => Err(crate::iterator::engine_error(vm, "cannot read result index")),
    }
}

/// 以 Set 语义（strict）按名写属性：写失败（非可写等）恢复原异常值。
fn rx_set_prop<H: VmHost>(
    vm: &mut H, obj: *mut JsObject, name: &str, val: JsValue, this_val: JsValue,
) -> Result<(), JsValue> {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    // setter 抛错时原异常值留在 uncaught 槽（优先恢复）；写保护失败无
    // uncaught，经格式化文本的 "TypeError: " 前缀恢复种类。
    match vm.ordinary_set(unsafe { &mut *obj }, si, val, this_val, true) {
        Ok(()) => Ok(()),
        Err(err) => Err(crate::iterator::engine_error(vm, &err)),
    }
}

/// live 语义读 flags 串：ToString(Get(rx, "flags"))，getter 抛错与
/// ToString 抛错均传播原异常。
pub fn rx_get_flags<H: VmHost>(vm: &mut H, rx: *mut JsObject, this_val: JsValue) -> Result<String, JsValue> {
    let val = rx_get_prop(vm, rx, "flags", this_val)?;
    // 回退文案用原始文本（"TypeError: " 前缀恢复错误种类）。
    let s = match oxide_runtime_api::to_string_value_full(val, vm) {
        Ok(v) => v,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    Ok(String::from_utf16_lossy(&vm.string_units(s)))
}

/// live 语义读布尔属性：ToBoolean(Get(rx, name))，getter 抛错传播原异常。
fn rx_get_bool_prop<H: VmHost>(vm: &mut H, rx: *mut JsObject, name: &str, this_val: JsValue) -> Result<bool, JsValue> {
    Ok(oxide_runtime_api::to_boolean(rx_get_prop(vm, rx, name, this_val)?))
}

/// 共享 RegExpExec(R, S)：Get(R, "exec") 须可调用，Call(exec, R, «S»)，
/// 结果非 null 非对象抛 TypeError。exec 自身的抛错恢复原异常值。
fn regexp_exec_call<H: VmHost>(
    vm: &mut H, rx: *mut JsObject, rx_val: JsValue, s_val: JsValue,
) -> Result<JsValue, JsValue> {
    let exec_val = rx_get_prop(vm, rx, "exec", rx_val)?;
    if !crate::iterator::is_callable(exec_val) {
        return Err(crate::error::create_type_error(vm, "RegExp exec is not callable"));
    }
    let s_arg = s_val;
    let result = match vm.call_function_sync(exec_val, rx_val, &[s_arg]) {
        Ok(r) => r,
        Err(_) => return Err(crate::iterator::engine_error(vm, "RegExp exec call failed")),
    };
    if !(result.is_null() || result.is_object()) {
        return Err(crate::error::create_type_error(vm, "RegExp exec result must be an object or null"));
    }
    Ok(result)
}

/// SpeciesConstructor(R, default)：constructor 为 undefined 回 default、
/// 非对象抛 TypeError；@@species 为 undefined/null 回 default、非构造器抛
/// TypeError。属性读抛错恢复原异常值。
fn species_constructor<H: VmHost>(
    vm: &mut H, r: *mut JsObject, r_val: JsValue, default: JsValue,
) -> Result<JsValue, JsValue> {
    let c = rx_get_prop(vm, r, "constructor", r_val)?;
    if c.is_undefined() {
        return Ok(default);
    }
    if !c.is_object() {
        return Err(crate::error::create_type_error(vm, "RegExp species constructor must be an object"));
    }
    let species_key = oxide_types::private_key::make_well_known_symbol_key(10);
    let s = match vm.ordinary_get(unsafe { &*c.as_js_object_ptr() }, species_key, r_val) {
        Ok(v) => v,
        Err(_) => return Err(crate::iterator::engine_error(vm, "cannot read @@species")),
    };
    if s.is_undefined() || s.is_null() {
        return Ok(default);
    }
    if !crate::array::is_constructor_value(s) {
        return Err(crate::error::create_type_error(vm, "RegExp @@species is not a constructor"));
    }
    Ok(s)
}

/// AdvanceStringIndex：零宽匹配后的推进——无条件前移，unicode 口径下起点对
/// 代理对跨两个单元，其余跨一个码元（越出串长同样前移，与实现语义一致）。
fn advance_string_index(units: &[u16], p: usize, unicode: bool) -> usize {
    if unicode
        && p < units.len()
        && (0xD800..=0xDBFF).contains(&units[p])
        && p + 1 < units.len()
        && (0xDC00..=0xDFFF).contains(&units[p + 1])
    {
        p + 2
    } else {
        p + 1
    }
}

/// GetSubstitution：非函数替换串的 `$` 引用展开（`$$`/`$&`/`` $` ``/`$'`/`$n`/`$<name>`）；
/// `$n` 越界（含 $0 与两位回退后仍越界）或未匹配组（None）按字面输出。
/// `$<name>` 从 named_captures 对象读取；不存在时字面输出 `$<` 开头部分。
fn get_substitution_units<H: VmHost>(
    vm: &mut H,
    matched: &[u16],
    s: &[u16],
    position: usize,
    captured: &[Option<Vec<u16>>],
    named_captures: Option<&JsObject>,
    replacement: &[u16],
) -> Vec<u16> {
    let mut out = Vec::new();
    let pos = position.min(s.len());
    let tail = (pos + matched.len()).min(s.len());
    let mut i = 0;
    while i < replacement.len() {
        if replacement[i] != 0x24 {
            out.push(replacement[i]);
            i += 1;
            continue;
        }
        let rest = &replacement[i..];
        if rest.starts_with(&[0x24, 0x24]) {
            out.push(0x24);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x26]) {
            out.extend_from_slice(matched);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x60]) {
            out.extend_from_slice(&s[..pos]);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x27]) {
            out.extend_from_slice(&s[tail..]);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x3c]) {
            // $<name> 命名组引用：扫描到 > 为止。
            if let Some(end) = rest[2..].iter().position(|&c| c == 0x3e) {
                let name_units = &rest[2..2 + end];
                let name_str = String::from_utf16_lossy(name_units);
                if let Some(obj) = named_captures {
                    let si = vm.kernel_core().perm_interner().intern(&name_str).0;
                    if si != 0 {
                        let key = JsValue::undefined();
                        if let Ok(cap) = vm.ordinary_get(obj, si, key) {
                            if !cap.is_undefined() {
                                if let Ok(cap_str) = to_string_units(vm, cap) {
                                    out.extend_from_slice(&cap_str);
                                }
                            }
                            // 组存在（含 undefined）：按规范替换为空串或捕获值，跳过 `$<name>`。
                            i += 2 + end + 1;
                            continue;
                        }
                    }
                }
                // 组不存在或无法读取：字面输出 `$<` 到 `>` 的部分。
                out.extend_from_slice(&rest[..end + 3]);
                i += end + 3;
            } else {
                // 没有闭合的 >：字面输出 `$<`。
                out.push(0x24);
                out.push(0x3c);
                i += 2;
            }
        } else if i + 1 < replacement.len() && (0x30..=0x39).contains(&replacement[i + 1]) {
            let digit_count = if i + 2 < replacement.len() && (0x30..=0x39).contains(&replacement[i + 2]) {
                2
            } else {
                1
            };
            let d1 = (replacement[i + 1] - 0x30) as usize;
            let mut index = if digit_count == 2 {
                d1 * 10 + (replacement[i + 2] - 0x30) as usize
            } else {
                d1
            };
            let mut ref_len = 1 + digit_count;
            if index > captured.len() && digit_count == 2 {
                index = d1;
                ref_len = 2;
            }
            if index == 0 {
                // $0 非替换符号，字面输出（且避开 0-1 下溢）。
                out.extend_from_slice(&replacement[i..i + ref_len]);
            } else if let Some(cap) = captured.get(index - 1) {
                // 未匹配组（None 槽）按空串替换，仅越界才字面回退。
                if let Some(u) = cap {
                    out.extend_from_slice(u);
                }
            } else {
                out.extend_from_slice(&replacement[i..i + ref_len]);
            }
            i += ref_len;
        } else {
            out.push(0x24);
            i += 1;
        }
    }
    out
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

/// `RegExp(pattern, flags)` 构造逻辑：用 regress 引擎编译模式（ECMAScript 语法，
/// 支持 backreference/lookaround/命名组/v-flag）；非法模式抛 SyntaxError。
/// 编译结果存于对象的 native_fn 槽。
pub fn regexp_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    // [[Source]]/[[Flags]] 是 ToString 结果的**单元序列**：`to_units_full` 直取
    // （lossy `to_string` 会把孤立 surrogate 单元折成 FFFD，破坏 source 往返）。
    // regress 编译用 lossy 文本（其匹配语义对孤立 surrogate 本即既有缺口，
    // 不因本处改变）。
    // 模式为已编译 RegExp 实例时 source/flags 从其实例属性 live 读取（flags
    // 参数缺省取实例 flags）；其余形态按 ToString 单元序列。
    let (pattern_units, flags_units) = if args.len() >= 2 {
        let pattern_val = vm.reg(args[1]);
        let re_ptr = pattern_val.as_js_object_ptr();
        if pattern_val.is_object() && !re_ptr.is_null() && unsafe { &*re_ptr }.is_regexp_obj() {
            let source_val = match rx_get_prop(vm, re_ptr, "source", pattern_val) {
                Ok(v) => v,
                Err(e) => return NativeResult::Err(e),
            };
            let source = match oxide_runtime_api::to_string_value_full(source_val, vm) {
                Ok(v) => v,
                Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
            };
            let flags_val = if args.len() >= 3 {
                vm.reg(args[2])
            } else {
                match rx_get_prop(vm, re_ptr, "flags", pattern_val) {
                    Ok(v) => v,
                    Err(e) => return NativeResult::Err(e),
                }
            };
            let flags = match oxide_runtime_api::to_string_value_full(flags_val, vm) {
                Ok(v) => v,
                Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
            };
            (vm.string_units(source).into_owned(), vm.string_units(flags).into_owned())
        } else {
            let p = match oxide_runtime_api::to_units_full(pattern_val, vm) {
                Ok(u) => u,
                // ToPrimitive 调 toString 抛错时原异常值留在 uncaught 槽，优先恢复。
                Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
            };
            let f = if args.len() >= 3 {
                match oxide_runtime_api::to_units_full(vm.reg(args[2]), vm) {
                    Ok(u) => u,
                    Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
                }
            } else {
                Vec::<u16>::new()
            };
            (p, f)
        }
    } else {
        (Vec::<u16>::new(), Vec::<u16>::new())
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
    obj.type_tag = JsObject::OBJ_TYPE_REGEXP;
    // source/flags 存实例字段：原型访问器从本字段读取，不作为自身属性枚举。
    obj.set_regexp_source(vm.new_string_units_owned(pattern_units));
    obj.set_regexp_flags(vm.new_string_units_owned(flags_units));

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
    let groups_val = build_groups_object(vm, &m, &text);
    let groups_si = vm.kernel_core().perm_interner().intern("groups").0;
    vm.set_or_create_prop_value(unsafe { &mut *arr }, groups_si, groups_val);

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
    let source = vm.lookup_str(re.get_regexp_source()).unwrap_or_default();
    let flags = vm.lookup_str(re.get_regexp_flags()).unwrap_or_default();
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

/// 读实例 flags 串（[[OriginalFlags]] 的物化形态）判单个 flag 码元。
///
/// # 边界与前提
/// - `re` 须为持有编译正则的对象（RegExp 实例或 matchAll 载体）；flags 缺失
///   时各 flag 一律 false。
/// - 直接读实例 regexp_flags 字段（避免通过 ordinary_get 触发原型访问器，
///   防止在 replace 等 builtin 调用期间触发 GC 导致悬垂）。
pub(crate) fn regexp_has_flag<H: VmHost>(vm: &mut H, re: &JsObject, unit: char) -> bool {
    vm.lookup_str(re.get_regexp_flags()).unwrap_or_default().contains(unit)
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

/// `RegExp.prototype.flags` getter：返回实例 flags 串。
pub fn regexp_get_flags<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    // proto 恒等判定：规范对 prototype 本身返回 undefined。
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
    NativeResult::Ok(re.get_regexp_flags())
}

/// `RegExp.prototype.source` getter：返回实例 source 串。
pub fn regexp_get_source<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    // proto 恒等判定：规范对 prototype 本身返回 "(?:)"（无 [[OriginalFlags]] 槽时）。
    if this_val.is_object() {
        let this_ptr = this_val.as_js_object_ptr();
        let proto_ptr = vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject;
        if !this_ptr.is_null() && this_ptr == proto_ptr {
            return NativeResult::Ok(vm.new_string_units_owned(vec!['(' as u16, '?' as u16, ':' as u16, ')' as u16]));
        }
    }
    let re_ptr = match get_regexp_ptr(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let re = unsafe { &*re_ptr };
    let source = re.get_regexp_source();
    let flags = re.get_regexp_flags();
    let source_str = vm.lookup_str(source).unwrap_or_default();
    let flags_str = vm.lookup_str(flags).unwrap_or_default();
    NativeResult::Ok(vm.new_string_units_owned(escape_regexp_pattern(&source_str, &flags_str)))
}

/// `EscapeRegExpPattern(pattern, flags)`：按规范转义 `\`、`/`（非 u/v 旗时）
/// 与行终止符，使结果可安全嵌入 `/pattern/flags` 字面量。
fn escape_regexp_pattern(pattern: &str, flags: &str) -> Vec<u16> {
    let has_uv = flags.contains('u') || flags.contains('v');
    let mut out: Vec<u16> = Vec::with_capacity(pattern.len() + 4);
    for ch in pattern.chars() {
        match ch {
            '/' | '\\' if !has_uv => {
                out.push('\\' as u16);
                out.push(ch as u16);
            }
            '\n' => {
                out.push('\\' as u16);
                out.push('n' as u16);
            }
            '\r' => {
                out.push('\\' as u16);
                out.push('r' as u16);
            }
            '\u{2028}' => {
                out.push('\\' as u16);
                out.push('u' as u16);
                out.push('2' as u16);
                out.push('0' as u16);
                out.push('2' as u16);
                out.push('8' as u16);
            }
            '\u{2029}' => {
                out.push('\\' as u16);
                out.push('u' as u16);
                out.push('2' as u16);
                out.push('0' as u16);
                out.push('2' as u16);
                out.push('9' as u16);
            }
            _ => {
                out.push(ch as u16);
            }
        }
    }
    out
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

/// `RegExp.prototype[Symbol.match](string)`：global 正则收集全部匹配串成数组，
/// 非 global 直接返回 exec 结果；无匹配返回 null。
///
/// # 步骤
/// 1. this 仅需对象（IsObject 门禁）。
/// 2. S = ToString(string)；flags/global/unicode 经 Get live 直读（unicode 读
///    无条件先于非 global 返回，读序与规范一致）。
/// 3. 非 global：返回共享 exec（GetMethod + Call）结果原值。
/// 4. global：ToIntegerOrInfinity(Get "lastIndex") 副作用读后 Set 0，再经共享
///    exec 循环逐匹配收集；零宽匹配按 fullUnicode 口径推进 lastIndex。
///
/// # 边界与前提
/// - 各属性读（flags/global/unicode/lastIndex/结果属性）均传播原异常。
/// - exec 结果须为 null 或对象，否则 TypeError。
///
/// # 副作用
/// - global 时 Set this.lastIndex（先归零，零宽命中后推进）。
pub fn regexp_symbol_match<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_this_obj(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = vm.reg(args[0]);
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };
    let s_val = haystack.to_value(vm);
    let units = haystack.as_match_text().units().to_vec();

    // live 读序：flags → global → unicode（unicode 无条件先于非 global 返回）。
    let _flags = match rx_get_flags(vm, re_ptr, this_val) {
        Ok(f) => f,
        Err(err) => return NativeResult::Err(err),
    };
    let is_global = match rx_get_bool_prop(vm, re_ptr, "global", this_val) {
        Ok(g) => g,
        Err(err) => return NativeResult::Err(err),
    };
    let full_unicode = match rx_get_bool_prop(vm, re_ptr, "unicode", this_val) {
        Ok(u) => u,
        Err(err) => return NativeResult::Err(err),
    };

    if !is_global {
        let result = match regexp_exec_call(vm, re_ptr, this_val, s_val) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        return NativeResult::Ok(result);
    }

    // lastIndex = ToIntegerOrInfinity(Get)：副作用读，getter 异常传播。
    let last_val = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let _last = oxide_runtime_api::to_integer_or_infinity(last_val);
    if let Err(err) = rx_set_prop(vm, re_ptr, "lastIndex", JsValue::int(0), this_val) {
        return NativeResult::Err(err);
    }

    // exec 循环：收集完整匹配串；零宽匹配按 fullUnicode 口径推进 lastIndex。
    let mut matches: Vec<JsValue> = Vec::new();
    loop {
        let result = match regexp_exec_call(vm, re_ptr, this_val, s_val) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        if result.is_null() {
            break;
        }
        let result_ptr = result.as_js_object_ptr();
        let matched_val = match rx_get_index(vm, result_ptr, 0, result) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(err),
        };
        let matched = match oxide_runtime_api::to_string_value_full(matched_val, vm) {
            Ok(v) => v,
            Err(_) => return NativeResult::Err(crate::iterator::engine_error(vm, "cannot stringify match")),
        };
        let matched_units = vm.string_units(matched).into_owned();
        if matched_units.is_empty() {
            let li_val = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(err),
            };
            let this_index = match to_length_value(vm, li_val) {
                Ok(n) => n,
                Err(err) => return NativeResult::Err(err),
            };
            let next_index = advance_string_index(&units, this_index, full_unicode);
            if let Err(err) = // 越出 i32 的大值（2^53 口径）以浮点形态写入。
                rx_set_prop(vm, re_ptr, "lastIndex", JsValue::float(next_index as f64), this_val)
            {
                return NativeResult::Err(err);
            }
        }
        matches.push(vm.new_string_units(&matched_units));
    }
    if matches.is_empty() {
        return NativeResult::Ok(JsValue::null());
    }
    NativeResult::Ok(crate::string::make_string_array_values(vm, matches))
}

/// `RegExp.prototype[Symbol.replace](string, replacement)`：按 exec 命中替换。
/// 字符串 replacement 展开 `$` 引用（`$$`/`$&`/`` $` ``/`$'`/`$n`），函数
/// replacement 逐匹配调用（groups 非 undefined 时追加为末参）；global 全替换，
/// 否则替换首个。
///
/// # 步骤
/// 1. this 仅需对象（IsObject 门禁）；S = ToString(string)。
/// 2. 非函数分支先 ToString(replacement)（Symbol 抛 TypeError）。
/// 3. flags/global/unicode 经 Get live 直读；global 时
///    ToIntegerOrInfinity(Get "lastIndex") 副作用读后 Set 0。
/// 4. 共享 exec 循环收集结果；零宽匹配按 fullUnicode 口径推进 lastIndex。
/// 5. 替换阶段按结果属性逐条展开（matched/captures/index/groups 均 Get 读取，
///    undefined 捕获不入表）；position 越上界钳到串长，乱序（position 回退）
///    的替换被忽略。
///
/// # 边界与前提
/// - 各属性读（flags/global/unicode/lastIndex/结果属性）均传播原异常。
/// - exec 结果须为 null 或对象，否则 TypeError。
///
/// # 副作用
/// - global 时 Set this.lastIndex（先归零，零宽命中后推进）。
pub fn regexp_symbol_replace<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_this_obj(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = vm.reg(args[0]);
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };
    let s_val = haystack.to_value(vm);
    let units = haystack.as_match_text().units().to_vec();
    let length_s = units.len();

    let replacer_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let functional = crate::iterator::is_callable(replacer_val);
    // 非函数分支的替换串先行 ToString（Symbol 抛 TypeError，异常传播）。
    let replacement_units: Option<Vec<u16>> = if functional {
        None
    } else {
        match replacement_units(vm, replacer_val) {
            Ok(u) => Some(u),
            Err(err) => return NativeResult::Err(err),
        }
    };

    // live 读序：flags → global → unicode。
    let _flags = match rx_get_flags(vm, re_ptr, this_val) {
        Ok(f) => f,
        Err(err) => return NativeResult::Err(err),
    };
    let is_global = match rx_get_bool_prop(vm, re_ptr, "global", this_val) {
        Ok(g) => g,
        Err(err) => return NativeResult::Err(err),
    };
    let full_unicode = match rx_get_bool_prop(vm, re_ptr, "unicode", this_val) {
        Ok(u) => u,
        Err(err) => return NativeResult::Err(err),
    };

    if is_global {
        // lastIndex = ToIntegerOrInfinity(Get)：副作用读，getter 异常传播。
        let last_val = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(err),
        };
        let _last = oxide_runtime_api::to_integer_or_infinity(last_val);
        if let Err(err) = rx_set_prop(vm, re_ptr, "lastIndex", JsValue::int(0), this_val) {
            return NativeResult::Err(err);
        }
    }

    // exec 循环：global 收集到 null 为止，非 global 至多一条。
    let mut results: Vec<JsValue> = Vec::new();
    loop {
        let result = match regexp_exec_call(vm, re_ptr, this_val, s_val) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        if result.is_null() {
            break;
        }
        results.push(result);
        if !is_global {
            break;
        }
        // 零宽命中：按 fullUnicode 口径推进 lastIndex。
        let result_ptr = result.as_js_object_ptr();
        let matched_val = match rx_get_index(vm, result_ptr, 0, result) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(err),
        };
        let matched = match oxide_runtime_api::to_string_value_full(matched_val, vm) {
            Ok(v) => v,
            Err(_) => return NativeResult::Err(crate::iterator::engine_error(vm, "cannot stringify match")),
        };
        if vm.string_units(matched).is_empty() {
            let li_val = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(err),
            };
            let this_index = match to_length_value(vm, li_val) {
                Ok(n) => n,
                Err(err) => return NativeResult::Err(err),
            };
            let next_index = advance_string_index(&units, this_index, full_unicode);
            if let Err(err) = // 越出 i32 的大值（2^53 口径）以浮点形态写入。
                rx_set_prop(vm, re_ptr, "lastIndex", JsValue::float(next_index as f64), this_val)
            {
                return NativeResult::Err(err);
            }
        }
    }

    // 单匹配（非 global）：无命中回原串 S。
    if !is_global {
        if results.is_empty() {
            return NativeResult::Ok(vm.new_string_units_owned(units));
        }
        let result = results[0];
        let result_ptr = result.as_js_object_ptr();
        let n_captures = match result_capture_count(vm, result_ptr, result) {
            Ok(n) => n,
            Err(err) => return NativeResult::Err(err),
        };
        let rec = match build_substitution_record(vm, result_ptr, result, length_s, n_captures) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        let replacement =
            match make_replacement(vm, functional, replacer_val, replacement_units.as_deref(), &rec, &units, s_val) {
                Ok(r) => r,
                Err(err) => return NativeResult::Err(err),
            };
        let mut out: Vec<u16> = Vec::with_capacity(length_s + replacement.len());
        out.extend_from_slice(&units[..rec.position]);
        out.extend_from_slice(&replacement);
        // 尾段自命中末尾（position + matchLength）起，被替换串本身不入结果。
        let tail_start = (rec.position + rec.matched.len()).min(length_s);
        out.extend_from_slice(&units[tail_start..]);
        return NativeResult::Ok(vm.new_string_units_owned(out));
    }

    // global：先定统一捕获宽（各结果 length 的 ToLength 上界减一），
    // 再逐条展开替换。
    let mut n_captures = 0usize;
    for result in &results {
        let result_ptr = result.as_js_object_ptr();
        let n = match result_capture_count(vm, result_ptr, *result) {
            Ok(n) => n,
            Err(err) => return NativeResult::Err(err),
        };
        n_captures = n_captures.max(n);
    }
    let mut out: Vec<u16> = Vec::with_capacity(length_s);
    let mut next_source_position = 0usize;
    for result in &results {
        let result = *result;
        let result_ptr = result.as_js_object_ptr();
        let rec = match build_substitution_record(vm, result_ptr, result, length_s, n_captures) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        // position 回退（乱序结果）的替换被忽略。
        if rec.position < next_source_position {
            continue;
        }
        let replacement =
            match make_replacement(vm, functional, replacer_val, replacement_units.as_deref(), &rec, &units, s_val) {
                Ok(r) => r,
                Err(err) => return NativeResult::Err(err),
            };
        out.extend_from_slice(&units[next_source_position..rec.position]);
        out.extend_from_slice(&replacement);
        next_source_position = rec.position + rec.matched.len();
    }
    if next_source_position < length_s {
        out.extend_from_slice(&units[next_source_position..]);
    }
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// 替换记录：matched（ToString 后单元）、position（ToInteger 后钳 [0, 串长]）、
/// captures（undefined 捕获为 None，其余为 ToString 单元）。
struct SubstitutionRecord {
    matched: Vec<u16>,
    position: usize,
    captures: Vec<Option<Vec<u16>>>,
    groups: JsValue,
}

/// 结果属性 ToString 收尾：完整强转（对象经 ToPrimitive，Symbol 抛
/// TypeError），转换异常恢复原异常值。
fn to_string_units<H: VmHost>(vm: &mut H, val: JsValue) -> Result<Vec<u16>, JsValue> {
    match oxide_runtime_api::to_string_value_full(val, vm) {
        Ok(v) => Ok(vm.string_units(v).into_owned()),
        Err(_) => Err(crate::iterator::engine_error(vm, "cannot stringify result property")),
    }
}

/// 结果捕获数（length 的 ToLength 减一，下界 0）：各属性读异常传播。
fn result_capture_count<H: VmHost>(vm: &mut H, result_ptr: *mut JsObject, result: JsValue) -> Result<usize, JsValue> {
    let len_val = rx_get_prop(vm, result_ptr, "length", result)?;
    let n = to_length_value(vm, len_val)?;
    Ok(n.saturating_sub(1))
}

/// 从单条 exec 结果对象按 GetSubstitution 口径读取 matched/index/captures/
/// groups：各属性读异常传播，捕获 undefined 以空串占位；捕获宽度由调用方给
/// （global 统一取各结果上界，非 global 取本结果自身）。
fn build_substitution_record<H: VmHost>(
    vm: &mut H, result_ptr: *mut JsObject, result: JsValue, length_s: usize, n_captures: usize,
) -> Result<SubstitutionRecord, JsValue> {
    let matched_val = rx_get_index(vm, result_ptr, 0, result)?;
    let matched = to_string_units(vm, matched_val)?;

    // index 经完整 ToNumber（对象触发 valueOf/toString）后再 ToInteger。
    let index_val = rx_get_prop(vm, result_ptr, "index", result)?;
    let num = match oxide_runtime_api::to_number_full(index_val, vm) {
        Ok(n) => n,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    let n = oxide_runtime_api::to_integer_or_infinity(JsValue::float(num));
    let position = if n.is_nan() || n <= 0.0 { 0 } else { n.min(length_s as f64) as usize };

    let mut captures: Vec<Option<Vec<u16>>> = Vec::with_capacity(n_captures);
    for i in 1..=n_captures {
        let cap_val = rx_get_index(vm, result_ptr, i as i32, result)?;
        if cap_val.is_undefined() {
            captures.push(None);
        } else {
            captures.push(Some(to_string_units(vm, cap_val)?));
        }
    }
    let groups = rx_get_prop(vm, result_ptr, "groups", result)?;
    Ok(SubstitutionRecord {
        matched,
        position,
        captures,
        groups,
    })
}

/// 生成单条替换文本：函数分支 Call(replacer, undefined, «matched, ...captures,
/// position, S, groups?»）后 ToString（groups 为 undefined 时不追加末参）；
/// 非函数分支 GetSubstitution 展开。
fn make_replacement<H: VmHost>(
    vm: &mut H, functional: bool, replacer_val: JsValue, replacement_units: Option<&[u16]>, rec: &SubstitutionRecord,
    s_units: &[u16], s_val: JsValue,
) -> Result<Vec<u16>, JsValue> {
    if functional {
        let mut cb_args: Vec<JsValue> = Vec::with_capacity(rec.captures.len() + 5);
        cb_args.push(vm.new_string_units(&rec.matched));
        for cap in &rec.captures {
            match cap {
                Some(u) => cb_args.push(vm.new_string_units(u)),
                None => cb_args.push(JsValue::undefined()),
            }
        }
        cb_args.push(JsValue::int(rec.position as i32));
        cb_args.push(s_val);
        if !rec.groups.is_undefined() {
            cb_args.push(rec.groups);
        }
        let repl_value = match vm.call_function_sync(replacer_val, JsValue::undefined(), &cb_args) {
            Ok(v) => v,
            Err(_) => return Err(crate::iterator::engine_error(vm, "replacer call failed")),
        };
        return to_string_units(vm, repl_value);
    }
    let groups_obj = if rec.groups.is_object() && !rec.groups.as_js_object_ptr().is_null() {
        Some(unsafe { &*rec.groups.as_js_object_ptr() })
    } else {
        None
    };
    Ok(get_substitution_units(
        vm,
        &rec.matched,
        s_units,
        rec.position,
        &rec.captures,
        groups_obj,
        replacement_units.unwrap_or(&[]),
    ))
}

/// `RegExp.prototype[Symbol.search](string)`：返回首个匹配位置，无匹配返回
/// -1。
///
/// # 步骤
/// 1. this 仅需对象（IsObject 门禁）；S = ToString(string)。
/// 2. previous = Get(rx, "lastIndex")（裸读不转换）；与 +0 非 SameValue 时
///    Set 0（SameValue 口径：-0 与 +0 相同，不触发写）。
/// 3. 共享 exec 求匹配。
/// 4. 恢复：当前 lastIndex 与 previous 非 SameValue 时 Set 回 previous。
/// 5. null 回 -1，否则 ToNumber(Get(match, "index"))。
///
/// # 边界与前提
/// - lastIndex 读写均 Set/Get 语义，异常传播原异常值。
///
/// # 副作用
/// - lastIndex 先归零再按原值恢复，exec 期间的写入不外泄。
pub fn regexp_symbol_search<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_this_obj(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = vm.reg(args[0]);
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };
    let s_val = haystack.to_value(vm);

    let previous = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    if !same_value_zero(previous) {
        if let Err(err) = rx_set_prop(vm, re_ptr, "lastIndex", JsValue::int(0), this_val) {
            return NativeResult::Err(err);
        }
    }

    let match_result = match regexp_exec_call(vm, re_ptr, this_val, s_val) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };

    let current = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    if !same_value(previous, current) {
        if let Err(err) = rx_set_prop(vm, re_ptr, "lastIndex", previous, this_val) {
            return NativeResult::Err(err);
        }
    }

    if match_result.is_null() {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let match_ptr = match_result.as_js_object_ptr();
    let index_val = match rx_get_prop(vm, match_ptr, "index", match_result) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let n = match oxide_runtime_api::to_number_full(index_val, vm) {
        Ok(n) => n,
        Err(_) => return NativeResult::Err(crate::iterator::engine_error(vm, "cannot convert index")),
    };
    NativeResult::Ok(JsValue::float(n))
}

/// SameValue(val, +0)：严格口径，-0 与 +0 不同（规范步骤用 SameValue 非
/// SameValueZero：-0 前值仍须触发 Set 0）。
fn same_value_zero(val: JsValue) -> bool {
    if val.is_int() {
        return val.as_int() == 0;
    }
    if val.is_double() {
        return val.as_double() == 0.0 && !val.as_double().is_sign_negative();
    }
    false
}

/// SameValue 全口径比较：同类型同值（-0 与 +0 不同），整数与浮点跨型按数值
/// 比较（符号位参与），其余类型不同恒 false。
fn same_value(a: JsValue, b: JsValue) -> bool {
    match (a.js_type(), b.js_type()) {
        (oxide_types::value::JsType::Int, oxide_types::value::JsType::Int) => a.as_int() == b.as_int(),
        (oxide_types::value::JsType::Double, oxide_types::value::JsType::Double) => {
            a.as_double() == b.as_double() && a.as_double().is_sign_negative() == b.as_double().is_sign_negative()
        }
        (oxide_types::value::JsType::Int, oxide_types::value::JsType::Double)
        | (oxide_types::value::JsType::Double, oxide_types::value::JsType::Int) => {
            let na = if a.is_int() { a.as_int() as f64 } else { a.as_double() };
            let nb = if b.is_int() { b.as_int() as f64 } else { b.as_double() };
            na == nb && na.is_sign_negative() == nb.is_sign_negative()
        }
        (oxide_types::value::JsType::Bool, oxide_types::value::JsType::Bool) => a.as_bool() == b.as_bool(),
        (oxide_types::value::JsType::String, oxide_types::value::JsType::String) => {
            // SAFETY: 两值均为字符串，按载荷形态逐单元比对。
            let (pa, pb) = unsafe { (&*a.as_string_ptr(), &*b.as_string_ptr()) };
            std::ptr::eq(pa, pb) || (pa.utf16_len() == pb.utf16_len() && pa.units() == pb.units())
        }
        (oxide_types::value::JsType::Null, oxide_types::value::JsType::Null)
        | (oxide_types::value::JsType::Undefined, oxide_types::value::JsType::Undefined)
        | (oxide_types::value::JsType::Symbol, oxide_types::value::JsType::Symbol) => {
            a.js_type() == b.js_type() && a == b
        }
        (oxide_types::value::JsType::Object, oxide_types::value::JsType::Object) => {
            a.as_js_object_ptr() == b.as_js_object_ptr()
        }
        _ => false,
    }
}

/// `RegExp.prototype[Symbol.split](string, limit)`：按 splitter（物种构造 +
/// 补 y 标志）的 exec 命中切分，捕获组作为分隔元素插入结果；limit 限制
/// 结果长度，缺省切分全部。
///
/// # 步骤
/// 1. this 仅需对象（IsObject 门禁）；S = ToString(string)。
/// 2. C = SpeciesConstructor(rx, %RegExp%)；flags = ToString(Get "flags")；
///    newFlags = flags 含 y 取原串、否则追加 y。
/// 3. splitter = Construct(C, «rx, newFlags»)。
/// 4. lim = 缺省 2^32-1、否则 ToUint32(limit)；lim == 0 直接回空数组。
/// 5. 空串特判：exec 非 null 回 []，否则回 [""]。
/// 6. p = ToLength(Get "lastIndex")、q = p；循环 q < 串长：Set
///    (splitter, "lastIndex", q) 后共享 exec；null 按 unicodeMatching 口径
///    推进 q；命中取 e = min(ToLength(Get(splitter, "lastIndex")), 串长)，
///    e ≠ p 时推片段 + 捕获组并 p = q = e，e == p 时零宽推进。
/// 7. 循环后无条件推尾段 S[p..]。
///
/// # 边界与前提
/// - splitter 可非 RegExp（物种产物），消费面纯走 exec + 属性读。
/// - 各属性读（flags/lastIndex/length/捕获组）均传播原异常。
pub fn regexp_symbol_split<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_this_obj(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = vm.reg(args[0]);
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };
    let s_val = haystack.to_value(vm);
    let units = haystack.as_match_text().units().to_vec();
    let size = units.len();

    // lim：缺省 2^32-1，否则 ToUint32（完整 ToNumber 传播转换异常；
    // NaN/±0 归零、±∞ 归 2^32-1、负数回绕）。
    let lim = if args.len() > 2 {
        let l = match oxide_runtime_api::to_number_full(vm.reg(args[2]), vm) {
            Ok(n) => n,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        if l.is_nan() || l == 0.0 {
            0usize
        } else if l.is_infinite() {
            u32::MAX as usize
        } else {
            l.trunc().rem_euclid(4_294_967_296.0) as usize
        }
    } else {
        u32::MAX as usize
    };
    if lim == 0 {
        return NativeResult::Ok(make_units_array(vm, Vec::new()));
    }

    // 物种构造 + y 标志：splitter 是后续唯一匹配面。
    let regexp_ctor = vm.session().builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let regexp_ctor_val = JsValue::from_js_object(regexp_ctor);
    let c = match species_constructor(vm, re_ptr, this_val, regexp_ctor_val) {
        Ok(c) => c,
        Err(err) => return NativeResult::Err(err),
    };
    let flags = match rx_get_flags(vm, re_ptr, this_val) {
        Ok(f) => f,
        Err(err) => return NativeResult::Err(err),
    };
    let unicode_matching = flags.contains('u');
    let new_flags = if flags.contains('y') { flags } else { format!("{flags}y") };
    let new_flags_val = vm.new_string_owned(new_flags);
    let splitter_val = match vm.construct_ctor(c, &[this_val, new_flags_val]) {
        Ok(s) => s,
        Err(e) => return NativeResult::Err(e),
    };
    if !splitter_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "RegExp split splitter must be an object"));
    }
    let splitter_ptr = splitter_val.as_js_object_ptr();

    // 空串特判：exec 非 null 回空数组，否则回单空串数组。
    if size == 0 {
        let z = match regexp_exec_call(vm, splitter_ptr, splitter_val, s_val) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        if z.is_null() {
            return NativeResult::Ok(make_units_array(vm, vec![Vec::new()]));
        }
        return NativeResult::Ok(make_units_array(vm, Vec::new()));
    }

    // p = ToLength(Get "lastIndex")，q 与 p 同值。
    let read_position = |vm: &mut H| -> Result<usize, JsValue> {
        let v = rx_get_prop(vm, re_ptr, "lastIndex", this_val)?;
        to_length_value(vm, v)
    };
    let mut p = match read_position(vm) {
        Ok(n) => n,
        Err(err) => return NativeResult::Err(err),
    };
    let mut q = p;

    // 结果推入触顶（n == lim）即按规范早退返回；循环自然结束则无条件推尾段。
    // 片段推字符串值，捕获组推原值（undefined 等不转换）。
    let mut parts: Vec<JsValue> = Vec::new();
    while q < size {
        if let Err(err) = rx_set_prop(vm, splitter_ptr, "lastIndex", JsValue::int(q as i32), splitter_val) {
            return NativeResult::Err(err);
        }
        let z = match regexp_exec_call(vm, splitter_ptr, splitter_val, s_val) {
            Ok(r) => r,
            Err(err) => return NativeResult::Err(err),
        };
        if z.is_null() {
            q = advance_string_index(&units, q, unicode_matching);
            continue;
        }
        let e = {
            let li_val = match rx_get_prop(vm, splitter_ptr, "lastIndex", splitter_val) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(err),
            };
            let n = match to_length_value(vm, li_val) {
                Ok(n) => n,
                Err(err) => return NativeResult::Err(err),
            };
            n.min(size)
        };
        if e != p {
            parts.push(vm.new_string_units(&units[p..q]));
            if parts.len() >= lim {
                return NativeResult::Ok(crate::string::make_string_array_values(vm, parts));
            }
            let cap_num = {
                let len_val = match rx_get_prop(vm, z.as_js_object_ptr(), "length", z) {
                    Ok(v) => v,
                    Err(err) => return NativeResult::Err(err),
                };
                match to_length_value(vm, len_val) {
                    Ok(n) => n,
                    Err(err) => return NativeResult::Err(err),
                }
            };
            for k in 1..cap_num {
                let cap_val = match rx_get_index(vm, z.as_js_object_ptr(), k as i32, z) {
                    Ok(v) => v,
                    Err(err) => return NativeResult::Err(err),
                };
                parts.push(cap_val);
                if parts.len() >= lim {
                    return NativeResult::Ok(crate::string::make_string_array_values(vm, parts));
                }
            }
            p = e;
            q = p;
        } else {
            q = advance_string_index(&units, q, unicode_matching);
        }
    }
    parts.push(vm.new_string_units(&units[p..size]));
    NativeResult::Ok(crate::string::make_string_array_values(vm, parts))
}

/// `RegExp.prototype[Symbol.matchAll](string)`：返回按 global 语义逐个产出
/// 匹配数组的迭代器。
///
/// # 步骤
/// 1. this 仅需对象（IsObject 门禁）；S = ToString(string)。
/// 2. flags = ToString(Get "flags")；isRegExp 旧式判定：Get(R, @@match) 为
///    布尔取之，否则 true（读序在 flags 之后）。
/// 3. matcher 构造：isRegExp 走 SpeciesConstructor + Construct(C, «R, flags»)，
///    非 RegExp 走 Construct(%RegExp%, «R, "g"»)。
/// 4. lastIndex = ToLength(Get(R, "lastIndex"))——只从 R 读一次；
///    Set(matcher, "lastIndex", lastIndex)。
/// 5. global/fullUnicode 由 flags 串判定（不读构造产物属性）。
/// 6. 迭代器包装保持三槽（input/index/re）形态。
///
/// # 边界与前提
/// - 各属性读（flags/@@match/lastIndex）均传播原异常。
///
/// # 副作用
/// - 构造 matcher（物种构造可返回任意对象）。
pub fn regexp_symbol_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let re_ptr = match get_this_obj(vm, args) {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let this_val = vm.reg(args[0]);
    let haystack = match regexp_text_arg(vm, args) {
        Ok(s) => s,
        Err(err) => return NativeResult::Err(err),
    };
    let s_val = haystack.to_value(vm);

    let flags = match rx_get_flags(vm, re_ptr, this_val) {
        Ok(f) => f,
        Err(err) => return NativeResult::Err(err),
    };

    // 旧式 IsRegExp：Get(R, @@match) 为布尔取之，否则 true。
    let match_key = oxide_types::private_key::make_well_known_symbol_key(1);
    let matcher_prop = match vm.ordinary_get(unsafe { &*re_ptr }, match_key, this_val) {
        Ok(v) => v,
        Err(_) => return NativeResult::Err(crate::iterator::engine_error(vm, "cannot read @@match")),
    };
    let is_regexp = match matcher_prop.js_type() {
        oxide_types::value::JsType::Bool => matcher_prop.as_bool(),
        _ => true,
    };

    let regexp_ctor = vm.session().builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let regexp_ctor_val = JsValue::from_js_object(regexp_ctor);
    let matcher_val = if is_regexp {
        let c = match species_constructor(vm, re_ptr, this_val, regexp_ctor_val) {
            Ok(c) => c,
            Err(err) => return NativeResult::Err(err),
        };
        let flags_val = vm.new_string_owned(flags);
        match vm.construct_ctor(c, &[this_val, flags_val]) {
            Ok(m) => m,
            Err(e) => return NativeResult::Err(e),
        }
    } else {
        let g_val = vm.new_string_owned("g".to_string());
        match vm.construct_ctor(regexp_ctor_val, &[this_val, g_val]) {
            Ok(m) => m,
            Err(e) => return NativeResult::Err(e),
        }
    };
    if !matcher_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "RegExp matchAll matcher must be an object"));
    }
    let matcher_ptr = matcher_val.as_js_object_ptr();

    // lastIndex 只从 R 读一次（ToLength），写入 matcher。
    let li_val = match rx_get_prop(vm, re_ptr, "lastIndex", this_val) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let last_index = match to_length_value(vm, li_val) {
        Ok(n) => n,
        Err(err) => return NativeResult::Err(err),
    };
    if let Err(err) = rx_set_prop(vm, matcher_ptr, "lastIndex", JsValue::int(last_index as i32), matcher_val) {
        return NativeResult::Err(err);
    }

    // 迭代器包装：三槽（input/index/re），next 由原型提供。
    let regexp_iter_proto = vm.session().builtin_world().regexp_string_iterator_proto.as_ptr() as *mut JsObject;
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(regexp_iter_proto)));
    let wrapper_obj = unsafe { &mut *wrapper };
    let input_si = vm.kernel_core().perm_interner().intern(crate::string::MALL_INPUT).0;
    let index_si = vm.kernel_core().perm_interner().intern(crate::string::MALL_INDEX).0;
    let re_si = vm.kernel_core().perm_interner().intern(crate::string::MALL_RE).0;
    // input 属性存完整转换后的字符串值（单元保真）；index 游标为码元口径。
    vm.set_or_create_prop_value(wrapper_obj, input_si, s_val);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(last_index as i32));
    vm.set_or_create_prop_value(wrapper_obj, re_si, matcher_val);
    NativeResult::Ok(JsValue::from_js_object(wrapper))
}
