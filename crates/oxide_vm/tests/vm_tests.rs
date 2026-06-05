use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::value::JsValue;
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
    assert!(obj_ref.get_inline_prop(0).is_double());
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
