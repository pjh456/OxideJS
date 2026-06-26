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

#[test]
fn reflect_global_and_methods_exist() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "typeof Reflect === 'object' && typeof Reflect.get === 'function' && typeof Reflect.construct === 'function'",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_get_set_has_delete_property() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var obj = {a: 1, b: 2}; \
         var getOk = Reflect.get(obj, 'a') === 1; \
         var setOk = Reflect.set(obj, 'c', 3) === true && obj.c === 3; \
         var hasOk = Reflect.has(obj, 'a') === true && Reflect.has(obj, 'z') === false; \
         var delOk = Reflect.deleteProperty(obj, 'a') === true && Reflect.has(obj, 'a') === false; \
         getOk && setOk && hasOk && delOk",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_own_keys_returns_own_property_names() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var obj = {a: 1, b: 2}; Reflect.set(obj, 'c', 3); Reflect.ownKeys(obj).join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "a,b,c");
}

#[test]
fn reflect_get_own_property_descriptor_returns_descriptor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var obj = {}; Reflect.defineProperty(obj, 'x', { value: 9, writable: true, enumerable: true, configurable: true }); \
         var d = Reflect.getOwnPropertyDescriptor(obj, 'x'); d.value === 9 && d.writable === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_prototype_and_extensible_methods() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var proto = {p: 1}; var obj = {}; \
         var setProto = Reflect.setPrototypeOf(obj, proto); \
         var getProto = Reflect.getPrototypeOf(obj) === proto; \
         var ext1 = Reflect.isExtensible(obj); \
         var prevent = Reflect.preventExtensions(obj); \
         var ext2 = Reflect.isExtensible(obj); \
         setProto && getProto && ext1 === true && prevent === true && ext2 === false",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_apply_calls_function() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function add(a, b) { return this.base + a + b; } var receiver = {base: 10}; var args = [1, 2]; Reflect.apply(add, receiver, args)",
    )
    .unwrap();
    assert_eq!(result.as_double(), 13.0);
}

#[test]
fn reflect_construct_throws_clear_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "try { Reflect.construct(function(){}, []) } catch (e) { e instanceof TypeError }",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_non_object_target_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { Reflect.get(1, 'x') } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}
