//! 分配成本 go/no-go 测量装置（Plan C 设计门，行为中性）。
//!
//! 两个测量面：
//! - 微基准：同一对象尺寸（`JsObject` 定长头 136B）下，现分配路径
//!   （bumpalo arena 分配加对象表登记）对新路径（堆 `Box::into_raw` 加对象表
//!   登记）的逐次成本，单线程，多轮取中位；
//! - 三例墙时间：gc_object / gc_array / mem_closure_chain 三个分配密集用例，
//!   与 js 压力基准同一 harness（编译一次、预热、每迭代新 VM 执行），记当前
//!   中位墙时间并对基线（`benchmark_baseline.json`）报差值。
//!
//! 本文件只测量不改引擎执行路径；退出码仅由用例执行成败驱动，go/no-go 判定
//! 由主 Agent 依据落档数字与噪声带作出。复跑命令：
//! `oxide bench --mode leak --filter alloc_ab`。

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_kernel::kernel::KernelCore;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_parser::Allocator;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::vm_pool::VmPool;

use crate::bench::baseline::load_baseline;
use crate::bench::BenchConfig;

/// 微基准固定分配次数：10 万对象，对象尺寸 136B，单轮总量约 13.6MB，
/// 足以摊薄计时开销、测出逐次分配成本。
const MICRO_ALLOC_N: usize = 100_000;
/// 微基准轮数：每路径三轮取中位，抗单轮噪声。
const MICRO_ROUNDS: usize = 3;

/// 三个分配密集用例（设计门回归面）：对象 churn、数组 churn、闭包链留存。
const WALL_CASES: [&str; 3] = ["gc_object", "gc_array", "mem_closure_chain"];

/// 运行分配成本 go/no-go 测量装置：先微基准，再三例墙时间。
pub fn run_alloc_ab(config: &BenchConfig, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>) -> ExitCode {
    let micro = alloc_strategy_microbench();
    eprintln!(
        "[alloc_ab] microbench n={MICRO_ALLOC_N} (JsObject 136B, 单线程, 三轮中位): \
bump={:.2} ns/alloc, box={:.2} ns/alloc, delta={:+.2} ns/alloc; \
释放相 bump={:.2} ns/alloc, box={:.2} ns/alloc",
        micro.bump_alloc,
        micro.box_alloc,
        micro.box_alloc - micro.bump_alloc,
        micro.bump_free,
        micro.box_free
    );

    let mut all_ok = true;
    for case in WALL_CASES {
        match run_wall_case(case, config, kernel, pool) {
            Ok((wall, baseline)) => match baseline {
                Some(b) => eprintln!(
                    "[alloc_ab] wall {case}: current={wall:.2} ms, baseline={b:.2} ms, delta={:+.2} ms ({:+.1}%)",
                    wall - b,
                    (wall - b) / b.max(1e-9) * 100.0
                ),
                None => eprintln!("[alloc_ab] wall {case}: current={wall:.2} ms (无基线条目)"),
            },
            Err(e) => {
                eprintln!("[alloc_ab] wall {case} 执行失败: {e}");
                all_ok = false;
            }
        }
    }
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// 微基准结果：两路径的分配相与释放相成本（ns/alloc，三轮中位）。
struct MicroResult {
    bump_alloc: f64,
    bump_free: f64,
    box_alloc: f64,
    box_free: f64,
}

/// 分配策略微基准：同一对象尺寸（`JsObject::new_empty`，136B）下，
/// 路径 A 为现分配路径（bumpalo arena 分配加对象表登记），路径 B 为新路径
/// （堆 `Box::into_raw` 加对象表登记）；释放相 A 为整 arena 归还（现 `reset`
/// 换新 Bump 同构），B 为逐对象 `Box::from_raw`。单线程稳态，ns/alloc。
fn alloc_strategy_microbench() -> MicroResult {
    let mut bump_alloc: Vec<f64> = Vec::new();
    let mut bump_free: Vec<f64> = Vec::new();
    let mut box_alloc: Vec<f64> = Vec::new();
    let mut box_free: Vec<f64> = Vec::new();

    for _ in 0..MICRO_ROUNDS {
        // 路径 A：bumpalo arena 分配加对象表登记（现 alloc_object 路径）。
        let bump = bumpalo::Bump::new();
        let mut arena_ptrs: Vec<*mut JsObject> = Vec::with_capacity(MICRO_ALLOC_N);
        let t0 = Instant::now();
        for _ in 0..MICRO_ALLOC_N {
            let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
            arena_ptrs.push(bump.alloc(obj));
        }
        bump_alloc.push(t0.elapsed().as_secs_f64() * 1e9 / MICRO_ALLOC_N as f64);
        let t0 = Instant::now();
        std::hint::black_box(&arena_ptrs);
        // 整 arena 归还：现释放路径（epoch.reset 换新 Bump 同构）。
        drop(bump);
        bump_free.push(t0.elapsed().as_secs_f64() * 1e9 / MICRO_ALLOC_N as f64);

        // 路径 B：堆 Box 分配加对象表登记（Box 化后新路径）。
        let mut box_ptrs: Vec<*mut JsObject> = Vec::with_capacity(MICRO_ALLOC_N);
        let t0 = Instant::now();
        for _ in 0..MICRO_ALLOC_N {
            let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
            box_ptrs.push(Box::into_raw(Box::new(obj)));
        }
        box_alloc.push(t0.elapsed().as_secs_f64() * 1e9 / MICRO_ALLOC_N as f64);
        let t0 = Instant::now();
        // 逐对象释放：Box 化后新释放路径（恰好一次约束在路径上）。
        for &p in &box_ptrs {
            // SAFETY: p 来自本循环的 Box::into_raw，无其他别名，恰好释放一次。
            unsafe {
                drop(Box::from_raw(p));
            }
        }
        box_free.push(t0.elapsed().as_secs_f64() * 1e9 / MICRO_ALLOC_N as f64);
    }

    MicroResult {
        bump_alloc: median(&bump_alloc),
        bump_free: median(&bump_free),
        box_alloc: median(&box_alloc),
        box_free: median(&box_free),
    }
}

/// 单用例墙时间：编译一次（计时循环外），预热后每迭代新 VM 执行（与 js
/// 压力基准同一 harness），返回当前中位墙毫秒与基线墙毫秒（基线缺失时
/// 为 None）。
fn run_wall_case(
    case: &str, config: &BenchConfig, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>,
) -> Result<(f64, Option<f64>), String> {
    let path = format!("tests/stress/{case}.js");
    let js = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败: {e}"))?;
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, &js).map_err(|e| format!("解析 {case} 失败: {e:?}"))?;
    let compiler = Compiler::new();
    let hash = compiled_module_hash(&program);
    let module = kernel
        .code_forge()
        .get_or_insert_with(hash, || compiler.compile(&program))
        .map_err(|e| format!("编译 {case} 失败: {e}"))?;

    for _ in 0..config.warmup {
        let mut guard = pool.spawn();
        let _ = guard.vm_mut().run(&module);
    }
    let mut walls: Vec<f64> = Vec::with_capacity(config.iterations as usize);
    for _ in 0..config.iterations {
        let mut guard = pool.spawn();
        let t0 = Instant::now();
        let result = guard.vm_mut().run(&module);
        let wall_ms = t0.elapsed().as_secs_f64() * 1e3;
        if result.is_err() {
            return Err(format!("执行 {case} 失败: {result:?}"));
        }
        walls.push(wall_ms);
    }

    let baseline = load_baseline().ok().and_then(|b| {
        b.entries
            .iter()
            .find(|e| e.test_name == case)
            .map(|e| e.wall_time_us as f64 / 1e3)
    });
    Ok((median(&walls), baseline))
}

/// 中位数：抗单轮尖峰，轮数取奇数。
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}
