use oxide_compiler::cache::CodeCache;
use oxide_compiler::compiler::Compiler;
use oxide_compiler::opcode::{self, OpCode};
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
    oxide_compiler::cache::structural_hash(&program)
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
        oxide_compiler::compiler::Constant::Number(42.0)
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
fn structural_cache_hit() {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, "1 + 2").expect("parse failed");
    let compiler = Compiler::new();
    let mut cache = CodeCache::new();

    let m1 = cache
        .get_or_compile(&program, &compiler)
        .expect("compile failed");
    let m2 = cache
        .get_or_compile(&program, &compiler)
        .expect("compile failed");

    assert_eq!(m1.bytecode, m2.bytecode);
}

#[test]
fn structural_cache_same_shape() {
    let hash_a = parse_to_hash("var x = 1 + 2; var y = x;");
    let hash_b = parse_to_hash("var a = 3 + 4; var b = a;");
    assert_eq!(hash_a, hash_b);
}

#[test]
fn structural_cache_different_shape() {
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
        oxide_compiler::compiler::Constant::Number(42.0)
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
