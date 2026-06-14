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
fn data_view_global_and_methods_exist() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "typeof DataView === 'function' && typeof DataView.prototype.getInt32 === 'function'",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_properties_reference_buffer_offset_and_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(16); var dv = new DataView(buf, 4, 8); \
         dv.buffer === buf && dv.byteOffset === 4 && dv.byteLength === 8",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_set_get_int32_big_endian() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(16); var dv = new DataView(buf); \
         dv.setInt32(0, 305419896, false); dv.getInt32(0, false)",
    )
    .unwrap();
    assert_eq!(result.as_int(), 305419896);
}

#[test]
fn data_view_set_get_int32_little_endian_exposes_first_byte() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(16); var dv = new DataView(buf); \
         dv.setInt32(0, 305419896, true); dv.getUint8(0)",
    )
    .unwrap();
    assert_eq!(result.as_int(), 120);
}

#[test]
fn data_view_set_get_uint16_and_int8() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(4); var dv = new DataView(buf); \
         var neg = -1; dv.setUint16(0, 65535, false); dv.setInt8(2, neg); \
         dv.getUint16(0, false) === 65535 && dv.getInt8(2) === -1",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_float64_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(8); var dv = new DataView(buf); \
         dv.setFloat64(0, 3.5, true); dv.getFloat64(0, true)",
    )
    .unwrap();
    assert_eq!(result.as_double(), 3.5);
}

#[test]
fn data_view_offset_view_writes_into_underlying_buffer() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buf = new ArrayBuffer(16); var base = new DataView(buf); var dv = new DataView(buf, 4, 8); \
         dv.setUint8(0, 77); base.getUint8(4)",
    )
    .unwrap();
    assert_eq!(result.as_int(), 77);
}

#[test]
fn data_view_is_array_buffer_view() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var dv = new DataView(new ArrayBuffer(1)); ArrayBuffer.isView(dv)").unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_rejects_non_array_buffer() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { new DataView({}) } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_rejects_out_of_bounds_access() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "try { var dv = new DataView(new ArrayBuffer(4)); dv.getInt32(2); } catch (e) { e instanceof RangeError }",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_to_string_is_identifiable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var dv = new DataView(new ArrayBuffer(1)); dv.toString()").unwrap();
    assert_eq!(to_str(&vm, result), "[object DataView]");
}
