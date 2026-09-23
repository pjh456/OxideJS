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

/// 抛指定错误类型判定器：返回捕获实例是否为该类型。
fn throws(src: &str, kind: &str) -> String {
    format!(
        "(function () {{ var threw = false; \
         try {{ {src}; }} catch (e) {{ threw = e instanceof {kind}; }} \
         return threw; }})()"
    )
}

/// wait：SAB 等值 → "timed-out"（单线程退化，无真阻塞）。
#[test]
fn eval_wait_timed_out() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new SharedArrayBuffer(8)); \
         ta[0] = 42; \
         return Atomics.wait(ta, 0, 42, 10) === 'timed-out' \
            && Atomics.wait(ta, 0, 42) === 'timed-out'; })()",
    );
}

/// wait：不等值 → "not-equal"。
#[test]
fn eval_wait_not_equal() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new SharedArrayBuffer(8)); \
         ta[0] = 7; \
         return Atomics.wait(ta, 0, 8, 0) === 'not-equal' \
            && Atomics.wait(ta, 0, 7.0, 5) === 'timed-out'; })()",
    );
}

/// wait：非 SAB 视图 → TypeError（且先于 index 强转：毒 index 不评估）。
#[test]
fn eval_wait_non_sab_type_error() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var ta = new Int32Array(new ArrayBuffer(8)); \
             var calls = 0; var poison = {{ valueOf: function () {{ calls = 1; throw new TypeError('p'); }} }}; \
             return {} && {} && calls === 0; }})()",
            throws("Atomics.wait(ta, 0, 0, 1)", "TypeError"),
            throws("Atomics.wait(ta, poison, 0, 1)", "TypeError"),
        ),
    );
}

/// wait：负 index → RangeError（非 "not-equal"）。
#[test]
fn eval_wait_negative_index_range_error() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var ta = new Int32Array(new SharedArrayBuffer(8)); \
             return {}; }})()",
            throws("Atomics.wait(ta, -1, 0, 1)", "RangeError"),
        ),
    );
}

/// wait：index == length（元素字节区间超缓冲）→ RangeError。
#[test]
fn eval_wait_out_of_range_range_error() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var ta = new Int32Array(new SharedArrayBuffer(16)); \
             return {} && {}; }})()",
            throws("Atomics.wait(ta, 4, 0, 1)", "RangeError"),
            throws("Atomics.wait(ta, 5, 0, 1)", "RangeError"),
        ),
    );
}

/// wait：BigInt64 视图合法（0n 等值 → "timed-out"）；BigUint64 视图 → TypeError。
#[test]
fn eval_wait_bigint64_arm() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var ta = new BigInt64Array(new SharedArrayBuffer(8)); \
             ta[0] = 0n; \
             return Atomics.wait(ta, 0, 0n, 1) === 'timed-out' && {}; }})()",
            throws("Atomics.wait(new BigUint64Array(new SharedArrayBuffer(8)), 0, 0n, 1)", "TypeError"),
        ),
    );
}

/// wait：浮点视图 → TypeError。
#[test]
fn eval_wait_float_ta_type_error() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             return {} && {}; }})()",
            throws("Atomics.wait(new Float32Array(new SharedArrayBuffer(4)), 0, 0, 1)", "TypeError"),
            throws("Atomics.wait(new Float64Array(new SharedArrayBuffer(8)), 0, 0, 1)", "TypeError"),
        ),
    );
}

/// wait：timeout 强转毒传播 + 归一（false/负值 → 0，等值仍 "timed-out"；
/// 对象 valueOf 抛错原样上抛）。
#[test]
fn eval_wait_timeout_coercion() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var ta = new Int32Array(new SharedArrayBuffer(8)); \
             ta[0] = 1; \
             return Atomics.wait(ta, 0, 1, false) === 'timed-out' \
                && Atomics.wait(ta, 0, 1, -3) === 'timed-out' \
                && Atomics.wait(ta, 0, 1, NaN) === 'timed-out' \
                && {}; }})()",
            throws(
                "Atomics.wait(ta, 0, 1, {valueOf: function () { throw new TypeError('p'); }})",
                "TypeError",
            ),
        ),
    );
}

/// wait：值强转毒传播（BigInt64 视图配数值/非整串语义；Int32 视图配 BigInt）。
#[test]
fn eval_wait_value_coercion() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var ta = new BigInt64Array(new SharedArrayBuffer(8)); \
             ta[0] = 5n; \
             var ab = new Int32Array(new SharedArrayBuffer(4)); \
             ab[0] = 3; \
             return Atomics.wait(ta, 0, '5', 1) === 'timed-out' \
                && Atomics.wait(ta, 0, true, 1) === 'not-equal' \
                && {} && {} && {}; }})()",
            throws("Atomics.wait(ta, 0, 0.5, 1)", "TypeError"),
            throws("Atomics.wait(ta, 0, null, 1)", "TypeError"),
            throws("Atomics.wait(ab, 0, 1n, 1)", "TypeError"),
        ),
    );
}

/// notify：SAB 无 waiter 面 → 恒 0。
#[test]
fn eval_notify_zero_no_waiters() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new SharedArrayBuffer(8)); \
         return Atomics.notify(ta, 0) === 0 && Atomics.notify(ta, 0, 3) === 0; })()",
    );
}

/// notify：非 SAB 视图 → 返 0 不抛（immutable 同形）。
#[test]
fn eval_notify_non_sab_returns_0() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "(function () { \
         var ta = new Int32Array(new ArrayBuffer(8)); \
         return Atomics.notify(ta, 0) === 0 && Atomics.notify(ta, 0, 100) === 0; })()",
    );
}

/// notify：负/越界 index → RangeError（先于非 SAB 早返 0）。
#[test]
fn eval_notify_negative_index_range_error() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var sab = new Int32Array(new SharedArrayBuffer(8)); \
             var ab = new Int32Array(new ArrayBuffer(8)); \
             return {} && {} && {}; }})()",
            throws("Atomics.notify(sab, -1)", "RangeError"),
            throws("Atomics.notify(sab, 9, 1)", "RangeError"),
            throws("Atomics.notify(ab, 9, 1)", "RangeError"),
        ),
    );
}

/// notify：count 各形（−3/+Inf/undefined/字符串/对象 → 0）+ 毒 count 先于
/// 非 SAB 早返 0 抛错。
#[test]
fn eval_notify_count_forms() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        &format!(
            "(function () {{ \
             var sab = new Int32Array(new SharedArrayBuffer(8)); \
             var ab = new Int32Array(new ArrayBuffer(8)); \
             var poison = {{ valueOf: function () {{ throw new TypeError('p'); }} }}; \
             return Atomics.notify(sab, 0, -3) === 0 \
                && Atomics.notify(sab, 0, Infinity) === 0 \
                && Atomics.notify(sab, 0) === 0 \
                && Atomics.notify(sab, 0, '33') === 0 \
                && Atomics.notify(sab, 0, {{ valueOf: function () {{ return 8; }} }}) === 0 \
                && Atomics.notify(ab, 0, -1) === 0 \
                && {}; }})()",
            throws("Atomics.notify(ab, 0, poison)", "TypeError"),
        ),
    );
}

/// 方法元数据钉：name/length（语料 descriptor 口径）。
#[test]
fn atomics_wait_notify_name_and_length() {
    let mut vm = Vm::new();
    truthy(
        &mut vm,
        "Atomics.wait.name === 'wait' && Atomics.wait.length === 4 \
         && Atomics.notify.name === 'notify' && Atomics.notify.length === 3",
    );
}
