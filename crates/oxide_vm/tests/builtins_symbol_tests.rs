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

// --- Symbol 作属性键 ---

#[test]
fn symbol_keys_stay_distinct() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s1=Symbol('a'), s2=Symbol('b'); var o={}; o[s1]=1; o[s2]=2; [o[s1],o[s2]]").unwrap();
    let arr = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(arr.get_prop_at(0).as_int(), 1);
    assert_eq!(arr.get_prop_at(1).as_int(), 2);
}

#[test]
fn symbol_keys_excluded_from_object_keys() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s=Symbol('x'); var o={a:1}; o[s]=2; Object.keys(o).length").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn symbol_keys_excluded_from_own_property_names() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s=Symbol('x'); var o={}; o[s]=1; Object.getOwnPropertyNames(o).length").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn symbol_keys_excluded_from_json_stringify() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s=Symbol('x'); var o={a:1}; o[s]=2; JSON.stringify(o)").unwrap();
    assert_eq!(to_str(&vm, result), "{\"a\":1}");
}

#[test]
fn symbol_keys_excluded_from_for_in() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s=Symbol('x'); var o={}; o[s]=1; var n=0; for (var k in o) n++; n").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn get_own_property_symbols_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s=Symbol('x'); var o={}; o[s]=7; var k=Object.getOwnPropertySymbols(o)[0]; o[k]").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn get_own_property_symbols_length() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s=Symbol('x'); var o={}; o[s]=1; Object.getOwnPropertySymbols(o).length").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn computed_symbol_key_object_literal() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o={[Symbol('k')]: 5, a: 1}; Object.keys(o).length").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn well_known_symbol_key_excluded_from_keys() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o={}; o[Symbol.iterator]=1; Object.keys(o).length").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn symbol_key_read_via_get_own_property_symbols_index() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s1=Symbol('a'), s2=Symbol('b'); var o={}; o[s1]=1; o[s2]=2; var ks=Object.getOwnPropertySymbols(o); var t=0; for (var i=0;i<ks.length;i++) t+=o[ks[i]]; t === 3",
    )
    .unwrap();
    assert!(result.as_bool());
}

// --- Symbol 原始值成员访问走 Symbol.prototype 链 ---

#[test]
fn symbol_primitive_to_string_via_symbol_proto() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol('66').toString()").unwrap();
    assert_eq!(to_str(&vm, result), "Symbol(66)");
}

#[test]
fn symbol_primitive_value_of_via_symbol_proto() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var s = Symbol('x'); s.valueOf() === s").unwrap();
    assert!(result.as_bool());
}

#[test]
fn symbol_primitive_chain_reaches_object_proto_method() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol('x').hasOwnProperty('description')").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn symbol_primitive_description_getter() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol('desc').description").unwrap();
    assert_eq!(to_str(&vm, result), "desc");
}

#[test]
fn symbol_without_description_returns_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Symbol().description").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn symbol_wrapper_object_description_unboxes() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object(Symbol('w')).description").unwrap();
    assert_eq!(to_str(&vm, result), "w");
}

#[test]
fn symbol_description_non_symbol_receiver_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "try { Object.getOwnPropertyDescriptor(Symbol.prototype, 'description').get.call(42); false } catch (e) { e instanceof TypeError }",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn symbol_proto_direct_description_access_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "try { Symbol.prototype.description; false } catch (e) { e instanceof TypeError }",
    )
    .unwrap();
    assert!(result.as_bool());
}
