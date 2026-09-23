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
