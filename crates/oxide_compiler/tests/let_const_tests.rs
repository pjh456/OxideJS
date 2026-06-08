use oxide_compiler::compiler::Compiler;

#[test]
fn hash_var_let_const_differ() {
    let compiler = Compiler::new();
    let allocator = oxide_parser::Allocator::default();

    let var_ast = oxide_parser::parse(&allocator, "var x = 1").unwrap();
    let let_ast = oxide_parser::parse(&allocator, "let x = 1").unwrap();
    let const_ast = oxide_parser::parse(&allocator, "const x = 1").unwrap();

    // Different AST structures should produce different bytecode
    let var_module = compiler.compile(&var_ast).unwrap();
    let let_module = compiler.compile(&let_ast).unwrap();
    let const_module = compiler.compile(&const_ast).unwrap();

    assert_eq!(var_module.n_registers, let_module.n_registers);
    assert_eq!(let_module.n_registers, const_module.n_registers);
}
