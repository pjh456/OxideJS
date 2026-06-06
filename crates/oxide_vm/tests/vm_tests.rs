use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
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

fn eval_val(source: &str) -> JsValue {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    let mut vm = Vm::new();
    vm.run(&module).expect("vm run failed")
}

#[test]
fn object_create_and_read() {
    let obj = eval_val("({a:1})");
    assert!(obj.is_object());
    let obj_ref = unsafe { &*obj.as_js_object_ptr() };
    assert_eq!(obj_ref.prop_count(), 1, "object should have 1 property");
    assert!(obj_ref.get_inline_prop(0).is_int());
}

#[test]
fn bench_1m_property_reads() {
    use std::time::Instant;

    let allocator = Allocator::default();
    let source = "({a: 1, b: 2, c: 3, d: 4, e: 5}).e";
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");

    let mut vm = Vm::new();
    vm.run(&module).expect("vm run failed");
    for _ in 0..100 {
        vm.rerun().ok();
    }

    const N: usize = 1_000_000;
    let start = Instant::now();
    for _ in 0..N {
        vm.rerun().ok();
    }
    let elapsed = start.elapsed();
    let ns_per = elapsed.as_nanos() as f64 / N as f64;
    println!(
        "1M IC property reads: {:.2}ms ({:.0} ns/read)",
        elapsed.as_secs_f64() * 1000.0,
        ns_per
    );
    if cfg!(debug_assertions) {
        println!("  (debug build -- skipping timing assertion)");
    } else {
        assert!(ns_per < 500.0, "{} ns/read exceeds 500ns target", ns_per);
    }
}

#[test]
fn eval_arithmetic_add() {
    assert_eq!(eval("1 + 2"), "3");
}

#[test]
fn eval_arithmetic_sub() {
    assert_eq!(eval("5 - 3"), "2");
}

#[test]
fn eval_arithmetic_mul() {
    assert_eq!(eval("3 * 4"), "12");
}

#[test]
fn eval_arithmetic_div() {
    assert_eq!(eval("10 / 2"), "5");
}

#[test]
fn eval_arithmetic_mod() {
    assert_eq!(eval("7 % 3"), "1");
}

#[test]
fn eval_comparison_eq_true() {
    assert_eq!(eval("1 == 1"), "true");
}

#[test]
fn eval_comparison_eq_false() {
    assert_eq!(eval("1 == 2"), "false");
}

#[test]
fn eval_neq() {
    assert_eq!(eval("1 != 2"), "true");
}

#[test]
fn eval_comparison_lt() {
    assert_eq!(eval("3 < 4"), "true");
}

#[test]
fn eval_comparison_gt() {
    assert_eq!(eval("4 > 5"), "false");
}

#[test]
fn eval_comparison_lte() {
    assert_eq!(eval("3 <= 3"), "true");
}

#[test]
fn eval_comparison_gte() {
    assert_eq!(eval("5 >= 4"), "true");
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
fn eval_precedence() {
    assert_eq!(eval("1 * 2 + 3"), "5");
}

#[test]
fn eval_negation() {
    assert_eq!(eval("-5"), "-5");
}

#[test]
fn eval_division_by_zero() {
    assert_eq!(eval("1 / 0"), "Infinity");
}

#[test]
fn eval_multi_stmt_last_value() {
    assert_eq!(eval("1; 2; 3"), "3");
}

#[test]
fn eval_var_declaration() {
    assert_eq!(eval("var x = 42"), "42");
}

#[test]
fn eval_if_true() {
    assert_eq!(eval("if (true) { 1 } else { 2 }"), "1");
}

#[test]
fn eval_if_false() {
    assert_eq!(eval("if (false) { 1 } else { 2 }"), "2");
}

#[test]
fn eval_if_without_else_true() {
    assert_eq!(eval("if (true) { 42 }"), "42");
}

#[test]
fn eval_if_without_else_false() {
    assert_eq!(eval("if (false) { 42 }"), "undefined");
}

#[test]
fn eval_dangling_else() {
    assert_eq!(eval("if (true) { if (false) { 1 } else { 2 } }"), "2");
}

#[test]
fn eval_while_zero_iterations() {
    assert_eq!(eval("var x = 0; while (false) { x = 1; } x"), "0");
}

#[test]
fn eval_while_multi_iterations() {
    assert_eq!(eval("var i = 0; while (i < 3) { i = i + 1; } i"), "3");
}

#[test]
fn eval_while_result_undefined() {
    assert_eq!(eval("while (false) { 1; }"), "undefined");
}

#[test]
fn eval_for_basic() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 3; i = i + 1) { r = r + i; } r"),
        "3"
    );
}

#[test]
fn eval_for_no_test() {
    assert_eq!(
        eval("var r = 0; for (i = 0; ; i = i + 1) { if (i >= 3) { break; } r = r + i; } r"),
        "3"
    );
}

#[test]
fn eval_for_no_init_update() {
    assert_eq!(eval("var i = 0; for (; i < 3; ) { i = i + 1; } i"), "3");
}

#[test]
fn eval_ternary_true() {
    assert_eq!(eval("true ? 1 : 2"), "1");
}

#[test]
fn eval_ternary_false() {
    assert_eq!(eval("false ? 1 : 2"), "2");
}

#[test]
fn eval_ternary_nested() {
    assert_eq!(eval("true ? (false ? 1 : 2) : 3"), "2");
}

#[test]
fn eval_not_true() {
    assert_eq!(eval("!true"), "false");
}

#[test]
fn eval_not_false() {
    assert_eq!(eval("!false"), "true");
}

#[test]
fn eval_not_zero() {
    assert_eq!(eval("!0"), "true");
}

#[test]
fn eval_not_string() {
    assert_eq!(eval("!'hello'"), "false");
}

#[test]
fn eval_and_short_circuit() {
    assert_eq!(eval("var x = 0; false && (x = 5); x"), "0");
}

#[test]
fn eval_and_truthy() {
    assert_eq!(eval("1 && 2"), "2");
}

#[test]
fn eval_and_falsy_first() {
    assert_eq!(eval("0 && 2"), "0");
}

#[test]
fn eval_or_short_circuit() {
    assert_eq!(eval("var x = 0; true || (x = 5); x"), "0");
}

#[test]
fn eval_or_truthy_first() {
    assert_eq!(eval("1 || 2"), "1");
}

#[test]
fn eval_or_falsy_both() {
    assert_eq!(eval("0 || false"), "false");
}

#[test]
fn eval_break_in_while() {
    assert_eq!(
        eval("var i = 0; while (true) { i = i + 1; if (i >= 3) { break; } } i"),
        "3"
    );
}

#[test]
fn eval_continue_in_while() {
    assert_eq!(
        eval("var i = 0; var r = 0; while (i < 5) { i = i + 1; if (i == 3) { continue; } r = r + 1; } r"),
        "4"
    );
}

#[test]
fn eval_break_in_nested_loop() {
    assert_eq!(
        eval("var x = 0; while (true) { while (true) { x = x + 1; if (x >= 3) { break; } } break; } x"),
        "3"
    );
}

#[test]
fn eval_break_in_for() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 10; i = i + 1) { if (i >= 3) { break; } r = r + i; } r"),
        "3"
    );
}

#[test]
fn eval_control_flow_complex() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 5; i = i + 1) { if (i == 2) { continue; } r = r + i; } r"),
        "8"
    );
}
