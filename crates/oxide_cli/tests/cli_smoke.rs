use std::process::Command;

#[test]
fn eval_simple_expression() {
    let output = Command::new("cargo")
        .args(["run", "--", "eval", "1 + 2"])
        .output()
        .expect("failed to run oxide eval");

    assert!(output.status.success(), "eval 1+2 should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("HALT"),
        "output should contain HALT bytecode: {stdout}"
    );
}

#[test]
fn eval_syntax_error() {
    let output = Command::new("cargo")
        .args(["run", "--", "eval", "function("])
        .output()
        .expect("failed to run oxide eval");

    assert!(
        !output.status.success(),
        "syntax error should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.is_empty(),
        "syntax error should produce stderr output"
    );
}

#[test]
fn run_file() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello.js");
    let output = Command::new("cargo")
        .args(["run", "--", "run", fixture])
        .output()
        .expect("failed to run oxide run");

    assert!(output.status.success(), "run hello.js should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("HALT"),
        "run output should contain bytecode: {stdout}"
    );
}

#[test]
fn bench_not_implemented() {
    let output = Command::new("cargo")
        .args(["run", "--", "bench"])
        .output()
        .expect("failed to run oxide bench");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not yet implemented"),
        "bench should print not-implemented message"
    );
}
