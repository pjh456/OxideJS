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

/// BigInt 参钉：ctor 与 slice 单参/双参/中间参四形均抛 TypeError。
#[test]
fn ab_bigint_argument_type_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "new ArrayBuffer(8n)").unwrap_err();
    assert!(err.contains("TypeError"), "ctor: {}", err);
    for src in [
        "new ArrayBuffer(8).slice(2n)",
        "new ArrayBuffer(8).slice(2n, 4n)",
        "new ArrayBuffer(8).slice(1n, 2n)",
    ] {
        let err = eval(&mut vm, src).unwrap_err();
        assert!(err.contains("TypeError"), "{}: {}", src, err);
    }
}

/// valueOf 异常透传钉：ctor 与 slice 的转换异常保留原异常值。
#[test]
fn ab_value_of_abort_passes_through() {
    let mut vm = Vm::new();
    let v = eval(&mut vm, "try { new ArrayBuffer({valueOf(){ throw 42 }}) } catch (e) { e }").unwrap();
    assert_eq!(v.as_int(), 42);
    let v = eval(&mut vm, "try { new ArrayBuffer(8).slice({valueOf(){ throw 7 }}) } catch (e) { e }").unwrap();
    assert_eq!(v.as_int(), 7);
}

/// fold 保留钉：slice ±Infinity 饱和 0/len，Number 分支语义不回退。
#[test]
fn ab_slice_infinity_fold_preserved() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "new ArrayBuffer(8).slice(Infinity).byteLength === 0 \
         && new ArrayBuffer(8).slice(-Infinity).byteLength === 8 \
         && new ArrayBuffer(8).slice(-Infinity, 3).byteLength === 3 \
         && new ArrayBuffer(8).slice(2, Infinity).byteLength === 6",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// RangeError 不回退钉：负值/超引擎上限抛 RangeError，分数截断、字符串强转。
#[test]
fn ab_length_range_error_not_regressed() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false; \
         try { new ArrayBuffer(-1); } catch (e) { t1 = e instanceof RangeError; } \
         try { new ArrayBuffer(2 ** 40); } catch (e) { t2 = e instanceof RangeError; } \
         t1 && t2 \
         && new ArrayBuffer(4.9).byteLength === 4 \
         && new ArrayBuffer('8').byteLength === 8",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// maxByteLength 访问器钉：描述符 {get 函数, set undefined, e:0, c:1}、
/// name/length、定长 42→42 / 0→0、resizable(4,{max 8})→8、detached→0、
/// this 四形 TypeError。
#[test]
fn ab_maxbytelength_accessor_pins() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'maxByteLength'); \
         var ok = d.set === undefined && typeof d.get === 'function' \
           && d.enumerable === false && d.configurable === true \
           && d.get.name === 'get maxByteLength' && d.get.length === 0; \
         var t1 = false, t2 = false, t3 = false, t4 = false; \
         try { d.get.call(ArrayBuffer.prototype); } catch (e) { t1 = e instanceof TypeError; } \
         try { d.get.call(undefined); } catch (e) { t2 = e instanceof TypeError; } \
         try { d.get.call({}); } catch (e) { t3 = e instanceof TypeError; } \
         try { d.get.call([]); } catch (e) { t4 = e instanceof TypeError; } \
         ok && t1 && t2 && t3 && t4 \
           && new ArrayBuffer(42).maxByteLength === 42 \
           && new ArrayBuffer(0).maxByteLength === 0 \
           && new ArrayBuffer(4, {maxByteLength: 8}).maxByteLength === 8",
    )
    .unwrap();
    assert!(result.as_bool());
    let result =
        eval(&mut vm, "var ab = new ArrayBuffer(1); $262.detachArrayBuffer(ab); ab.maxByteLength === 0").unwrap();
    assert!(result.as_bool());
}

/// immutable 访问器钉：描述符形、markImmutable 前后 false→true、this 两形。
#[test]
fn ab_immutable_accessor_pins() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'immutable'); \
         var ok = d.set === undefined && typeof d.get === 'function' \
           && d.enumerable === false && d.configurable === true \
           && d.get.name === 'get immutable' && d.get.length === 0; \
         var ab = new ArrayBuffer(2); \
         var before = ab.immutable; \
         ab.markImmutable(); \
         var t1 = false, t2 = false; \
         try { d.get.call({}); } catch (e) { t1 = e instanceof TypeError; } \
         try { d.get.call(undefined); } catch (e) { t2 = e instanceof TypeError; } \
         ok && before === false && ab.immutable === true && t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// detached 访问器钉：描述符形、附着 false → $262 detach → true、this 两形。
#[test]
fn ab_detached_accessor_pins() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'detached'); \
         var ok = d.set === undefined && typeof d.get === 'function' \
           && d.enumerable === false && d.configurable === true \
           && d.get.name === 'get detached' && d.get.length === 0; \
         var ab = new ArrayBuffer(1); \
         var before = ab.detached; \
         $262.detachArrayBuffer(ab); \
         var t1 = false, t2 = false; \
         try { d.get.call({}); } catch (e) { t1 = e instanceof TypeError; } \
         try { d.get.call(undefined); } catch (e) { t2 = e instanceof TypeError; } \
         ok && before === false && ab.detached === true && t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// byteLength detached 臂改点钉：detach 后读 0（原 TypeError 臂）。
#[test]
fn ab_bytelength_detached_zero() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var ab = new ArrayBuffer(1); $262.detachArrayBuffer(ab); ab.byteLength === 0").unwrap();
    assert!(result.as_bool());
}

/// resizable detached 不受影响钉：resizable(1,{max 1}) detach 后仍 true。
#[test]
fn ab_resizable_detached_unchanged() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(1, {maxByteLength: 1}); $262.detachArrayBuffer(ab); ab.resizable === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// own maxByteLength 数据属性与原型访问器并存读同值钉（resizable 专属）。
#[test]
fn ab_maxbytelength_own_still_resizable_only() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, {maxByteLength: 8}); \
         Object.getOwnPropertyDescriptor(ab, 'maxByteLength').value === 8 && ab.maxByteLength === 8",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// markImmutable 返回接收者并置位钉：返回 === ab、immutable true、二调幂等、
/// byteLength 不变。
#[test]
fn ab_markimmutable_returns_this_and_sets_flag() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4); \
         var ret = ab.markImmutable(); \
         ret === ab && ab.immutable === true && ab.markImmutable() === ab \
         && ab.immutable === true && ab.byteLength === 4",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// markImmutable resizable 源钉：TypeError。
#[test]
fn ab_markimmutable_resizable_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t = false; \
         try { new ArrayBuffer(4, {maxByteLength: 8}).markImmutable(); } catch (e) { t = e instanceof TypeError; } \
         t",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// markImmutable detached 源钉：$262 detach 后 TypeError。
#[test]
fn ab_markimmutable_detached_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4); $262.detachArrayBuffer(ab); \
         var t = false; \
         try { ab.markImmutable(); } catch (e) { t = e instanceof TypeError; } \
         t",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// markImmutable this 品牌校验钉：原型/undefined/{}/[] 四形 TypeError。
#[test]
fn ab_markimmutable_this_checks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false, t3 = false, t4 = false; \
         try { ArrayBuffer.prototype.markImmutable(); } catch (e) { t1 = e instanceof TypeError; } \
         try { ArrayBuffer.prototype.markImmutable.call(undefined); } catch (e) { t2 = e instanceof TypeError; } \
         try { ArrayBuffer.prototype.markImmutable.call({}); } catch (e) { t3 = e instanceof TypeError; } \
         try { ArrayBuffer.prototype.markImmutable.call([]); } catch (e) { t4 = e instanceof TypeError; } \
         t1 && t2 && t3 && t4",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// P2-1 门钉：TA 界内整数键 detached 前 NumericValid(0)、detached 后
/// NumericInvalid（232.19.2 Get 臂 detached 界内键用例的前置钉）。
#[test]
fn ab_ta_view_length_detached_zero() {
    let mut vm = Vm::new();
    eval(
        &mut vm,
        "var ab = new ArrayBuffer(8); \
         globalThis.__ab = ab; globalThis.__ta = new Uint8Array(ab); true",
    )
    .unwrap();
    let si = vm.kernel_core().perm_interner().intern("0").0;
    let ta_val = eval(&mut vm, "globalThis.__ta").unwrap();
    // SAFETY: __ta 为已晋升 session 的 TypedArray 对象，借用止于断言、不跨下一次 eval。
    let ta_obj = unsafe { &*ta_val.as_js_object_ptr() };
    assert_eq!(
        oxide_builtins::typed_array::ta_index_gate(&vm, ta_obj, si),
        oxide_builtins::typed_array::TaIndexGate::NumericValid(0)
    );
    eval(&mut vm, "$262.detachArrayBuffer(globalThis.__ab); true").unwrap();
    let ta_val = eval(&mut vm, "globalThis.__ta").unwrap();
    let ta_obj = unsafe { &*ta_val.as_js_object_ptr() };
    assert_eq!(
        oxide_builtins::typed_array::ta_index_gate(&vm, ta_obj, si),
        oxide_builtins::typed_array::TaIndexGate::NumericInvalid
    );
}

/// transfer 拷贝/增减/零填充/源 detach 钉：grow 补零、same 原样、shrink
/// 截断；transfer(0) 产物附着 0 长（区别于 detach）。
#[test]
fn ab_transfer_copies_grows_shrinks_and_detaches() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function fill(ab) { var v = new Uint8Array(ab); v[0]=1; v[1]=2; v[2]=3; v[3]=4; return ab; } \
         var a = new Uint8Array(fill(new ArrayBuffer(4)).transfer(5)); \
         a[0]===1 && a[1]===2 && a[2]===3 && a[3]===4 && a[4]===0 \
         && new Uint8Array(fill(new ArrayBuffer(4)).transfer(4)).length === 4 \
         && new Uint8Array(fill(new ArrayBuffer(4)).transfer(2)).join(',') === '1,2' \
         && fill(new ArrayBuffer(4)).transfer(0).detached === false \
         && fill(new ArrayBuffer(4)).transfer(0).byteLength === 0",
    )
    .unwrap();
    assert!(result.as_bool());
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4); ab.transfer(2); ab.detached === true && ab.byteLength === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// transfer 保持性钉：resizable 源 → 产物 resizable 同上限，产物附着。
#[test]
fn ab_transfer_preserves_resizability() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var dest = new ArrayBuffer(4, {maxByteLength: 8}).transfer(5); \
         dest.resizable === true && dest.maxByteLength === 8 \
         && dest.detached === false && dest.byteLength === 5",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// transferToFixedLength 钉：resizable 源 → 产物恒定长、源 detach。
#[test]
fn ab_transfer_to_fixed_length_fixed() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, {maxByteLength: 8}); \
         var dest = ab.transferToFixedLength(6); \
         dest.resizable === false && dest.maxByteLength === 6 && dest.byteLength === 6 \
         && ab.detached === true && ab.byteLength === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// transferToImmutable 钉：产物 immutable + 恒定长、源 detach；显式 longer
/// 零填充。
#[test]
fn ab_transfer_to_immutable_sets_flag() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4); \
         var dest = ab.transferToImmutable(); \
         dest.immutable === true && dest.resizable === false && ab.detached === true \
         && new Uint8Array(new ArrayBuffer(4).transferToImmutable(9)).length === 9 \
         && new Uint8Array(new ArrayBuffer(4).transferToImmutable(9))[8] === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// newLength 求值序与强转钉：valueOf→toString 序 TypeError、2^53 RangeError、
/// valueOf 内 detach 后走 detached 判定（求值先于守卫）。
#[test]
fn ab_transfer_new_length_order_and_coercion() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var log = []; \
         var nl = { toString() { log.push('toString'); return {}; }, \
                    valueOf() { log.push('valueOf'); return {}; } }; \
         var t1 = false; \
         try { new ArrayBuffer(0).transfer(nl); } catch (e) { t1 = e instanceof TypeError; } \
         t1 && log.length === 2 && log[0] === 'valueOf' && log[1] === 'toString' \
         && (function () { var t2 = false, t3 = false, e2 = '?'; \
              try { new ArrayBuffer(0).transfer(2 ** 53); } catch (e) { t2 = e instanceof RangeError; } \
              return t2; })() \
         && (function () { var ab = new ArrayBuffer(8); var t3 = false; \
              try { ab.transfer({ valueOf() { $262.detachArrayBuffer(ab); return 1; } }); } \
              catch (e) { t3 = e instanceof TypeError; } \
              return t3; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// detached 源钉：三方法同形 TypeError。
#[test]
fn ab_transfer_detached_source_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = new ArrayBuffer(4); a.transfer(); \
         var t1 = false, t2 = false, t3 = false; \
         try { a.transfer(); } catch (e) { t1 = e instanceof TypeError; } \
         try { a.transferToFixedLength(); } catch (e) { t2 = e instanceof TypeError; } \
         try { a.transferToImmutable(); } catch (e) { t3 = e instanceof TypeError; } \
         t1 && t2 && t3",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// immutable 源钉：三方法同形 TypeError，newLength 读取先于 mutability 守卫。
#[test]
fn ab_transfer_immutable_source_type_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var iab = new ArrayBuffer(4).transferToImmutable(); \
         var calls = []; \
         var nl = { valueOf() { calls.push('valueOf'); return 1; } }; \
         var t1 = false, t2 = false, t3 = false; \
         try { iab.transfer(nl); } catch (e) { t1 = e instanceof TypeError; } \
         try { iab.transferToFixedLength(); } catch (e) { t2 = e instanceof TypeError; } \
         try { iab.transferToImmutable(); } catch (e) { t3 = e instanceof TypeError; } \
         t1 && t2 && t3 && calls.length === 1 && calls[0] === 'valueOf'",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 界校验钉：resizable 源超真实上限 RangeError、超引擎分配上界 RangeError。
#[test]
fn ab_transfer_range_and_alloc_limits() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false; \
         try { new ArrayBuffer(4, {maxByteLength: 8}).transfer(9); } catch (e) { t1 = e instanceof RangeError; } \
         try { new ArrayBuffer(0).transfer(2 ** 32); } catch (e) { t2 = e instanceof RangeError; } \
         t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 窗口钉：全窗/前缀/后缀/负 start/负 end/start 越界/end 越界/
/// start 超 end/空窗九形长度，源不 detach。
#[test]
fn ab_slice_window_pins() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); \
         var bl = function (s) { return ab.slice(s[0], s[1]).byteLength; }; \
         return bl([undefined, undefined]) === 8 && bl([0, 3]) === 3 && bl([6, undefined]) === 2 \
         && bl([-2]) === 2 && bl([0, -2]) === 6 && bl([20]) === 0 \
         && bl([2, 20]) === 6 && bl([5, 2]) === 0 && bl([3, 3]) === 0 \
         && ab.detached === false })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 空窗钉：产物附着 0 长（区别于 detach 空缓冲）。
#[test]
fn ab_slice_empty_window_not_detached() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var r = new ArrayBuffer(4).slice(3, 3); \
         return r.byteLength === 0 && r.detached === false && r.immutable === false })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 内容钉：[1..8] 源 slice(2,6) → [3,4,5,6]，产物独立于源。
#[test]
fn ab_slice_content_copy() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); var v = new Uint8Array(ab); \
         for (var i = 0; i < 8; i++) v[i] = i + 1; \
         var d = new Uint8Array(ab.slice(2, 6)); \
         return d.length === 4 && d[0] === 3 && d[1] === 4 && d[2] === 5 && d[3] === 6 \
         && (v[0] = 90, new Uint8Array(ab.slice(2, 6))[0] === 3) })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 构造窗内源收缩钉：species 构造器内 resize(1) 后返回 AB(8) →
/// 不抛、结果 8 长、前 1 字节为源现存、余零填充（活 currentLen 夹拷贝）。
#[test]
fn ab_slice_source_shrink_in_construct() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8, {maxByteLength: 16}); \
         var v = new Uint8Array(ab); v[0] = 9; \
         var c = {}; c[Symbol.species] = function (len) { \
             ab.resize(1); return new ArrayBuffer(8); }; \
         ab.constructor = c; \
         var r = ab.slice(); \
         var d = new Uint8Array(r); \
         return r.byteLength === 8 && ab.byteLength === 1 \
         && d[0] === 9 && d[1] === 0 && d[6] === 0 && d[7] === 0 })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 物种回落三形钉：constructor undefined / species undefined /
/// species null → 默认原型 + 内容正确（防回归钉，语料现巧合绿）。
#[test]
fn ab_slice_species_default_fallbacks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var ab1 = new ArrayBuffer(8); ab1.constructor = undefined; \
         var r1 = ab1.slice(); \
         var c2 = {}; var ab2 = new ArrayBuffer(8); ab2.constructor = c2; \
         var r2 = ab2.slice(); \
         var c3 = {}; c3[Symbol.species] = null; var ab3 = new ArrayBuffer(8); ab3.constructor = c3; \
         var r3 = ab3.slice(); \
         var v = new Uint8Array(ab1); v[0] = 7; \
         var d = new Uint8Array(ab1.slice()); \
         return Object.getPrototypeOf(r1) === ArrayBuffer.prototype && r1.byteLength === 8 \
         && Object.getPrototypeOf(r2) === ArrayBuffer.prototype \
         && Object.getPrototypeOf(r3) === ArrayBuffer.prototype \
         && d[0] === 7 })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 物种交付钉：构造器实参 «8» 逐次捕获、返回自建 AB → 交付原值且
/// 内容拷贝入产物。
#[test]
fn ab_slice_species_custom_returns() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var calls = []; var c = {}; \
         c[Symbol.species] = function (len) { calls.push(len); return new ArrayBuffer(8); }; \
         var ab = new ArrayBuffer(8); var v = new Uint8Array(ab); \
         for (var i = 0; i < 8; i++) v[i] = i + 1; \
         ab.constructor = c; \
         var r = ab.slice(); \
         var d = new Uint8Array(r); \
         return r.byteLength === 8 && calls.join(',') === '8' \
         && d[0] === 1 && d[7] === 8 && ab.detached === false })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 物种 TypeError 五形钉：constructor null/true/\"\"、species
/// 对象/Function.prototype 均 TypeError。
#[test]
fn ab_slice_species_type_error_family() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { function ty(fn) { try { fn(); return false; } catch (e) { return e instanceof TypeError; } } \
         var ab = new ArrayBuffer(8); \
         ab.constructor = null; var t1 = ty(function () { ab.slice(); }); \
         ab.constructor = true; var t2 = ty(function () { ab.slice(); }); \
         ab.constructor = ''; var t3 = ty(function () { ab.slice(); }); \
         var c = {}; ab.constructor = c; \
         c[Symbol.species] = {}; var t4 = ty(function () { ab.slice(); }); \
         c[Symbol.species] = Function.prototype; var t5 = ty(function () { ab.slice(); }); \
         return t1 && t2 && t3 && t4 && t5 })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 结果五检前四形钉：物种返回非 AB / 返回 O 自身 / 返回小缓冲 →
/// TypeError；返回大缓冲 → 交付（内容只填前缀）。
#[test]
fn ab_slice_species_result_checks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { function ty(fn) { try { fn(); return false; } catch (e) { return e instanceof TypeError; } } \
         var a1 = new ArrayBuffer(8); var c1 = {}; \
         c1[Symbol.species] = function () { return {}; }; a1.constructor = c1; \
         var t1 = ty(function () { a1.slice(); }); \
         var a2 = new ArrayBuffer(8); var c2 = {}; \
         c2[Symbol.species] = function () { return a2; }; a2.constructor = c2; \
         var t2 = ty(function () { a2.slice(); }); \
         var a3 = new ArrayBuffer(8); var c3 = {}; \
         c3[Symbol.species] = function () { return new ArrayBuffer(4); }; a3.constructor = c3; \
         var t3 = ty(function () { a3.slice(); }); \
         var a4 = new ArrayBuffer(8); var v = new Uint8Array(a4); v[0] = 5; \
         var c4 = {}; c4[Symbol.species] = function () { return new ArrayBuffer(10); }; a4.constructor = c4; \
         var r4 = a4.slice(); var d4 = new Uint8Array(r4); \
         return t1 && t2 && t3 && r4.byteLength === 10 && d4[0] === 5 && d4[8] === 0 })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// slice 源 detach 四形钉：源已 detach（参数未读）、start/end valueOf 内
/// detach、species 构造内 detach（构造后重检步）均 TypeError。
#[test]
fn ab_slice_source_detach_family() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { function ty(fn) { try { fn(); return false; } catch (e) { return e instanceof TypeError; } } \
         var a1 = new ArrayBuffer(8); $262.detachArrayBuffer(a1); \
         var calls1 = []; var s1 = { valueOf: function () { calls1.push('s'); return 0; } }; \
         var t1 = ty(function () { a1.slice(s1); }); \
         var a2 = new ArrayBuffer(8); \
         var s2 = { valueOf: function () { $262.detachArrayBuffer(a2); return 0; } }; \
         var t2 = ty(function () { a2.slice(s2); }); \
         var a3 = new ArrayBuffer(8); \
         var e3 = { valueOf: function () { $262.detachArrayBuffer(a3); return 2; } }; \
         var t3 = ty(function () { a3.slice(1, e3); }); \
         var a4 = new ArrayBuffer(8); var c4 = {}; \
         c4[Symbol.species] = function (len) { $262.detachArrayBuffer(a4); return new ArrayBuffer(len); }; \
         a4.constructor = c4; var t4 = ty(function () { a4.slice(); }); \
         return t1 && calls1.length === 0 && t2 && t3 && t4 })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// sliceToImmutable 界与缺省钉：(undefined,undefined)/(6,undefined)/负/越界/
/// 空窗长度形 + immutable 产物形 + prop-desc 描述符与 name/length。
#[test]
fn ab_sti_bounds_and_defaults() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var ab = new ArrayBuffer(8); \
         var bl = function (s) { return ab.sliceToImmutable(s[0], s[1]).byteLength; }; \
         return bl([undefined, undefined]) === 8 && bl([6, undefined]) === 2 \
         && bl([6]) === 2 && bl([-2]) === 2 && bl([0, -2]) === 6 \
         && bl([20]) === 0 && bl([2, 20]) === 6 && bl([5, 2]) === 0 && bl([3, 3]) === 0 \
         && ab.sliceToImmutable().immutable === true \
         && ab.sliceToImmutable().resizable === false })()",
    )
    .unwrap();
    assert!(result.as_bool());
    let result = eval(
        &mut vm,
        "(function () { var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'sliceToImmutable'); \
         return typeof d.value === 'function' && d.value.name === 'sliceToImmutable' \
         && d.value.length === 2 && d.writable === true && d.enumerable === false \
         && d.configurable === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// sliceToImmutable 源增缩钉：grows 形产物 [4,5,6] 源 12 长；shrinks 第一
/// block 同形源 8 长；resize 到解析界下 → RangeError。
#[test]
fn ab_sti_grow_shrink_bounds() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var source = new ArrayBuffer(10, { maxByteLength: 12 }); \
         var view = new Uint8Array(source); \
         for (var i = 0; i < 10; i++) view[i] = i + 1; \
         var start = { valueOf: function () { source.resize(11); return -7; } }; \
         var end = { valueOf: function () { source.resize(12); return -4; } }; \
         var dest = source.sliceToImmutable(start, end); \
         var dv = new Uint8Array(dest); \
         var ok = dv.length === 3 && dv[0] === 4 && dv[1] === 5 && dv[2] === 6 && source.byteLength === 12; \
         var source2 = new ArrayBuffer(10, { maxByteLength: 10 }); \
         var view2 = new Uint8Array(source2); \
         for (var j = 0; j < 10; j++) view2[j] = j + 1; \
         var s2 = { valueOf: function () { source2.resize(9); return -7; } }; \
         var e2 = { valueOf: function () { source2.resize(8); return -4; } }; \
         var d2 = new Uint8Array(source2.sliceToImmutable(s2, e2)); \
         ok = ok && d2.length === 3 && d2[0] === 4 && d2[1] === 5 && d2[2] === 6 && source2.byteLength === 8; \
         source2.resize(10); \
         var e3 = { valueOf: function () { source2.resize(5); return -4; } }; \
         var t3 = false; \
         try { source2.sliceToImmutable(s2, e3); } catch (err) { t3 = err instanceof RangeError; } \
         return ok && t3; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// sliceToImmutable 源 detach 两形钉：源已 detach → TypeError 且参数未读；
/// end.valueOf 内 detach → TypeError，calls 双记。
#[test]
fn ab_sti_source_detach_family() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { function ty(fn) { try { fn(); return false; } catch (e) { return e instanceof TypeError; } } \
         var a1 = new ArrayBuffer(8); $262.detachArrayBuffer(a1); \
         var calls1 = []; \
         var s1 = { valueOf: function () { calls1.push('s'); return 0; } }; \
         var e1 = { valueOf: function () { calls1.push('e'); return 1; } }; \
         var t1 = ty(function () { a1.sliceToImmutable(s1, e1); }); \
         var a2 = new ArrayBuffer(8); \
         var calls2 = []; \
         var s2 = { valueOf: function () { calls2.push('start.valueOf'); return 0; } }; \
         var e2 = { valueOf: function () { $262.detachArrayBuffer(a2); calls2.push('end.valueOf'); return 1; } }; \
         var t2 = ty(function () { a2.sliceToImmutable(s2, e2); }); \
         return t1 && calls1.length === 0 && t2 && calls2.join(',') === 'start.valueOf,end.valueOf'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// sliceToImmutable 源后改无影响钉：源写/resize/detach 后产物内容不变、
/// 产物恒 8 长 immutable。
#[test]
fn ab_sti_modify_source() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var source = new ArrayBuffer(8, { maxByteLength: 8 }); \
         var view = new Uint8Array(source); \
         for (var i = 0; i < 8; i++) view[i] = i + 1; \
         var dest = source.sliceToImmutable(); \
         var dv = new Uint8Array(dest); \
         var ok = dv.length === 8 && dv[0] === 1 && dv[7] === 8 && dest.immutable === true; \
         view[0] = 86; \
         ok = ok && new Uint8Array(dest)[0] === 1; \
         source.resize(4); \
         ok = ok && dest.byteLength === 8 && new Uint8Array(dest)[7] === 8; \
         $262.detachArrayBuffer(source); \
         return ok && new Uint8Array(dest)[0] === 1 && source.detached === true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// sliceToImmutable 品牌钉：非对象/普通对象/数组/函数/DataView/TypedArray
/// 六形 TypeError 且参数未读；`new (实例.sliceToImmutable)()` 形态
/// TypeError（nonconstructor，成员链绑定使 new 目标即方法本身）。
#[test]
fn ab_sti_brand_family() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { var fn = ArrayBuffer.prototype.sliceToImmutable; \
         var calls = []; \
         var s = { valueOf: function () { calls.push('s'); return 0; } }; \
         function ty(v) { calls.length = 0; \
             try { fn.call(v, s); return false; } catch (e) { return e instanceof TypeError; } } \
         var ok = ty(undefined) && ty(null) && ty({}) && ty([]) && ty(function () {}) \
         && ty(new DataView(new ArrayBuffer(8), 0)) && ty(new Int8Array(8)); \
         var calls2 = []; \
         var s2 = { valueOf: function () { calls2.push('s'); return 0; } }; \
         var t2 = false, t3 = false; \
         var nab = new ArrayBuffer(8); \
         try { new nab.sliceToImmutable(); t3 = true; } catch (e) { t2 = e instanceof TypeError; t3 = false; } \
         return ok && calls.length === 0 && t2 && calls2.length === 0 && !t3; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}
