use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtins_debug;
use crate::builtins_error;

use super::common::try_string;
use super::{
    as_units, code_point_count, find_units, is_trim_unit, map_well_formed_segments, rfind_units, take_code_points,
    this_units,
};

/// 安全地将 f64 转为 usize：NaN/±Inf/负值→0，超 u64 上限→usize::MAX。
fn f64_to_usafe_usize(v: f64) -> usize {
    if v.is_finite() && v >= 0.0 {
        let u = v as u64;
        if u <= usize::MAX as u64 {
            u as usize
        } else {
            usize::MAX
        }
    } else {
        0
    }
}

/// 安全地将 f64 转为 i32：NaN/±Inf→0，超 i32 范围→饱和。
fn f64_to_i32(v: f64) -> i32 {
    if v.is_finite() && v >= i32::MIN as f64 && v <= i32::MAX as f64 {
        v as i32
    } else if v.is_nan() || v.is_infinite() {
        0
    } else if v > 0.0 {
        i32::MAX
    } else {
        i32::MIN
    }
}

/// 安全地将 f64 转为 isize：NaN/±Inf→0，超 isize 范围→饱和。
fn f64_to_isize(v: f64) -> isize {
    if v.is_finite() && v >= isize::MIN as f64 && v <= isize::MAX as f64 {
        v as isize
    } else if v.is_nan() || v.is_infinite() {
        0
    } else if v > 0.0 {
        isize::MAX
    } else {
        isize::MIN
    }
}

// ── 静态方法 / 构造 ─────────────────────────────────────────────────────

/// `String.fromCharCode(...codes)`：把各参数按 ToUint32 低 16 位转为单元拼接
/// 为字符串（surrogate 区间合法产出孤立 surrogate 单元）。
pub fn string_from_char_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.fromCharCode called with {} args", args.len());
    let mut units: Vec<u16> = Vec::new();
    for &arg_reg in args.iter().skip(1) {
        let code = oxide_runtime_api::to_uint32(vm.reg(arg_reg)) & 0xFFFF;
        units.push(code as u16);
    }
    NativeResult::Ok(vm.new_string_units_owned(units))
}

/// `String.fromCodePoint(...codes)`：把各参数按 ToNumber 语义转成 code point
/// （0..0x10FFFF 的整数）拼接为字符串；非整数、NaN 或越界抛 RangeError，
/// Symbol 抛 TypeError。surrogate 区间按现行规范接受（产出孤立 surrogate
/// 单元，单元载荷可承载）。
pub fn string_from_code_point<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.fromCodePoint called with {} args", args.len());
    let mut units: Vec<u16> = Vec::new();
    // 经 native_arg_count/native_arg_at 读取：大实参集（如 harness 的
    // `String.fromCodePoint.apply(null, codePoints)` 每块 10000 码位）在寄存器
    // 窗口外走 spill 溢出区，小实参集与既有寄存器路径一致。
    for i in 0..vm.native_arg_count(args) {
        let n = match oxide_runtime_api::to_number_full(vm.native_arg_at(args, i), vm) {
            Ok(n) => n,
            Err(_) => {
                // ToNumber 触发对象 valueOf/toString 抛出的原生异常须原样传播，
                // 否则会被展平为普通 Error 丢失原始异常对象。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        };
        if !n.is_finite() || n.trunc() != n || n < 0.0 || n > 0x10FFFF as f64 {
            return NativeResult::Err(crate::error::create_range_error(vm, "Invalid code point"));
        }
        let code = n as u32;
        if code < 0x10000 {
            units.push(code as u16);
        } else {
            let v = code - 0x10000;
            units.push(0xD800 + (v >> 10) as u16);
            units.push(0xDC00 + (v & 0x3FF) as u16);
        }
    }
    NativeResult::Ok(vm.new_string_units_owned(units))
}

/// `String.prototype.valueOf`：返回包装对象的原始字符串；其它 this 抛 TypeError。
pub fn string_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.valueOf called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    if this_val.is_string() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_string_obj() {
                return NativeResult::Ok(obj.get_prop_at(0));
            }
        }
    }
    builtins_error!("String.prototype.valueOf: invalid receiver");
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "String.prototype.valueOf called on non-String object",
    ))
}

/// JS `String()` 构造逻辑：把参数转成字符串（单元保真）；new 语义返回
/// `[[StringData]]` 包装对象。
pub fn string_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let string_proto = vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject;
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_ctor = if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if ptr.is_null() {
            false
        } else {
            let proto_ptr = unsafe { (*ptr).proto().as_js_object_ptr() };
            !proto_ptr.is_null() && std::ptr::eq(proto_ptr, string_proto)
        }
    } else {
        false
    };

    let str_val = if args.len() > 1 {
        let v = vm.reg(args[1]);
        if is_ctor && v.is_symbol() {
            // new String(Symbol)：构造器路径走 ToString（规范步骤 3b），
            // 对 Symbol 抛 TypeError——与函数调用路径（3a 描述串）相异。
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
        }
        // 对象参数须经 ToPrimitive/ToString 完整转换；函数调用路径的 Symbol
        // 走 SymbolDescriptiveString（规范步骤 3a，不经 ToString）。
        let v = oxide_runtime_api::to_string_for_string_constructor(v, vm);
        match v {
            Ok(s) => vm.new_string_owned(s),
            Err(_) => {
                // ToString on an object may throw via toString/valueOf; propagate the original exception.
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    } else {
        vm.new_string("")
    };

    if !is_ctor {
        return NativeResult::Ok(str_val);
    }

    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_STRING_OBJ;
    obj.push_prop(str_val);
    NativeResult::Ok(this_val)
}

/// `String.prototype.toString`：返回包装对象的原始字符串；其它 this 抛 TypeError。
pub fn string_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_string() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_string_obj() {
                return NativeResult::Ok(obj.get_prop_at(0));
            }
        }
    }
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "String.prototype.toString called on non-String object",
    ))
}

// ── 查找 / 索引 ─────────────────────────────────────────────────────────

/// `String.prototype.indexOf(searchString, position)`：按码元查找首次出现位置。
pub fn string_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.indexOf called with {} args", args.len());
    // 参数转换先行：search 缺省为 undefined（经 ToString 得 "undefined" 参与查找），
    // position 缺省 0；均可能触发对象 ToString/ToNumber（&mut 路径）。
    // 单元序列落地为 owned：借用落地后与后续 &mut 调用无交叠。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN))
    } else {
        0
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len();
    let pos = pos_raw.min(n);

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }
    if let Some(idx) = find_units(&s, &search, pos) {
        return NativeResult::Ok(JsValue::int(idx as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `String.prototype.includes(searchString, position)`：是否包含子串。
pub fn string_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.includes called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN))
    } else {
        0
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let pos = pos_raw.min(s.len());

    if search.is_empty() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    NativeResult::Ok(JsValue::bool(find_units(&s, &search, pos).is_some()))
}

/// `String.prototype.charAt(index)`：返回指定码元位置的 1 单元字符串；越界返回空串。
pub fn string_char_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charAt called with {} args", args.len());
    // index 可能触发对象 ToNumber（&mut 路径），先行转换。
    let idx = if args.len() >= 2 {
        f64_to_i32(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN))
    } else {
        0
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    if idx < 0 || idx as usize >= s.len() {
        return NativeResult::Ok(vm.new_string(""));
    }
    let u = s[idx as usize];
    // ASCII 单元走单字符缓存零分配，其余（含孤立 surrogate）回落 1 单元串创建。
    match vm.single_unit(u) {
        Some(v) => NativeResult::Ok(v),
        None => NativeResult::Ok(vm.new_string_units(&[u])),
    }
}

/// `String.prototype.charCodeAt(index)`：返回指定码元位置的 code unit；越界返回 NaN。
pub fn string_char_code_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charCodeAt called with {} args", args.len());
    // index 可能触发对象 ToNumber（&mut 路径），先行转换。
    let idx = if args.len() >= 2 {
        f64_to_i32(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN))
    } else {
        0
    };
    let s = try_string!(this_units(vm, args));
    if idx < 0 || idx as usize >= s.len() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    NativeResult::Ok(JsValue::int(s[idx as usize] as i32))
}

/// `String.prototype.codePointAt(pos)`：按码元位置取 code point（surrogate 对
/// 合并）；越界或孤立代理返回 undefined。
pub fn string_code_point_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.codePointAt called with {} args", args.len());
    // pos 转换先行（&mut 路径），后取 this 单元序列定位。
    let pos = if args.len() > 1 {
        let pos_val = vm.reg(args[1]);
        if pos_val.is_symbol() {
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a number"));
        }
        match vm.coerce_number_bounded(pos_val) {
            Ok(n) => n,
            Err(_) => {
                // pos 对象 valueOf/toString 抛出的异常须原样传播。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert argument to a number"));
            }
        }
    } else {
        0.0
    };
    let pos = if pos.is_nan() { 0.0 } else { pos.trunc() };
    if pos < 0.0 || pos > u32::MAX as f64 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let pos = pos as usize;
    let s = try_string!(this_units(vm, args));
    if pos >= s.len() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let first = s[pos];
    if (0xD800..=0xDBFF).contains(&first) && pos + 1 < s.len() {
        let second = s[pos + 1];
        if (0xDC00..=0xDFFF).contains(&second) {
            let cp = 0x10000 + (((first - 0xD800) as u32) << 10) + (second - 0xDC00) as u32;
            return NativeResult::Ok(JsValue::int(cp as i32));
        }
    }
    NativeResult::Ok(JsValue::int(first as i32))
}

/// `String.prototype.lastIndexOf(searchString, position)`：从后往前查找首次
/// 出现位置（起始位置 ≤ position，position 缺省串长）。
pub fn string_last_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.lastIndexOf called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        let p = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if p.is_nan() {
            None
        } else {
            Some(p as usize)
        }
    } else {
        None
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len();
    let pos = match pos_raw {
        Some(p) => p.min(n),
        None => n,
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }
    // 窗口 = 前 pos+1 个码元：起始位置 ≤ pos 的末次出现。
    let window = (pos + 1).min(n);
    if let Some(idx) = rfind_units(&s, &search, window) {
        return NativeResult::Ok(JsValue::int(idx as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

// ── 切片 / 变换 ─────────────────────────────────────────────────────────

/// `String.prototype.concat(...strings)`：拼接 this 与各参数返回新字符串
/// （单元口径，参数经完整 ToString）。
pub fn string_concat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.concat called with {} args", args.len());
    let mut units = try_string!(this_units(vm, args)).into_owned();
    for &arg_reg in args.iter().skip(1) {
        match oxide_runtime_api::to_string_value_full(vm.reg(arg_reg), vm) {
            Ok(v) => units.extend(vm.string_units(v).as_ref()),
            Err(_) => {
                // ToString on an object may throw via toString/valueOf; propagate the original exception.
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    }
    NativeResult::Ok(vm.new_string_units_owned(units))
}

/// `String.prototype.slice(start, end)`：按码元区间（支持负索引）取子串。
pub fn string_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.slice called with {} args", args.len());
    // 位置参数先行（&mut 转换），后借 this 取子串。
    let start_raw = if args.len() > 1 {
        Some(f64_to_i32(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN)))
    } else {
        None
    };
    let end_raw = if args.len() > 2 {
        Some(f64_to_i32(vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN)))
    } else {
        None
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len() as i32;
    let start = match start_raw {
        Some(v) => {
            if v < 0 {
                (n + v).max(0)
            } else {
                v.min(n)
            }
        }
        None => 0,
    };
    let end = match end_raw {
        Some(v) => {
            if v < 0 {
                (n + v).max(0)
            } else {
                v.min(n)
            }
        }
        None => n,
    };
    // 码元区间精确切片（孤立 surrogate 按单元保留，不吸附字符边界）。
    let out: Vec<u16> = if start < end {
        let (a, b) = (start as usize, end as usize);
        s[a.min(s.len())..b.min(s.len())].to_vec()
    } else {
        Vec::new()
    };
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.substring(start, end)`：取子串，start/end 自动对调且取非负。
pub fn string_substring<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substring called with {} args", args.len());
    // 寄存器取值先行（纯函数），后借 this 取子串。
    let start_arg = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let end_arg = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len() as i32;
    let mut start = match start_arg {
        Some(v) => {
            let v = oxide_runtime_api::to_integer_or_infinity(v);
            if v.is_nan() || v < 0.0 {
                0
            } else {
                (v as i32).min(n)
            }
        }
        None => 0,
    };
    let mut end = match end_arg {
        Some(v) => {
            let v = oxide_runtime_api::to_integer_or_infinity(v);
            if v.is_nan() || v < 0.0 {
                0
            } else {
                (v as i32).min(n)
            }
        }
        None => n,
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let out: Vec<u16> = s[start as usize..end as usize].to_vec();
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.substr(start, length)`：从 start 起取 length 个码元
/// （Annex B，支持负 start）。
pub fn string_substr<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substr called with {} args", args.len());
    // 寄存器取值先行（纯函数），后借 this 截取。
    let start_arg = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let length_arg = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let len = s.len() as isize;
    let start = match start_arg {
        Some(v) => {
            let n = f64_to_isize(oxide_runtime_api::to_integer_or_infinity(v));
            if n < 0 {
                (len + n).max(0)
            } else {
                n.min(len)
            }
        }
        None => 0,
    } as usize;
    let length = match length_arg {
        Some(v) => f64_to_isize(oxide_runtime_api::to_integer_or_infinity(v)).max(0) as usize,
        None => len as usize - start,
    };
    let count = length.min(len as usize - start);
    let out: Vec<u16> = s[start..start + count].to_vec();
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.at(index)`：按码元索引取 1 单元字符串（支持负索引）；
/// 越界返回 undefined。
pub fn string_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.at called with {} args", args.len());
    let idx = if args.len() > 1 {
        f64_to_i32(oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1])))
    } else {
        0
    };
    let s = try_string!(this_units(vm, args));
    let len = s.len() as i32;
    let idx = if idx < 0 { len + idx } else { idx };
    if idx < 0 || idx >= len {
        return NativeResult::Ok(JsValue::undefined());
    }
    let u = s[idx as usize];
    match vm.single_unit(u) {
        Some(v) => NativeResult::Ok(v),
        None => NativeResult::Ok(vm.new_string_units(&[u])),
    }
}

/// `String.prototype.toUpperCase`：全大写转换（良形段映射，孤立 surrogate
/// 单元原样保留）。
pub fn string_to_upper_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toUpperCase called with {} args", args.len());
    let s = try_string!(this_units(vm, args));
    let out = map_well_formed_segments(&s, |seg| seg.to_uppercase());
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.toLowerCase`：全小写转换（良形段映射，孤立 surrogate
/// 单元原样保留）。
pub fn string_to_lower_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toLowerCase called with {} args", args.len());
    let s = try_string!(this_units(vm, args));
    let out = map_well_formed_segments(&s, |seg| seg.to_lowercase());
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.trim`：去除两端白空格（单元口径）。
pub fn string_trim<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trim called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len();
    let mut start = 0;
    while start < n && is_trim_unit(s[start]) {
        start += 1;
    }
    let mut end = n;
    while end > start && is_trim_unit(s[end - 1]) {
        end -= 1;
    }
    NativeResult::Ok(vm.new_string_units_owned(s[start..end].to_vec()))
}

/// `String.prototype.trimStart`：去除头部白空格。
pub fn string_trim_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trimStart called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let mut start = 0;
    while start < s.len() && is_trim_unit(s[start]) {
        start += 1;
    }
    NativeResult::Ok(vm.new_string_units_owned(s[start..].to_vec()))
}

/// `String.prototype.trimEnd`：去除尾部白空格。
pub fn string_trim_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trimEnd called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let mut end = s.len();
    while end > 0 && is_trim_unit(s[end - 1]) {
        end -= 1;
    }
    NativeResult::Ok(vm.new_string_units_owned(s[..end].to_vec()))
}

/// `String.prototype.repeat(count)`：重复字符串 count 次（当前上限 10000 防滥用）。
pub fn string_repeat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.repeat called with {} args", args.len());
    // count 转换先行（&mut 路径），后借 this 重复。
    let n = if args.len() > 1 {
        f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN)).min(10000)
    } else {
        1
    };
    let s = try_string!(this_units(vm, args));
    let mut out = Vec::with_capacity(s.len() * n);
    for _ in 0..n {
        out.extend_from_slice(&s);
    }
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.padStart(targetLength, padString)`：在头部补足 padString
/// 到目标码点数。
pub fn string_pad_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padStart called with {} args", args.len());
    // 参数转换先行（&mut 路径）：targetLength 与 padString 均可能触发对象转换。
    let target_arg = if args.len() > 1 {
        Some(f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN)))
    } else {
        None
    };
    let pad_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 规范口径：padString 为 undefined（含显式传入）回落空格填充。
    let pad: Vec<u16> = if pad_val.is_undefined() {
        vec![0x20]
    } else {
        try_string!(as_units(vm, pad_val)).into_owned()
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let s_len = code_point_count(&s);
    let target = target_arg.unwrap_or(s_len);
    if target > 10000 {
        builtins_error!("String.prototype.padStart: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    if s_len >= target || pad.is_empty() {
        return NativeResult::Ok(vm.new_string_units_owned(s.to_vec()));
    }
    let needed = target - s_len;
    let pad_len = code_point_count(&pad).max(1);
    let reps = needed.div_ceil(pad_len);
    let mut pad_rep = Vec::with_capacity(pad.len() * reps);
    for _ in 0..reps {
        pad_rep.extend_from_slice(&pad);
    }
    let prefix = take_code_points(&pad_rep, needed);
    let mut out = Vec::with_capacity(prefix.len() + s.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(&s);
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.padEnd(targetLength, padString)`：在尾部补足 padString
/// 到目标码点数。
pub fn string_pad_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padEnd called with {} args", args.len());
    // 参数转换先行（&mut 路径）：targetLength 与 padString 均可能触发对象转换。
    let target_arg = if args.len() > 1 {
        Some(f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN)))
    } else {
        None
    };
    let pad_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 规范口径：padString 为 undefined（含显式传入）回落空格填充。
    let pad: Vec<u16> = if pad_val.is_undefined() {
        vec![0x20]
    } else {
        try_string!(as_units(vm, pad_val)).into_owned()
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let s_len = code_point_count(&s);
    let target = target_arg.unwrap_or(s_len);
    if target > 10000 {
        builtins_error!("String.prototype.padEnd: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    if s_len >= target || pad.is_empty() {
        return NativeResult::Ok(vm.new_string_units_owned(s.to_vec()));
    }
    let needed = target - s_len;
    let pad_len = code_point_count(&pad).max(1);
    let reps = needed.div_ceil(pad_len);
    let mut pad_rep = Vec::with_capacity(pad.len() * reps);
    for _ in 0..reps {
        pad_rep.extend_from_slice(&pad);
    }
    let suffix = take_code_points(&pad_rep, needed);
    let mut out = Vec::with_capacity(s.len() + suffix.len());
    out.extend_from_slice(&s);
    out.extend_from_slice(suffix);
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.startsWith(searchString, position)`：是否以指定子串开头
/// （position 起的前缀码元比较，不做字符边界吸附——规格口径）。
pub fn string_starts_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.startsWith called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN))
    } else {
        0
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let pos = pos_raw.min(s.len());
    let result = if search.is_empty() { true } else { s[pos..].starts_with(&search) };
    NativeResult::Ok(JsValue::bool(result))
}

/// `String.prototype.endsWith(searchString, endPosition)`：是否以指定子串结尾
/// （截断至 endPosition 的后缀码元比较，不做字符边界吸附——规格口径）。
pub fn string_ends_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.endsWith called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    let end_pos_raw = if args.len() > 2 {
        f64_to_usafe_usize(vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN))
    } else {
        usize::MAX
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let end_pos = end_pos_raw.min(s.len());
    let result = if search.is_empty() { true } else { s[..end_pos].ends_with(&search) };
    NativeResult::Ok(JsValue::bool(result))
}
