use num_bigint::BigInt;
use oxide_runtime_api::{to_primitive, NativeResult, ToPrimitiveHint, VmHost};
use oxide_types::value::JsValue;

/// JS `BigInt(value)` 构造逻辑（ES2020 §20.2.1.1）：
/// - `new BigInt()` 抛 TypeError（BigInt 不可 new）
/// - 无参 → `0n`
/// - 其余参数先 ToPrimitive(number)，Number 走 NumberToBigInt，其余走 ToBigInt。
pub fn bigint_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() > 1 {
        let this_val = vm.reg(args[0]);
        if this_val.is_object() {
            // new 语义：BigInt 不可 new，抛 TypeError。
            return NativeResult::Err(crate::error::create_type_error(vm, "BigInt is not a constructor"));
        }
    }
    // 无参 BigInt() → 0n（注意 BigInt(undefined) 必须抛 TypeError）。
    if args.len() <= 1 {
        return NativeResult::Ok(vm.new_bigint(BigInt::from(0)));
    }
    let val = vm.reg(args[1]);
    // ToPrimitive(value, number)：对象先出盒，用户 @@toPrimitive/valueOf/toString
    // 抛出的异常要原样传播（只触发一次，见 constructor-coercion 测试）。
    let prim = match to_primitive(val, ToPrimitiveHint::Number, vm) {
        Ok(p) => p,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a BigInt"));
        }
    };
    // 步骤 3：Type(prim) 为 Number → NumberToBigInt。
    if prim.is_int() {
        return NativeResult::Ok(vm.new_bigint(BigInt::from(prim.as_int())));
    }
    if prim.is_double() {
        let d = prim.as_double();
        if d.is_nan() || d.is_infinite() || d.trunc() != d {
            return NativeResult::Err(crate::error::create_range_error(
                vm,
                "The number cannot be converted to a BigInt because it is not an integer",
            ));
        }
        return NativeResult::Ok(vm.new_bigint(BigInt::from(d.trunc() as i128)));
    }
    // 步骤 4：其它原始类型走 ToBigInt。
    match to_bigint(vm, prim) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(e),
    }
}

/// `BigInt.prototype.toString(radix)`：thisBigIntValue 后按 radix 输出。
pub fn bigint_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    let v = match this_bigint_value(vm, this_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let radix = if args.len() > 1 {
        let radix_arg = vm.reg(args[1]);
        if radix_arg.is_undefined() {
            10u32
        } else {
            // radix 走 ToIntegerOrInfinity：ToNumber 拒绝 Symbol/BigInt，
            // 对象先 ToPrimitive（用户异常原样传播），NaN/±0 归 0。
            let raw = match coerce_number_or_throw(vm, radix_arg) {
                Ok(n) => n,
                Err(e) => return NativeResult::Err(e),
            };
            let r = if raw.is_nan() || raw == 0.0 {
                0.0
            } else if raw.is_infinite() {
                raw
            } else {
                raw.trunc()
            };
            if !(2.0..=36.0).contains(&r) {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "toString() radix must be between 2 and 36",
                ));
            }
            r as u32
        }
    } else {
        10u32
    };
    let text = vm.bigint_value(v).to_str_radix(radix);
    NativeResult::Ok(vm.new_string(&text))
}

/// `BigInt.prototype.toLocaleString`：返回与 toString() 相同的十进制字符串。
pub fn bigint_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    bigint_to_string(vm, args)
}

/// `BigInt.prototype.valueOf`：返回包装的 BigInt 原始值。
pub fn bigint_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    match this_bigint_value(vm, this_val) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(e),
    }
}

/// `BigInt.asIntN(bits, bigint)`：ToIndex(bits) + ToBigInt(bigint)，
/// 结果按二进制补码截断为 bits 位有符号整数。
pub fn bigint_as_int_n<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bits = match to_index(vm, args) {
        Ok(b) => b,
        Err(e) => return NativeResult::Err(e),
    };
    let arg = bigint_arg(vm, args);
    let val = match to_bigint(vm, arg) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let v = vm.bigint_value(val);
    if bits == 0 {
        return NativeResult::Ok(vm.new_bigint(BigInt::from(0)));
    }
    let modulus = BigInt::from(1) << bits;
    let half = BigInt::from(1) << (bits - 1);
    let mut m = v % &modulus;
    if m.sign() == num_bigint::Sign::Minus {
        m += &modulus;
    }
    if m >= half {
        return NativeResult::Ok(vm.new_bigint(m - modulus));
    }
    NativeResult::Ok(vm.new_bigint(m))
}

/// `BigInt.asUintN(bits, bigint)`：按 `2^bits` 取模的无符号截断。
pub fn bigint_as_uint_n<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bits = match to_index(vm, args) {
        Ok(b) => b,
        Err(e) => return NativeResult::Err(e),
    };
    let arg = bigint_arg(vm, args);
    let val = match to_bigint(vm, arg) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    let v = vm.bigint_value(val);
    if bits == 0 {
        return NativeResult::Ok(vm.new_bigint(BigInt::from(0)));
    }
    let modulus = BigInt::from(1) << bits;
    let mut m = v % &modulus;
    if m.sign() == num_bigint::Sign::Minus {
        m += &modulus;
    }
    NativeResult::Ok(vm.new_bigint(m))
}

/// thisBigIntValue：BigInt 原样返回；包装对象解盒；其它类型 TypeError。
fn this_bigint_value<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<JsValue, JsValue> {
    if this_val.is_bigint() {
        return Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.proto().is_object() {
                let proto_ptr = obj.proto().as_js_object_ptr();
                let bigint_proto =
                    vm.session().builtin_world().bigint_proto.as_ptr() as *mut oxide_types::object::JsObject;
                if !proto_ptr.is_null() && std::ptr::eq(proto_ptr, bigint_proto) {
                    let v = obj.get_prop_at(0);
                    if v.is_bigint() {
                        return Ok(v);
                    }
                }
            }
        }
    }
    Err(crate::error::create_type_error(
        vm,
        "BigInt.prototype method called on incompatible receiver",
    ))
}

/// ToNumber 且拒绝 Symbol/BigInt（ToIntegerOrInfinity 的 ToNumber 语义）。
fn coerce_number_or_throw<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let prim = match to_primitive(value, ToPrimitiveHint::Number, vm) {
        Ok(p) => p,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
        }
    };
    if prim.is_symbol() || prim.is_bigint() {
        return Err(crate::error::create_type_error(vm, "Cannot convert a Symbol or BigInt value to a number"));
    }
    Ok(oxide_runtime_api::to_number(prim))
}

/// ToIndex（§7.1.17）：ToIntegerOrInfinity 后必须为有限且 `[0, 2^53-1]` 内的整数。
fn to_index<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<usize, JsValue> {
    // ToIndex 作用在第一个参数（bits）上；缺省为 undefined。
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let n = coerce_number_or_throw(vm, value)?;
    if n.is_nan() {
        return Ok(0);
    }
    // ToIntegerOrInfinity 先截断（-0.9 → -0，不越界）；再检查有限且 [0, 2^53-1]。
    let int = if n == 0.0 {
        0.0
    } else if n.is_infinite() {
        n
    } else {
        n.trunc()
    };
    if !(0.0..=9007199254740991.0).contains(&int) {
        return Err(crate::error::create_range_error(vm, "Invalid index"));
    }
    Ok(int as usize)
}

/// 取 asIntN/asUintN 的第二个参数；缺省为 undefined。
fn bigint_arg<H: VmHost>(vm: &mut H, args: &[u8]) -> JsValue {
    if args.len() > 2 {
        vm.reg(args[2])
    } else {
        JsValue::undefined()
    }
}

/// ToBigInt 抽象操作（§7.1.14）：BigInt 原样；String → StringToBigInt；
/// Boolean → 0/1；Number/Symbol/undefined/null → TypeError；对象先 ToPrimitive 再递归。
fn to_bigint<H: VmHost>(vm: &mut H, val: JsValue) -> Result<JsValue, JsValue> {
    if val.is_bigint() {
        return Ok(val);
    }
    if val.is_bool() {
        return Ok(vm.new_bigint(BigInt::from(if val.as_bool() { 1 } else { 0 })));
    }
    if val.is_int() {
        // ToBigInt(Number) 抛 TypeError（与构造器不同：构造器先 ToPrimitive 再
        // NumberToBigInt；ToBigInt 直接拒绝 Number）。
        return Err(crate::error::create_type_error(vm, "Cannot convert a Number value to a BigInt"));
    }
    if val.is_double() {
        return Err(crate::error::create_type_error(vm, "Cannot convert a Number value to a BigInt"));
    }
    if val.is_string() {
        let s = oxide_runtime_api::to_string(val);
        return match string_to_bigint(vm, &s) {
            Ok(v) => Ok(v),
            Err(e) => Err(e),
        };
    }
    if val.is_object() {
        let prim = match to_primitive(val, ToPrimitiveHint::Default, vm) {
            Ok(p) => p,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return Err(exc);
                }
                return Err(crate::error::create_type_error(vm, "Cannot convert value to a BigInt"));
            }
        };
        return to_bigint(vm, prim);
    }
    // undefined / null / symbol。
    Err(crate::error::create_type_error(vm, "Cannot convert value to a BigInt"))
}

/// StringToBigInt（§7.1.14 步骤）：去首尾空白，可选 +/- 号与 0x/0o/0b 前缀；
/// 空串 → 0n；非法 → SyntaxError。
fn string_to_bigint<H: VmHost>(vm: &mut H, s: &str) -> Result<JsValue, JsValue> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(vm.new_bigint(BigInt::from(0)));
    }
    let (neg, rest) = if let Some(r) = trimmed.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = trimmed.strip_prefix('+') {
        (false, r)
    } else {
        (false, trimmed)
    };
    // StringIntegerLiteral：符号只允许出现在纯十进制前；0x/0o/0b 前缀前带
    // +/-（如 "-0x1"）属于非法语法（test262 constructor-from-string-syntax-errors）。
    let (radix, digits) = if let Some(hex) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        if neg {
            return Err(crate::error::create_syntax_error(
                vm,
                "Cannot convert string to BigInt: invalid integer literal",
            ));
        }
        (16u32, hex)
    } else if let Some(oct) = rest.strip_prefix("0o").or_else(|| rest.strip_prefix("0O")) {
        if neg {
            return Err(crate::error::create_syntax_error(
                vm,
                "Cannot convert string to BigInt: invalid integer literal",
            ));
        }
        (8u32, oct)
    } else if let Some(bin) = rest.strip_prefix("0b").or_else(|| rest.strip_prefix("0B")) {
        if neg {
            return Err(crate::error::create_syntax_error(
                vm,
                "Cannot convert string to BigInt: invalid integer literal",
            ));
        }
        (2u32, bin)
    } else {
        (10u32, rest)
    };
    if digits.is_empty() {
        return Err(crate::error::create_syntax_error(
            vm,
            "Cannot convert string to BigInt: invalid integer literal",
        ));
    }
    let m = BigInt::parse_bytes(digits.as_bytes(), radix).ok_or_else(|| {
        crate::error::create_syntax_error(vm, "Cannot convert string to BigInt: invalid integer literal")
    })?;
    Ok(vm.new_bigint(if neg { -m } else { m }))
}
