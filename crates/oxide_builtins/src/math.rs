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
    // Sequenced (not a tuple literal) so the two coercions take &mut H one at a time.
    let a = if args.len() > 1 { num(vm, args[1]) } else { f64::NAN };
    let b = if args.len() > 2 { num(vm, args[2]) } else { f64::NAN };
    (a, b)
}

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

pub fn math_atan2<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::float(a.atan2(b)))
}

pub fn math_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let x = arg1(vm, args);
    let r = if x < 0.0 { (x - 0.5).ceil() } else { (x + 0.5).floor() };
    NativeResult::Ok(JsValue::float(r))
}

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

pub fn math_clz32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = arg1(vm, args) as u32;
    NativeResult::Ok(JsValue::int(n.leading_zeros() as i32))
}

pub fn math_fround<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::float(arg1(vm, args) as f32 as f64))
}

pub fn math_hypot<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::float(a.hypot(b)))
}

pub fn math_imul<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::int((a as i32).wrapping_mul(b as i32)))
}

pub fn math_pow<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (a, b) = arg2(vm, args);
    NativeResult::Ok(JsValue::float(a.powf(b)))
}

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

pub fn math_random<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    vm.step_rng();
    NativeResult::Ok(JsValue::float(vm.math_rng_value()))
}
