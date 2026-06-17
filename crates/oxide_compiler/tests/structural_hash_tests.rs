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

fn parse_to_compiled_hash(source: &str) -> u64 {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    oxide_compiler::compiler::compiled_module_hash(&program)
}

#[test]
fn structural_hash_hit() {
    let forge = CodeForge::new();
    let compiler = Compiler::new();
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, "1 + 2").expect("parse failed");

    let m1 = forge.get_or_compile(&program, &compiler).expect("first compile");
    let m2 = forge.get_or_compile(&program, &compiler).expect("second compile");

    assert_eq!((m1.bytecode.len(), m1.n_registers), (m2.bytecode.len(), m2.n_registers));
}

#[test]
fn structural_hash_same_shape() {
    let hash_a = parse_to_hash("var x = 1 + 2; var y = x;");
    let hash_b = parse_to_hash("var a = 1 + 2; var b = a;");
    assert_eq!(hash_a, hash_b);
}

#[test]
fn structural_hash_identifier_renaming_remains_normalized() {
    let hash_a = parse_to_hash("var left = 1; left + left");
    let hash_b = parse_to_hash("var right = 1; right + right");
    assert_eq!(hash_a, hash_b);
}

#[test]
fn compiled_module_hash_distinguishes_same_shape_different_identifier_loads() {
    let hash_a = parse_to_compiled_hash("var x = 1; var y = 2; x");
    let hash_b = parse_to_compiled_hash("var x = 1; var y = 2; y");
    assert_ne!(hash_a, hash_b);
}

#[test]
fn compiled_module_hash_distinguishes_assignment_targets() {
    let hash_a = parse_to_compiled_hash("var x = 1; x = 2; x");
    let hash_b = parse_to_compiled_hash("var x = 1; y = 2; x");
    assert_ne!(hash_a, hash_b);
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
    assert_ne!(hash_a, hash_b, "different for-loop init/test/update must produce different hashes");
}

#[test]
fn regression_conditionalexpression_hash() {
    let hash_a = parse_to_hash("a?1:2");
    let hash_b = parse_to_hash("a?3:4");
    assert_ne!(hash_a, hash_b, "different ternary branches must produce different hashes");

    let hash_c = parse_to_hash("a?1:2");
    let hash_d = parse_to_hash("a?1:2||3");
    assert_ne!(hash_c, hash_d, "different expression types with same test must produce different hashes");
}

#[test]
fn hash_compound_vs_simple_assign() {
    let hash_a = parse_to_hash("x=1");
    let hash_b = parse_to_hash("x+=1");
    assert_ne!(hash_a, hash_b, "x=1 and x+=1 must produce different hashes");
}

#[test]
fn object_literal_data_get_set_shapes_differ() {
    let data = parse_to_hash("var o = { x: 1 };");
    let getter = parse_to_hash("var o = { get x() { return 1; } };");
    let setter = parse_to_hash("var o = { set x(v) { this.y = v; } };");

    assert_ne!(data, getter, "data and getter properties must hash differently");
    assert_ne!(data, setter, "data and setter properties must hash differently");
    assert_ne!(getter, setter, "getter and setter properties must hash differently");
}

#[test]
fn class_method_getter_static_setter_shapes_differ() {
    let method = parse_to_hash("class A { x() { return 1; } }");
    let getter = parse_to_hash("class A { get x() { return 1; } }");
    let static_setter = parse_to_hash("class A { static set x(v) { this.y = v; } }");

    assert_ne!(method, getter, "method and getter must hash differently");
    assert_ne!(method, static_setter, "method and static setter must hash differently");
    assert_ne!(getter, static_setter, "getter and static setter must hash differently");
}
