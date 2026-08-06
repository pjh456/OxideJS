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
    // `var` 无暂时性死区且提升到函数作用域，块级 `var x = x` 读取的是
    // 同一已初始化绑定——JS 合法，非错误。
    let module = compile_source("var x = 1; { var x = x; }");
    assert!(!module.bytecode.is_empty(), "var self-init in block should compile");
}

#[test]
fn symbol_table_duplicate_var_is_legal() {
    // JavaScript 中同作用域重复 `var` 合法（严格模式亦然）；
    // 只有重复 `let`/`const` 才是 SyntaxError。
    let module = compile_source("var x = 1; var x = 2;");
    assert!(!module.bytecode.is_empty(), "duplicate var should compile");
}

#[test]
fn symbol_table_undeclared_auto_global() {
    let module = compile_source("x = 5;");
    assert!(!module.bytecode.is_empty(), "undeclared assignment should auto-create global");
}
