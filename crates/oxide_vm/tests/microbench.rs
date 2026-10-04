//! 毫秒级热路径微基准套件：方向性过滤器，不是门禁。
//!
//! 十一个 JS 循环用例加两个 intern 直调用例；JS 用例每用例 1 个预热 run 加 3 个测量
//! run，每 run 均为同一模块的 `Vm::run`（新建 VM 共享同一 KernelCore，免每用例重建
//! builtin world），3 轮取中位；intern 用例为 Rust 直调，三轮各二万五千次取中位，
//! 未命中用例每轮换新键（同一键第二轮起走命中快路径，中位即被命中成本污染）。
//! 每用例断言 run 结果值等于期望值、intern 用例 black_box 返回值，防死代码消除。
//!
//! 角色是方向性过滤器：候选优化在微基准上无方向性收益即直接否决，不跑端到端；
//! 有方向性收益须进锚点层（release 端到端）方向确认才采信。不设阈值判定——
//! debug 构建噪声大，阈值会产 flaky 红，debug 数字仅过滤，不作绝对比较。
//!
//! 锚点层（方向确认，release）复用现有端到端 harness：`oxide bench js --filter
//! prop_nested` 加 `--filter gc_object`，各 1 预热 3 迭代，总耗时不足一分钟（含增量
//! 构建）。锚点用例：prop_nested（IC 加属性面，基线 132.86 毫秒）与 gc_object（对象
//! 构造面，基线 122.77 毫秒）——覆盖最高频热路径两面的两用例，按用例名固定，数值
//! 随基线重锚漂移。微基准赢的候选须经锚点层同方向确认才采信，锚点持平则不采信。
//!
//! 测量语义：每次 `run()` 从冷 IC 起步（宿主字节码的 IC 扩展字恒零，IC 写回只落
//! Vm 的 COW 私有拷贝，跨 run 不持久），IC 学习 miss 成本每 run 起点重付；
//! 预热 run 的作用是分配器、代码缓存与表代际预热，不是 IC 预热。
//!
//! 循环规模按「debug 全套件执行段 5 秒内」约束调参：debug 构建每指令税高（无内联、
//! 分派 match 开销主导），各用例单轮 28 至 126 毫秒，十三用例合计约 4.8 秒。
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

/// GC churn 用例专用 kernel（64KB 阈值，gc_mark_sweep bench 同款）：阈值须低于单
/// run 分配包络（约 108KB），否则循环规模内不触发收集，测不到 sweep 面。
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
            config.session_gc_threshold = 64 * 1024;
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
/// `assert_gc` 仅 GC churn 用例为真：每 run 后断言至少一次收集，
/// 防阈值高于分配包络时用例静默退化为测分配路径。
fn measure_case(
    name: &str, kernel: &Arc<KernelCore>, module: &Arc<CompiledModule>, n_ops: usize, expected: f64, assert_gc: bool,
) {
    // 串行化：测量段独占执行，避免并发测试线程争核。
    let _guard = BENCH_LOCK.lock().unwrap();

    // 预热 run：分配器、代码缓存与表代际预热（IC 每 run 冷起步，预热不预热 IC）。
    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
    let result = vm.run(module).expect("预热 run");
    expect_number(result, expected, name);
    if assert_gc {
        assert_gc_triggered(&vm);
    }

    // 测量轮：每轮新建 VM。
    let mut walls_ms: Vec<f64> = Vec::with_capacity(MEASURE_RUNS);
    for _ in 0..MEASURE_RUNS {
        let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
        let t0 = Instant::now();
        let result = vm.run(module).expect("测量 run");
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        expect_number(result, expected, name);
        if assert_gc {
            assert_gc_triggered(&vm);
        }
        walls_ms.push(ms);
    }

    let median_ms = median(&walls_ms);
    eprintln!(
        "micro {name}: {n_ops} ops, {median_ms:.2} ms, {:.2} ns/op ({}) [{PROFILE}]",
        median_ms * 1e6 / n_ops as f64,
        walls_ms.iter().map(|m| format!("{m:.2}")).collect::<Vec<_>>().join(", ")
    );
}

/// 断言本次 run 至少触发一次 GC 收集（gc_mark_sweep bench「GC 真触发」门禁同款）：
/// 防阈值高于分配包络时 churn 用例静默退化为测分配路径。
fn assert_gc_triggered(vm: &Vm) {
    assert!(vm.session_gc_stats().total_collections > 0, "GC churn 用例未触发收集：阈值高于分配包络");
}

/// 纯算术循环：分发主循环每指令税基线（GC 安全点、步数采样、解码与分派）。
#[test]
fn micro_dispatch_arith() {
    let source = "var x = 0; for (var i = 0; i < 1000; i++) { x = x + i - i * 2 + (i % 3); } x";
    let module = compile_module(source);
    measure_case("dispatch_arith", &shared_kernel(), &module, 1_000, -498_501.0, false);
}

/// 单态对象属性读循环：IC 命中路径。
#[test]
fn micro_ic_get_mono() {
    let source = "var a = { x: 1 }; var sum = 0; for (var i = 0; i < 1800; i++) { sum += a.x; } sum";
    let module = compile_module(source);
    measure_case("ic_get_mono", &shared_kernel(), &module, 1_800, 1_800.0, false);
}

/// 4 形轮换读（4 槽容量内）：IC 多态命中，每 shape 一次学习 miss 后全命中。
#[test]
fn micro_ic_get_poly4() {
    let source = "var a = { x: 1 }, b = { x: 2, y: 3 }, c = { x: 4, z: 5 }, d = { x: 6, y: 7, z: 8 }; \
                  var sum = 0; for (var i = 0; i < 1500; i++) { var t = [a, b, c, d][i % 4]; sum += t.x; } sum";
    let module = compile_module(source);
    measure_case("ic_get_poly4", &shared_kernel(), &module, 1_500, 4_875.0, false);
}

/// 5 形轮换读（超 4 槽容量）：FIFO 逐访问滚动，每次访问都 miss，IC 抖动上限。
#[test]
fn micro_ic_get_poly5() {
    let source = "var s = [{ x: 1 }, { x: 2, y: 3 }, { x: 4, z: 5 }, { x: 6, y: 7, z: 8 }, { x: 9, a: 1, b: 2 }]; \
                  var sum = 0; for (var i = 0; i < 1500; i++) { sum += s[i % 5].x; } sum";
    let module = compile_module(source);
    measure_case("ic_get_poly5", &shared_kernel(), &module, 1_500, 6_600.0, false);
}

/// 属性写循环：写侧路径（写侧未走 IC 命中的面）。
#[test]
fn micro_ic_set() {
    let source = "var o = { x: 0 }; for (var i = 0; i < 2200; i++) { o.x = i; } o.x";
    let module = compile_module(source);
    measure_case("ic_set", &shared_kernel(), &module, 2_200, 2_199.0, false);
}

/// 小函数调用循环：字节码调用与内联路径。
#[test]
fn micro_call_bytecode() {
    let source = "function f(n) { return n * 2 + 1; } var r = 0; \
                  for (var i = 0; i < 750; i++) { r += f(i); } r";
    let module = compile_module(source);
    measure_case("call_bytecode", &shared_kernel(), &module, 750, 562_500.0, false);
}

/// push 循环：数组增长路径。
#[test]
fn micro_array_push() {
    let source = "var a = []; for (var i = 0; i < 1300; i++) { a.push(i); } a.length";
    let module = compile_module(source);
    measure_case("array_push", &shared_kernel(), &module, 1_300, 1_300.0, false);
}

/// 字符串拼接循环：s = s + "y"，拼接与字符串 GC 路径。
#[test]
fn micro_string_concat() {
    let source = "var s = \"x\"; for (var i = 0; i < 800; i++) { s = s + \"y\"; } s.length";
    let module = compile_module(source);
    measure_case("string_concat", &shared_kernel(), &module, 800, 801.0, false);
}

/// 强转循环：+x 每轮两次 to_number 强转。加数取二进可精确表示值，
/// 期望值不随浮点累积漂移。
#[test]
fn micro_coerce_tonumber() {
    let source = "var x = \"42\"; var y = \"3.5\"; var r = 0; \
                  for (var i = 0; i < 1500; i++) { r += (+x) + (+y); } r";
    let module = compile_module(source);
    measure_case("coerce_tonumber", &shared_kernel(), &module, 1_500, 68_250.0, false);
}

/// GC churn 循环：闭包加对象加数组，每轮全部死亡，64KB 低阈值低于单 run 分配
/// 包络（约 108KB），执行期两档收集（epoch 晋升加 session 原地 sweep）触发；
/// 每 run 断言至少一次收集，防用例静默退化为测分配路径。
#[test]
fn micro_gc_churn() {
    let source = "var t = 0; for (var i = 0; i < 500; i++) { \
                  var f = function() { return i; }; var o = { a: i, b: [i, i + 1] }; \
                  t += f() + o.a + o.b.length; } t";
    let module = compile_module(source);
    measure_case("gc_churn", &churn_kernel(), &module, 500, 250_500.0, true);
}

/// 纯对象字面量循环：两键字面量构造加属性读（gc_object 形态），覆盖对象构造
/// 热路径（静态键装载期预内部化、构造期直读侧表）与分配 churn（GC sweep 面由
/// micro_gc_churn 覆盖）。debug 实测每迭代约 77 微秒（端到端 gc_object 每迭代
/// 约 2.5 微秒的三十倍），按五秒预算取七百迭代（方向判定只依赖相对变化，规模
/// 不改变相对差）。
#[test]
fn micro_object_literal() {
    let source = "var sum = 0; for (var i = 0; i < 700; i++) { \
                  var obj = { a: i, b: i * 2 }; sum += obj.a + obj.b; } sum";
    let module = compile_module(source);
    measure_case("object_literal", &shared_kernel(), &module, 700, 733_950.0, false);
}

// ── intern 直调用例（Rust 直调，分配成本微基准同款范式）──────────────────────

/// intern 直调固定次数：五万次。十万次口径下全套件执行段超五秒预算，
/// 按预算降规模（不删用例）；二万五千次仍足以摊薄计时开销、测出逐次成本。
const INTERN_N: usize = 25_000;

/// 测量单个 intern 直调用例：三轮各二万五千次取中位，每次调用返回值经 black_box
/// 防死代码消除，输出一行 `micro <case>: <n> ops, <median> ms, <ns/op> ns/op (r1, r2, r3)`。
///
/// # 边界与前提
/// - `key` 只在计时循环内调用，全局下标跨轮（第轮次乘 INTERN_N 加轮内下标），
///   键构造须由调用方在计时循环外完成，隔离 intern 成本
/// - 未命中语义要求每轮键互不重复：同一键第二轮起走命中快路径，中位即被命中
///   成本污染；命中用例的键可忽略下标
///
/// # 副作用
/// - 未命中轮向共享 perm_interner 追加唯一键（永久字符串惰性物化），仅测试进程内
///   有界增长，不改引擎行为
///
/// # 注意事项
/// - 首轮含 intern 表增长与分配器预热成本（约两倍稳态），三轮中位天然吸收该单轮
///   尖峰，与 JS 用例同口径
fn measure_intern_case<'a>(name: &str, kernel: &Arc<KernelCore>, key: impl Fn(usize) -> &'a str) {
    // 串行化：计时循环独占执行，避免并发测试线程争核。
    let _guard = BENCH_LOCK.lock().unwrap();
    let interner = kernel.perm_interner();

    let mut walls_ms: Vec<f64> = Vec::with_capacity(MEASURE_RUNS);
    for round in 0..MEASURE_RUNS {
        let t0 = Instant::now();
        let mut last: (u32, u64) = (0, 0);
        for i in 0..INTERN_N {
            last = std::hint::black_box(interner.intern(key(round * INTERN_N + i)));
        }
        std::hint::black_box(&last);
        walls_ms.push(t0.elapsed().as_secs_f64() * 1e3);
    }

    let median_ms = median(&walls_ms);
    eprintln!(
        "micro {name}: {INTERN_N} ops, {median_ms:.2} ms, {:.2} ns/op ({}) [{PROFILE}]",
        median_ms * 1e6 / INTERN_N as f64,
        walls_ms.iter().map(|m| format!("{m:.2}")).collect::<Vec<_>>().join(", ")
    );
}

/// intern 命中路径：重复 intern 同一键，首次调用插入后其余全部走无锁候选读加
/// 短读锁快路径。
#[test]
fn micro_intern_hit() {
    measure_intern_case("intern_hit", &shared_kernel(), |_| "identical_key");
}

/// intern 未命中路径：每轮各二万五千个全新唯一键，全部走慢路径（写锁追加加永久
/// 字符串物化）。键在计时循环外预建，隔离 intern 成本。
#[test]
fn micro_intern_miss() {
    let keys: Vec<String> = (0..MEASURE_RUNS * INTERN_N).map(|i| format!("unique_key_{i}")).collect();
    measure_intern_case("intern_miss", &shared_kernel(), |i| &keys[i]);
}
