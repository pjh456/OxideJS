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
fn typed_array_globals_exist() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "typeof Int8Array === 'function' && typeof Uint8Array === 'function' && \
         typeof Uint8ClampedArray === 'function' && typeof Int16Array === 'function' && \
         typeof Uint16Array === 'function' && typeof Int32Array === 'function' && \
         typeof Uint32Array === 'function' && typeof Float32Array === 'function' && \
         typeof Float64Array === 'function' && typeof BigInt64Array === 'function' && \
         typeof BigUint64Array === 'function'",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_length_constructor_sets_metadata() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Int16Array(4); ta.length === 4 && ta.byteLength === 8 && ta.byteOffset === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_fill_and_at_read_elements() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Int8Array(4); ta.fill(7); ta.at(0) === 7 && ta.at(3) === 7").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_uint8_clamped_clamps_values() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Uint8ClampedArray(2); ta.fill(300); ta.at(0) === 255").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_slice_copies_values() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Int8Array([1, 2, 3, 4]); var out = ta.slice(1, 3); \
         out.length === 2 && out.at(0) === 2 && out.at(1) === 3",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_subarray_shares_buffer() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Int8Array([1, 2, 3, 4]); var sub = ta.subarray(1, 3); \
         sub.fill(9); ta.at(1) === 9 && ta.at(2) === 9",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_set_copies_from_array() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Int32Array(4); var src = [5, 6]; var offset = 1; \
         ta.set(src, offset); ta.at(0) === 0 && ta.at(1) === 5 && ta.at(2) === 6",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_constructs_from_array_buffer() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(8); var view = new Int32Array(buf); \
         view.length === 2 && view.byteLength === 8 && view.buffer === buf && ArrayBuffer.isView(view)",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_views_share_data_with_data_view() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(8); var dv = new DataView(buf); var ta = new Uint8Array(buf); \
         ta.fill(12); dv.getUint8(0) === 12 && dv.getUint8(7) === 12",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_float64_roundtrips() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Float64Array(1); ta.fill(3.5); ta.at(0)").unwrap();
    assert_eq!(result.as_double(), 3.5);
}

#[test]
fn typed_array_bigint_fallback_returns_float() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new BigInt64Array(1); ta.fill(42); ta.at(0)").unwrap();
    assert_eq!(result.as_double(), 42.0);
}

#[test]
fn typed_array_to_string_is_identifiable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Uint8Array(1); ta.toString()").unwrap();
    assert_eq!(to_str(&vm, result), "[object TypedArray]");
}
