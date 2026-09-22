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
fn typed_array_bigint_roundtrips_bigint_values() {
    let mut vm = Vm::new();
    // BigInt64Array 元素读写为真 BigInt 值（非 f64 近似）。
    let result = eval(&mut vm, "var ta = new BigInt64Array(1); ta.fill(42n); ta.at(0)").unwrap();
    assert!(result.is_bigint());
    assert_eq!(vm.bigint_value(result).to_string(), "42");
}

#[test]
fn typed_array_bigint_fill_with_number_throws() {
    let mut vm = Vm::new();
    // 数值 TA 写 BigInt 值按规范抛 TypeError；BigInt TA 写 Number 同样抛 TypeError。
    let result = eval(
        &mut vm,
        "var ok = true; \
         try { new BigInt64Array(1).fill(42); ok = false; } catch (e) { ok = e instanceof TypeError; } \
         try { new Int8Array(1)[0] = 1n; ok = ok && false; } catch (e) { ok = ok && e instanceof TypeError; } \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_to_string_is_identifiable() {
    let mut vm = Vm::new();
    // own toString 走 Array join 语义（Uint8Array(1) → "0")；
    // @@toStringTag 经 Object.prototype.toString 仍可辨识。
    let result = eval(
        &mut vm,
        "var ta = new Uint8Array(1); Object.prototype.toString.call(ta) + \";\" + ta.toString()",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "[object Uint8Array];0");
}

#[test]
fn typed_array_element_write_roundtrips() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Int32Array(2); ta[0] = 42; ta[1] = ta[0] * 2; ta[1] === 84").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_element_write_clamps_int8() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Int8Array(1); ta[0] = 300; ta[0]").unwrap();
    assert_eq!(result.as_int(), 44);
}

#[test]
fn typed_array_out_of_bounds_write_is_ignored() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ta = new Int8Array(1); ta[5] = 1; ta.length === 1 && ta[5] === undefined").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bypes_per_element_on_ctor_and_proto() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "Int8Array.BYTES_PER_ELEMENT === 1 && Float64Array.BYTES_PER_ELEMENT === 8 && \
         Int8Array.prototype.BYTES_PER_ELEMENT === 1 && Uint32Array.prototype.BYTES_PER_ELEMENT === 4",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_define_property_writes_element() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Uint8Array(3); Object.defineProperty(ta, 1, { value: 9 }); ta[1] === 9",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_reflect_set_writes_to_receiver() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Uint8Array(3); var o = {}; \
         Reflect.set(ta, 1, 5, o) && o[1] === 5 && ta[1] === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint64_precision_roundtrip() {
    let mut vm = Vm::new();
    // 2^63-1（i64::MAX）超出 f64 精确范围：f64 会舍入为 2^63，真 BigInt 存储读回必须原值。
    let result = eval(
        &mut vm,
        "var a = new BigInt64Array(1); a[0] = 9223372036854775807n; a[0] === 9223372036854775807n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint64_write_overflow_wraps() {
    let mut vm = Vm::new();
    // 2^63 写入 i64 槽按二进制补码回读为 -2^63。
    let result = eval(
        &mut vm,
        "var a = new BigInt64Array(1); a[0] = 9223372036854775808n; a[0] === -9223372036854775808n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_biguint64_precision_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = new BigUint64Array(1); a[0] = 18446744073709551615n; a[0] === 18446744073709551615n && a[0] > Number.MAX_SAFE_INTEGER",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint_negative_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var a = new BigInt64Array(1); a[0] = -1n; a[0] === -1n").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint_mod_2_64_truncation() {
    let mut vm = Vm::new();
    // 写入按 mod 2^64 截断：2^70 + 5 的低 64 位为 5。
    let result = eval(&mut vm, "var a = new BigInt64Array(1); a[0] = 1180591620717411303429n; a[0] === 5n").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint_write_number_throws() {
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "try { new BigInt64Array(1)[0] = 1; false } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_numeric_write_bigint_throws() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { new Int8Array(1)[0] = 1n; false } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_constructor_bigint_length() {
    let mut vm = Vm::new();
    // 长度参数走 ToIndex：BigInt 实参在 ToNumber 步抛 TypeError，不截断建数组；
    // 布尔/null 等原语按数值转换语义建数组。
    let result = eval(
        &mut vm,
        "(function () { var t = false; \
         try { new BigInt64Array(5n); } catch (e) { t = e instanceof TypeError; } \
         return t && new Int8Array(true).length === 1 && new Int8Array(null).length === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint_from_array_and_set() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = new BigInt64Array([1n, 2n]); \
         a.length === 2 && a[0] === 1n && a[1] === 2n && \
         BigInt64Array.from([3n]).at(0) === 3n && \
         (function () { var t = new BigInt64Array(2); t.set([9n, 8n]); return t[0] === 9n && t[1] === 8n; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint_sort_uses_bigint_compare() {
    let mut vm = Vm::new();
    // 超出 f64 精度的 BigInt 排序仍按 BigInt 值升序（f64 中转会静默错序）。
    let result = eval(
        &mut vm,
        "var a = new BigInt64Array([9223372036854775807n, -1n, 0n, 1n]); a.sort(); \
         a[0] === -1n && a[1] === 0n && a[2] === 1n && a[3] === 9223372036854775807n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_in_operator_uses_has_property_semantics() {
    // `in` 按 HasProperty 判定：TypedArray 整数索引按视图长度判在界，
    // 越界索引判不存在。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Int8Array(2); \
         (0 in ta) && (1 in ta) && !(2 in ta) && !(3 in ta) && \
         ('length' in ta) && !('foo' in ta)",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn in_operator_finds_array_elements_on_proto_chain() {
    // 原型链上的数组元素区索引判存在：Object.create([1,2]) 的 0/1 在界、2 越界。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o = Object.create([1, 2]); (0 in o) && (1 in o) && !(2 in o)").unwrap();
    assert!(result.as_bool());
}

#[test]
fn in_operator_ordinary_object_zero_drift() {
    // 普通对象 `in` 既有行为零漂移：自有属性在、未定义键不在、原型链属性在、
    // 无整型键的对象不判索引存在。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var base = { x: 1 }; var o = Object.create(base); \
         ('x' in o) && !('y' in o) && ('toString' in o) && \
         !(0 in Object.create({}))",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn in_operator_array_explicit_undefined_named_prop_present() {
    // 数组显式 undefined 自有命名属性判在场：HasProperty 按自身槽存在性
    // 判定，值语义（显式 undefined）不参与；HasOwnProperty 同面。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = [1]; a.x = undefined; \
         ('x' in a) && a.hasOwnProperty('x') && Object.hasOwn(a, 'x') && \
         !('y' in a) && !a.hasOwnProperty('y')",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn in_operator_ordinary_explicit_undefined_named_prop_zero_drift() {
    // 普通对象显式 undefined 命名槽零漂移守卫：`in`/hasOwnProperty 判在，
    // 读值仍为 undefined。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var o = {}; o.x = undefined; \
         ('x' in o) && o.hasOwnProperty('x') && o.x === undefined",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_bigint_methods_receive_bigint_values() {
    let mut vm = Vm::new();
    // map/filter/reduce 回调收到的元素是真 BigInt；map 结果按元素类型转换回写。
    let result = eval(
        &mut vm,
        "var a = new BigInt64Array([1n, 2n]); \
         var seen = []; \
         var m = a.map(function (v) { seen.push(typeof v); return v + 1n; }); \
         seen[0] === 'bigint' && seen[1] === 'bigint' && m[0] === 2n && m[1] === 3n && \
         a.filter(function (v) { return v > 1n; }).length === 1 && \
         a.reduce(function (acc, v) { return acc + v; }, 0n) === 3n",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_to_string_tag_undefined_for_non_ta_this() {
    // 原型自身（无 [[TypedArrayName]] 内部槽）与基元/非 TA 对象 this 读值均为
    // undefined，规范语义不抛错。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var g = Object.getOwnPropertyDescriptor(TypedArray.prototype, Symbol.toStringTag).get; \
         g.call(TypedArray.prototype) === undefined && \
         g.call(null) === undefined && g.call(42) === undefined && \
         g.call('x') === undefined && g.call({}) === undefined && g.call([]) === undefined",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_to_string_tag_accessor_name_label() {
    // getter 的 name/length/set 描述符面按规范钉。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(TypedArray.prototype, Symbol.toStringTag); \
         d.set === undefined && d.enumerable === false && d.configurable === true && \
         d.get.name === 'get [Symbol.toStringTag]' && d.get.length === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_instance_tag_and_arraybuffer_proto_tag() {
    // 实例读类型名（经 Object.prototype.toString 与直读两面）；
    // ArrayBuffer.prototype 的 tag 为数据属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "new Int16Array(2)[Symbol.toStringTag] === 'Int16Array' && \
         Object.prototype.toString.call(new BigUint64Array(1)) === '[object BigUint64Array]' && \
         Object.prototype.toString.call(new ArrayBuffer(4)) === '[object ArrayBuffer]' && \
         ArrayBuffer.prototype[Symbol.toStringTag] === 'ArrayBuffer' && \
         Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, Symbol.toStringTag).writable === false",
    )
    .unwrap();
    assert!(result.as_bool());
}
