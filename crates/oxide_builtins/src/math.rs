use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

fn num<H: VmHost>(vm: &mut H, reg: u8) -> f64 {
    vm.coerce_number_bounded(vm.reg(reg)).unwrap_or(f64::NAN)
}

fn arg1<H: VmHost>(vm: &mut H, args: &[u8]) -> f64 {
    if args.len() < 2 {
        f64::NAN
    } else {
        num(vm, args[1])
    }
}

fn arg2<H: VmHost>(vm: &mut H, args: &[u8]) -> (f64, f64) {
    // 顺序求值（非元组字面量）：两次 coercion 各自单独借用 &mut H。
    let a = if args.len() > 1 { num(vm, args[1]) } else { f64::NAN };
    let b = if args.len() > 2 { num(vm, args[2]) } else { f64::NAN };
    (a, b)
}

/// `Math.abs`：返回参数的绝对值。int 参数走 int 运算，其余按 double 返回。
pub fn math_abs<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let x = vm.reg(args[1]);
    if x.is_int() {
        NativeResult::Ok(JsValue::int(x.as_int().abs()))
    } else {
        NativeResult::Ok(JsValue::float(num(vm, args[1]).abs()))
    }
}

macro_rules! math_unary {
    ($name:ident, $op:ident) => {
        /// 一元数学函数（`Math.acos` 等）：对首个参数做 Rust 同名浮点运算，
        /// 返回 double 结果；缺参或不可转数字时按 NaN 处理。
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            NativeResult::Ok(JsValue::float(arg1(vm, args).$op()))
        }
    };
}

math_unary!(math_acos, acos);
math_unary!(math_acosh, acosh);
math_unary!(math_asin, asin);
math_unary!(math_asinh, asinh);
math_unary!(math_atan, atan);
math_unary!(math_atanh, atanh);
math_unary!(math_cbrt, cbrt);
math_unary!(math_ceil, ceil);
math_unary!(math_cos, cos);
math_unary!(math_cosh, cosh);
math_unary!(math_exp, exp);
math_unary!(math_expm1, exp_m1);
math_unary!(math_floor, floor);
math_unary!(math_log, ln);
math_unary!(math_log10, log10);
math_unary!(math_log1p, ln_1p);
math_unary!(math_log2, log2);
math_unary!(math_sin, sin);
math_unary!(math_sinh, sinh);
math_unary!(math_sqrt, sqrt);
math_unary!(math_tan, tan);
math_unary!(math_tanh, tanh);
math_unary!(math_trunc, trunc);

/// `Math.atan2(y, x)`：返回 y/x 的反正切角（弧度）。
pub fn math_atan2<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::float(a.atan2(b)))
}

/// `Math.round`：四舍五入到最接近的整数，.5 时向正无穷取整（JS 语义）。
pub fn math_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let x = arg1(vm, args);
    let r = if x < 0.0 { (x - 0.5).ceil() } else { (x + 0.5).floor() };
    NativeResult::Ok(JsValue::float(r))
}

/// `Math.sign`：返回 1 / -1 / 0 / -0 / NaN。
pub fn math_sign<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let x = arg1(vm, args);
    if x.is_nan() {
        NativeResult::Ok(JsValue::float(f64::NAN))
    } else if x > 0.0 {
        NativeResult::Ok(JsValue::int(1))
    } else if x < 0.0 {
        NativeResult::Ok(JsValue::int(-1))
    } else {
        NativeResult::Ok(JsValue::float(x))
    }
}

/// `Math.clz32`：返回 32 位无符号整数表示的前导零个数。
pub fn math_clz32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = arg1(vm, args) as u32;
    NativeResult::Ok(JsValue::int(n.leading_zeros() as i32))
}

/// `Math.fround`：把 double 舍入到 float32 精度再还原为 double。
pub fn math_fround<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::float(arg1(vm, args) as f32 as f64))
}

/// IEEE 754 binary16（float16）舍入：f64 → binary16（round-to-nearest-even）→ 还原为 f64。
///
/// # 步骤
/// 1. 拆 f64 位（符号 / 指数 / 尾数），特殊值（±0 / 无穷 / NaN）直接保位
/// 2. 正规 f16 路径：53 位有效位收缩到 11 位（丢 42 位）做 RNE，进位溢出则指数 +1
/// 3. 次正规 f16 路径：指数 < -14 时右移舍入到 10 位尾数（步长 2⁻²⁴），
///    溢出进位到最小正规 2⁻¹⁴；指数 < -25 时塌缩到 ±0
///
/// # 边界与前提
/// - RNE 半位判定：丢弃部分 > 半位进位；== 半位且保留位末位为 1 进位（round-half-even）
/// - 尾数进位溢出到指数后若超出 f16 指数上限 15 → ±Infinity（与规范溢出点一致）
/// - 输出直接以 f16 的 u16 位模式给出，供 Math.f16round 还原为 f64
///   或 DataView setFloat16 写缓冲复用
pub(crate) fn f64_to_f16_bits(x: f64) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 63) & 1) as u16;
    let exp = ((bits >> 52) & 0x7FF) as i32;
    let mant = bits & 0xF_FFFF_FFFF_FFFF;

    // 零与次正规 f64（exp==0）：粒度不足 binary16，直接塌缩到 ±0。
    if exp == 0 {
        return sign << 15;
    }
    // 无穷 / NaN：exp 字段全 1，NaN 尾数置非零位（保持 NaN 语义）。
    if exp == 0x7FF {
        let mant16 = if mant == 0 { 0 } else { 0x200 };
        return (sign << 15) | 0x7C00 | mant16;
    }

    let e = exp - 1023; // 真实二进制指数（含隐含位）。
    let significand = (1u64 << 52) | mant; // 53 位有效位。

    if e > 15 {
        // 值 ≥ 2¹⁶：必大于溢出舍入阈值（65520 起进 Inf），直接返回 ±Infinity。
        return (sign << 15) | 0x7C00;
    }
    if e >= -14 {
        // 正规 f16：11 位有效位，收缩 42 位做 RNE。
        let shift = 42;
        let dropped = significand & ((1u64 << shift) - 1);
        let half = 1u64 << (shift - 1);
        let mut q = significand >> shift;
        if dropped > half || (dropped == half && (q & 1) == 1) {
            q += 1;
        }
        if q == (1u64 << 11) {
            // 11 位有效位溢出进位到指数；指数再溢出 → Infinity。
            let e16 = e + 1;
            if e16 > 15 {
                return (sign << 15) | 0x7C00;
            }
            return (sign << 15) | (((e16 + 15) as u16) << 10);
        }
        return (sign << 15) | (((e + 15) as u16) << 10) | ((q & 0x3FF) as u16);
    }
    if e < -25 {
        // 小于最小次正规一半（2⁻²⁵）→ RNE 塌缩到 ±0。
        return sign << 15;
    }
    // 次正规 f16：值 = 10 位尾数 × 2⁻²⁴，右移 (28 - e) 位做 RNE。
    let shift = (28 - e) as u32;
    let dropped = significand & ((1u64 << shift) - 1);
    let half = 1u64 << (shift - 1);
    let mut q = significand >> shift;
    if dropped > half || (dropped == half && (q & 1) == 1) {
        q += 1;
    }
    // q 进位到 1024 → 最小正规 2⁻¹⁴（exp 字段 1，尾数 0）；否则次正规（exp 字段 0）。
    if q == 1024 {
        return (sign << 15) | (1 << 10);
    }
    (sign << 15) | (q as u16)
}

/// IEEE 754 binary16 → f64 精确还原（f16 有效位 ≤ 11 bit，f64 全可精确表示）。
/// 供 Math.f16round 与 DataView getFloat16 复用。
pub(crate) fn f16_bits_to_f64(h: u16) -> f64 {
    let sign = ((h >> 15) & 1) as f64;
    let exp = ((h >> 10) & 0x1F) as i32;
    let mant = (h & 0x3FF) as f64;
    let v = if exp == 0 {
        // 零与次正规：mant × 2⁻²⁴。
        mant * 2.0f64.powi(-24)
    } else if exp == 0x1F {
        if mant == 0.0 {
            return if sign == 1.0 { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        return f64::NAN;
    } else {
        // 正规：(1 + mant/2¹⁰) × 2^(exp-15)。
        (1.0 + mant / 1024.0) * 2.0f64.powi(exp - 15)
    };
    if sign == 1.0 {
        -v
    } else {
        v
    }
}

/// `Math.f16round`：把 double 舍入到 binary16 精度再还原为 double。
pub fn math_f16round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::float(f16_bits_to_f64(f64_to_f16_bits(arg1(vm, args)))))
}

/// `Math.hypot`：返回 sqrt(a²+b²)（当前仅支持两个参数）。
pub fn math_hypot<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::float(a.hypot(b)))
}

/// `Math.imul`：按 32 位整数做 wrap-around 乘法。
pub fn math_imul<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::int((a as i32).wrapping_mul(b as i32)))
}

/// `Math.pow(base, exp)`：返回幂运算结果。
pub fn math_pow<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::float(a.powf(b)))
}

/// `Math.max`：返回参数中的最大值；任一无参时返回 -Infinity，任一 NaN 时返回 NaN。
pub fn math_max<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NEG_INFINITY));
    }
    let mut m = f64::NEG_INFINITY;
    for &r in args.iter().skip(1) {
        let x = num(vm, r);
        if x.is_nan() {
            return NativeResult::Ok(JsValue::float(f64::NAN));
        }
        if x > m {
            m = x;
        }
    }
    NativeResult::Ok(JsValue::float(m))
}

/// `Math.min`：返回参数中的最小值；无参时返回 Infinity，任一 NaN 时返回 NaN。
pub fn math_min<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::INFINITY));
    }
    let mut m = f64::INFINITY;
    for &r in args.iter().skip(1) {
        let x = num(vm, r);
        if x.is_nan() {
            return NativeResult::Ok(JsValue::float(f64::NAN));
        }
        if x < m {
            m = x;
        }
    }
    NativeResult::Ok(JsValue::float(m))
}

/// `Math.random`：推进 RNG 并返回 [0, 1) 的伪随机浮点数。
pub fn math_random<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    vm.step_rng();
    NativeResult::Ok(JsValue::float(vm.math_rng_value()))
}
