use oxide_compiler::compiler::Compiler;
use oxide_compiler::opcode::{self, OpCode};
use oxide_parser::Allocator;

fn compile_source(source: &str) -> oxide_compiler::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

#[test]
fn compile_simple_expr() {
    let module = compile_source("1 + 2");

    assert!(!module.bytecode.is_empty());
    assert!(module.bytecode.len() >= 4);

    let last = opcode::opcode(*module.bytecode.last().unwrap());
    assert_eq!(last, OpCode::HALT);
}

#[test]
fn compile_constants() {
    let module = compile_source("42");

    assert!(!module.constants.is_empty());
    assert_eq!(
        module.constants[0],
        oxide_compiler::compiler::Constant::Int(42)
    );
}

#[test]
fn compile_negation() {
    let module = compile_source("-5");

    let has_neg = module
        .bytecode
        .iter()
        .any(|&i| opcode::opcode(i) == OpCode::NEG);
    assert!(has_neg);
}

#[test]
fn compile_multiple_stmts() {
    let module = compile_source("1; 2; 3;");

    let load_count = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::LOAD_CONST)
        .count();
    assert!(load_count >= 3);
}

#[test]
fn compile_binary_ops() {
    let tests = [
        ("3 * 4", OpCode::MUL),
        ("10 / 2", OpCode::DIV),
        ("7 % 3", OpCode::MOD),
        ("5 - 2", OpCode::SUB),
    ];

    for (src, expected_op) in tests {
        let module = compile_source(src);
        let has_op = module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == expected_op);
        assert!(has_op, "expected {:?} in '{src}'", expected_op);
    }
}

#[test]
fn compile_ternary_emits_jumps() {
    let module = compile_source("true ? 1 : 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "ternary should contain JMP_IF_FALSE"
    );
}

#[test]
fn compile_logical_not() {
    let module = compile_source("!true");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::NOT),
        "!true should emit NOT opcode"
    );
}

#[test]
fn compile_logical_and_simple() {
    let module = compile_source("1 && 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::AND),
        "simple && should emit AND opcode"
    );
}

#[test]
fn compile_logical_or_simple() {
    let module = compile_source("0 || 1");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::OR),
        "simple || should emit OR opcode"
    );
}

#[test]
fn regression_coalesce_consistency() {
    let result = std::panic::catch_unwind(|| {
        compile_source("a ?? b");
    });
    assert!(
        result.is_err(),
        "a ?? b should produce an error (not silently misbehave)"
    );
}

#[test]
fn compile_strict_eq() {
    let module = compile_source("1 === 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::STRICT_EQ),
        "1 === 2 should emit STRICT_EQ opcode"
    );
}

#[test]
fn compile_strict_neq() {
    let module = compile_source("1 !== 2");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::STRICT_NEQ),
        "1 !== 2 should emit STRICT_NEQ opcode"
    );
}

#[test]
fn compile_unary_plus() {
    let module = compile_source("+'hello'");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::UNARY_PLUS),
        "+'hello' should emit UNARY_PLUS opcode"
    );
}

#[test]
fn compile_typeof_strict_eq() {
    let module = compile_source("typeof 42 === 'number'");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::TYPEOF),
        "should emit TYPEOF opcode"
    );
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::STRICT_EQ),
        "should emit STRICT_EQ opcode"
    );
}

#[test]
fn compile_strict_eq_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("1 === 2");
    });
    assert!(result.is_ok(), "1 === 2 should compile without error");
}

#[test]
fn compile_unary_plus_no_error() {
    let result = std::panic::catch_unwind(|| {
        compile_source("+'hello'");
    });
    assert!(result.is_ok(), "+'hello' should compile without error");
}
