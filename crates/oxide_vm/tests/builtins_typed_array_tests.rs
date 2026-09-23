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

#[test]
fn typed_array_species_accessor_descriptor() {
    // 抽象构造器 @@species 访问器描述符四项 + getter name/length +
    // receiver 直读（派生类沿静态原型链解析得自身的前提）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(TypedArray, Symbol.species); \
         d.set === undefined && d.enumerable === false && d.configurable === true && \
         typeof d.get === 'function' && d.get.name === 'get [Symbol.species]' && d.get.length === 0 && \
         d.get.call(Uint8Array) === Uint8Array && d.get.call(Float64Array) === Float64Array",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_no_own_on_concrete_ctor() {
    // 具体构造器无 own @@species（链上继承抽象构造器），内置构造器
    // 解析得自身。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "Object.getOwnPropertyDescriptor(Uint8Array, Symbol.species) === undefined && \
         Object.getOwnPropertyDescriptor(BigInt64Array, Symbol.species) === undefined && \
         Uint8Array[Symbol.species] === Uint8Array && Int32Array[Symbol.species] === Int32Array",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_subclass_resolves_self() {
    // 派生类静态链解析 @@species 得自身构造器。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { class Sub extends Uint8Array {} return Sub[Symbol.species] === Sub; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_subclass_methods_instanceof() {
    // slice/map/filter/subarray 四法结果均保持派生类身份。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { class Sub extends Uint8Array {} \
         var s = new Sub(2); \
         return s.slice(0) instanceof Sub && \
         s.map(function (v) { return v; }) instanceof Sub && \
         s.filter(function (v) { return true; }) instanceof Sub && \
         s.subarray(0) instanceof Sub; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_subclass_of_from() {
    // of/from 走构造（new）形态：派生类 super() 与 new.target 传播成立，
    // 结果为子类实例且元素正确。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { class Sub extends Uint8Array {} \
         var o = Sub.of(1, 2); var f = Sub.from([3, 4]); \
         return o instanceof Sub && o.length === 2 && o.at(0) === 1 && o.at(1) === 2 && \
         f instanceof Sub && f.length === 2 && f.at(0) === 3 && f.at(1) === 4; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_insufficient_length_throws() {
    // species 返回长度不足的目标（slice/map 请求 3 得 2）抛 TypeError。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var t = true; \
         var s = new Uint8Array(3); \
         s.constructor = { [Symbol.species]: function (n) { return new Uint8Array(n - 1); } }; \
         try { s.slice(0); t = false; } catch (e) { t = e instanceof TypeError; } \
         var m = new Uint8Array(3); \
         m.constructor = { [Symbol.species]: function (n) { return new Uint8Array(n - 1); } }; \
         try { m.map(function (v) { return v; }); t = t && false; } \
         catch (e) { t = t && e instanceof TypeError; } \
         return t; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_non_ctor_and_non_ta_throws() {
    // species 构造结果非 TypedArray（filter 得空函数构造体 / map 得普通对象）
    // 抛 TypeError（toReversed/toSorted/with 走 SameType 忽略 species，不在其列）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var t = true; \
         var n = new Uint8Array(1); \
         n.constructor = { [Symbol.species]: function () {} }; \
         try { n.filter(function (v) { return true; }); t = false; } \
         catch (e) { t = e instanceof TypeError; } \
         var r = new Uint8Array(1); \
         r.constructor = { [Symbol.species]: function () { return {}; } }; \
         try { r.map(function (v) { return v; }); t = t && false; } \
         catch (e) { t = t && e instanceof TypeError; } \
         return t; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_ctor_getter_throws() {
    // constructor 访问器抛错经完整 Get 原样传播（非 TypeError 吞并）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array(2); \
         Object.defineProperty(ta, 'constructor', { get() { throw new RangeError('ctor-get'); } }); \
         try { ta.slice(0); return false; } catch (e) { \
         return e instanceof RangeError && e.message === 'ctor-get'; } })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_species_getter_throws() {
    // @@species 访问器抛错原样传播。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array(2); \
         ta.constructor = { get [Symbol.species]() { throw new RangeError('species-get'); } }; \
         try { ta.map(function (v) { return v; }); return false; } catch (e) { \
         return e instanceof RangeError && e.message === 'species-get'; } })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_subarray_three_args() {
    // subarray 的 species 实参为 (buffer, byteOffset, count) 三参形态，
    // 默认臂经同三参内建构造产出共享 buffer 视图。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array(8); var captured; \
         ta.constructor = { [Symbol.species]: function (b, o, l) { \
         captured = [b === ta.buffer, o, l]; return new Uint8Array(b, o, l); } }; \
         var out = ta.subarray(2, 6); \
         return captured[0] === true && captured[1] === 2 && captured[2] === 4 && \
         out.buffer === ta.buffer && out.byteOffset === 2 && out.length === 4; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_slice_alias_cascade_exact_values() {
    // 别名连锁：species 返回与源共享 buffer 的偏移视图时，slice 逐元素
    // 读→转换→写交错，读须看到前一轮写入后的值（node 精确期望 20,20,20,60）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([10, 20, 30, 40, 50, 60]); \
         ta.constructor = { [Symbol.species]: function () { return new Uint8Array(ta.buffer, 2); } }; \
         var out = ta.slice(1, 4); \
         return out.length === 4 && out.at(0) === 20 && out.at(1) === 20 && \
         out.at(2) === 20 && out.at(3) === 60; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_species_use_default_ctor() {
    // constructor 无 @@species（读得 undefined）时回退接收者类型的内建构造器。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var s = new Uint8Array(3); s.constructor = {}; \
         var m = s.map(function (v) { return v; }); \
         return m instanceof Uint8Array && Object.getPrototypeOf(m) === Uint8Array.prototype \
         && m.length === 3 && m.at(0) === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_same_type_immutable_methods_ignore_species() {
    // toReversed/toSorted/with 走 TypedArrayCreateSameType（规范语义：忽略
    // species）：species 构造器不被调用，结果为接收者同类型的内建实例。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2, 3]); var called; \
         ta.constructor = { [Symbol.species]: function (n) { \
         called = true; return new Int32Array(n); } }; \
         var r = ta.toReversed(); var so = ta.toSorted(); var w = ta.with(1, 9); \
         return called === undefined && \
         Object.getPrototypeOf(r) === Uint8Array.prototype && \
         Object.getPrototypeOf(so) === Uint8Array.prototype && \
         Object.getPrototypeOf(w) === Uint8Array.prototype && \
         r.length === 3 && r.at(0) === 3 && so.at(0) === 1 && w.at(1) === 9; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_plain_call_and_construct_zero_drift() {
    // 普通调用（无 new）与构造调用双路径零漂移：结果均为内建实例；
    // 派生类显式构造器 super() 后可继续扩展属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var plain = Uint8Array(2); var constructed = new Uint8Array(2); \
         if (!(plain instanceof Uint8Array && constructed instanceof Uint8Array)) return false; \
         if (plain.length !== 2 || constructed.length !== 2) return false; \
         class S extends Uint8Array { constructor(len) { super(len); this.marked = true; } } \
         var s = new S(2); var f = S.from([7, 8]); \
         return s instanceof S && s.marked === true && f instanceof S && \
         f.length === 2 && f.at(0) === 7 && f.at(1) === 8; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_of_from_result_length_live_check() {
    // of/from 结果长校验取 live 口径：构造器窗口内把 auto 视图收缩到请求
    // 长度之下时抛 TypeError（长度不足）；未收缩的正常臂不受影响。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
          function C() { var ab = new ArrayBuffer(16, { maxByteLength: 16 }); \
          var t = new Int8Array(ab); ab.resize(2); return t; } \
          var o, f; \
          try { o = Int8Array.of.call(C, 1, 2, 3); } catch (e) { o = e instanceof TypeError; } \
          try { f = Int8Array.from.call(C, [1, 2, 3]); } catch (e) { f = e instanceof TypeError; } \
          var ok = o === true && f === true; \
          var n = Int8Array.of(1, 2, 3); \
          return ok && n.length === 3 && n.at(0) === 1 && n.at(2) === 3; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_detached_entry_validate_before_callback_check() {
    // 回调族入口校验先于回调检查：detached 源上非函数回调与有效回调均抛
    // TypeError（detached 守卫臂），种类与消息不回归。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
          var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); ab.transfer(); \
          var t1, t2; \
          try { ta.map('nope'); } catch (e) { t1 = e instanceof TypeError; } \
          try { ta.forEach(function (v) { return v; }); } catch (e) { t2 = e instanceof TypeError; } \
          return t1 === true && t2 === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_plain_member_call_in_class_ctor_not_construct() {
    // 类构造器帧内对 TA 构造器的成员式普通调用是普通形态：返回全新 TA，
    // receiver 不物化（长度与 buffer 不动），receiver 为普通对象时亦不改建。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { class Sub extends Uint8Array { constructor() { \
          super(4); this.f = Uint8Array; var r = this.f(2); this.r = r; this.len = this.length; \
          var o = {}; o.f = Uint8Array; var p = o.f(2); this.p = p; this.o = o; } } \
          var s = new Sub(); \
          return s.r !== s && s.r instanceof Uint8Array && \
          Object.getPrototypeOf(s.r) === Uint8Array.prototype && s.r.length === 2 && \
          s.len === 4 && s.length === 4 && s.buffer.byteLength === 4 && \
          s.p !== s.o && s.p.length === 2 && typeof s.o.length === 'undefined'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gate_get_numeric_invalid_undefined() {
    // 数字无效键（分数/负/"-0"/越界整数）读 undefined：exotic 数字臂不走
    // 原型链，原型同键抛 getter 不被触达。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42, 43]); \
         var keys = ['1.1', '-1', '-0', '2']; \
         for (var i = 0; i < keys.length; i++) { \
           Object.defineProperty(Int8Array.prototype, keys[i], { \
             get: function () { throw new Error('OrdinaryGet was called'); } }); } \
         for (var i = 0; i < keys.length; i++) { \
           if (ta[keys[i]] !== undefined) return false; } \
         return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gate_has_numeric_invalid_false() {
    // 数字无效键 in 判 false：exotic 数字臂不走原型链，原型同键数据属性
    // 不被触达（防假阳）。length-1 样本覆盖越界整数键。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
         var keys = ['1.1', '0.000001', '-1', '-0']; \
         for (var i = 0; i < keys.length; i++) { \
           Int8Array.prototype[keys[i]] = 'test262'; } \
         Int8Array.prototype[1] = 'test262'; \
         for (var i = 0; i < keys.length; i++) { \
           if (keys[i] in ta) return false; } \
         return !(1 in ta); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gate_ordinary_arm_proto_walk() {
    // 非规范数字串（round-trip 不成）落普通属性路径：缺失读 undefined，
    // 原型预置后继承读值且 in 判 true；"+1" 与 "1" 独立成键互不干扰。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
         var keys = ['1.0', '+1', '1000000000000000000000', '0.0000001']; \
         for (var i = 0; i < keys.length; i++) { \
           if (ta[keys[i]] !== undefined) return false; } \
         for (var i = 0; i < keys.length; i++) { \
           Int8Array.prototype[keys[i]] = 'test262'; \
           if (ta[keys[i]] !== 'test262') return false; \
           if (!(keys[i] in ta)) return false; } \
         return ta['1'] === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gate_valid_arm_no_drift() {
    // 界内整数臂回归钉：读元素值、in 判 true、length 不变（Valid 臂零漂移）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ta = new Int8Array([42, 7]); \
         ta[0] === 42 && (0 in ta) && (1 in ta) && ta.length === 2",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gate_detached_numeric_invalid() {
    // detach 后 live 长 0：界内整数键归数字无效，读 undefined、in 判 false。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); \
          ta[0] = 9; ab.transfer(); \
          return ta[0] === undefined && !(0 in ta); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_numeric_invalid_self_silent() {
    // 数字无效键（分数/负数/"-0"）自写：不建属性、读 undefined，原型预置
    // 同键抛 getter/setter 不被触达。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
          var keys = ['1.1', '-1', '-0']; \
          for (var i = 0; i < keys.length; i++) { \
            Object.defineProperty(Int8Array.prototype, keys[i], { \
              get: function () { throw new Error('proto getter must not fire'); }, \
              set: function () { throw new Error('proto setter must not fire'); }, \
              configurable: true }); } \
          for (var i = 0; i < keys.length; i++) { \
            ta[keys[i]] = 7; \
            if (Object.prototype.hasOwnProperty.call(ta, keys[i])) return false; \
            if (ta[keys[i]] !== undefined) return false; } \
          return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_numeric_invalid_other_no_side_effect() {
    // 数字无效键 + receiver ≠ 自身：全 receiver 类 Reflect.set 恒 true，
    // target 与 receiver 均无新属性，valueOf 零次，原型 accessor 不触发。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var calls = 0; \
          var value = { valueOf: function () { calls++; return 2.3; } }; \
          var keys = [1, 1.5, -1]; \
          for (var i = 0; i < keys.length; i++) { var key = keys[i]; \
            Object.defineProperty(Int8Array.prototype, key, { \
              get: function () { throw new Error('getter must not fire'); }, \
              set: function () { throw new Error('setter must not fire'); }, \
              configurable: true }); \
            var target = new Int8Array([0]); \
            var receiver = {}; \
            if (Reflect.set(target, key, value, receiver) !== true) return false; \
            if (Object.prototype.hasOwnProperty.call(target, key)) return false; \
            if (Object.prototype.hasOwnProperty.call(receiver, key)) return false; \
            receiver = new Int8Array([1]); \
            if (Reflect.set(target, key, value, receiver) !== true) return false; \
            if (Object.prototype.hasOwnProperty.call(target, key)) return false; \
            if (Object.prototype.hasOwnProperty.call(receiver, key)) return false; \
            receiver = Object.defineProperty({}, key, { \
              get: function () { return 1; }, \
              set: function () { throw new Error('setter must not fire'); }, \
              configurable: true }); \
            if (Reflect.set(target, key, value, receiver) !== true) return false; \
            if (receiver[key] !== 1) return false; \
            receiver = Object.preventExtensions({}); \
            if (Reflect.set(target, key, value, receiver) !== true) return false; \
            if (Object.prototype.hasOwnProperty.call(receiver, key)) return false; \
            delete Int8Array.prototype[key]; } \
          return calls === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_ordinary_key_property() {
    // 非规范数值串（round-trip 不成）落普通属性：建属性、readback 值、
    // 二次写覆盖、defineProperty 不可写后 Reflect.set false。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var keys = ['1.0', '+1', '1000000000000000000000', '0.0000001']; \
          for (var i = 0; i < keys.length; i++) { \
            var sample = new Int8Array([42]); \
            if (Reflect.set(sample, keys[i], 'ecma262') !== true) return false; \
            if (sample[keys[i]] !== 'ecma262') return false; \
            if (Reflect.set(sample, keys[i], 'es3000') !== true) return false; \
            if (sample[keys[i]] !== 'es3000') return false; \
            Object.defineProperty(sample, keys[i], { value: undefined, writable: false, configurable: true }); \
            if (Reflect.set(sample, keys[i], 42) !== false) return false; \
            if (sample[keys[i]] !== undefined) return false; } \
          return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_valid_receiver_table() {
    // 界内规范键 + receiver ≠ 自身六臂表：空对象建属性（值原样零强转）/
    // 同长 TA 强转写 / 短 TA false 零强转 / 不可扩展 false / own accessor
    // false 不调 setter / own 不可写 false。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var calls = 0; \
          var value = { valueOf: function () { calls++; return 2.3; } }; \
          var target = new Int8Array([0]); var receiver = {}; \
          if (Reflect.set(target, 0, value, receiver) !== true) return false; \
          if (target[0] !== 0) return false; \
          if (receiver[0] !== value) return false; \
          target = new Int8Array([0]); receiver = new Int8Array([1]); \
          if (Reflect.set(target, 0, new Number(2.3), receiver) !== true) return false; \
          if (target[0] !== 0) return false; \
          if (receiver[0] !== 2) return false; \
          target = new Int8Array([0, 0]); receiver = new Int8Array([1]); \
          if (Reflect.set(target, 1, value, receiver) !== false) return false; \
          if (target[1] !== 0) return false; \
          if (Object.prototype.hasOwnProperty.call(receiver, 1)) return false; \
          target = new Int8Array([0]); receiver = Object.preventExtensions({}); \
          if (Reflect.set(target, 0, value, receiver) !== false) return false; \
          if (Object.prototype.hasOwnProperty.call(receiver, 0)) return false; \
          target = new Int8Array([0]); \
          receiver = { get 0() { return 1; }, set 0(v) { throw new Error('setter must not fire'); } }; \
          if (Reflect.set(target, 0, value, receiver) !== false) return false; \
          if (receiver[0] !== 1) return false; \
          target = new Int8Array([0]); \
          receiver = Object.defineProperty({}, 0, { value: 1, writable: false, configurable: true }); \
          if (Reflect.set(target, 0, value, receiver) !== false) return false; \
          if (receiver[0] !== 1) return false; \
          return calls === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_receiver_ta_oob_plain_obj() {
    // 普通对象 + 原型链 TA + 越界数值键：写入路由到 receiver 的 [[Set]]
    // （强转后丢弃，valueOf 恰一次），基对象上不建属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var receiver = new Int32Array(10); \
          var obj = Object.create(receiver); \
          var called = 0; \
          var value = { valueOf: function () { called++; return 1; } }; \
          if (Reflect.set(obj, 100, value, receiver) !== true) return false; \
          if (called !== 1) return false; \
          if (Object.prototype.hasOwnProperty.call(obj, 100)) return false; \
          if (Object.prototype.hasOwnProperty.call(receiver, 100)) return false; \
          return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_coerce_kind_preserved() {
    // 强转抛错 kind 保真：valueOf 抛原错误原样传播（非 TypeError 包裹）；
    // 字符串转 BigInt 非法值保 SyntaxError。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var sample = new Float64Array([42]); \
          var thrown = null; \
          try { sample['0'] = { valueOf: function () { throw new TypeError('boom'); } }; } \
          catch (e) { thrown = e; } \
          if (!(thrown instanceof TypeError) || thrown.message !== 'boom') return false; \
          var bsample = new BigInt64Array([42n]); \
          var bthrown = null; \
          try { bsample['0'] = '1.1'; } catch (e) { bthrown = e; } \
          if (!(bthrown instanceof SyntaxError)) return false; \
          return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_detach_coerce_then_silent() {
    // detach 后数值键自写：平凡值静默（不建属性）；抛错 valueOf 抛原错误
    // （kind 保真）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); \
          var sample = new BigInt64Array(ab); \
          sample[0] = 42n; \
          ab.transfer(); \
          sample[0] = 1n; \
          if (sample[0] !== undefined) return false; \
          sample['1.1'] = 1n; \
          if (sample['1.1'] !== undefined) return false; \
          var thrown = null; \
          try { sample['0'] = { valueOf: function () { throw new RangeError('detach coerce'); } }; } \
          catch (e) { thrown = e; } \
          if (!(thrown instanceof RangeError)) return false; \
          return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_set_resize_convert_first() {
    // 强转先于界判（整数键）：valueOf 期间 buffer resize 到界内，强转后
    // live 复判界内写成功（零长键 0 原越界）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(0, { maxByteLength: 1 }); \
          var ta = new Int8Array(ab); \
          var index = 0; \
          var value = { valueOf: function () { ab.resize(1); return 100; } }; \
          ta[index] = value; \
          return ta.length === 1 && ta[0] === 100; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

// ── DefineOwnProperty 数值索引臂 ───────────────────────────────────────────

#[test]
fn ta_dop_valid_write() {
    // 界内规范键 + 全 true 数据描述符：定义成功、元素写入、length 不变
    // （Reflect 与 Object.defineProperty 同臂）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array(4); \
          var r1 = Reflect.defineProperty(ta, '1', { value: 200, writable: true, enumerable: true, configurable: true }); \
          Object.defineProperty(ta, 3, { value: 7, writable: true, enumerable: true, configurable: true }); \
          return r1 === true && ta[1] === 200 && ta[3] === 7 && ta.length === 4 && ta[0] === 0 && ta[2] === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_partial_and_generic() {
    // 部分描述符（缺省字段非显式 false）与通用描述符（无 [[Value]] 读当前
    // 元素回写）均定义成功；元素值除显式 value 外不变（NaN 样本）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Float64Array(2); \
          ta[0] = NaN; ta[1] = NaN; \
          var r1 = Reflect.defineProperty(ta, '0', { value: 5 }); \
          var r2 = Reflect.defineProperty(ta, '1', { writable: true, enumerable: true, configurable: true }); \
          return r1 === true && ta[0] === 5 && r2 === true && Number.isNaN(ta[1]); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_constraint_false() {
    // 字段在场四检查（writable/enumerable/configurable 显式 false、accessor
    // 描述符）：Reflect 投影 false、元素不动、不建自有属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int16Array(2); ta[0] = 9; \
          var shapes = [ \
            { value: 3, writable: false, enumerable: true, configurable: true }, \
            { value: 3, writable: true, enumerable: false, configurable: true }, \
            { value: 3, writable: true, enumerable: true, configurable: false }, \
            { get: function () { return 1; }, set: function () {} } ]; \
          for (var i = 0; i < shapes.length; i++) { \
            if (Reflect.defineProperty(ta, '0', shapes[i]) !== false) return false; \
            if (ta[0] !== 9) return false; } \
          return Object.getOwnPropertyNames(ta).join(',') === '0,1'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_constraint_throws() {
    // 同四约束形经 Object.defineProperty：抛 TypeError、元素不动、无属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int16Array(2); ta[0] = 9; \
          var t1 = 0; \
          try { Object.defineProperty(ta, '0', { value: 3, writable: false, enumerable: true, configurable: true }); } \
          catch (e) { if (!(e instanceof TypeError)) return false; t1 = 1; } \
          var t2 = 0; \
          try { Object.defineProperty(ta, '0', { get: function () { return 1; } }); } \
          catch (e) { if (!(e instanceof TypeError)) return false; t2 = 1; } \
          return t1 === 1 && t2 === 1 && ta[0] === 9 && Object.getOwnPropertyNames(ta).join(',') === '0,1'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_numeric_invalid_false() {
    // 数字无效键（"-0"/负/越界/分数）：false、不抛、元素不动、不建属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array(2); ta[0] = 5; \
          var keys = ['-0', '-1', '2', '0.1', '0.000001', '3.5']; \
          for (var i = 0; i < keys.length; i++) { \
            var d = { value: 1, writable: true, enumerable: true, configurable: true }; \
            if (Reflect.defineProperty(ta, keys[i], d) !== false) return false; \
            if (ta[0] !== 5) return false; } \
          return Object.getOwnPropertyNames(ta).join(',') === '0,1'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_ordinary_key_property() {
    // 非规范数字串键 round-trip 不成：定义真实自有属性（数据与访问器），
    // 键 "1.0" 与 "1" 独立互不干扰。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array(1); \
          var keys = ['1.0', '+1', '1000000000000000000000', '0.0000001']; \
          for (var i = 0; i < keys.length; i++) { \
            var r = Reflect.defineProperty(ta, keys[i], { value: i + 1, writable: true, enumerable: true, configurable: true }); \
            if (r !== true) return false; \
            if (!Object.prototype.hasOwnProperty.call(ta, keys[i])) return false; \
            if (ta[keys[i]] !== i + 1) return false; } \
          var r2 = Reflect.defineProperty(ta, '1.0', { get: function () { return 'baz'; }, configurable: true }); \
          return r2 === true && ta['1.0'] === 'baz' && ta['1'] === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_detach_false_no_throw() {
    // detach 后 live 长 0：全数值键归数字无效，false 不抛（投错值不触发
    // 强转）、不建属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); ta[0] = 7; \
          ab.transfer(); \
          var keys = ['0', '-1', '1.1', '-0', '2']; \
          for (var i = 0; i < keys.length; i++) { \
            var r = Reflect.defineProperty(ta, keys[i], { \
              value: { valueOf: function () { throw new Error('no'); } }, \
              writable: true, enumerable: true, configurable: true }); \
            if (r !== false) return false; } \
          return Object.getOwnPropertyNames(ta).length === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_detach_in_valueof_true() {
    // valueOf 期 detach：强转成功、写期 live 复判 detach 静默——定义 true、
    // 元素保持 undefined。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); \
          var v = { valueOf: function () { ab.transfer(); return 9; } }; \
          var r = Reflect.defineProperty(ta, '0', { value: v, writable: true, enumerable: true, configurable: true }); \
          return r === true && ta[0] === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_value_throws_original() {
    // 强转期用户回调抛出原值经专用槽重抛：catch 收到原对象身份（构造器
    // 身份不可丢）、不建属性。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array(2); \
          function Marker() { this.tag = 'marker'; } \
          var marker = new Marker(); \
          var v = { valueOf: function () { throw marker; } }; \
          var caught = null; \
          try { Object.defineProperty(ta, '0', { value: v, writable: true, enumerable: true, configurable: true }); \
            return false; } \
          catch (e) { caught = e; } \
          return caught === marker && caught.tag === 'marker' && Object.getOwnPropertyNames(ta).join(',') === '0,1'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_bigint_content_conversion() {
    // BigInt 元素内容强转：Number 值 TypeError、非整数字符串 SyntaxError、
    // BigInt 值成功写入。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new BigInt64Array(2); \
          var t1 = 0; \
          try { Object.defineProperty(ta, '0', { value: 42, writable: true, enumerable: true, configurable: true }); } \
          catch (e) { if (!(e instanceof TypeError)) return false; t1 = 1; } \
          var t2 = 0; \
          try { Object.defineProperty(ta, '0', { value: '1.5', writable: true, enumerable: true, configurable: true }); } \
          catch (e) { if (!(e instanceof SyntaxError)) return false; t2 = 1; } \
          var r3 = Reflect.defineProperty(ta, '1', { value: 7n, writable: true, enumerable: true, configurable: true }); \
          return t1 === 1 && t2 === 1 && r3 === true && ta[1] === 7n; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_non_extensible() {
    // 不可扩展 TA：界内索引定义不受限；数字无效/新命名/新 symbol 键 false；
    // 既有真实命名属性重定义不受限。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array(2); ta[0] = 1; ta.newProp = 5; \
          Object.preventExtensions(ta); \
          var r1 = Reflect.defineProperty(ta, '1', { value: 9, writable: true, enumerable: true, configurable: true }); \
          var r2 = Reflect.defineProperty(ta, '2', { value: 9, writable: true, enumerable: true, configurable: true }); \
          var r3 = Reflect.defineProperty(ta, 'brandnew', { value: 1, writable: true, enumerable: true, configurable: true }); \
          var r4 = Reflect.defineProperty(ta, 'newProp', { value: 6, writable: true, enumerable: true, configurable: true }); \
          var r5 = Reflect.defineProperty(ta, Symbol('s'), { value: 1, writable: true, enumerable: true, configurable: true }); \
          return r1 === true && ta[1] === 9 && r2 === false && r3 === false && r4 === true && ta.newProp === 6 && r5 === false; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_int_key_same_arm() {
    // 整数键（非字符串）与字符串键同臂：界内定义成功、detach 后归数字无效。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); \
          var r1 = Reflect.defineProperty(ta, 1, { value: 4, writable: true, enumerable: true, configurable: true }); \
          if (r1 !== true || ta[1] !== 4) return false; \
          ab.transfer(); \
          var r2 = Reflect.defineProperty(ta, 0, { value: 7, writable: true, enumerable: true, configurable: true }); \
          return r2 === false; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_dop_get_dir_arm4() {
    // 非规范数字串键 + accessor 描述符：定义真实 accessor 属性、getter 经
    // 普通读路径触发（Get 目录 key-is-not-canonical-index 联绿臂）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array(2); \
          var r = Reflect.defineProperty(ta, '+1', { get: function () { return 'baz'; }, set: function () {}, configurable: true }); \
          return r === true && ta['+1'] === 'baz'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_valid_in_range_false() {
    // 界内索引删除失败（sloppy false）：元素与 length 不变，原型同键抛 getter
    // 不被触达（界内臂不查自身命名属性也不走原型链）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42, 43]); \
          Object.defineProperty(Int8Array.prototype, '0', { \
            get: function () { throw new Error('proto getter must not fire'); }, \
            configurable: true }); \
          var r0 = delete ta[0]; \
          var r1 = delete ta['0']; \
          var r2 = delete ta[1]; \
          return r0 === false && r1 === false && r2 === false \
            && ta[0] === 42 && ta[1] === 43 && ta.length === 2; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_valid_strict_typeerror() {
    // 严格模式界内索引删除抛 TypeError（构造器断言）；元素不动、length 不变。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "'use strict'; (function () { var ta = new Int8Array([42, 43]); \
          var thrown = false; \
          try { delete ta[1]; } \
          catch (e) { if (!(e instanceof TypeError)) return false; thrown = true; } \
          return thrown && ta[0] === 42 && ta[1] === 43 && ta.length === 2; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_int_minus_zero() {
    // -0 两相分臂：int 相（ta[-0] → 键 "0" 界内）删除失败；串相（"-0" 数字
    // 无效）删除成功且不建属性；元素不动。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
          var rInt = delete ta[-0]; \
          var rStr = delete ta['-0']; \
          return rInt === false && rStr === true \
            && ta[0] === 42 \
            && !Object.prototype.hasOwnProperty.call(ta, '-0'); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_oob_true() {
    // 越界 / 负索引删除成功（true）：元素不动、无自身属性、严格模式不抛。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "'use strict'; (function () { var ta = new Int8Array([42]); \
          var ok = delete ta[-1] && delete ta[1] && delete ta['-1'] && delete ta['1'] && delete ta['4294967295']; \
          var keys = ['-1', '1', '4294967295']; \
          for (var i = 0; i < keys.length; i++) { \
            if (Object.prototype.hasOwnProperty.call(ta, keys[i])) return false; } \
          return ok && ta[0] === 42 && ta.length === 1; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_numeric_invalid_strings() {
    // 数字无效串（非整数 / NaN / ±Infinity / "-0" 特例）删除恒成功：不建自身
    // 属性，原型同键抛 getter 不被触达（数字无效臂不走原型链）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
          var keys = ['1.1', '0.000001', '-0', 'Infinity', 'NaN']; \
          for (var i = 0; i < keys.length; i++) { \
            Object.defineProperty(Int8Array.prototype, keys[i], { \
              get: function () { throw new Error('OrdinaryDelete was called'); }, \
              configurable: true }); } \
          for (var i = 0; i < keys.length; i++) { \
            if (!delete ta[keys[i]]) return false; \
            if (Object.prototype.hasOwnProperty.call(ta, keys[i])) return false; } \
          return ta[0] === 42; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_ordinary_own_property() {
    // 非 round-trip 串（Ordinary 臂）落真实自身属性路径：不可配置 accessor
    // 删除失败且 getter 保留可读；可配置数据属性删除成功、hasOwn 转假。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array(2); \
          var rDef = Object.defineProperty(ta, '1.0', { \
            get: function () { return 'g'; }, configurable: false }); \
          var d1 = delete ta['1.0']; \
          var d2 = delete ta['+1']; \
          Object.defineProperty(ta, '+1', { value: 9, configurable: true }); \
          var d3 = delete ta['+1']; \
          return rDef === ta && d1 === false && ta['1.0'] === 'g' \
            && d2 === true && d3 === true \
            && !Object.prototype.hasOwnProperty.call(ta, '+1'); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_symbol_and_reflect() {
    // symbol 键缺失删除成功、建自身属性后删除成功；Reflect.deleteProperty
    // 界内索引 false 不抛、越界 true（恒不抛契约）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
          var s = Symbol('s'); \
          var r1 = delete ta[s]; \
          ta[s] = 'x'; \
          var r2 = delete ta[s]; \
          var r3 = Reflect.deleteProperty(ta, 0); \
          var r4 = Reflect.deleteProperty(ta, 9); \
          return r1 === true && r2 === true && r3 === false && r4 === true \
            && ta[0] === 42; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_delete_live_zero_all_true() {
    // detach 后 live 长 0：全数值键归数字无效，成员形与 Reflect 删除恒成功、
    // 无自身属性、不抛。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); \
           ta[0] = 7; \
           ab.transfer(); \
           var r1 = delete ta[0]; \
           var r2 = delete ta['-0']; \
           var r3 = delete ta['1.1']; \
           var r4 = delete ta['1']; \
           var r5 = Reflect.deleteProperty(ta, 0); \
           return r1 === true && r2 === true && r3 === true && r4 === true && r5 === true \
             && !Object.prototype.hasOwnProperty.call(ta, '0'); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_index_desc() {
    // gOPD 界内整数键：元素值 + 常量四字段（writable/enumerable/configurable
    // 恒真，不读 buffer 状态）；BigInt 族同形。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42, 7]); \
           var d = Object.getOwnPropertyDescriptor(ta, '0'); \
           var di = Object.getOwnPropertyDescriptor(ta, 0); \
           if (!d || d.value !== 42 || d.writable !== true \
               || d.enumerable !== true || d.configurable !== true) return false; \
           if (!di || di.value !== 42 || di.writable !== true \
               || di.enumerable !== true || di.configurable !== true) return false; \
           var b = new BigInt64Array([42n, 43n]); \
           var db = Object.getOwnPropertyDescriptor(b, 1); \
           return !!db && db.value === 43n && db.writable === true \
             && db.enumerable === true && db.configurable === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_oob_numeric_undefined() {
    // 数字无效键（越界/负/"-0"/分数）gOPD 恒 undefined：不建属性、不查形状。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
           var keys = ['-1', '-42', '1', '42', '-0', '1.1', '0.1']; \
           for (var i = 0; i < keys.length; i++) { \
             if (Object.getOwnPropertyDescriptor(ta, keys[i]) !== undefined) return false; } \
           return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_ordinary_real_prop() {
    // 非规范数字串键落真实 own 属性路径：完整描述符原样返回；无定义键
    // undefined（非 fall-through 吞属性）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
           Object.defineProperty(ta, '1.0', { value: 7 }); \
           var d = Object.getOwnPropertyDescriptor(ta, '1.0'); \
           if (!d || d.value !== 7 || d.writable !== false \
               || d.enumerable !== false || d.configurable !== false) return false; \
           return Object.getOwnPropertyDescriptor(ta, 'undef') === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_symbol_real_prop() {
    // symbol 键真实 own 属性：定义后 gOPD 返回其描述符，无定义 undefined。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
           var s = Symbol('s'); \
           if (Object.getOwnPropertyDescriptor(ta, s) !== undefined) return false; \
           Object.defineProperty(ta, s, { value: 'x', enumerable: true, configurable: false }); \
           var d = Object.getOwnPropertyDescriptor(ta, s); \
           return !!d && d.value === 'x' && d.writable === false \
             && d.enumerable === true && d.configurable === false; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_detached_undefined() {
    // detach 后 live 长 0：全数值键 gOPD 恒 undefined（元素臂不触达）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var ta = new Int8Array(ab); \
           ta[0] = 9; \
           ab.transfer(); \
           return Object.getOwnPropertyDescriptor(ta, 0) === undefined \
             && Object.getOwnPropertyDescriptor(ta, '0') === undefined \
             && Object.getOwnPropertyDescriptor(ta, '-1') === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_subarray_view_offset() {
    // subarray 视图偏移：视图下标 0 的元素是底层偏移后第 2 个元素（与
    // 元素读同口径）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var base = new Int8Array([11, 22, 33]); \
           var view = base.subarray(1); \
           var d0 = Object.getOwnPropertyDescriptor(view, 0); \
           var d1 = Object.getOwnPropertyDescriptor(view, 1); \
           return !!d0 && d0.value === 22 && !!d1 && d1.value === 33; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_length_undefined_unchanged() {
    // TA length 非 own 属性（经原型访问器读）：gOPD('length') 恒 undefined，
    // 防 TA 臂误吞 length 键。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
           return Object.getOwnPropertyDescriptor(ta, 'length') === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_int_key_same_arm() {
    // int 键与规范串键同臂：描述符值一致（ToKey 后同路）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([11, 22]); \
           var di = Object.getOwnPropertyDescriptor(ta, 1); \
           var ds = Object.getOwnPropertyDescriptor(ta, '1'); \
           return !!di && !!ds && di.value === 22 && ds.value === 22 \
             && di.value === ds.value; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_hasown_index_true() {
    // hasOwn 面：界内整数键（int 与规范串）恒 true；数字无效键（"-0"/分数）
    // 无属性 false。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42, 7]); \
           if (ta.hasOwnProperty(0) !== true) return false; \
           if (ta.hasOwnProperty('1') !== true) return false; \
           if (Object.hasOwn(ta, 0) !== true) return false; \
           if (ta.hasOwnProperty('-0') !== false) return false; \
           if (ta.hasOwnProperty('1.1') !== false) return false; \
           return ta.hasOwnProperty(2) === false; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_pie_index_true() {
    // propertyIsEnumerable 面：界内整数键恒可枚举；越界与数字无效键 false。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
           if (ta.propertyIsEnumerable(0) !== true) return false; \
           if (ta.propertyIsEnumerable('0') !== true) return false; \
           if (ta.propertyIsEnumerable('1.1') !== false) return false; \
           return ta.propertyIsEnumerable(5) === false; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_reflect_gopd() {
    // Reflect.getOwnPropertyDescriptor 经共享核消费 TA 臂：界内键元素值，
    // 越界与数字无效键 undefined。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
           var d = Reflect.getOwnPropertyDescriptor(ta, 0); \
           if (!d || d.value !== 42 || d.writable !== true \
               || d.enumerable !== true || d.configurable !== true) return false; \
           return Reflect.getOwnPropertyDescriptor(ta, 9) === undefined \
             && Reflect.getOwnPropertyDescriptor(ta, '-1') === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopd_real_prop_attrs_passthrough() {
    // Ordinary 键真属性四字段原样返回（非常量四真）：钉 fall-through 槽位路径
    // 不被 TA 臂覆盖——configurable 为 false 即非常量描述符。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([42]); \
            Object.defineProperty(ta, '1.0', { \
              value: 7, writable: true, enumerable: true, configurable: false }); \
            var d = Object.getOwnPropertyDescriptor(ta, '1.0'); \
            if (!d || d.value !== 7) return false; \
            if (d.writable !== true || d.enumerable !== true || d.configurable !== false) return false; \
            return ta.hasOwnProperty('1.0') === true \
              && ta.propertyIsEnumerable('1.0') === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_ownkeys_integer_indexes() {
    // ownKeys 元素键 = 0..live 长升序：len-3 三键、len-4 四键、subarray 视图
    // 相对下标、len-0 空、BigInt 族同形。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2, 3]); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1,2') return false; \
            if (Reflect.ownKeys(new Uint8Array(4)).join(',') !== '0,1,2,3') return false; \
            var base = new Int8Array(4); var sub = base.subarray(2); \
            if (Reflect.ownKeys(sub).join(',') !== '0,1') return false; \
            if (Reflect.ownKeys(new Uint8Array(0)).length !== 0) return false; \
            var b = new BigInt64Array([1n, 2n, 3n]); \
            return Reflect.ownKeys(b).join(',') === '0,1,2'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_ownkeys_order_indexes_strings_symbols() {
    // 三段序：整数下标升序 → 真串键创建序 → 符号创建序；原型同键（含整数
    // 索引形态）不入 ownKeys。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2]); \
            ta.test262 = 'a'; ta.ecma262 = 'b'; \
            var s1 = Symbol('s1'), s2 = Symbol('s2'); \
            ta[s1] = 1; ta[s2] = 2; \
            Uint8Array.prototype[3] = 'x'; Uint8Array.prototype.bar = 'y'; \
            var ks = Reflect.ownKeys(ta); \
            return ks.length === 6 && ks[0] === '0' && ks[1] === '1' \
              && ks[2] === 'test262' && ks[3] === 'ecma262' && ks[4] === s1 && ks[5] === s2; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_keys_values_entries_strings_only() {
    // keys = 下标 + 可枚举真串键（符号不入、e:false 真属性不入）；values/entries
    // 同序含元素值与真属性值。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2]); \
            ta.foo = 42; \
            Object.defineProperty(ta, 'bar', { value: 7, enumerable: false }); \
            if (Object.keys(ta).join(',') !== '0,1,foo') return false; \
            var vs = Object.values(ta); \
            if (vs.length !== 3 || vs[0] !== 1 || vs[1] !== 2 || vs[2] !== 42) return false; \
            var es = Object.entries(ta); \
            return es.length === 3 && es[0][0] === '0' && es[0][1] === 1 \
              && es[1][0] === '1' && es[1][1] === 2 && es[2][0] === 'foo' && es[2][1] === 42; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_gopn_gopds_include_indexes() {
    // getOwnPropertyNames = 下标 + 真串键；getOwnPropertyDescriptors 下标描述符
    // 常量四字段（value + w/e/c 全真），真属性原样。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2]); \
            ta.foo = 42; \
            if (Object.getOwnPropertyNames(ta).join(',') !== '0,1,foo') return false; \
            var ds = Object.getOwnPropertyDescriptors(ta); \
            var d0 = ds['0']; \
            if (!d0 || d0.value !== 1 || d0.writable !== true \
                || d0.enumerable !== true || d0.configurable !== true) return false; \
            var dfoo = ds['foo']; \
            return !!dfoo && dfoo.value === 42 \
              && dfoo.writable === true && dfoo.enumerable === true && dfoo.configurable === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_for_in_indexes_then_strings() {
    // for-in = 下标升序 + 真串键（符号不入）；len-0 与 detach 形仅真属性、无下标。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2]); \
            ta.foo = 42; var s = Symbol('s'); ta[s] = 1; \
            var ks = []; for (var k in ta) ks.push(k); \
            if (ks.length !== 3 || ks[0] !== '0' || ks[1] !== '1' || ks[2] !== 'foo') return false; \
            var e = new Uint8Array(0); \
            var ke = 0; for (var k2 in e) ke++; \
            if (ke !== 0) return false; \
            var ab = new ArrayBuffer(8); var d = new Uint8Array(ab); d.foo = 1; \
            ab.transfer(); \
            var kd = []; for (var k3 in d) kd.push(k3); \
            return kd.length === 1 && kd[0] === 'foo'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_assign_spread_rest_copy_indexes() {
    // Object.assign / 展开 / 解构 rest 均拷下标值 + 可枚举真属性（rest 排除
    // 已绑定键）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Uint8Array([1, 2]); \
            ta.foo = 9; \
            var o1 = Object.assign({}, ta); \
            if (Object.keys(o1).sort().join(',') !== '0,1,foo') return false; \
            if (o1['0'] !== 1 || o1['1'] !== 2 || o1.foo !== 9) return false; \
            var o2 = { ...ta }; \
            if (o2['0'] !== 1 || o2['1'] !== 2 || o2.foo !== 9) return false; \
            var { foo, ...rest } = ta; \
            return Object.keys(rest).sort().join(',') === '0,1' \
              && rest['0'] === 1 && rest['1'] === 2 && rest.foo === undefined; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_resize_auto_follows_live_length() {
    // auto 视图 ownKeys 跟随 live 长：grow/shrink/边界（maxByteLength）四步；
    // offset 超 buffer 越界形零枚。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(4, { maxByteLength: 12 }); \
            var ta = new Uint8Array(ab); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1,2,3') return false; \
            ab.resize(8); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1,2,3,4,5,6,7') return false; \
            ab.resize(2); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1') return false; \
            ab.resize(12); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1,2,3,4,5,6,7,8,9,10,11') return false; \
            var v = new Uint8Array(ab, 12); \
            return Reflect.ownKeys(v).length === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_resize_fixed_clip_oob_empty() {
    // fixed 视图（offset 2 len 2）：grow 与窗口内 shrink 键集保持 0,1；
    // buffer 收缩裁出窗口即越界零枚。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8, { maxByteLength: 16 }); \
            var ta = new Uint8Array(ab, 2, 2); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1') return false; \
            ab.resize(16); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1') return false; \
            ab.resize(4); \
            if (Reflect.ownKeys(ta).join(',') !== '0,1') return false; \
            ab.resize(3); \
            return Reflect.ownKeys(ta).length === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_detached_empty_enum() {
    // detach 后 live 长 0：ownKeys/keys/values/for-in 仅真属性（序不变、无下标），
    // 真属性值不动。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); \
            var ta = new Uint8Array(ab); \
            ta.foo = 42; var s = Symbol('s'); ta[s] = 5; \
            ab.transfer(); \
            var ks = Reflect.ownKeys(ta); \
            if (ks.length !== 2 || ks[0] !== 'foo' || ks[1] !== s) return false; \
            if (Object.keys(ta).join(',') !== 'foo') return false; \
            var vs = Object.values(ta); \
            if (vs.length !== 1 || vs[0] !== 42) return false; \
            var kf = []; for (var k in ta) kf.push(k); \
            return kf.length === 1 && kf[0] === 'foo' && ta.foo === 42; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn ta_integrity_freeze_seal_no_element_touch() {
    // 完整性家族跳键：seal 后 isSealed 真、元素写仍通、加真属性失败、键集不变、
    // 无损坏（读/长/缓冲不动）；freeze len-0 → isFrozen 真；fresh len>0 非冻结；
    // PE 的 TA 判 sealed（元素键描述符 c:true 口径）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ta = new Int8Array([1, 2]); \
            Object.seal(ta); \
            if (Object.isSealed(ta) !== true) return false; \
            ta[0] = 9; \
            if (ta[0] !== 9 || ta[1] !== 2 || ta.length !== 2) return false; \
            if (Reflect.defineProperty(ta, 'x', { value: 1, configurable: true }) !== false) return false; \
            if (Reflect.ownKeys(ta).join(',') !== '0,1') return false; \
            var f = new Int8Array(0); \
            Object.freeze(f); \
            if (Object.isFrozen(f) !== true) return false; \
            var t2 = new Int8Array([5, 6]); \
            if (Object.isFrozen(t2) !== false || Object.isSealed(t2) !== false) return false; \
            var t3 = new Int8Array(2); \
            Object.preventExtensions(t3); \
            return Object.isSealed(t3) === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}
