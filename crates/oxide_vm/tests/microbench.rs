//! 毫秒级热路径微基准套件：方向性过滤器，不是门禁。
//!
//! 十个 JS 循环用例，每用例 1 个预热 run 加 3 个测量 run，每 run 均为同一模块的
//! `Vm::run`（新建 VM 共享同一 KernelCore，免每用例重建 builtin world），3 轮取中位。
//! 每用例断言 run 结果值等于期望值，防死代码消除。
//!
//! 角色是方向性过滤器：候选优化在微基准上无方向性收益即直接否决，不跑端到端；
//! 有方向性收益须进锚点层（release 端到端）方向确认才采信。不设阈值判定——
//! debug 构建噪声大，阈值会产 flaky 红，debug 数字仅过滤，不作绝对比较。
//!
//! 测量语义：`run()` 不清 IC（IC 扩展字在模块字节码内跨 run 持久），同一模块连续
//! 多 run 时 IC 从首个 run 起持续热态；预热 run 吸收 IC 学习 miss、分配器与代码
//! 缓存预热，测量轮全部热态。
//!
//! 循环规模按「debug 全套件执行段 5 秒内」约束调参：debug 构建每指令税高（无内联、
//! 分派 match 开销主导），各用例单轮 30 至 130 毫秒，十用例四轮合计约 4 秒。
//! 规模不是测量语义的一部分，方向判定只依赖相对变化，跨宿主对比绝对值无意义。
//!
//! 日常入口 `cargo test -p oxide_vm micro`（名称过滤）；
//! `cargo test -p oxide_vm --release micro` 跑 release 口径。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

/// 每用例测量轮数：三轮取中位，抗单轮尖峰。
const MEASURE_RUNS: usize = 3;

/// 构建 profile 标签：debug 数字仅过滤，不作绝对比较。
#[cfg(debug_assertions)]
const PROFILE: &str = "debug";
#[cfg(not(debug_assertions))]
const PROFILE: &str = "release";

/// 全部用例共享同一 kernel：builtin world 只建一次，各用例 VM 经 with_kernel_core 直建。
static SHARED_KERNEL: OnceLock<Arc<KernelCore>> = OnceLock::new();

/// GC churn 用例专用 kernel（512KB 阈值，gc_mark_sweep bench 同款）：默认阈值过高，
/// 循环规模内不触发 GC，测不到 sweep 面。
static CHURN_KERNEL: OnceLock<Arc<KernelCore>> = OnceLock::new();

/// 串行化各用例测量段：cargo test 并发跑测试，多线程争核会污染计时。
static BENCH_LOCK: Mutex<()> = Mutex::new(());

fn shared_kernel() -> Arc<KernelCore> {
    SHARED_KERNEL.get_or_init(|| KernelCore::new(KernelConfig::minimal())).clone()
}

fn churn_kernel() -> Arc<KernelCore> {
    CHURN_KERNEL
        .get_or_init(|| {
            let mut config = KernelConfig::minimal();
            config.session_gc_threshold = 512 * 1024;
            KernelCore::new(config)
        })
        .clone()
}

/// 编译 JS 源为模块（计时循环外）。
fn compile_module(source: &str) -> Arc<CompiledModule> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Arc::new(Compiler::new().compile(&program).expect("compile"))
}

/// 解出数值：int 与 double 两种表示都转 f64（期望值都是二的五十三次方内整数，精确）。
fn value_as_f64(result: JsValue) -> f64 {
    if result.is_int() {
        result.as_int() as f64
    } else if result.is_double() {
        result.as_double()
    } else {
        panic!("结果应为数字，实际 {result:?}")
    }
}

/// 断言 run 结果值等于期望值（防死代码消除）。
fn expect_number(result: JsValue, expected: f64, label: &str) {
    assert_eq!(value_as_f64(result), expected, "{label} 结果值不符");
}

/// 中位数：抗单轮尖峰，轮数取奇数。
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// 测量单用例：1 个预热 run 加 3 个测量 run，每 run 新建 VM 共享同一 kernel，
/// 三轮取中位后输出一行 `micro <case>: <n> ops, <median> ms, <ns/op> ns/op (r1, r2, r3)`。
fn measure_case(name: &str, kernel: &Arc<KernelCore>, module: &Arc<CompiledModule>, n_ops: usize, expected: f64) {
    // 串行化：测量段独占执行，避免并发测试线程争核。
    let _guard = BENCH_LOCK.lock().unwrap();

    // 预热 run：吸收 IC 学习 miss、分配器与代码缓存预热。
    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
    let result = vm.run(module).expect("预热 run");
    expect_number(result, expected, name);

    // 测量轮：IC 与缓存全部热态，每轮新建 VM。
    let mut walls_ms: Vec<f64> = Vec::with_capacity(MEASURE_RUNS);
    for _ in 0..MEASURE_RUNS {
        let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
        let t0 = Instant::now();
        let result = vm.run(module).expect("测量 run");
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        expect_number(result, expected, name);
        walls_ms.push(ms);
    }

    let median_ms = median(&walls_ms);
    eprintln!(
        "micro {name}: {n_ops} ops, {median_ms:.2} ms, {:.2} ns/op ({}) [{PROFILE}]",
        median_ms * 1e6 / n_ops as f64,
        walls_ms.iter().map(|m| format!("{m:.2}")).collect::<Vec<_>>().join(", ")
    );
}

/// 纯算术循环：分发主循环每指令税基线（GC 安全点、步数采样、解码与分派）。
#[test]
fn micro_dispatch_arith() {
    let source = "var x = 0; for (var i = 0; i < 1000; i++) { x = x + i - i * 2 + (i % 3); } x";
    let module = compile_module(source);
    measure_case("dispatch_arith", &shared_kernel(), &module, 1_000, -498_501.0);
}

/// 单态对象属性读循环：IC 命中路径。
#[test]
fn micro_ic_get_mono() {
    let source = "var a = { x: 1 }; var sum = 0; for (var i = 0; i < 1800; i++) { sum += a.x; } sum";
    let module = compile_module(source);
    measure_case("ic_get_mono", &shared_kernel(), &module, 1_800, 1_800.0);
}

/// 4 形轮换读（4 槽容量内）：IC 多态命中，每 shape 一次学习 miss 后全命中。
#[test]
fn micro_ic_get_poly4() {
    let source = "var a = { x: 1 }, b = { x: 2, y: 3 }, c = { x: 4, z: 5 }, d = { x: 6, y: 7, z: 8 }; \
                  var sum = 0; for (var i = 0; i < 1500; i++) { var t = [a, b, c, d][i % 4]; sum += t.x; } sum";
    let module = compile_module(source);
    measure_case("ic_get_poly4", &shared_kernel(), &module, 1_500, 4_875.0);
}

/// 5 形轮换读（超 4 槽容量）：FIFO 逐访问滚动，每次访问都 miss，IC 抖动上限。
#[test]
fn micro_ic_get_poly5() {
    let source = "var s = [{ x: 1 }, { x: 2, y: 3 }, { x: 4, z: 5 }, { x: 6, y: 7, z: 8 }, { x: 9, a: 1, b: 2 }]; \
                  var sum = 0; for (var i = 0; i < 1500; i++) { sum += s[i % 5].x; } sum";
    let module = compile_module(source);
    measure_case("ic_get_poly5", &shared_kernel(), &module, 1_500, 6_600.0);
}

/// 属性写循环：写侧路径（写侧未走 IC 命中的面）。
#[test]
fn micro_ic_set() {
    let source = "var o = { x: 0 }; for (var i = 0; i < 2200; i++) { o.x = i; } o.x";
    let module = compile_module(source);
    measure_case("ic_set", &shared_kernel(), &module, 2_200, 2_199.0);
}

/// 小函数调用循环：字节码调用与内联路径。
#[test]
fn micro_call_bytecode() {
    let source = "function f(n) { return n * 2 + 1; } var r = 0; \
                  for (var i = 0; i < 750; i++) { r += f(i); } r";
    let module = compile_module(source);
    measure_case("call_bytecode", &shared_kernel(), &module, 750, 562_500.0);
}

/// push 循环：数组增长路径。
#[test]
fn micro_array_push() {
    let source = "var a = []; for (var i = 0; i < 1300; i++) { a.push(i); } a.length";
    let module = compile_module(source);
    measure_case("array_push", &shared_kernel(), &module, 1_300, 1_300.0);
}

/// 字符串拼接循环：s = s + "y"，拼接与字符串 GC 路径。
#[test]
fn micro_string_concat() {
    let source = "var s = \"x\"; for (var i = 0; i < 800; i++) { s = s + \"y\"; } s.length";
    let module = compile_module(source);
    measure_case("string_concat", &shared_kernel(), &module, 800, 801.0);
}

/// 强转循环：+x 每轮两次 to_number 强转。加数取二进可精确表示值，
/// 期望值不随浮点累积漂移。
#[test]
fn micro_coerce_tonumber() {
    let source = "var x = \"42\"; var y = \"3.5\"; var r = 0; \
                  for (var i = 0; i < 1500; i++) { r += (+x) + (+y); } r";
    let module = compile_module(source);
    measure_case("coerce_tonumber", &shared_kernel(), &module, 1_500, 68_250.0);
}

/// GC churn 循环：闭包加对象加数组，每轮全部死亡，512KB 低阈值下执行期两档收集
/// （epoch 晋升加 session 原地 sweep）触发。规模取单 run 约一千五百个对象
/// （分配包络超阈值），保证至少一次收集。
#[test]
fn micro_gc_churn() {
    let source = "var t = 0; for (var i = 0; i < 500; i++) { \
                  var f = function() { return i; }; var o = { a: i, b: [i, i + 1] }; \
                  t += f() + o.a + o.b.length; } t";
    let module = compile_module(source);
    measure_case("gc_churn", &churn_kernel(), &module, 500, 250_500.0);
}
