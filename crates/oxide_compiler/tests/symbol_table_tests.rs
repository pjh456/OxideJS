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
fn symbol_table_declare_and_lookup() {
    let module = compile_source("var x = 42;");
    assert!(!module.bytecode.is_empty());
    assert_eq!(module.constants[0], oxide_compiler::compiler::Constant::Int(42));
}

#[test]
fn symbol_table_nested_scopes() {
    let module = compile_source("var x = 1; if (true) { var y = 2; }");
    assert!(!module.bytecode.is_empty(), "nested scopes should compile");
    let jmp_count = module
        .bytecode
        .iter()
        .filter(|&&i| opcode::opcode(i) == OpCode::JMP_IF_FALSE)
        .count();
    assert!(jmp_count >= 1, "nested if should have JMP_IF_FALSE");
}

#[test]
fn symbol_table_tdz_global_shadow() {
    let result = std::panic::catch_unwind(|| {
        compile_source("var x = 1; { var x = x; }");
    });
    assert!(result.is_err(), "TDZ: accessing x in its own initializer inside a block should error");
}

#[test]
fn symbol_table_duplicate_var() {
    let result = std::panic::catch_unwind(|| {
        compile_source("var x = 1; var x = 2;");
    });
    assert!(
        result.is_err(),
        "duplicate var declaration in same scope should error (strict-mode semantics)"
    );
}

#[test]
fn symbol_table_undeclared_auto_global() {
    let module = compile_source("x = 5;");
    assert!(!module.bytecode.is_empty(), "undeclared assignment should auto-create global");
}
