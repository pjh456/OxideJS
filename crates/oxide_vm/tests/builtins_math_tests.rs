use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

#[test]
fn math_abs_negative() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.abs(-42)").unwrap();
    assert!((result.as_double() - 42.0).abs() < 0.0001);
}

#[test]
fn math_sqrt() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sqrt(16)").unwrap();
    assert!((result.as_double() - 4.0).abs() < 0.0001);
}

#[test]
fn math_pow() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.pow(2, 10)").unwrap();
    assert!((result.as_double() - 1024.0).abs() < 0.0001);
}

#[test]
fn math_ceil() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.ceil(3.14)").unwrap();
    assert!((result.as_double() - 4.0).abs() < 0.0001);
}

#[test]
fn math_floor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.floor(3.14)").unwrap();
    assert!((result.as_double() - 3.0).abs() < 0.0001);
}

#[test]
fn math_round() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.round(3.6)").unwrap();
    assert!((result.as_double() - 4.0).abs() < 0.0001);
}

#[test]
fn math_max() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max(1, 5)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn math_min() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.min(10, 5)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn math_sin() {
    let mut vm = Vm::new();
    eval(&mut vm, "Math.sin(0)").unwrap();
}

#[test]
fn math_cos() {
    let mut vm = Vm::new();
    eval(&mut vm, "Math.cos(0)").unwrap();
}

#[test]
fn math_random_in_range() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.random()").unwrap();
    assert!(result.as_double() >= 0.0);
    assert!(result.as_double() < 1.0);
}

#[test]
fn math_pi() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.PI").unwrap();
    assert!((result.as_double() - std::f64::consts::PI).abs() < 0.0001);
}

#[test]
fn math_sign_positive() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sign(42)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn math_sign_negative() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sign(-42)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn math_trunc() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.trunc(3.9)").unwrap();
    assert!((result.as_double() - 3.0).abs() < 0.0001);
}

#[test]
fn math_acosh() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.acosh(1)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_asinh() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.asinh(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_atanh() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.atanh(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_clz32() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.clz32(0)").unwrap();
    assert_eq!(result.as_int(), 32);
}

#[test]
fn math_expm1() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.expm1(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_fround() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.fround(1.5)").unwrap();
    assert!((result.as_double() - 1.5).abs() < 0.0001);
}

#[test]
fn math_log1p() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.log1p(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_constant_e() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.E").unwrap();
    assert!((result.as_double() - std::f64::consts::E).abs() < 0.0001);
}

#[test]
fn math_constant_ln10() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LN10").unwrap();
    assert!((result.as_double() - std::f64::consts::LN_10).abs() < 0.0001);
}

#[test]
fn math_constant_ln2() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LN2").unwrap();
    assert!((result.as_double() - std::f64::consts::LN_2).abs() < 0.0001);
}

#[test]
fn math_constant_log10e() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LOG10E").unwrap();
    assert!((result.as_double() - std::f64::consts::LOG10_E).abs() < 0.0001);
}

#[test]
fn math_constant_log2e() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LOG2E").unwrap();
    assert!((result.as_double() - std::f64::consts::LOG2_E).abs() < 0.0001);
}

#[test]
fn math_constant_sqrt1_2() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.SQRT1_2").unwrap();
    assert!((result.as_double() - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.0001);
}

#[test]
fn math_constant_sqrt2() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.SQRT2").unwrap();
    assert!((result.as_double() - std::f64::consts::SQRT_2).abs() < 0.0001);
}

#[test]
fn math_sum_precise_basic() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sumPrecise([1, 2, 3])").unwrap();
    assert!((result.as_double() - 6.0).abs() < 1e-9);

    // 精确和：f64 逐次累加 0.1+0.1 恰好得 0.2，此例钉位宽边界外的尾差路径。
    let result = eval(&mut vm, "Math.sumPrecise([0.1, 0.1])").unwrap();
    assert_eq!(result.as_double(), 0.2);

    // 大指数对精确抵消。
    let result = eval(&mut vm, "Math.sumPrecise([1e308, -1e308])").unwrap();
    assert_eq!(result.as_double(), 0.0);
}

#[test]
fn math_sum_precise_zero_sign() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object.is(Math.sumPrecise([]), -0)").unwrap();
    assert_eq!(result, JsValue::bool(true));

    let result = eval(&mut vm, "Object.is(Math.sumPrecise([-0]), -0)").unwrap();
    assert_eq!(result, JsValue::bool(true));

    // -0 后跟 +0：终态 finite，精确和 0 → +0。
    let result = eval(&mut vm, "Object.is(Math.sumPrecise([-0, 0]), 0)").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn math_sum_precise_nan_and_infinity() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isNaN(Math.sumPrecise([NaN]))").unwrap();
    assert_eq!(result, JsValue::bool(true));

    // 异号无穷互抵 → NaN。
    let result = eval(&mut vm, "Number.isNaN(Math.sumPrecise([Infinity, -Infinity]))").unwrap();
    assert_eq!(result, JsValue::bool(true));

    let result = eval(&mut vm, "Math.sumPrecise([Infinity, Infinity])").unwrap();
    assert_eq!(result.as_double(), f64::INFINITY);
}

#[test]
fn math_sum_precise_type_errors() {
    let mut vm = Vm::new();
    // RequireObjectCoercible 与不可迭代两种 TypeError 形态。
    let result = eval(&mut vm, "try { Math.sumPrecise() } catch (e) { e.name }").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "TypeError");

    let result = eval(&mut vm, "try { Math.sumPrecise({}) } catch (e) { e.name }").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "TypeError");

    // 非 Number 元素：抛 TypeError 且不触发任何强转（coercions 保持 0）。
    let result = eval(
        &mut vm,
        "(function () {
            var c = 0;
            var r = '';
            var o = { valueOf: function () { c++ } };
            try { Math.sumPrecise([o]) } catch (e) { r = e.name + ':' + c; }
            return r;
        })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "TypeError:0");
}

#[test]
fn math_sum_precise_closes_iterator_once() {
    let mut vm = Vm::new();
    // 元素非 Number：next 恰一次、return 恰一次（IteratorClose 序位）。
    let result = eval(
        &mut vm,
        "(function () {
            var next = 0, ret = 0;
            var iterable = {
                [Symbol.iterator]: function () {
                    return {
                        next: function () { next++; return { done: false, value: {} }; },
                        return: function () { ret++; return {}; }
                    };
                }
            };
            try { Math.sumPrecise(iterable) } catch (e) {}
            return next + ':' + ret;
        })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "1:1");
}

#[test]
fn math_sum_precise_descriptor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sumPrecise.length + '|' + Math.sumPrecise.name").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "1|sumPrecise");
}
