use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;

fn parse_to_hash(source: &str) -> u64 {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    oxide_compiler::compiler::structural_hash(&program)
}

fn compile_source(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let compiler = Compiler::new();
    compiler.compile(&program).expect("compile failed")
}

// -- Arrow function structural hash tests --

#[test]
fn hash_arrow_expression_body() {
    // Expression-body arrows at top level compile successfully
    let hash = parse_to_hash("var f = () => 42;");
    assert!(hash != 0, "arrow function should produce a non-zero hash");
}

#[test]
fn hash_arrow_different_params() {
    let hash_a = parse_to_hash("var f = (x) => x;");
    let hash_b = parse_to_hash("var f = (x, y) => x;");
    assert_ne!(hash_a, hash_b, "different param counts must produce different hashes");
}

#[test]
fn hash_arrow_vs_regular_function() {
    let hash_a = parse_to_hash("var f = () => 42;");
    let hash_b = parse_to_hash("var f = function() { return 42; };");
    assert_ne!(hash_a, hash_b, "arrow and regular functions must have different hashes");
}

#[test]
fn hash_arrow_block_body() {
    let hash = parse_to_hash("var f = (a, b) => { return a + b; };");
    assert!(hash != 0, "arrow with block body should produce a non-zero hash");
}

#[test]
fn hash_arrow_same_shape() {
    // Same structural shape: different variable name, same arrow expression
    let hash_a = parse_to_hash("var f = () => 1 + 2;");
    let hash_b = parse_to_hash("var g = () => 1 + 2;");
    assert_eq!(hash_a, hash_b, "same-shaped arrow functions should have same hash");
}

// -- Arrow function compilation tests --

#[test]
fn compile_arrow_expression_body() {
    let module = compile_source("var f = () => 42; f();");
    // Verify the sub_module for the arrow is marked as is_arrow
    let arrow_found = module.sub_modules.iter().any(|m| m.is_arrow);
    assert!(arrow_found, "arrow function sub_module should have is_arrow=true");
}

#[test]
fn compile_arrow_block_body() {
    let module = compile_source("var f = (a, b) => { return a + b; }; f(1, 2);");
    let arrow_found = module.sub_modules.iter().any(|m| m.is_arrow);
    assert!(arrow_found, "arrow with block body should have is_arrow=true");
}

#[test]
fn compile_arrow_name_inference() {
    let module = compile_source("var myArrow = () => 42;");
    // The sub_module for the arrow should have function_name set
    let named = module
        .sub_modules
        .iter()
        .any(|m| m.is_arrow && m.function_name.as_deref() == Some("myArrow"));
    assert!(named, "arrow function should have function_name='myArrow' from name inference");
}

#[test]
fn compile_regular_function_not_arrow() {
    let module = compile_source("var f = function() { return 42; }; f();");
    // Regular FunctionExpression should NOT have is_arrow=true
    let has_arrow = module.sub_modules.iter().any(|m| m.is_arrow);
    assert!(!has_arrow, "regular function expression should not be marked as arrow");
}

#[test]
fn compile_arrow_single_param_no_parens() {
    let module = compile_source("var double = x => x * 2; double(5);");
    let arrow_found = module.sub_modules.iter().any(|m| m.is_arrow);
    assert!(arrow_found, "single-param arrow without parens should compile");
}

#[test]
fn compile_arrow_multiple_params_with_parens() {
    let module = compile_source("var sum = (a, b) => a + b; sum(1, 2);");
    let arrow_found = module.sub_modules.iter().any(|m| m.is_arrow);
    assert!(arrow_found, "multi-param arrow with parens should compile");
}
