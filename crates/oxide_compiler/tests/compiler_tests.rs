use oxide_compiler::compiler::Compiler;
use oxide_compiler::opcode::{self, OpCode};
use oxide_kernel::code_forge::CodeForge;
use oxide_parser::Allocator;

fn compile_source(source: &str) -> oxide_compiler::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

fn parse_to_hash(source: &str) -> u64 {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    oxide_compiler::compiler::structural_hash(&program)
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
fn structural_hash_hit() {
    let forge = CodeForge::new();
    let compiler = Compiler::new();
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, "1 + 2").expect("parse failed");

    let m1 = forge
        .get_or_compile(&program, &compiler)
        .expect("first compile");
    let m2 = forge
        .get_or_compile(&program, &compiler)
        .expect("second compile");

    assert_eq!(
        (m1.bytecode.len(), m1.n_registers),
        (m2.bytecode.len(), m2.n_registers)
    );
}

#[test]
fn structural_hash_same_shape() {
    let hash_a = parse_to_hash("var x = 1 + 2; var y = x;");
    let hash_b = parse_to_hash("var a = 3 + 4; var b = a;");
    assert_eq!(hash_a, hash_b);
}

#[test]
fn structural_hash_different_shape() {
    let hash_a = parse_to_hash("1 + 2");
    let hash_b = parse_to_hash("1 + 2 * 3");
    assert_ne!(hash_a, hash_b);
}

#[test]
fn compile_var_declaration() {
    let module = compile_source("var x = 42;");
    assert!(!module.bytecode.is_empty());
    assert_eq!(
        module.constants[0],
        oxide_compiler::compiler::Constant::Int(42)
    );
}

#[test]
fn compile_return_nothing() {
    let module = compile_source("function f() { return; }");
    let last = opcode::opcode(*module.bytecode.last().unwrap());
    assert_eq!(last, OpCode::HALT);
}

#[test]
fn compile_return_value() {
    let module = compile_source("function f() { return 42; }");
    assert!(
        !module.bytecode.is_empty(),
        "function declaration should produce bytecode"
    );
}

#[test]
fn compile_if_else_emits_jmp_if_false() {
    let module = compile_source("if (true) { 1 } else { 2 }");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "if/else should contain JMP_IF_FALSE"
    );
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::JMP),
        "if/else should contain JMP"
    );
}

#[test]
fn compile_while_emits_jump_back() {
    let module = compile_source("while (true) { 1 }");
    let jmp_ops: Vec<_> = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::JMP)
        .collect();
    assert!(!jmp_ops.is_empty(), "while should contain JMP (backward)");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "while should contain JMP_IF_FALSE"
    );
}

#[test]
fn compile_for_emits_jumps() {
    let module = compile_source("for (i=0; i<3; i=i+1) { 1 }");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE),
        "for should contain JMP_IF_FALSE"
    );
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
fn compile_break_in_loop() {
    let module = compile_source("while (true) { break; }");
    assert!(
        !module.bytecode.is_empty(),
        "break should compile without error"
    );
}

#[test]
fn compile_continue_in_loop() {
    let module = compile_source("while (true) { continue; }");
    assert!(
        !module.bytecode.is_empty(),
        "continue should compile without error"
    );
}

#[test]
fn compile_break_outside_loop_errors() {
    let result = std::panic::catch_unwind(|| {
        compile_source("break;");
    });
    assert!(result.is_err(), "break outside loop should error");
}

#[test]
fn compile_continue_outside_loop_errors() {
    let result = std::panic::catch_unwind(|| {
        compile_source("continue;");
    });
    assert!(result.is_err(), "continue outside loop should error");
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
fn compile_nested_if() {
    let module = compile_source("var a=1,b=0; if (a) { if (b) { 1 } else { 2 } }");
    let jmp_if_false_count = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE)
        .count();
    assert!(
        jmp_if_false_count >= 2,
        "nested if should have 2+ JMP_IF_FALSE"
    );
}

#[test]
fn compile_empty_while_body() {
    let module = compile_source("while (false) {}");
    assert!(!module.bytecode.is_empty(), "empty while should compile");
}
