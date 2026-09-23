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

/// 构造器终校验 detach 形：proto getter 内 detach → TypeError，
/// 偏移 ToNumber 恰发生一次（重取点在强转之后）。
#[test]
fn dv_ctor_custom_proto_detach_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(8); \
         var calls = 0; \
         var byteOffset = { valueOf() { calls++; return 0; } }; \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { \
           get() { $262.detachArrayBuffer(buffer); return DataView.prototype; } \
         }); \
         var err = null; \
         try { Reflect.construct(DataView, [buffer, byteOffset], newTarget); } catch (e) { err = e; } \
         err instanceof TypeError && calls === 1",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// auto 缺省长终校验重算：proto getter 内 resize(2) 后存储长按 live' − offset 重算。
#[test]
fn dv_ctor_custom_proto_resize_valid_by_offset() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(3, { maxByteLength: 3 }); \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { \
           get: function() { buffer.resize(2); return DataView.prototype; } \
         }); \
         var result = Reflect.construct(DataView, [buffer, 2], newTarget); \
         result.constructor === DataView && result.byteLength === 0 && result.byteOffset === 2",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 显式长终校验：resize 至 2 后 1 + 1 ≤ 2 → 成功且 byteLength 保持静态 1。
#[test]
fn dv_ctor_custom_proto_resize_valid_by_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(3, { maxByteLength: 3 }); \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { \
           get: function() { buffer.resize(2); return DataView.prototype; } \
         }); \
         var result = Reflect.construct(DataView, [buffer, 1, 1], newTarget); \
         result.constructor === DataView && result.byteLength === 1",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 终校验 offset 臂：resize(1) 后 offset 2 > live' → RangeError。
#[test]
fn dv_ctor_custom_proto_resize_invalid_by_offset() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(3, { maxByteLength: 3 }); \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { \
           get: function() { buffer.resize(1); return DataView.prototype; } \
         }); \
         var err = null; \
         try { Reflect.construct(DataView, [buffer, 2], newTarget); } catch (e) { err = e; } \
         err instanceof RangeError",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 终校验显式长臂：resize(2) 后 1 + 2 > live' → RangeError。
#[test]
fn dv_ctor_custom_proto_resize_invalid_by_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(3, { maxByteLength: 3 }); \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { \
           get: function() { buffer.resize(2); return DataView.prototype; } \
         }); \
         var err = null; \
         try { Reflect.construct(DataView, [buffer, 1, 2], newTarget); } catch (e) { err = e; } \
         err instanceof RangeError",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// byteLength auto 活读序列：3 → grow 4 → shrink 2 → 边界 0 → 越界 TypeError。
#[test]
fn dv_bytelength_auto_tracks_resize() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, { maxByteLength: 5 }); \
         var dv = new DataView(ab, 1); \
         var ok = dv.byteLength === 3; \
         ab.resize(5); ok = ok && dv.byteLength === 4; \
         ab.resize(3); ok = ok && dv.byteLength === 2; \
         ab.resize(1); ok = ok && dv.byteLength === 0; \
         ab.resize(0); \
         try { dv.byteLength; ok = false; } catch (e) { ok = ok && e instanceof TypeError; } \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// byteLength fixed 视图：shrink 到 end 之下 → TypeError，grow 回静态长。
#[test]
fn dv_bytelength_fixed_shrink_typeerror() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, { maxByteLength: 5 }); \
         var dv = new DataView(ab, 1, 2); \
         var ok = dv.byteLength === 2; \
         ab.resize(5); ok = ok && dv.byteLength === 2; \
         ab.resize(3); ok = ok && dv.byteLength === 2; \
         ab.resize(2); \
         try { dv.byteLength; ok = false; } catch (e) { ok = ok && e instanceof TypeError; } \
         ab.resize(3); ok = ok && dv.byteLength === 2; \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// byteOffset getter detach 守卫：detached 缓冲 → TypeError。
#[test]
fn dv_byteoffset_detach_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(1); \
         var sample = new DataView(buffer, 0); \
         $262.detachArrayBuffer(buffer); \
         var err = null; \
         try { sample.byteOffset; } catch (e) { err = e; } \
         err instanceof TypeError",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// byteOffset getter OOB 守卫：auto 视图 shrink 至 offset 之下 → TypeError。
#[test]
fn dv_byteoffset_auto_shrink_typeerror() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ab = new ArrayBuffer(4, { maxByteLength: 5 }); \
         var dv = new DataView(ab, 1); \
         var ok = dv.byteOffset === 1; \
         ab.resize(0); \
         try { dv.byteOffset; ok = false; } catch (e) { ok = ok && e instanceof TypeError; } \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// get 面 OOB：fixed 视图（0,16）resize(8) 后读 → TypeError（非 RangeError）。
#[test]
fn dv_get_shrunk_buffer_typeerror() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(24, { maxByteLength: 32 }); \
         var sample = new DataView(buffer, 0, 16); \
         buffer.resize(16); \
         var ok = sample.getInt8(0) === 0; \
         buffer.resize(8); \
         try { sample.getInt8(0); ok = false; } catch (e) { ok = ok && e instanceof TypeError; } \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// set 面 OOB：同形写面 → TypeError。
#[test]
fn dv_set_shrunk_buffer_typeerror() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(24, { maxByteLength: 32 }); \
         var sample = new DataView(buffer, 0, 16); \
         buffer.resize(8); \
         var err = null; \
         try { sample.setInt8(0, 30); } catch (e) { err = e; } \
         err instanceof TypeError",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// set 面 immutable 守卫先于一切强转：偏移与值 valueOf 零调用。
#[test]
fn dv_set_immutable_before_conversion() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var iab = (new ArrayBuffer(8)).transferToImmutable(); \
         var view = new DataView(iab); \
         var calls = []; \
         var byteOffset = { valueOf() { calls.push('byteOffset.valueOf'); return 0; } }; \
         var value = { valueOf() { calls.push('value.valueOf'); return '1'; } }; \
         view.getInt8(byteOffset) === 0 && \
         (function() { \
           calls = []; \
           try { view.setInt8(byteOffset, value); return false; } catch (e) { return e instanceof TypeError; } \
         })() && calls.length === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// get 面允许 immutable 缓冲：正常读回初值。
#[test]
fn dv_get_immutable_ok() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var iab = (new ArrayBuffer(8)).transferToImmutable(); \
         var src = new DataView(new ArrayBuffer(8)); \
         src.setInt8(3, 42); \
         var view = new DataView(iab); \
         view.getInt8(0) === 0 && new DataView(iab).byteLength === 8",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 缓冲完好时视图相对越界保持 RangeError 种类；auto 视图 grow 后新区可读。
#[test]
fn dv_get_oob_kind_split() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var buffer = new ArrayBuffer(12); \
         var sample = new DataView(buffer, 0); \
         var err = null; \
         try { sample.getInt8(12); } catch (e) { err = e; } \
         var ok = err instanceof RangeError; \
         var ab = new ArrayBuffer(4, { maxByteLength: 8 }); \
         var auto = new DataView(ab, 0); \
         ab.resize(8); \
         ok = ok && auto.byteLength === 8 && auto.getUint8(7) === 0; \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 定长缓冲缺省长构造 + 初校验序钉：offset > bufferByteLength 的
/// RangeError 先于 proto getter（getter 抛 Test262Error 不可见）。
#[test]
fn dv_ctor_fixed_default_len_static() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var ok = new DataView(new ArrayBuffer(8), 2).byteLength === 6; \
         var buffer = new ArrayBuffer(3); \
         var getterCalled = false; \
         var newTarget = function() {}.bind(null); \
         Object.defineProperty(newTarget, 'prototype', { \
           get: function() { getterCalled = true; throw new Error('getter'); } \
         }); \
         var err = null; \
         try { Reflect.construct(DataView, [buffer, 4], newTarget); } catch (e) { err = e; } \
         ok = ok && err instanceof RangeError && !getterCalled; \
         ok",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 构造基础形：缺省长视图，buffer 指向 SAB 本体。
#[test]
fn dv_sab_ctor_basic() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var sab = new SharedArrayBuffer(8); \
         var dv = new DataView(sab); \
         dv.byteLength === 8 && dv.byteOffset === 0 && dv.buffer === sab \
         && Object.getPrototypeOf(dv) === DataView.prototype",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 显式 offset/length 形：视图界与 buffer 指向。
#[test]
fn dv_sab_ctor_offset_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var sab = new SharedArrayBuffer(8); \
         var dv = new DataView(sab, 2, 4); \
         dv.byteLength === 4 && dv.byteOffset === 2 && dv.buffer === sab",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 构造越界三形：offset > live / 显式长越界 / 负 offset 均 RangeError。
#[test]
fn dv_sab_ctor_oob() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var sab = new SharedArrayBuffer(4); \
         function kind(expr) { try { expr(); return 'none'; } catch (e) { return e.name; } } \
         return kind(function () { new DataView(sab, 5); }) === 'RangeError' \
           && kind(function () { new DataView(sab, 0, 5); }) === 'RangeError' \
           && kind(function () { new DataView(sab, -1); }) === 'RangeError'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 读写核回卷：各宽 get/set 位模式与同形 AB 视图对照同值。
#[test]
fn dv_sab_get_set_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var sab = new SharedArrayBuffer(32); \
         var dv = new DataView(sab); \
         dv.setInt8(0, -7); \
         dv.setUint8(1, 200); \
         dv.setInt32(2, 0x01020304, true); \
         dv.setInt32(6, 0x01020304, false); \
         dv.setFloat64(8, 1.5); \
         dv.setBigInt64(16, 42n); \
         dv.setBigUint64(24, 18446744073709551615n); \
         var ab = new ArrayBuffer(32); \
         var ref = new DataView(ab); \
         ref.setInt8(0, -7); \
         ref.setUint8(1, 200); \
         ref.setInt32(2, 0x01020304, true); \
         ref.setInt32(6, 0x01020304, false); \
         ref.setFloat64(8, 1.5); \
         ref.setBigInt64(16, 42n); \
         ref.setBigUint64(24, 18446744073709551615n); \
         return dv.getInt8(0) === ref.getInt8(0) \
           && dv.getUint8(1) === ref.getUint8(1) \
           && dv.getInt32(2, true) === ref.getInt32(2, true) \
           && dv.getInt32(6, false) === ref.getInt32(6, false) \
           && dv.getFloat64(8, true) === ref.getFloat64(8, true) \
           && dv.getBigInt64(16, true) === ref.getBigInt64(16, true) \
           && dv.getBigUint64(24, false) === ref.getBigUint64(24, false); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 读越界：跨视图尾 getInt32 → RangeError。
#[test]
fn dv_sab_get_oob() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var dv = new DataView(new SharedArrayBuffer(4)); \
         try { dv.getInt32(1); return false; } catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB live 读：定长视图静态界 + growable SAB auto 视图随 grow 活读。
#[test]
fn dv_sab_live_getters() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var sab = new SharedArrayBuffer(8); \
         var fixed = new DataView(sab, 1, 3); \
         var ok = fixed.byteOffset === 1 && fixed.byteLength === 3; \
         var growable = new SharedArrayBuffer(4, { maxByteLength: 8 }); \
         var auto = new DataView(growable); \
         ok = ok && auto.byteLength === 4 && auto.byteOffset === 0; \
         growable.grow(8); \
         return ok && auto.byteLength === 8 && auto.byteOffset === 0; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 原型链：buffer 属性返回 SAB 本体 + custom-proto 臂
/// （newTarget 自定义原型经 GpFC 落位）。
#[test]
fn dv_sab_proto_chain() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var sab = new SharedArrayBuffer(4); \
         var dv = new DataView(sab); \
         var ok = dv.buffer === sab; \
         var newTarget = function() {}.bind(null); \
         newTarget.prototype = {}; \
         var dv2 = Reflect.construct(DataView, [sab], newTarget); \
         return ok && Object.getPrototypeOf(dv2) === newTarget.prototype; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// DV-over-SAB 品牌负钉：非缓冲区对象（裸对象 / TypedArray / null）构造
/// 首检均 TypeError。
#[test]
fn dv_sab_brand_reject() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         function kind(expr) { try { expr(); return 'none'; } catch (e) { return e.name; } } \
         var ta = new Int8Array(4); \
         return kind(function () { new DataView({}); }) === 'TypeError' \
           && kind(function () { new DataView(ta); }) === 'TypeError' \
           && kind(function () { new DataView(null); }) === 'TypeError'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}
