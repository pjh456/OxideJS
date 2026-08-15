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

// --- 类型转换基础设施测试 ---
// eval() 返回最终值的 Display；以下断言都针对 bool/number 结果，
// 避免字符串引用的歧义。

// ToPrimitive：装箱 Number 经 valueOf 解包。
#[test]
fn to_primitive_number_boxed() {
    assert_eq!(eval("+new Number(42)"), "42");
}

// ToPrimitive：装箱 String 在 == 中强转为原始值（对象 vs 字符串）。
#[test]
fn to_primitive_string_boxed() {
    assert_eq!(eval("new String('x') == 'x'"), "true");
}

// ToPrimitive：优先调用 obj[Symbol.toPrimitive]（回调执行并返回 7）。
#[test]
fn to_primitive_symbol_hint_invoked() {
    assert_eq!(eval("var o = {}; o[Symbol.toPrimitive] = function (h) { return 7; }; +o"), "7");
}

// ToNumber：to_number_full 对 Symbol 抛 TypeError。
// 注意：一元加号路径用 coerce_number_bounded（对 Symbol 不抛），因此运算符层的
// +Symbol() 得到 NaN；见下个测试。
#[test]
fn to_number_full_symbol_throws() {
    let mut vm = Vm::new();
    let sym = JsValue::symbol(0);
    let result = coercion::to_number_full(sym, &mut vm);
    assert!(result.is_err(), "to_number_full(Symbol) must error, got {result:?}");
    assert!(result.unwrap_err().contains("Symbol"), "expected a Symbol TypeError message");
}

// 运算符层实况：+Symbol 为 NaN（Symbol 抛出未接入算术路径）。
#[test]
fn unary_plus_symbol_is_nan() {
    assert_eq!(eval("+Symbol('x')"), "NaN");
}

// 抽象相等：字符串与数字互相强转。
#[test]
fn abstract_eq_string_number() {
    assert_eq!(eval("'1' == 1"), "true");
}

// 抽象相等：布尔转数字的递归 + 字符串转数字。
#[test]
fn abstract_eq_bool_number() {
    assert_eq!(eval("'1' == true"), "true");
}

// 抽象相等：对象 vs 数字经 ToPrimitive（[42] -> "42" -> 42）。
#[test]
fn abstract_eq_object_primitive() {
    assert_eq!(eval("[42] == 42"), "true");
}

// 抽象相等：null == undefined（回归锚，规范第 2-3 步）。
#[test]
fn abstract_eq_null_undefined() {
    assert_eq!(eval("null == undefined"), "true");
}

// 抽象相等：null == 0 为 false（规范落到第 14 步）。
#[test]
fn abstract_eq_null_zero_mismatch() {
    assert_eq!(eval("null == 0"), "false");
}

// 抽象相等：同类型委托给严格相等。
#[test]
fn abstract_eq_same_type_string() {
    assert_eq!(eval("'abc' == 'abc'"), "true");
}

// 严格相等：+0 === -0 为 true（仅验证）。
#[test]
fn strict_eq_signed_zero_eval() {
    assert_eq!(eval("+0 === -0"), "true");
}

// 严格相等：NaN === NaN 为 false（仅验证）。
#[test]
fn strict_eq_nan_eval() {
    assert_eq!(eval("NaN === NaN"), "false");
}

// 严格相等：int 0 与 double -0.0 混合表示按数值语义相等（=== 的 ±0 修复）。
#[test]
fn strict_equality_int_zero_vs_float_neg_zero() {
    assert!(coercion::strict_equality(JsValue::int(0), JsValue::float(-0.0)));
    assert!(coercion::strict_equality(JsValue::int(0), JsValue::float(0.0)));
    assert!(!coercion::strict_equality(JsValue::int(1), JsValue::float(-0.0)));
}

// 严格相等：字面量 0（int）与 -0（double）跨表示 === 为 true。
#[test]
fn eval_int_zero_vs_neg_zero_strict() {
    assert_eq!(eval("0 === -0"), "true");
    assert_eq!(eval("-0 === -0"), "true");
    assert_eq!(eval("0 !== -0"), "false");
    assert_eq!(eval("42 === 42.0"), "true");
}

// 抽象相等：0 == -0 跨表示按数值相等。
#[test]
fn eval_int_zero_vs_neg_zero_loose() {
    assert_eq!(eval("0 == -0"), "true");
    assert_eq!(eval("0 != -0"), "false");
}

// SameValue：int 0 与 double -0.0 视为不同（Object.is 依赖）。
#[test]
fn same_value_int_zero_vs_float_neg_zero() {
    assert!(!coercion::same_value(JsValue::int(0), JsValue::float(-0.0)));
    assert!(coercion::same_value(JsValue::int(0), JsValue::float(0.0)));
}

// SameValueZero：int 0 与 double -0.0 视为相同（includes 依赖）。
#[test]
fn same_value_zero_int_zero_vs_float_neg_zero() {
    assert!(coercion::same_value_zero(JsValue::int(0), JsValue::float(-0.0)));
    assert!(coercion::same_value_zero(JsValue::float(f64::NAN), JsValue::float(f64::NAN)));
    assert!(coercion::same_value_zero(JsValue::float(f64::NAN), JsValue::float(-f64::NAN)));
    assert!(!coercion::same_value_zero(JsValue::int(1), JsValue::float(2.0)));
}

// eval 实况：Object.is / indexOf / includes 的 ±0 与 NaN 语义。
#[test]
fn eval_signed_zero_object_is_and_array_methods() {
    assert_eq!(eval("Object.is(0, -0)"), "false");
    assert_eq!(eval("Object.is(0, 0)"), "true");
    assert_eq!(eval("[0].indexOf(-0)"), "0");
    assert_eq!(eval("[0].includes(-0)"), "true");
    assert_eq!(eval("[NaN].indexOf(NaN)"), "-1");
    assert_eq!(eval("[NaN].includes(NaN)"), "true");
}

// typeof 结果断言需要真实字符串内容，单独用 (Vm, JsValue) 形态的辅助。
fn eval_ty(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&module)?;
    Ok((vm, result))
}

// typeof：js_type 分派 + perm_string 的结果与语言类型一一对应。
#[test]
fn eval_typeof_all_kinds() {
    let cases: &[(&str, &str)] = &[
        ("typeof 1", "number"),
        ("typeof 1.5", "number"),
        ("typeof NaN", "number"),
        ("typeof 's'", "string"),
        ("typeof true", "boolean"),
        ("typeof null", "object"),
        ("typeof undefined", "undefined"),
        ("typeof function(){}", "function"),
        ("typeof Symbol('x')", "symbol"),
        ("typeof 1n", "bigint"),
        ("typeof {}", "object"),
        ("typeof []", "object"),
    ];
    for (src, expected) in cases {
        let (vm, result) = eval_ty(src).unwrap_or_else(|e| panic!("{src} failed: {e}"));
        let actual = vm.lookup_str(result).unwrap_or_default();
        assert_eq!(actual, *expected, "{src} 的 typeof 结果不符");
    }
}

// 算术 +：对象操作数经 valueOf 强转。
#[test]
fn add_object_valueof_coercion() {
    assert_eq!(eval("var o = {valueOf: function () { return 42; }}; o + 1"), "43");
}

// 关系比较：对象操作数经 ToPrimitive 后按字符串比较。
#[test]
fn relational_object_coercion() {
    assert_eq!(eval("[3] > [2]"), "true");
}

// Date 对象在 == 中强转为自身时间戳（默认 hint -> valueOf）。
#[test]
fn date_object_equals_timestamp() {
    assert_eq!(eval("new Date(0) == 0"), "true");
}
