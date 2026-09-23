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

fn truthy(vm: &mut Vm, source: &str) {
    let result = eval(vm, source).unwrap();
    assert!(result.as_bool(), "expected truthy for: {}", source);
}

/// Atomics 为全局纯对象：typeof 'object'、proto 落 Object.prototype、
/// `@@toStringTag` 为 "Atomics"，对其调用抛 TypeError（非函数）。
#[test]
fn atomics_is_plain_object() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "typeof Atomics === 'object' \
         && Object.getPrototypeOf(Atomics) === Object.prototype \
         && Atomics[Symbol.toStringTag] === 'Atomics'",
    );
    truthy(
        &mut vm,
        "(function () { \
         var threw = false; \
         try { Atomics(); } catch (e) { threw = e instanceof TypeError; } \
         return threw; })()",
    );
}

/// 方法元数据钉：name 标签与 length（语料 length 口径）。
#[test]
fn atomics_method_name_and_length() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "Atomics.load.name === 'load' && Atomics.load.length === 2 \
         && Atomics.store.length === 3 && Atomics.exchange.length === 3 \
         && Atomics.add.length === 3 && Atomics.compareExchange.length === 4 \
         && Atomics.isLockFree.length === 1",
    );
}

/// load：读指定索引（ArrayBuffer 后接整数视图可操作，不强制共享）。
#[test]
fn atomics_load_int32() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new ArrayBuffer(16)); \
         ta[2] = 0x01008081; \
         return Atomics.load(ta, 2) === 0x01008081 && Atomics.load(ta, 0) === 0; })()",
    );
}

/// store：写入后读回返回（宽度归一，语料同值原样返回）。
#[test]
fn atomics_store_returns_stored_value() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new ArrayBuffer(16)); \
         var v = Atomics.store(ta, 1, 0x01008081); \
         return v === 0x01008081 && ta[1] === 0x01008081; })()",
    );
}

/// exchange：返回旧值、写入新值。
#[test]
fn atomics_exchange_returns_old_value() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new ArrayBuffer(16)); \
         ta[0] = 7; \
         var old = Atomics.exchange(ta, 0, 42); \
         return old === 7 && ta[0] === 42; })()",
    );
}

/// add/sub：返回旧值、写入结果（有符号域）。
#[test]
fn atomics_add_sub_return_old() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new ArrayBuffer(16)); \
         ta[0] = 10; \
         var r1 = Atomics.add(ta, 0, 5); var a1 = ta[0]; \
         var r2 = Atomics.sub(ta, 0, 3); var a2 = ta[0]; \
         return r1 === 10 && a1 === 15 && r2 === 15 && a2 === 12; })()",
    );
}

/// and/or/xor：返回旧值、按位运算写入。
#[test]
fn atomics_and_or_xor_return_old() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Uint8Array(new ArrayBuffer(16)); \
         ta[0] = 0b11001100; \
         var r1 = Atomics.and(ta, 0, 0b10101010); var a1 = ta[0]; \
         var r2 = Atomics.or(ta, 0, 0b00000001); var a2 = ta[0]; \
         var r3 = Atomics.xor(ta, 0, 0b00000110); var a3 = ta[0]; \
         return r1 === 0b11001100 && a1 === 0b10001000 \
            && r2 === 0b10001000 && a2 === 0b10001001 \
            && r3 === 0b10001001 && a3 === 0b10001111; })()",
    );
}

/// compareExchange：相等才写替换值，恒返回旧值。
#[test]
fn atomics_compare_exchange_semantics() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new ArrayBuffer(16)); \
         ta[0] = 5; \
         var no = Atomics.compareExchange(ta, 0, 9, 100); var b0 = ta[0]; \
         var yes = Atomics.compareExchange(ta, 0, 5, 100); var b1 = ta[0]; \
         return no === 5 && b0 === 5 && yes === 5 && b1 === 100; })()",
    );
}

/// BigInt64 宽度：BigInt 域运算、返回旧值。
#[test]
fn atomics_bigint64_load_add() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new BigInt64Array(new ArrayBuffer(32)); \
         ta[0] = 10n; \
         var r = Atomics.add(ta, 0, 5n); \
         return Atomics.load(ta, 0) === 15n && r === 10n; })()",
    );
}

/// isLockFree：1/2/4/8 真、余假。
#[test]
fn atomics_is_lock_free_values() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "Atomics.isLockFree(1) === true && Atomics.isLockFree(2) === true \
         && Atomics.isLockFree(4) === true && Atomics.isLockFree(8) === true \
         && Atomics.isLockFree(0) === false && Atomics.isLockFree(3) === false \
         && Atomics.isLockFree(16) === false",
    );
}

/// 非整数宽度视图（Float32/Float64/Uint8Clamped）抛 TypeError。
#[test]
fn atomics_rejects_non_integer_views() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         function throws(fn) { try { fn(); return false; } catch (e) { return e instanceof TypeError; } } \
         return throws(function () { Atomics.load(new Float32Array(new ArrayBuffer(4)), 0); }) \
            && throws(function () { Atomics.store(new Float64Array(new ArrayBuffer(8)), 0, 1); }) \
            && throws(function () { Atomics.load(new Uint8ClampedArray(new ArrayBuffer(4)), 0); }); })()",
    );
}
