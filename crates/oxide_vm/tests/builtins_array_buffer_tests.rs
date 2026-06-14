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
    vm.kernel_core()
        .string_forge()
        .lookup(val.as_string_index())
        .unwrap_or_default()
}

#[test]
fn array_buffer_global_and_static_method_exist() {
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "typeof ArrayBuffer === 'function' && typeof ArrayBuffer.isView === 'function'").unwrap();
    assert!(result.as_bool());
}

#[test]
fn array_buffer_constructor_sets_byte_length_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var buf = new ArrayBuffer(8); buf.byteLength").unwrap();
    assert_eq!(result.as_int(), 8);
}

#[test]
fn array_buffer_slice_returns_copied_buffer_with_length() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var buf = new ArrayBuffer(8); var slice = buf.slice(2, 6); slice.byteLength").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn array_buffer_slice_normalizes_negative_indices() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(8); var start = -4; var end = -1; buf.slice(start, end).byteLength",
    )
    .unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn array_buffer_is_view_returns_false_for_buffer_and_plain_object() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(8); ArrayBuffer.isView(buf) === false && ArrayBuffer.isView({}) === false",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn array_buffer_negative_length_throws_range_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { new ArrayBuffer(-1) } catch (e) { e instanceof RangeError }").unwrap();
    assert!(result.as_bool());
}

#[test]
fn array_buffer_to_string_is_identifiable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var buf = new ArrayBuffer(1); buf.toString()").unwrap();
    assert_eq!(to_str(&vm, result), "[object ArrayBuffer]");
}
