use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::value::JsValue;

/// JS `BigInt()` 构造逻辑：把参数转成 BigInt 值。
///
/// 普通调用返回原始 BigInt；`new BigInt()` 抛 TypeError（BigInt 不可构造）。
/// 参数转换规则（最小子集）：
/// - 无参数 → `0n`
/// - BigInt → 原样返回
/// - Number → 截断为整数（非整数抛 RangeError）
/// - String → 解析十进制整数
/// - Boolean → `0n`/`1n`
/// - 其它类型 → TypeError
pub fn bigint_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() > 1 {
        let this_val = vm.reg(args[0]);
        if this_val.is_object() {
            // new 语义：BigInt 不可 new，抛 TypeError。
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "BigInt is not a constructor",
            ));
        }
    }
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match to_bigint_value(vm, val) {
        Ok(v) => NativeResult::Ok(v),
        Err(err) => NativeResult::Err(err),
    }
}

/// `BigInt.prototype.toString`：返回 BigInt 的十进制字符串。
pub fn bigint_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_bigint() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "BigInt.prototype.toString called on incompatible receiver",
        ));
    }
    let v = unsafe { oxide_runtime_api::bigint_data(this_val) };
    NativeResult::Ok(vm.new_string(&oxide_runtime_api::bigint_to_string(v)))
}

/// `BigInt.prototype.valueOf`：返回包装的 BigInt 原始值。
pub fn bigint_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_bigint() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.proto().is_object() {
                let proto_ptr = obj.proto().as_js_object_ptr();
                let bigint_proto = vm.session().builtin_world().bigint_proto.as_ptr() as *mut oxide_types::object::JsObject;
                if !proto_ptr.is_null() && std::ptr::eq(proto_ptr, bigint_proto) {
                    let v = obj.get_prop_at(0);
                    if v.is_bigint() {
                        return NativeResult::Ok(v);
                    }
                }
            }
        }
    }
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "BigInt.prototype.valueOf called on incompatible receiver",
    ))
}

/// 把原始值转为 BigInt 值（`BigInt(value)` 的参数转换）。
fn to_bigint_value<H: VmHost>(vm: &mut H, val: JsValue) -> Result<JsValue, JsValue> {
    if val.is_bigint() {
        return Ok(val);
    }
    if val.is_undefined() {
        return Ok(vm.new_bigint(0));
    }
    if val.is_bool() {
        return Ok(vm.new_bigint(if val.as_bool() { 1 } else { 0 }));
    }
    if val.is_int() {
        return Ok(vm.new_bigint(val.as_int() as i128));
    }
    if val.is_double() {
        let d = val.as_double();
        if d.is_nan() || d.is_infinite() {
            return Err(crate::error::create_range_error(vm, "The number cannot be converted to a BigInt because it is not an integer"));
        }
        if d.trunc() != d {
            return Err(crate::error::create_range_error(vm, "The number cannot be converted to a BigInt because it is not an integer"));
        }
        return Ok(vm.new_bigint(d.trunc() as i128));
    }
    if val.is_string() {
        let s = unsafe { oxide_runtime_api::string_data(val) }.to_string();
        let s = s.trim();
        let sign = if let Some(rest) = s.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = s.strip_prefix('+') {
            (false, rest)
        } else {
            (false, s)
        };
        let (radix, digits) = if let Some(hex) = sign.1.strip_prefix("0x").or_else(|| sign.1.strip_prefix("0X")) {
            (16u32, hex)
        } else if let Some(oct) = sign.1.strip_prefix("0o").or_else(|| sign.1.strip_prefix("0O")) {
            (8u32, oct)
        } else if let Some(bin) = sign.1.strip_prefix("0b").or_else(|| sign.1.strip_prefix("0B")) {
            (2u32, bin)
        } else {
            (10u32, sign.1)
        };
        let magnitude = match i128::from_str_radix(digits, radix) {
            Ok(m) => m,
            Err(_) => {
                return Err(crate::error::create_syntax_error(
                    vm,
                    "Cannot convert string to BigInt: invalid integer literal",
                ))
            }
        };
        return Ok(vm.new_bigint(if sign.0 { -magnitude } else { magnitude }));
    }
    Err(crate::error::create_type_error(
        vm,
        "Cannot convert value to a BigInt",
    ))
}
