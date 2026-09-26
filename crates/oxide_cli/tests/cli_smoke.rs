use std::process::Command;

#[test]
fn eval_simple_expression() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["eval", "1 + 2"])
        .output()
        .expect("failed to run oxide eval");

    assert!(output.status.success(), "eval 1+2 should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim() == "3", "output should be '3': {stdout}");
}

#[test]
fn eval_syntax_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["eval", "function("])
        .output()
        .expect("failed to run oxide eval");

    assert!(!output.status.success(), "syntax error should exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.is_empty(), "syntax error should produce stderr output");
}

#[test]
fn run_file() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello.js");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["run", fixture])
        .output()
        .expect("failed to run oxide run");

    assert!(output.status.success(), "run hello.js should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim() == "3", "run output should be '3': {stdout}");
}

#[test]
fn bench_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["bench", "--help"])
        .output()
        .expect("failed to run oxide bench --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--iterations"), "bench --help should list --iterations flag");
}

#[test]
fn eval_trace_flag_pc_lines() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["eval", "--trace", "1 + 2"])
        .output()
        .expect("failed to run oxide eval --trace");

    assert!(output.status.success(), "eval --trace should exit 0");
    // trace 行只走 stderr，stdout 保持纯结果。
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "3", "--trace 不应污染 stdout: {stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.lines().all(|l| l.starts_with("pc=")), "trace 行应全部为 pc 行: {stderr}");
    assert!(stderr.contains("LOAD_CONST"), "trace 应含 LOAD_CONST: {stderr}");
    assert!(stderr.contains("ADD"), "trace 应含 ADD: {stderr}");
    assert!(stderr.contains("HALT"), "trace 应含 HALT: {stderr}");
}

#[test]
fn eval_no_trace_flag_no_pc_lines() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["eval", "1 + 2"])
        .output()
        .expect("failed to run oxide eval");

    assert!(output.status.success(), "eval should exit 0");
    // 默认路径零 trace 输出。
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("pc="), "默认路径不应有 trace 行: {stderr}");
}
