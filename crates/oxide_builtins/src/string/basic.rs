use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::array::to_integer_or_infinity_bounded;
use crate::builtins_debug;
use crate::builtins_error;

use super::common::try_string;
use super::{as_units, find_units, is_trim_unit, map_well_formed_segments, rfind_units, this_units};

// ── 静态方法 / 构造 ─────────────────────────────────────────────────────

/// `String.fromCharCode(...codes)`：各参数经 ToUint16(ToNumber) 转单元拼接
/// 为字符串（surrogate 区间合法产出孤立 surrogate 单元）。
///
/// ToNumber 可抛：Symbol 抛 TypeError，BigInt 抛 TypeError，对象 ToPrimitive
/// 触发 valueOf/toString 抛出的原生异常原样传播。
pub fn string_from_char_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.fromCharCode called with {} args", args.len());
    let mut units: Vec<u16> = Vec::new();
    // 经 native_arg_count/native_arg_at 读取：大实参集在寄存器窗口外走 spill
    // 溢出区，小实参集与既有寄存器路径一致。
    for i in 0..vm.native_arg_count(args) {
        let arg_val = vm.native_arg_at(args, i);
        // ToNumber(BigInt) 按规范抛 TypeError；to_number_full 的 BigInt 快路径
        // 静默转换，须在此显式拦截。
        if arg_val.is_bigint() {
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a BigInt value to a number"));
        }
        let n = match oxide_runtime_api::to_number_full(arg_val, vm) {
            Ok(n) => n,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        };
        // ToUint16：floorMod(n, 2^16)；NaN/±0/±Infinity 归零。
        let code = if n == 0.0 || !n.is_finite() {
            0
        } else {
            n.trunc().rem_euclid(65536.0) as u16
        };
        units.push(code);
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
                return NativeResult::Ok(obj.boxed_value());
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
            Err(msg) => {
                // ToString on an object may throw via toString/valueOf; propagate the original exception.
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                // 无在途异常时按格式化文本恢复 kind 与消息（装箱 Symbol 走 ToString
                // 抛 TypeError，消息须与原始异常一致）。
                return NativeResult::Err(crate::error::create_from_text(vm, &msg));
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
    // 构造期物化：boxed_value 载荷与字符索引/length 固有属性同批落地。
    oxide_runtime_api::materialize_string_box(vm, obj, str_val);
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
                return NativeResult::Ok(obj.boxed_value());
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
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len();
    // position：ToIntegerOrInfinity 传播式，负值归 0、+Inf 归 len。
    let pos = if args.len() > 2 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        (p.max(0.0).min(n as f64)) as usize
    } else {
        0
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }
    if let Some(idx) = find_units(&s, &search, pos) {
        return NativeResult::Ok(JsValue::int(idx as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `String.prototype.includes(searchString, position)`：是否包含子串；search 为
/// RegExp 抛 TypeError（普通对象带抛错 @@match getter 时传播原异常）。
pub fn string_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.includes called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    // IsRegExp 前置判：RegExp search 抛 TypeError；getter 抛错恢复原异常上抛。
    let is_regexp = match super::is_regexp_live(vm, search_val) {
        Ok(b) => b,
        Err(e) => return NativeResult::Err(e),
    };
    if is_regexp {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "First argument to String.prototype.includes must not be a regular expression",
        ));
    }
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // position：ToIntegerOrInfinity 传播式，负值归 0、+Inf 归 len。
    let pos = if args.len() > 2 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        (p.max(0.0).min(s.len() as f64)) as usize
    } else {
        0
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    NativeResult::Ok(JsValue::bool(find_units(&s, &search, pos).is_some()))
}

/// `String.prototype.charAt(index)`：返回指定码元位置的 1 单元字符串；越界返回空串。
pub fn string_char_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charAt called with {} args", args.len());
    // index 可能触发对象 ToNumber（&mut 路径），先行转换；ToIntegerOrInfinity
    // 传播式，负值/越界（含 ±Inf）返回空串。
    let idx = if args.len() >= 2 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        0.0
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    if idx < 0.0 || idx >= s.len() as f64 {
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
    // index 可能触发对象 ToNumber（&mut 路径），先行转换；ToIntegerOrInfinity
    // 传播式，负值/越界（含 ±Inf）返回 NaN。
    let idx = if args.len() >= 2 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        0.0
    };
    let s = try_string!(this_units(vm, args));
    if idx < 0.0 || idx >= s.len() as f64 {
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
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len();
    // position：ToIntegerOrInfinity 传播式（NaN → 全长，legacy 位置窗口），
    // 负值归 0、+Inf 归 len；缺参 → len。
    let pos = if args.len() > 2 {
        let p = match vm.coerce_number_bounded(vm.reg(args[2])) {
            Ok(p) => p,
            Err(msg) => return NativeResult::Err(crate::array::from_engine_error(vm, &msg)),
        };
        if p.is_nan() {
            n
        } else {
            (p.max(0.0).min(n as f64)) as usize
        }
    } else {
        n
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }
    // 窗口 = 前 pos+searchLen 个码元：起始位置 ≤ pos 的末次出现。
    let window = (pos + search.len()).min(n);
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
    // this 单元序列先行（&mut 借用落地），后转两个位置参数。
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let n = s.len();
    // start：缺参 → 0；ToIntegerOrInfinity 传播式，相对折叠（负从尾数、+Inf 归 len）。
    let start = if args.len() > 1 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        if p < 0.0 {
            (n as f64 + p).max(0.0) as usize
        } else {
            p.min(n as f64) as usize
        }
    } else {
        0
    };
    // end：缺参/显式 undefined → len（规范 GetRelativeEnd），其余同相对折叠。
    let end = if args.len() > 2 {
        let end_val = vm.reg(args[2]);
        if end_val.is_undefined() {
            n
        } else {
            let p = match to_integer_or_infinity_bounded(vm, end_val) {
                Ok(p) => p,
                Err(exc) => return NativeResult::Err(exc),
            };
            if p < 0.0 {
                (n as f64 + p).max(0.0) as usize
            } else {
                p.min(n as f64) as usize
            }
        }
    } else {
        n
    };
    // 码元区间精确切片（孤立 surrogate 按单元保留，不吸附字符边界）。
    let out: Vec<u16> = if start < end {
        s[start.min(s.len())..end.min(s.len())].to_vec()
    } else {
        Vec::new()
    };
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.substring(start, end)`：取子串，start/end 自动对调且取非负；
/// 显式 undefined 端视缺参（end → len，node 口径）。
pub fn string_substring<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substring called with {} args", args.len());
    // 寄存器取值先行（纯函数），后借 this 取子串。
    let start_arg = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let end_arg = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // ToIntegerOrInfinity 传播式：NaN/负 → 0、+Inf → len，取 min。
    let mut start = match start_arg {
        Some(v) => {
            let p = match to_integer_or_infinity_bounded(vm, v) {
                Ok(p) => p,
                Err(exc) => return NativeResult::Err(exc),
            };
            if p < 0.0 {
                0.0
            } else {
                p.min(s.len() as f64)
            }
        }
        None => 0.0,
    };
    // 显式 undefined 端视缺参（node 口径：end → len），其余 ToIntegerOrInfinity
    // 传播式：NaN/负 → 0、+Inf → len，取 min。
    let mut end = match end_arg {
        Some(v) if v.is_undefined() => s.len() as f64,
        Some(v) => {
            let p = match to_integer_or_infinity_bounded(vm, v) {
                Ok(p) => p,
                Err(exc) => return NativeResult::Err(exc),
            };
            if p < 0.0 {
                0.0
            } else {
                p.min(s.len() as f64)
            }
        }
        None => s.len() as f64,
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
    let len = s.len();
    // start：ToIntegerOrInfinity 传播式，相对折叠（负从尾数、+Inf 归 len）。
    let start = match start_arg {
        Some(v) => {
            let p = match to_integer_or_infinity_bounded(vm, v) {
                Ok(p) => p,
                Err(exc) => return NativeResult::Err(exc),
            };
            if p < 0.0 {
                (len as f64 + p).max(0.0) as usize
            } else {
                p.min(len as f64) as usize
            }
        }
        None => 0,
    };
    // length：负 → 0；+Inf 取剩余全部。
    let length = match length_arg {
        Some(v) => {
            let p = match to_integer_or_infinity_bounded(vm, v) {
                Ok(p) => p,
                Err(exc) => return NativeResult::Err(exc),
            };
            p.max(0.0) as usize
        }
        None => len - start,
    };
    let count = length.min(len - start);
    let out: Vec<u16> = s[start..start + count].to_vec();
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.at(index)`：按码元索引取 1 单元字符串（支持负索引）；
/// 越界返回 undefined。
pub fn string_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.at called with {} args", args.len());
    // index：ToIntegerOrInfinity 传播式，负索引从尾折算，越界（含 ±Inf）→ undefined。
    let idx = if args.len() > 1 {
        match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        0.0
    };
    let s = try_string!(this_units(vm, args));
    let len = s.len() as f64;
    let idx = if idx < 0.0 { len + idx } else { idx };
    if idx < 0.0 || idx >= len {
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

/// `String.prototype.repeat(count)`：重复字符串 count 次。
pub fn string_repeat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.repeat called with {} args", args.len());
    // count 转换先行（&mut 路径）：ToIntegerOrInfinity 传播式，负值与 +Inf
    // 抛 RangeError（规范步），后借 this 重复。
    let n = if args.len() > 1 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        if p < 0.0 || p.is_infinite() {
            return NativeResult::Err(crate::error::create_range_error(vm, "Invalid count argument"));
        }
        p
    } else {
        1.0
    };
    let s = try_string!(this_units(vm, args));
    // 空串或 0 次直接返回空串（规范步 7），免大 count 下空循环。
    if s.is_empty() || n == 0.0 {
        return NativeResult::Ok(vm.new_string_units_owned(Vec::new()));
    }
    // 重复后总长超 2^53-1 规范上限抛 RangeError（规范步 9）。
    if (s.len() as f64) * n > 9007199254740991.0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    let n = n as usize;
    let mut out = Vec::with_capacity(s.len() * n);
    for _ in 0..n {
        out.extend_from_slice(&s);
    }
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.padStart(targetLength, padString)`：在头部补足 padString
/// 到目标码元数。
pub fn string_pad_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padStart called with {} args", args.len());
    // 可观察操作序由规范钉死：receiver ToString 先行，后 targetLength 转换，
    // 再 padString ToString。
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // targetLength：ToIntegerOrInfinity 传播式，负值归 0；+Inf 经既有 10000
    // 上限检查自然落 RangeError。
    let target = if args.len() > 1 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        p.max(0.0)
    } else {
        s.len() as f64
    };
    // 规范口径：padString 为 undefined（含显式传入）回落空格填充。
    let pad_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let pad: Vec<u16> = if pad_val.is_undefined() {
        vec![0x20]
    } else {
        try_string!(as_units(vm, pad_val)).into_owned()
    };
    // 目标长度按码元（非码点）口径：pad 亦按码元截断。
    let s_len = s.len();
    let target = target as usize;
    if s_len >= target {
        return NativeResult::Ok(vm.new_string_units_owned(s));
    }
    if target > 10000 {
        builtins_error!("String.prototype.padStart: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    if pad.is_empty() {
        return NativeResult::Ok(vm.new_string_units_owned(s));
    }
    let needed = target - s_len;
    let pad_len = pad.len();
    let reps = needed.div_ceil(pad_len);
    let mut pad_rep = Vec::with_capacity(pad.len() * reps);
    for _ in 0..reps {
        pad_rep.extend_from_slice(&pad);
    }
    let prefix = &pad_rep[..needed];
    let mut out = Vec::with_capacity(prefix.len() + s.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(&s);
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.padEnd(targetLength, padString)`：在尾部补足 padString
/// 到目标码元数。
pub fn string_pad_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padEnd called with {} args", args.len());
    // 可观察操作序由规范钉死：receiver ToString 先行，后 targetLength 转换，
    // 再 padString ToString。
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // targetLength：ToIntegerOrInfinity 传播式，负值归 0；+Inf 经既有 10000
    // 上限检查自然落 RangeError。
    let target = if args.len() > 1 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[1])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        p.max(0.0)
    } else {
        s.len() as f64
    };
    // 规范口径：padString 为 undefined（含显式传入）回落空格填充。
    let pad_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let pad: Vec<u16> = if pad_val.is_undefined() {
        vec![0x20]
    } else {
        try_string!(as_units(vm, pad_val)).into_owned()
    };
    // 目标长度按码元（非码点）口径：pad 亦按码元截断。
    let s_len = s.len();
    let target = target as usize;
    if s_len >= target {
        return NativeResult::Ok(vm.new_string_units_owned(s));
    }
    if target > 10000 {
        builtins_error!("String.prototype.padEnd: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    if pad.is_empty() {
        return NativeResult::Ok(vm.new_string_units_owned(s));
    }
    let needed = target - s_len;
    let pad_len = pad.len();
    let reps = needed.div_ceil(pad_len);
    let mut pad_rep = Vec::with_capacity(pad.len() * reps);
    for _ in 0..reps {
        pad_rep.extend_from_slice(&pad);
    }
    let suffix = &pad_rep[..needed];
    let mut out = Vec::with_capacity(s.len() + suffix.len());
    out.extend_from_slice(&s);
    out.extend_from_slice(suffix);
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.startsWith(searchString, position)`：是否以指定子串开头
/// （position 起的前缀码元比较，不做字符边界吸附——规格口径）；search 为
/// RegExp 抛 TypeError（普通对象带抛错 @@match getter 时传播原异常）。
pub fn string_starts_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.startsWith called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    // IsRegExp 前置判：RegExp search 抛 TypeError；getter 抛错恢复原异常上抛。
    let is_regexp = match super::is_regexp_live(vm, search_val) {
        Ok(b) => b,
        Err(e) => return NativeResult::Err(e),
    };
    if is_regexp {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "First argument to String.prototype.startsWith must not be a regular expression",
        ));
    }
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // position：ToIntegerOrInfinity 传播式，负值归 0、+Inf 归 len。
    let pos = if args.len() > 2 {
        let p = match to_integer_or_infinity_bounded(vm, vm.reg(args[2])) {
            Ok(p) => p,
            Err(exc) => return NativeResult::Err(exc),
        };
        (p.max(0.0).min(s.len() as f64)) as usize
    } else {
        0
    };
    let result = if search.is_empty() { true } else { s[pos..].starts_with(&search) };
    NativeResult::Ok(JsValue::bool(result))
}

/// `String.prototype.endsWith(searchString, endPosition)`：是否以指定子串结尾
/// （截断至 endPosition 的后缀码元比较，不做字符边界吸附——规格口径）；search
/// 为 RegExp 抛 TypeError（普通对象带抛错 @@match getter 时传播原异常）。
pub fn string_ends_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.endsWith called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search: Vec<u16> = try_string!(as_units(vm, search_val)).into_owned();
    // IsRegExp 前置判：RegExp search 抛 TypeError；getter 抛错恢复原异常上抛。
    let is_regexp = match super::is_regexp_live(vm, search_val) {
        Ok(b) => b,
        Err(e) => return NativeResult::Err(e),
    };
    if is_regexp {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "First argument to String.prototype.endsWith must not be a regular expression",
        ));
    }
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // endPosition：缺参/显式 undefined → len（规范步），其余 ToIntegerOrInfinity
    // 传播式折叠，负值归 0、+Inf 归 len。
    let end_pos = if args.len() > 2 {
        let end_val = vm.reg(args[2]);
        if end_val.is_undefined() {
            s.len()
        } else {
            let p = match to_integer_or_infinity_bounded(vm, end_val) {
                Ok(p) => p,
                Err(exc) => return NativeResult::Err(exc),
            };
            (p.max(0.0).min(s.len() as f64)) as usize
        }
    } else {
        s.len()
    };
    let result = if search.is_empty() { true } else { s[..end_pos].ends_with(&search) };
    NativeResult::Ok(JsValue::bool(result))
}

// ── Annex B HTML 方法族 ─────────────────────────────────────────────────
//
// 13 个 HTML 包装方法共用 CreateHTML 语义：this 经 RequireObjectCoercible +
// ToString 后拼 `<tag attr="值">串</tag>`；attr 为空串则无属性段，属性值
// 经完整 ToString（缺参得 "undefined"），值内 0x0022（双引号）替换为
// `&quot;`，其余单元原样保留。

/// CreateHTML 语义拼接：`<tag attr="值">串</tag>` 片段；attr 为空则无属性段，
/// 属性值内 0x0022 替换为 `&quot;`。
fn create_html(s: &[u16], tag: &str, attr: &str, attr_value: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len() + tag.len() * 2 + attr.len() + attr_value.len() * 2 + 8);
    out.push(b'<' as u16);
    out.extend(tag.encode_utf16());
    if !attr.is_empty() {
        out.push(b' ' as u16);
        out.extend(attr.encode_utf16());
        out.push(b'=' as u16);
        out.push(b'"' as u16);
        for &u in attr_value {
            if u == 0x0022 {
                out.extend(b"&quot;".iter().map(|&b| b as u16));
            } else {
                out.push(u);
            }
        }
        out.push(b'"' as u16);
    }
    out.push(b'>' as u16);
    out.extend_from_slice(s);
    out.push(b'<' as u16);
    out.push(b'/' as u16);
    out.extend(tag.encode_utf16());
    out.push(b'>' as u16);
    out
}

/// `String.prototype.anchor(name)`：返回 `<a name="值">串</a>` 形 HTML 片段；
/// 属性值经完整 ToString（缺参得 "undefined"），值内双引号替换为 `&quot;`。
pub fn string_anchor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.anchor called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v: Vec<u16> = try_string!(as_units(vm, val)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "a", "name", &v)))
}

/// `String.prototype.big()`：返回 `<big>串</big>` 形 HTML 片段。
pub fn string_big<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.big called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "big", "", &[])))
}

/// `String.prototype.blink()`：返回 `<blink>串</blink>` 形 HTML 片段。
pub fn string_blink<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.blink called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "blink", "", &[])))
}

/// `String.prototype.bold()`：返回 `<b>串</b>` 形 HTML 片段。
pub fn string_bold<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.bold called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "b", "", &[])))
}

/// `String.prototype.fixed()`：返回 `<tt>串</tt>` 形 HTML 片段。
pub fn string_fixed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.fixed called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "tt", "", &[])))
}

/// `String.prototype.fontcolor(colour)`：返回 `<font color="值">串</font>` 形
/// HTML 片段；属性值经完整 ToString（缺参得 "undefined"），值内双引号替换为
/// `&quot;`。
pub fn string_fontcolor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.fontcolor called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v: Vec<u16> = try_string!(as_units(vm, val)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "font", "color", &v)))
}

/// `String.prototype.fontsize(size)`：返回 `<font size="值">串</font>` 形
/// HTML 片段；属性值经完整 ToString（缺参得 "undefined"），值内双引号替换为
/// `&quot;`。
pub fn string_fontsize<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.fontsize called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v: Vec<u16> = try_string!(as_units(vm, val)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "font", "size", &v)))
}

/// `String.prototype.italics()`：返回 `<i>串</i>` 形 HTML 片段。
pub fn string_italics<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.italics called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "i", "", &[])))
}

/// `String.prototype.link(url)`：返回 `<a href="值">串</a>` 形 HTML 片段；
/// 属性值经完整 ToString（缺参得 "undefined"），值内双引号替换为 `&quot;`。
pub fn string_link<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.link called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v: Vec<u16> = try_string!(as_units(vm, val)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "a", "href", &v)))
}

/// `String.prototype.small()`：返回 `<small>串</small>` 形 HTML 片段。
pub fn string_small<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.small called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "small", "", &[])))
}

/// `String.prototype.strike()`：返回 `<strike>串</strike>` 形 HTML 片段。
pub fn string_strike<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.strike called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "strike", "", &[])))
}

/// `String.prototype.sub()`：返回 `<sub>串</sub>` 形 HTML 片段。
pub fn string_sub<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.sub called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "sub", "", &[])))
}

/// `String.prototype.sup()`：返回 `<sup>串</sup>` 形 HTML 片段。
pub fn string_sup<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.sup called with {} args", args.len());
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    NativeResult::Ok(vm.new_string_units_owned(create_html(&s, "sup", "", &[])))
}
