use std::process::Command;

fn run_eval(js: &str) -> std::process::Output {
    Command::new("cargo")
        .args(["run", "--", "eval", js])
        .output()
        .expect("failed to run oxide eval")
}

#[test]
fn cli_e2e_arithmetic_precedence() {
    let output = run_eval("1 + 2 * 3");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "7");
}

#[test]
fn cli_e2e_arithmetic_infinity() {
    let output = run_eval("1 / 0");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "Infinity");
}

#[test]
fn cli_e2e_comparison_gt() {
    let output = run_eval("5 > 3");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
}

#[test]
fn cli_e2e_comparison_nan() {
    let output = run_eval("0 == 1");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "false");
}

#[test]
fn cli_e2e_coercion_not_empty_string() {
    let output = run_eval("!''");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
}

#[test]
fn cli_e2e_coercion_empty_string_falsy() {
    let output = run_eval("if ('') { 1 } else { 2 }");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2");
}

#[test]
fn cli_e2e_var_binding_declaration() {
    let output = run_eval("var x = 1; var y = 2; x + y");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");
}

#[test]
fn cli_e2e_var_binding_auto_global() {
    let output = run_eval("x = 5; x");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "5");
}

#[test]
fn cli_e2e_control_flow_if_else() {
    let output = run_eval("if (true) { 1 } else { 2 }");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
}

#[test]
fn cli_e2e_control_flow_for_loop() {
    let output = run_eval("for (i=0; i<3; i=i+1) { } i");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");
}

#[test]
fn cli_e2e_logical_and_short_circuit() {
    let output = run_eval("var x=0; false && (x=5); x");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
}

#[test]
fn cli_e2e_logical_or_fallback() {
    let output = run_eval("0 || 42");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
}

#[test]
fn cli_e2e_object_property_read() {
    let output = run_eval("({a:1}).a");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
}

#[test]
fn cli_e2e_object_missing_property() {
    let output = run_eval("({a:1}).b");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "undefined");
}

#[test]
fn cli_e2e_syntax_error() {
    let output = run_eval("function(");
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).trim().is_empty());
}

#[test]
fn cli_e2e_control_flow_undefined_result() {
    let output = run_eval("while (false) { }");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "undefined");
}
