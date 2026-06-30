use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;

fn compile_source(source: &str) -> CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

#[test]
fn symbol_table_declare_and_lookup() {
    let module = compile_source("var x = 42;");
    assert!(!module.bytecode.is_empty());
    assert_eq!(module.constants[0], Constant::Int(42));
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
fn symbol_table_var_self_init_in_block_is_legal() {
    // `var` has no temporal dead zone and hoists to the function scope, so a block-level
    // `var x = x` reads the same already-initialized binding — legal in JS, not an error.
    let module = compile_source("var x = 1; { var x = x; }");
    assert!(!module.bytecode.is_empty(), "var self-init in block should compile");
}

#[test]
fn symbol_table_duplicate_var_is_legal() {
    // Duplicate `var` in the same scope is legal in JavaScript (even in strict mode);
    // only duplicate `let`/`const` declarations are a SyntaxError.
    let module = compile_source("var x = 1; var x = 2;");
    assert!(!module.bytecode.is_empty(), "duplicate var should compile");
}

#[test]
fn symbol_table_undeclared_auto_global() {
    let module = compile_source("x = 5;");
    assert!(!module.bytecode.is_empty(), "undeclared assignment should auto-create global");
}
