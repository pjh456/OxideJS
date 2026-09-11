use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
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

#[test]
fn data_view_get_big_int64_returns_bigint() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new DataView(new ArrayBuffer(8)).getBigInt64(0)").unwrap();
    assert!(result.is_bigint());
}

#[test]
fn data_view_set_big_int64_with_number_throws() {
    let mut vm = Vm::new();
    // ToBigInt 语义：Number 入参抛 TypeError。
    let result = eval(
        &mut vm,
        "try { new DataView(new ArrayBuffer(8)).setBigInt64(0, 1); false } catch (e) { e instanceof TypeError }",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_big_int64_precision_roundtrip() {
    let mut vm = Vm::new();
    // 2^63-1（i64::MAX）超出 f64 精确范围，真 BigInt 语义读回必须原值。
    let result = eval(
        &mut vm,
        "var dv = new DataView(new ArrayBuffer(8)); \
         dv.setBigInt64(0, 9223372036854775807n); dv.getBigInt64(0) === 9223372036854775807n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_big_uint64_precision_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var dv = new DataView(new ArrayBuffer(8)); \
         dv.setBigUint64(0, 18446744073709551615n, true); dv.getBigUint64(0, true) === 18446744073709551615n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_big_int64_little_endian_byte_order() {
    let mut vm = Vm::new();
    // 小端写入后首字节为低字节；同一字节序读回原值。
    let result = eval(
        &mut vm,
        "var dv = new DataView(new ArrayBuffer(8)); \
         dv.setBigInt64(0, 0x0102030405060708n, true); \
         dv.getUint8(0) === 8 && dv.getBigInt64(0, true) === 0x0102030405060708n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn data_view_set_big_int64_mod_2_64_truncation() {
    let mut vm = Vm::new();
    // 超 64 位值按 mod 2^64 截断写入。
    let result = eval(
        &mut vm,
        "var dv = new DataView(new ArrayBuffer(8)); \
         dv.setBigInt64(0, 1180591620717411303429n); dv.getBigInt64(0) === 5n",
    )
    .unwrap();
    assert!(result.as_bool());
}
