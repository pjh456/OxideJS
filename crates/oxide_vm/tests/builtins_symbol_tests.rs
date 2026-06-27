use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

// --- Symbol constructor ---

#[test]
fn symbol_creates_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol()").unwrap();
    assert!(result.is_symbol());
}

#[test]
fn symbol_with_description() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol('hello') !== Symbol('hello')").unwrap();
    assert!(result.as_bool());
}

#[test]
fn symbol_unique_identity() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var a = Symbol('x'); var b = Symbol('x'); a === b").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn symbol_same_reference_equals() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s = Symbol('x'); s === s").unwrap();
    assert!(result.as_bool());
}

// --- typeof Symbol ---

#[test]
fn typeof_symbol_returns_symbol() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol()").unwrap();
    assert_eq!(to_str(&vm, result), "symbol");
}

#[test]
fn typeof_symbol_variable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s = Symbol('test'); typeof s").unwrap();
    assert_eq!(to_str(&vm, result), "symbol");
}

// --- Symbol well-known properties ---

#[test]
fn symbol_match_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol.match").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

#[test]
fn symbol_replace_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol.replace").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

#[test]
fn symbol_search_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol.search").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

#[test]
fn symbol_split_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol.split").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

// --- Symbol coercion ---

#[test]
fn symbol_is_truthy() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol() ? true : false").unwrap();
    assert!(result.as_bool());
}

// --- Symbol types ---

#[test]
fn symbol_not_equals_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol() === {}").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn symbol_not_equals_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol() === 'symbol'").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn symbol_iterator_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol.iterator").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

#[test]
fn symbol_for_reuses_registered_symbol() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s1 = Symbol.for('shared'); var s2 = Symbol.for('shared'); s1 === s2").unwrap();
    assert!(result.as_bool());
}

#[test]
fn symbol_for_is_distinct_from_symbol_constructor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s1 = Symbol.for('shared'); var s2 = Symbol('shared'); s1 === s2").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn symbol_key_for_returns_registered_key() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol.keyFor(Symbol.for('shared'))").unwrap();
    assert_eq!(to_str(&vm, result), "shared");
}

#[test]
fn symbol_key_for_unregistered_symbol_returns_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol.keyFor(Symbol('shared'))").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn symbol_key_for_non_symbol_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { Symbol.keyFor(42) } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

// --- Symbol.hasInstance ---

#[test]
fn symbol_has_instance_exists() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof Symbol.hasInstance").unwrap();
    assert_eq!(to_str(&vm, result), "object");
}

#[test]
fn has_instance_overrides_instanceof() {
    let mut vm = Vm::new();
    let source = r#"
        class C { static [Symbol.hasInstance](v) { return typeof v === 'string'; } }
        'hello' instanceof C
    "#;
    let result = eval(&mut vm, source).unwrap();
    assert!(result.as_bool());
}

#[test]
fn has_instance_non_callable_falls_through() {
    let mut vm = Vm::new();
    let source = r#"
        class D {}
        D[Symbol.hasInstance] = 42;
        ({} instanceof D) === false
    "#;
    let result = eval(&mut vm, source).unwrap();
    assert!(result.as_bool());
}

#[test]
fn has_instance_coerces_return_to_boolean() {
    let mut vm = Vm::new();
    let source = r#"
        class C { static [Symbol.hasInstance](v) { return 'truthy'; } }
        'anything' instanceof C
    "#;
    let result = eval(&mut vm, source).unwrap();
    assert!(result.as_bool());
}

#[test]
fn has_instance_preserves_ordinary_has_instance() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "[] instanceof Array").unwrap();
    assert!(result.as_bool());
}
