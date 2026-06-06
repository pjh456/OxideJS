use oxide_compiler::compiler::Compiler;
use oxide_kernel::code_forge::CodeForge;
use oxide_parser::Allocator;

#[allow(dead_code)]
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
    let hash_b = parse_to_hash("var a = 1 + 2; var b = a;");
    assert_eq!(hash_a, hash_b);
}

#[test]
fn structural_hash_different_init_values() {
    let hash_a = parse_to_hash("var x = 1 + 2;");
    let hash_b = parse_to_hash("var y = 3 + 4;");
    assert_ne!(hash_a, hash_b);
}

#[test]
fn structural_hash_different_string_literals() {
    let hash_a = parse_to_hash(r#"var x = "hello";"#);
    let hash_b = parse_to_hash(r#"var x = "world";"#);
    assert_ne!(hash_a, hash_b);
}

#[test]
fn structural_hash_different_booleans() {
    let hash_a = parse_to_hash("var x = true;");
    let hash_b = parse_to_hash("var x = false;");
    assert_ne!(hash_a, hash_b);
}

#[test]
fn structural_hash_different_shape() {
    let hash_a = parse_to_hash("1 + 2");
    let hash_b = parse_to_hash("1 + 2 * 3");
    assert_ne!(hash_a, hash_b);
}

#[test]
fn regression_forstatement_hash() {
    let hash_a = parse_to_hash("for(i=0;i<3;i++){1}");
    let hash_b = parse_to_hash("for(;;){1}");
    assert_ne!(
        hash_a, hash_b,
        "different for-loop init/test/update must produce different hashes"
    );
}

#[test]
fn regression_conditionalexpression_hash() {
    let hash_a = parse_to_hash("a?1:2");
    let hash_b = parse_to_hash("a?3:4");
    assert_ne!(
        hash_a, hash_b,
        "different ternary branches must produce different hashes"
    );

    let hash_c = parse_to_hash("a?1:2");
    let hash_d = parse_to_hash("a?1:2||3");
    assert_ne!(
        hash_c, hash_d,
        "different expression types with same test must produce different hashes"
    );
}
