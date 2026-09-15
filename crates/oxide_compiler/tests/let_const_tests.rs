use oxide_compiler::compiler::Compiler;

#[test]
fn hash_var_let_const_differ() {
    let compiler = Compiler::new();
    let allocator = oxide_parser::Allocator::default();

    let var_ast = oxide_parser::parse(&allocator, "var x = 1").unwrap();
    let let_ast = oxide_parser::parse(&allocator, "let x = 1").unwrap();
    let const_ast = oxide_parser::parse(&allocator, "const x = 1").unwrap();

    let var_module = compiler.compile(&var_ast).unwrap();
    let let_module = compiler.compile(&let_ast).unwrap();
    let const_module = compiler.compile(&const_ast).unwrap();

    // let 与 const 词法对称，寄存器数一致。
    assert_eq!(let_module.n_registers, const_module.n_registers);

    // 顶层 var 因提升预声明与全局属性同步序言，编译产物与 let 不同。
    assert_ne!(var_module.bytecode, let_module.bytecode);
}
