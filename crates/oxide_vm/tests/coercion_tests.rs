use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_runtime_api as coercion;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn eval_coercion_null_equals_null() {
    assert_eq!(eval("null == null"), "true");
}

#[test]
fn eval_coercion_bool_equals_int() {
    assert_eq!(eval("false == 0"), "true");
}

#[test]
fn eval_not_falsy() {
    assert_eq!(eval("!0"), "true");
}

#[test]
fn test_same_value_signed_zero() {
    assert!(!coercion::same_value(JsValue::float(0.0), JsValue::float(-0.0)));
}

#[test]
fn test_strict_equality_signed_zero() {
    assert!(coercion::strict_equality(JsValue::float(0.0), JsValue::float(-0.0)));
}

#[test]
fn test_same_value_nan() {
    assert!(coercion::same_value(JsValue::float(f64::NAN), JsValue::float(f64::NAN)));
}

#[test]
fn test_same_value_type_mismatch() {
    assert!(!coercion::same_value(JsValue::int(1), JsValue::bool(true)));
}

#[test]
fn test_same_value_int_float_equal() {
    assert!(coercion::same_value(JsValue::int(1), JsValue::float(1.0)));
    assert!(!coercion::same_value(JsValue::int(1), JsValue::float(2.0)));
}

#[test]
fn test_strict_equality_nan() {
    assert!(!coercion::strict_equality(JsValue::float(f64::NAN), JsValue::float(f64::NAN)));
}

#[test]
fn test_strict_equality_type_mismatch() {
    assert!(!coercion::strict_equality(JsValue::int(1), JsValue::bool(true)));
}

#[test]
fn test_strict_equality_int_float_equal() {
    assert!(coercion::strict_equality(JsValue::int(1), JsValue::float(1.0)));
    assert!(!coercion::strict_equality(JsValue::int(1), JsValue::float(2.0)));
}

#[test]
fn test_strict_equality_null_undefined() {
    assert!(!coercion::strict_equality(JsValue::null(), JsValue::undefined()));
}

#[test]
fn test_to_int32_and_to_uint32() {
    let mut vm = Vm::new();
    let string_three = vm.new_string("3");
    let string_bad = vm.new_string("x");

    assert_eq!(coercion::to_int32(string_three), 3);
    assert_eq!(coercion::to_int32(string_bad), 0);
    assert_eq!(coercion::to_int32(JsValue::undefined()), 0);
    assert_eq!(coercion::to_int32(JsValue::float(1.9)), 1);
    assert_eq!(coercion::to_int32(JsValue::float(-1.9)), -1);
    assert_eq!(coercion::to_int32(JsValue::float(f64::NAN)), 0);
    assert_eq!(coercion::to_int32(JsValue::float(f64::INFINITY)), 0);
    assert_eq!(coercion::to_uint32(JsValue::int(-1)), 4_294_967_295);
}

#[test]
fn primitive_property_write_throws_clean_type_error() {
    for source in ["'abc'.x = 1", "(5).foo = 1", "true.foo = 1"] {
        let err = eval(source);
        assert!(err.contains("TypeError"), "expected TypeError for {source}, got {err}");
        assert!(
            !err.contains("IC_SET_PROP on non-object")
                && !err.contains("SET_PROP on non-object")
                && !err.contains("SET_PROP_DYNAMIC on non-object"),
            "unexpected internal opcode message for {source}: {err}"
        );
    }
}

// --- Phase 14.0.1: type-conversion infrastructure (ENG-13) ---
// SC-1..SC-8 from ROADMAP. eval() returns the Display of the final value;
// every assertion below targets a bool/number result to avoid string-quoting
// ambiguity.

// SC-1 ToPrimitive: boxed Number unwraps via valueOf (was "NaN" before).
#[test]
fn to_primitive_number_boxed() {
    assert_eq!(eval("+new Number(42)"), "42");
}

// SC-1 ToPrimitive: boxed String coerces to its primitive in == (object vs string).
#[test]
fn to_primitive_string_boxed() {
    assert_eq!(eval("new String('x') == 'x'"), "true");
}

// SC-1 ToPrimitive: obj[Symbol.toPrimitive] is consulted (callback runs, returns 7).
#[test]
fn to_primitive_symbol_hint_invoked() {
    assert_eq!(eval("var o = {}; o[Symbol.toPrimitive] = function (h) { return 7; }; +o"), "7");
}

// SC-2 ToNumber: to_number_full throws TypeError on a Symbol (per D-02).
// Note: the unary-plus operator path uses coerce_number_bounded (no symbol
// throw) so `+Symbol()` yields NaN at the operator level; see test below.
#[test]
fn to_number_full_symbol_throws() {
    let mut vm = Vm::new();
    let sym = JsValue::symbol(0);
    let result = coercion::to_number_full(sym, &mut vm);
    assert!(result.is_err(), "to_number_full(Symbol) must error, got {result:?}");
    assert!(result.unwrap_err().contains("Symbol"), "expected a Symbol TypeError message");
}

// SC-2/SC-7 operator-level reality: +Symbol is NaN (symbol-throw not wired into arithmetic).
#[test]
fn unary_plus_symbol_is_nan() {
    assert_eq!(eval("+Symbol('x')"), "NaN");
}

// SC-3 Abstract Equality: string vs number coerces.
#[test]
fn abstract_eq_string_number() {
    assert_eq!(eval("'1' == 1"), "true");
}

// SC-3 Abstract Equality: bool->number recursion + string->number.
#[test]
fn abstract_eq_bool_number() {
    assert_eq!(eval("'1' == true"), "true");
}

// SC-3 Abstract Equality: object vs number via ToPrimitive ([42] -> "42" -> 42).
#[test]
fn abstract_eq_object_primitive() {
    assert_eq!(eval("[42] == 42"), "true");
}

// SC-3 Abstract Equality: null == undefined (regression guard, spec step 2-3).
#[test]
fn abstract_eq_null_undefined() {
    assert_eq!(eval("null == undefined"), "true");
}

// SC-3 Abstract Equality: null == 0 is false (spec falls through to step 14).
#[test]
fn abstract_eq_null_zero_mismatch() {
    assert_eq!(eval("null == 0"), "false");
}

// SC-3 Abstract Equality: same-type delegates to strict equality.
#[test]
fn abstract_eq_same_type_string() {
    assert_eq!(eval("'abc' == 'abc'"), "true");
}

// SC-4 Strict Equality: +0 === -0 is true (verify-only; already correct).
#[test]
fn strict_eq_signed_zero_eval() {
    assert_eq!(eval("+0 === -0"), "true");
}

// SC-4 Strict Equality: NaN === NaN is false (verify-only; already correct).
#[test]
fn strict_eq_nan_eval() {
    assert_eq!(eval("NaN === NaN"), "false");
}

// SC-5 Arithmetic +: object operand coerces via valueOf.
#[test]
fn add_object_valueof_coercion() {
    assert_eq!(eval("var o = {valueOf: function () { return 42; }}; o + 1"), "43");
}

// SC-6 Relational: object operands coerce via ToPrimitive then compare as strings.
#[test]
fn relational_object_coercion() {
    assert_eq!(eval("[3] > [2]"), "true");
}

// SC-1/D-02 Date: Date object coerces to its timestamp in == (default hint -> valueOf).
#[test]
fn date_object_equals_timestamp() {
    assert_eq!(eval("new Date(0) == 0"), "true");
}
