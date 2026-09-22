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

/// 原型 byteLength 描述符钉：get 为函数、set 缺失、不可枚举、可配置。
#[test]
fn ab_bytelength_proto_is_accessor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
         typeof d.get === 'function' && d.set === undefined \
         && d.enumerable === false && d.configurable === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 访问器 getter 函数对象元数据钉：name 标签与 length。
#[test]
fn ab_bytelength_getter_name_and_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
         d.get.name === 'get byteLength' && d.get.length === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 访问器返回值钉：构造长度经原型访问器读回。
#[test]
fn ab_bytelength_returns_constructed_len() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new ArrayBuffer(0).byteLength === 0 && new ArrayBuffer(42).byteLength === 42").unwrap();
    assert!(result.as_bool());
}

/// receiver 校验钉：非 ArrayBuffer this（原型自身 / undefined）抛 TypeError。
#[test]
fn ab_bytelength_this_checks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
         var proto_threw = false; var undef_threw = false; \
         try { ArrayBuffer.prototype.byteLength } catch (e) { proto_threw = e instanceof TypeError; } \
         try { d.get.call(undefined) } catch (e) { undef_threw = e instanceof TypeError; } \
         proto_threw && undef_threw",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 实例无 own byteLength 属性钉（构造器不写数据属性）。
#[test]
fn ab_instance_has_no_own_bytelength() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object.getOwnPropertyNames(new ArrayBuffer(8)).length === 0").unwrap();
    assert!(result.as_bool());
}

/// 可 resize 实例 own maxByteLength 数据属性钉：描述符 {value, w:0, e:0, c:0}；
/// 定长实例无该 own 属性。
#[test]
fn ab_ctor_options_own_max_property() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, {maxByteLength: 8}); \
         var d = Object.getOwnPropertyDescriptor(ab, 'maxByteLength'); \
         d !== undefined && d.value === 8 && d.writable === false \
         && d.enumerable === false && d.configurable === false \
         && Object.getOwnPropertyDescriptor(new ArrayBuffer(4), 'maxByteLength') === undefined",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// options 非对象钉：null/boolean/symbol/bigint/string/number/undefined 一律定长。
#[test]
fn ab_ctor_options_non_object() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "[null, true, Symbol(3), 1n, 'string', 9, undefined] \
         .every(function (o) { return new ArrayBuffer(0, o).resizable === false; })",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// options.maxByteLength 为 undefined 或 options 为空对象 → 定长。
#[test]
fn ab_ctor_options_max_undefined() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "new ArrayBuffer(0, {}).resizable === false \
         && new ArrayBuffer(0, {maxByteLength: undefined}).resizable === false",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// length > maxByteLength → RangeError。
#[test]
fn ab_ctor_len_gt_max_range_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false; \
         try { new ArrayBuffer(1, {maxByteLength: 0}); } catch (e) { t1 = e instanceof RangeError; } \
         try { new ArrayBuffer(5, {maxByteLength: 4}); } catch (e) { t2 = e instanceof RangeError; } \
         t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// maxByteLength: 0 是合法可 resize 上限（非定长）：resizable true、
/// resize(0) 为 no-op、resize(1) 越界 RangeError。
#[test]
fn ab_ctor_max_zero_is_resizable() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(0, {maxByteLength: 0}); \
         var ok1 = ab.resizable === true && ab.maxByteLength === 0; \
         ab.resize(0); \
         var ok2 = ab.byteLength === 0 && ab.resizable === true; \
         var threw = false; \
         try { ab.resize(1); } catch (e) { threw = e instanceof RangeError; } \
         ok1 && ok2 && threw",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// options.maxByteLength 经 ToIndex 强转：字符串 '8' → 8，分数 4.5 → 截断 4。
#[test]
fn ab_ctor_max_coerced() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = new ArrayBuffer(4, {maxByteLength: '8'}); \
         var f = new ArrayBuffer(4, {maxByteLength: 4.5}); \
         s.resizable === true && s.maxByteLength === 8 \
         && f.resizable === true && f.maxByteLength === 4",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// options.maxByteLength 访问器抛错 → 原异常值传播。
#[test]
fn ab_ctor_max_poisoned_propagates() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var options = { get maxByteLength() { throw new TypeError('poisoned'); } }; \
         var threw = false; \
         try { new ArrayBuffer(0, options); } catch (e) { threw = e instanceof TypeError && e.message === 'poisoned'; } \
         threw",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// options.maxByteLength 为 Symbol → TypeError；对象形强转序 valueOf → toString。
#[test]
fn ab_ctor_max_symbol_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var threw = false; \
         try { new ArrayBuffer(0, {maxByteLength: Symbol()}); } catch (e) { threw = e instanceof TypeError; } \
         var log = []; \
         var opts = { maxByteLength: { \
           toString: function() { log.push('toString'); return {}; }, \
           valueOf: function() { log.push('valueOf'); return {}; } } }; \
         try { new ArrayBuffer(0, opts); } catch (e) {} \
         threw && log.length === 2 && log[0] === 'valueOf' && log[1] === 'toString'",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// length 参 ToIndex 语义钉：负分数截 0、字符串/NaN/分数按截断。
#[test]
fn ab_ctor_length_toindex() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "new ArrayBuffer(-0.1).byteLength === 0 \
         && new ArrayBuffer('42').byteLength === 42 \
         && new ArrayBuffer(NaN).byteLength === 0 \
         && new ArrayBuffer(0.9).byteLength === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// length 参强转期抛错 → 原异常值传播。
#[test]
fn ab_ctor_length_abort_propagates() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var len = { valueOf: function() { throw new RangeError('abort'); } }; \
         var threw = false; \
         try { new ArrayBuffer(len); } catch (e) { threw = e instanceof RangeError && e.message === 'abort'; } \
         threw",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 分配期上界钉：2^32（过 ToIndex 后撞引擎上限）与 2^53（ToLength 夹取失配）均 RangeError。
#[test]
fn ab_ctor_length_alloc_limit() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false; \
         try { new ArrayBuffer(2 ** 32); } catch (e) { t1 = e instanceof RangeError; } \
         try { new ArrayBuffer(2 ** 53); } catch (e) { t2 = e instanceof RangeError; } \
         t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// GetPrototypeFromConstructor 钉：newTarget 为 Object → 原型取 Object.prototype；
/// newTarget.prototype 非对象 → 回落 %ArrayBuffer.prototype%。
#[test]
fn ab_ctor_newtarget_proto() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "Object.getPrototypeOf(Reflect.construct(ArrayBuffer, [8], Object)) === Object.prototype \
         && (function () { function nt() {} nt.prototype = undefined; \
              return Object.getPrototypeOf(Reflect.construct(ArrayBuffer, [1], nt)) === ArrayBuffer.prototype; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 普通调用形态（new.target 缺失）→ TypeError。
#[test]
fn ab_ctor_call_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false; \
         try { ArrayBuffer(); } catch (e) { t1 = e instanceof TypeError; } \
         try { ArrayBuffer(10); } catch (e) { t2 = e instanceof TypeError; } \
         t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 语义序钉：原型读先于分配期上界拒绝（7PiB length + 抛错 prototype getter）。
#[test]
fn ab_ctor_proto_before_alloc() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function DummyError() {} \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { get: function() { throw new DummyError(); } }); \
         var threw = false; \
         try { Reflect.construct(ArrayBuffer, [7 * 1125899906842624], newTarget); } catch (e) { threw = e instanceof DummyError; } \
         threw",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// resize grow 钉：前缀保留 + 零填充，视图 live 见新长度。
#[test]
fn ab_resize_grow_preserves_and_zeros() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(2, {maxByteLength: 5}); \
         var view = new Uint8Array(ab); view[0] = 1; view[1] = 2; \
         ab.resize(5); \
         var v2 = new Uint8Array(ab); \
         ab.byteLength === 5 && v2.length === 5 \
         && v2[0] === 1 && v2[1] === 2 && v2[2] === 0 && v2[3] === 0 && v2[4] === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// resize shrink 钉：截断丢尾字节，再 grow 前缀保留 + 零填充。
#[test]
fn ab_resize_shrink_truncate() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, {maxByteLength: 8}); \
         var view = new Uint8Array(ab); view[0] = 1; view[1] = 2; \
         ab.resize(1); \
         var ok1 = ab.byteLength === 1 && new Uint8Array(ab).length === 1; \
         ab.resize(4); \
         var v2 = new Uint8Array(ab); \
         ok1 && ab.byteLength === 4 && v2[0] === 1 && v2[1] === 0 && v2[2] === 0 && v2[3] === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// resize(0) 不 detach 钉：byteLength 0、resizable 保持、slice 不抛、可再 grow。
#[test]
fn ab_resize_zero_stays_attached() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(2, {maxByteLength: 4}); \
         ab.resize(0); \
         var ok1 = ab.byteLength === 0 && ab.resizable === true && ab.slice().byteLength === 0; \
         ab.resize(2); \
         ok1 && ab.byteLength === 2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// resize 界判钉：负值/超 max → RangeError；分数截断、NaN → 0。
#[test]
fn ab_resize_range_checks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, {maxByteLength: 8}); \
         var t1 = false, t2 = false; \
         try { ab.resize(-1); } catch (e) { t1 = e instanceof RangeError; } \
         try { ab.resize(9); } catch (e) { t2 = e instanceof RangeError; } \
         ab.resize(2.5); \
         var t3 = ab.byteLength === 2; \
         ab.resize(NaN); \
         var t4 = ab.byteLength === 0; \
         ab.resize(-0.5); \
         t1 && t2 && t3 && t4 && ab.byteLength === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 定长缓冲 resize 钉：0/3/4/5 四形均 TypeError，长度不变。
#[test]
fn ab_resize_non_resizable_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4); \
         var t0 = false, t3 = false, t4 = false, t5 = false; \
         try { ab.resize(0); } catch (e) { t0 = e instanceof TypeError; } \
         try { ab.resize(3); } catch (e) { t3 = e instanceof TypeError; } \
         try { ab.resize(4); } catch (e) { t4 = e instanceof TypeError; } \
         try { ab.resize(5); } catch (e) { t5 = e instanceof TypeError; } \
         t0 && t3 && t4 && t5 && ab.byteLength === 4",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// resize this 品牌校验钉：原型/undefined/{}/[] 四形均 TypeError。
#[test]
fn ab_resize_this_checks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false, t3 = false, t4 = false; \
         try { ArrayBuffer.prototype.resize(); } catch (e) { t1 = e instanceof TypeError; } \
         try { ArrayBuffer.prototype.resize.call(undefined); } catch (e) { t2 = e instanceof TypeError; } \
         try { ArrayBuffer.prototype.resize.call({}); } catch (e) { t3 = e instanceof TypeError; } \
         try { ArrayBuffer.prototype.resize.call([]); } catch (e) { t4 = e instanceof TypeError; } \
         t1 && t2 && t3 && t4",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// resizable 访问器钉：描述符 {get 函数, set undefined, e:0, c:1}、name/length、
/// 定长 false / 可 resize true、this 校验两形。
#[test]
fn ab_resizable_accessor_pins() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable'); \
         var ok = d.set === undefined && typeof d.get === 'function' \
           && d.enumerable === false && d.configurable === true \
           && d.get.name === 'get resizable' && d.get.length === 0; \
         var fixed = new ArrayBuffer(1); \
         var resizable = new ArrayBuffer(1, {maxByteLength: 1}); \
         var getter = d.get; \
         var t1 = false, t2 = false; \
         try { getter.call({}); } catch (e) { t1 = e instanceof TypeError; } \
         try { getter.call(undefined); } catch (e) { t2 = e instanceof TypeError; } \
         ok && fixed.resizable === false && resizable.resizable === true && t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}
