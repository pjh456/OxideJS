use num_bigint::BigInt;
use num_traits::{FromPrimitive, One, Signed, ToPrimitive, Zero};
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

// ── Math.sumPrecise：精确数学和 ──

/// sumPrecise 累加状态机的五态（规范 20.1.3.21）。
#[derive(Clone, Copy, PartialEq)]
enum SumState {
    MinusZero,
    Finite,
    PlusInf,
    MinusInf,
    NotANumber,
}

/// 有限 f64 → 精确整数 `x·2^1074`（正常数 `(2^52+mant)·2^(e_f+1022)`、次正规 `mant`），
/// 供多个项在整数域无损失累加。
///
/// # 边界与前提
/// - 输入保证有限（±∞/NaN 由调用方状态机先行拦截）。
/// - 位宽上限 2100 位，单次 BigInt 构造代价可忽略。
fn finite_to_bigint(x: f64) -> BigInt {
    let bits = x.to_bits();
    let sign = (bits >> 63) == 1;
    let exp = (bits >> 52) & 0x7FF;
    let mant = bits & 0xF_FFFF_FFFF_FFFF;
    let mag = if exp == 0 {
        // 次正规：|x| = mant · 2^-1074，整数即尾数本身。
        BigInt::from_u64(mant).expect("u64 必可转 BigInt")
    } else {
        // 正常数：(1.mant) · 2^e_f = (2^52+mant) · 2^(e_f-52)。
        let e_f = exp as i64 - 1023;
        let significand = BigInt::from_u64((1u64 << 52) | mant).expect("u64 必可转 BigInt");
        significand << (e_f + 1022) as usize
    };
    if sign {
        -mag
    } else {
        mag
    }
}

/// 精确整数和 `S`（每项 `x·2^1074` 之和）→ f64：对 `S·2^-1074` 的一次 RNE 舍入。
///
/// # 边界与前提
/// - 零和 → +0（符号面由状态机 minus-zero 臂负责，此处不产生 -0）。
/// - p < 52：次正规区，`S` 全 52 位内，精确无舍入。
/// - 52 ≤ p ≤ 2097：高 53 位 RNE（半位进位按 tie-even），进位溢出指数 +1。
/// - p > 2097 或进位后指数超 1023：±Infinity。
fn exact_sum_to_f64(s: &BigInt) -> f64 {
    if s.is_zero() {
        return 0.0;
    }
    let sign_bit = if s.is_negative() { 1u64 << 63 } else { 0 };
    let a = s.abs();

    // p = 最高位 0-based 下标（顶段非零位数加低位段整段宽度）。
    let (_, digits) = a.to_u64_digits();
    let top = digits.last().expect("非零 BigInt 至少一段");
    let p = 64 * (digits.len() - 1) + 64 - top.leading_zeros() as usize - 1;

    // 溢出面：超过最大 f64 的 2^1074 刻度（p = 2097）。
    if p > 2097 {
        return if s.is_negative() { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if p < 52 {
        // 次正规 f64 尾数字段 = S 本体（值 = 尾数字段 · 2^-1074），S < 2^52 必入 u64，
        // 精确无舍入还原。
        return f64::from_bits(sign_bit | a.to_u64().expect("次正规和必入 u64"));
    }
    // p ≥ 52：高 53 位 keep + 余位 drop 做 RNE。
    let shift = p - 52;
    let mut keep = a.clone() >> shift;
    if shift > 0 {
        let drop = a & ((BigInt::one() << shift) - 1);
        let half = BigInt::one() << (shift - 1);
        // tie-even：keep 末位为 1 时进位（keep 至多 53 位，必入 u64）。
        let keep_odd = (keep.to_u64().expect("53 位必入 u64") & 1) == 1;
        if drop > half || (drop == half && keep_odd) {
            keep += 1;
        }
    }
    let mut e = p as i64 - 1074;
    if keep == (BigInt::one() << 53) {
        // 进位溢出 53 位：归一化（指数 +1），再超 f64 指数上限即溢出。
        keep >>= 1;
        e += 1;
    }
    if e > 1023 {
        return if s.is_negative() { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let mant: BigInt = keep - (BigInt::one() << 52);
    f64::from_bits(sign_bit | ((e + 1023) as u64) << 52 | mant.to_u64().expect("52 位尾数必入 u64"))
}

/// `Math.sumPrecise(items)`：逐项迭代 `items` 求有限项的精确数学和，
/// 终态一次 RNE 舍入为 double 返回（非逐次 f64 累加）。
///
/// # 步骤
/// 1. `items` 为 null/undefined → TypeError（RequireObjectCoercible）。
/// 2. GetIterator + GetIteratorDirect，循环 IteratorStepValue 取元素。
/// 3. 元素数 ≥ 2^53 → RangeError；元素非 Number → TypeError；两路均先
///    IteratorClose 再传播原异常（close 期间 return 抛错时原异常胜出）。
/// 4. 五态状态机驱动：NaN → not-a-number 停摆；±∞ 按异号互抵规则翻转；
///    有限项非 -0 且状态为 minus-zero/finite 时转 finite 并整数域累加。
/// 5. done 后按终态映射返回值。
///
/// # 边界与前提
/// - 元素判定只走 `is_int() || is_double()`，绝不做 ToNumber（带 valueOf/
///   toString 的对象不触发任何强转）。
/// - -0 元素不改状态、不参与累加；全 -0 列表返回 -0。
/// - int 臂精确入 f64（i32 范围内双表示无损）。
pub fn math_sum_precise<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let items = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if items.is_null() || items.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert undefined or null to object"));
    }
    // GetIterator（包装器带 next/return），再 GetIteratorDirect 缓存 next。
    let iterator = match crate::iterator::make_iterator_for_value(vm, items) {
        Ok(it) => it,
        Err(err) => return NativeResult::Err(err),
    };
    let (iterated, next) = match crate::iterator::get_iterator_direct(vm, iterator) {
        Ok(pair) => pair,
        Err(err) => return NativeResult::Err(err),
    };

    let mut state = SumState::MinusZero;
    let mut sum = BigInt::zero();
    let mut count = 0u64;
    loop {
        let elem = match crate::iterator::iterator_record_step(vm, next, iterated) {
            Ok(Some(v)) => v,
            Ok(None) => {
                // done：终态映射。
                let result = match state {
                    SumState::NotANumber => f64::NAN,
                    SumState::PlusInf => f64::INFINITY,
                    SumState::MinusInf => f64::NEG_INFINITY,
                    SumState::MinusZero => -0.0,
                    SumState::Finite => exact_sum_to_f64(&sum),
                };
                return NativeResult::Ok(JsValue::float(result));
            }
            Err(err) => {
                // 迭代器自身抛错：先关再传播原异常。
                let _ = crate::iterator::iterator_close_record(vm, iterated, Some(err));
                return NativeResult::Err(err);
            }
        };
        count += 1;
        if count >= 1u64 << 53 {
            let err = crate::error::create_range_error(vm, "too many elements");
            let _ = crate::iterator::iterator_close_record(vm, iterated, Some(err));
            return NativeResult::Err(err);
        }
        // 元素类型面：非 Number 先关迭代器再抛 TypeError。
        if !elem.is_int() && !elem.is_double() {
            let err = crate::error::create_type_error(vm, "Math.sumPrecise requires Number elements");
            let _ = crate::iterator::iterator_close_record(vm, iterated, Some(err));
            return NativeResult::Err(err);
        }
        let x = if elem.is_int() { elem.as_int() as f64 } else { elem.as_double() };
        // 五态推进：-0 元素判据须显式符号臂（Rust 中 -0.0 == 0.0 为真）。
        if x.is_nan() {
            state = SumState::NotANumber;
        } else if x == f64::INFINITY {
            state = if state == SumState::MinusInf {
                SumState::NotANumber
            } else {
                SumState::PlusInf
            };
        } else if x == f64::NEG_INFINITY {
            state = if state == SumState::PlusInf {
                SumState::NotANumber
            } else {
                SumState::MinusInf
            };
        } else {
            let is_minus_zero = x == 0.0 && x.is_sign_negative();
            if !is_minus_zero && (state == SumState::MinusZero || state == SumState::Finite) {
                state = SumState::Finite;
                sum += finite_to_bigint(x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_to_bigint_roundtrip() {
        // 往返对：常见值 / 极值 / 最小最大次正规。
        for x in [0.1f64, 1e308, -1e308, 5e-324, 2.2250738585072014e-308, f64::MAX, -0.5] {
            assert_eq!(exact_sum_to_f64(&finite_to_bigint(x)), x);
        }
    }

    #[test]
    fn exact_sum_to_f64_boundaries() {
        // 零和符号面与 f64 最大值/溢出边界。
        assert_eq!(exact_sum_to_f64(&BigInt::zero()), 0.0);
        assert!(exact_sum_to_f64(&BigInt::zero()).is_sign_positive());
        // ±f64::MAX 的 2^1074 刻度恰在可表示边界（p = 2097），±2× 溢出。
        let max = finite_to_bigint(f64::MAX);
        assert_eq!(exact_sum_to_f64(&max), f64::MAX);
        assert_eq!(exact_sum_to_f64(&(BigInt::zero() - &max)), -f64::MAX);
        assert_eq!(exact_sum_to_f64(&(max.clone() + &max)), f64::INFINITY);
        assert_eq!(exact_sum_to_f64(&(-max.clone() - &max)), f64::NEG_INFINITY);

        // sum.js 代表性向量：大指数对抵消后的精确尾差。
        let mut sum = BigInt::zero();
        for x in [1e308f64, 1e308, 0.1, 0.1, 1e30, 0.1, -1e30, -1e308, -1e308] {
            sum += finite_to_bigint(x);
        }
        assert_eq!(exact_sum_to_f64(&sum), 0.30000000000000004);
    }
}
