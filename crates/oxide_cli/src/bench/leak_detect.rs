use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;
use oxide_vm::vm_pool::VmPool;

use crate::bench::metrics::MetricCollection;
use crate::bench::output::format_json;
use crate::bench::BenchConfig;

/// 滑动窗口采样器：对 `(iteration, value)` 序列做线性回归，
/// 当斜率明显为正且拟合度 R² 高时判定为内存泄漏。
pub struct LeakSampler {
    window: VecDeque<(usize, f64)>,
    window_size: usize,
}

impl LeakSampler {
    /// 以给定窗口大小创建采样器。
    pub fn new(window_size: usize) -> Self {
        Self {
            window: VecDeque::new(),
            window_size,
        }
    }

    /// 加入一次采样；样本足够且回归显著时返回泄漏判定。
    pub fn add_sample(&mut self, iteration: usize, value: f64) -> Option<LeakVerdict> {
        self.window.push_back((iteration, value));
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        if self.window.len() < 10 {
            return None;
        }
        let points: Vec<(usize, f64)> = self.window.iter().copied().collect();
        let (slope, r2) = linreg(&points);
        if r2 > 0.9 && slope > 0.0 {
            Some(LeakVerdict { slope, r2 })
        } else {
            None
        }
    }
}

/// 泄漏判定的回归结果：线性斜率和拟合优度 R²。
pub struct LeakVerdict {
    pub slope: f64,
    pub r2: f64,
}

/// 对 `(iteration, value)` 序列做普通最小二乘，返回 `(slope, R²)`。
///
/// # 边界与前提
/// - 序列零方差时 R² 记 1.0（退化为完美拟合，斜率 0 不触发泄漏判定）；
/// - 少于 2 个样本返回 `(0, 0)`。
fn linreg(points: &[(usize, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    if n < 2.0 {
        return (0.0, 0.0);
    }
    let sum_x: f64 = points.iter().map(|(i, _)| *i as f64).sum();
    let sum_y: f64 = points.iter().map(|(_, v)| *v).sum();
    let sum_xy: f64 = points.iter().map(|(i, v)| *i as f64 * v).sum();
    let sum_x2: f64 = points.iter().map(|(i, _)| (*i as f64).powi(2)).sum();
    let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x * sum_x);
    let intercept = (sum_y - slope * sum_x) / n;
    let y_mean = sum_y / n;
    let ss_res: f64 = points.iter().map(|(i, v)| (v - (slope * *i as f64 + intercept)).powi(2)).sum();
    let ss_tot: f64 = points.iter().map(|(_, v)| (v - y_mean).powi(2)).sum();
    let r2 = if ss_tot == 0.0 { 1.0 } else { 1.0 - ss_res / ss_tot };
    (slope, r2)
}

/// 读取当前进程驻留集大小（`/proc/self/status` 的 VmRSS 行，单位 KB）。
///
/// # 边界与前提
/// 读取或解析失败（非 Linux 宿主、status 文件异常）返回 `None`。
pub fn read_vmrss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb = rest.split_whitespace().next()?;
            return kb.parse().ok();
        }
    }
    None
}

/// 反复运行固定 JS 脚本，跟踪 session/code-forge/symbol 等内存指标，
/// 用 `LeakSampler` 回归检测随迭代次数增长的内存占用。
pub fn run_leak_detect(config: &BenchConfig, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>) -> ExitCode {
    let js =
        "var ITERATIONS = 1000; var obj = {}; for (var i = 0; i < ITERATIONS; i++) { obj['key' + i] = i; } obj['key0']";
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, js) {
        Ok(p) => p,
        Err(_) => return ExitCode::FAILURE,
    };
    let compiler = Compiler::new();
    let hash = compiled_module_hash(&program);
    let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
        Ok(m) => m,
        Err(_) => return ExitCode::FAILURE,
    };

    let mut samplers: Vec<(&str, LeakSampler)> = vec![
        ("session_objects", LeakSampler::new(20)),
        ("session_bytes", LeakSampler::new(20)),
        ("code_forge_entries", LeakSampler::new(20)),
        ("symbol_registry", LeakSampler::new(20)),
    ];

    let mut results: Vec<MetricCollection> = Vec::new();

    for i in 0..config.iterations as usize {
        let start = Instant::now();
        let mut guard = pool.spawn();
        let vm = guard.vm_mut();
        let _ = vm.run(&module);
        let elapsed = start.elapsed();

        for (name, sampler) in &mut samplers {
            let value: f64 = match *name {
                "session_objects" => vm.session_object_count() as f64,
                "session_bytes" => vm.session_bytes_allocated() as f64,
                "code_forge_entries" => kernel.code_forge().len() as f64,
                "symbol_registry" => vm.symbol_registry_len() as f64,
                _ => continue,
            };
            if let Some(verdict) = sampler.add_sample(i, value) {
                eprintln!(
                    "[LEAK] {}: slope={:.6} R²={:.4} over {} samples",
                    name,
                    verdict.slope,
                    verdict.r2,
                    sampler.window.len(),
                );
            }
        }

        if i == config.iterations as usize - 1 {
            let guard = pool.spawn();
            let vm = guard.vm();
            results.push(MetricCollection {
                test_name: "leak_detect".to_string(),
                wall_time_us: elapsed.as_micros() as u64,
                session_objects: vm.session_object_count(),
                session_bytes: vm.session_bytes_allocated(),
                epoch_objects: vm.epoch_object_count(),
                epoch_bytes: 0,
                gc_trigger_count: vm.session_gc_stats().total_collections,
                gc_bytes_freed: vm.session_gc_stats().last_collection_bytes_freed,
                gc_objects_scanned: vm.session_gc_stats().last_collection_objects_scanned,
                gc_collection_us: vm.session_gc_stats().last_collection_duration_us,
                instruction_count: vm.instruction_count(),
                compile_time_us: 0,
                exec_time_us: 0,
                ic_hit_rate: vm.ic_hit_rate(),
                ic_hits: vm.ic_hit_count(),
                ic_misses: vm.ic_miss_count(),
                peak_bytes: vm.session_bytes_peak() as u64,
                retained_bytes: vm.session_bytes_allocated() as u64,
                retained_objects: vm.session_object_count() as u64,
            });
        }
    }

    let json = format_json(&results);
    println!("{}", json);
    ExitCode::SUCCESS
}

/// S1 校准用例：共享 kernel 下循环 3000 次「新建 VM → 跑微源 → drop」，
/// 每 100 次采一次 VmRSS，度量 VM 创建/销毁路径（含 session 收尾统一
/// 释放）的内存增量，期望走平。
///
/// # 注意事项
/// 不走 VM 池：池的锁/condvar 噪音与本测面无关；出斜率先查 harness
/// 污染（allocator 归还 OS 延迟），再疑收尾释放不全。
pub fn run_mem_vm_creation_leak(kernel: &Arc<KernelCore>) -> ExitCode {
    const ITERATIONS: usize = 3000;
    const SAMPLE_EVERY: usize = 100;

    let js = "1 + 1";
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, js) {
        Ok(p) => p,
        Err(_) => return ExitCode::FAILURE,
    };
    let compiler = Compiler::new();
    let hash = compiled_module_hash(&program);
    let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
        Ok(m) => m,
        Err(_) => return ExitCode::FAILURE,
    };

    let mut sampler = LeakSampler::new(20);
    let mut series: Vec<(usize, f64)> = Vec::new();
    for i in 0..ITERATIONS {
        {
            let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
            if let Err(e) = vm.run(&module) {
                eprintln!("[vm_creation] iteration {i} run failed: {e}");
                return ExitCode::FAILURE;
            }
        }
        if i % SAMPLE_EVERY == 0 {
            if let Some(kb) = read_vmrss_kb() {
                let v = kb as f64;
                series.push((i, v));
                if let Some(verdict) = sampler.add_sample(i, v) {
                    eprintln!(
                        "[LEAK] vm_creation rss_kb: slope={:.6} R²={:.4} over {} samples",
                        verdict.slope, verdict.r2, 20
                    );
                }
            }
        }
    }
    report_series("vm_creation", "iter", &series)
}

/// S2 校准用例：单 VM 每轮「新键写脏 object/array/string 三家族原型 →
/// full_reset」，共 3000 轮，每 10 轮采一次 VmRSS，输出选择性重建释放
/// 路径的每轮内存增量（斜率 + 窗口总增量）。
///
/// # 注意事项
/// 键名逐轮递增保证走新键写入路径（既有槽位写不 bump 世代、不脏家族）。
/// 跨轮继承锚点（登记表对象数 / global 属性槽数）采样自每轮 full_reset
/// 后：wrapper 复用与槽位原位更新生效时两者应持平，增长即泄漏签名。
/// RSS 残差模型：object/function 家族脏重建每轮钉住 4 件本体（重指遗漏
/// 兜底，不进门）+ 逐轮唯一键在共享内核 shape/perm 缓存的追加式增长
/// （append-only 缓存，非 session 数据泄漏）。
pub fn run_mem_dirty_rebuild_leak(kernel: &Arc<KernelCore>) -> ExitCode {
    const ROUNDS: usize = 3000;
    const SAMPLE_EVERY: usize = 10;

    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
    let compiler = Compiler::new();

    let mut sampler = LeakSampler::new(20);
    let mut series: Vec<(usize, f64)> = Vec::new();
    // 跨轮继承锚点（登记表对象数 / global 属性槽数）：wrapper 复用与槽位
    // 原位更新生效时两者应跨轮持平，增长即泄漏的直接签名（无页粒度噪音）。
    let mut first_anchor: Option<(usize, usize)> = None;
    let mut last_anchor: (usize, usize) = (0, 0);
    for round in 0..ROUNDS {
        // 键名逐轮递增：新键写才 bump 原型世代，重建才有脏家族可换。
        let source = format!(
            "Object.prototype['d{round}'] = 1; Array.prototype['d{round}'] = 2; String.prototype['d{round}'] = 3;"
        );
        let allocator = Allocator::default();
        let program = match oxide_parser::parse(&allocator, &source) {
            Ok(p) => p,
            Err(_) => return ExitCode::FAILURE,
        };
        let module = match compiler.compile(&program) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[dirty_rebuild] round {round} compile failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = vm.run(&module) {
            eprintln!("[dirty_rebuild] round {round} run failed: {e}");
            return ExitCode::FAILURE;
        }
        vm.full_reset();
        if round % SAMPLE_EVERY == 0 {
            let registry = vm.session().builtin_world().leaked_object_count();
            let global_slots = unsafe { (*vm.session().global_object().as_ptr()).prop_vec_len() };
            if first_anchor.is_none() {
                first_anchor = Some((registry, global_slots));
            }
            last_anchor = (registry, global_slots);
            if let Some(kb) = read_vmrss_kb() {
                let v = kb as f64;
                series.push((round, v));
                if let Some(verdict) = sampler.add_sample(round, v) {
                    eprintln!(
                        "[LEAK] dirty_rebuild rss_kb: slope={:.6} R²={:.4} over {} samples",
                        verdict.slope, verdict.r2, 20
                    );
                }
            }
        }
    }
    if let Some((r0, g0)) = first_anchor {
        let (r1, g1) = last_anchor;
        eprintln!(
            "[dirty_rebuild] anchors: registry {} -> {} ({:+}) global_slots {} -> {} ({:+})",
            r0,
            r1,
            r1 as i64 - r0 as i64,
            g0,
            g1,
            g1 as i64 - g0 as i64
        );
    }
    report_series("dirty_rebuild", "round", &series)
}

/// 校准用例：循环「创建 3-upvalue 闭包 → 写 global 临时槽（晋升 session）→
/// 撤根引用 → 完整收集（死分支）」，每轮产生一个死闭包，每 500 轮采一次
/// VmRSS，度量死闭包 upvalue 列表的释放路径，期望斜率走平。
///
/// # 注意事项
/// 死对象的 upvalue 列表 Box 无其他持有者（仅存活对象被克隆并共享同一 Box
/// 与原件）：死分支不释放则死亡即永久泄漏——账目/peak/retained 全程无感
/// （死对象死亡即出表），唯一可观测量是进程 RSS。upvalue cell 的寿命绑定
/// full_reset，逐轮累积会掩盖斜率，故每窗口换新 VM，窗口边界随 teardown
/// 统一释放（兼覆盖收尾统一释放的不双放守卫）。
pub fn run_mem_closure_dead_leak(kernel: &Arc<KernelCore>) -> ExitCode {
    const ROUNDS_PER_WINDOW: usize = 5000;
    const WINDOWS: usize = 20;
    const SAMPLE_EVERY: usize = 500;

    let allocator = Allocator::default();
    let mut modules = Vec::new();
    for js in [
        "globalThis.c = (function() { var a = 1; var b = 2; var c = 3; return function() { return a + b + c; }; })(); 0",
        "globalThis.c = undefined; 0",
    ] {
        let program = match oxide_parser::parse(&allocator, js) {
            Ok(p) => p,
            Err(_) => return ExitCode::FAILURE,
        };
        let compiler = Compiler::new();
        let hash = compiled_module_hash(&program);
        let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
            Ok(m) => m,
            Err(_) => return ExitCode::FAILURE,
        };
        modules.push(module);
    }
    let (create, unroot) = (&modules[0], &modules[1]);

    let mut sampler = LeakSampler::new(20);
    let mut series: Vec<(usize, f64)> = Vec::new();
    for window in 0..WINDOWS {
        let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
        for round in 0..ROUNDS_PER_WINDOW {
            if let Err(e) = vm.run(create) {
                eprintln!("[closure_dead_leak] window {window} round {round} create run failed: {e}");
                return ExitCode::FAILURE;
            }
            vm.collect_session_gc();
            if let Err(e) = vm.run(unroot) {
                eprintln!("[closure_dead_leak] window {window} round {round} unroot run failed: {e}");
                return ExitCode::FAILURE;
            }
            vm.collect_session_gc();
            let i = window * ROUNDS_PER_WINDOW + round;
            if i % SAMPLE_EVERY == 0 {
                if let Some(kb) = read_vmrss_kb() {
                    let v = kb as f64;
                    series.push((i, v));
                    if let Some(verdict) = sampler.add_sample(i, v) {
                        eprintln!(
                            "[LEAK] closure_dead_leak rss_kb: slope={:.6} R²={:.4} over {} samples",
                            verdict.slope, verdict.r2, 20
                        );
                    }
                }
            }
        }
    }
    report_series("closure_dead_leak", "iter", &series)
}

/// 校准用例：重源单次 run 后 `full_reset()`，读双 arena 保留锚
/// （`arena_retained_bytes`，reset 边界换新 Bump 后恰 0），再 50 轮轻源
/// （每轮 `full_reset()`）采 VmRSS，度量 full_reset 归还路径的跨轮内存
/// 增量（斜率 + 窗口总增量），期望走平。
///
/// # 注意事项
/// 不走 VM 池：drop clean 路径与池内归还走同一 `full_reset`，池锁/condvar
/// 噪音与本测面无关。重源瞬时对象全 epoch 分配、不逃逸 global；锚 == 0 为
/// 确定性主门槛（in-engine，免分配器噪音），RSS 为次锚（进程级，仅判
/// 峰后无新增量——归还 chunk 可能滞留分配器自由链不回 OS，不要求回落
/// 峰前）。
pub fn run_mem_pool_high_water(kernel: &Arc<KernelCore>) -> ExitCode {
    const LIGHT_ROUNDS: usize = 50;

    let heavy_js = "var t = 0; for (var i = 0; i < 200000; i++) { var o = { s: 'abcdefghij' + i, a: [i, i + 1, i + 2] }; t += o.s.length + o.a.length; } t";
    let light_js = "1 + 1";
    let compile_one = |js: &str| {
        let allocator = Allocator::default();
        let program = oxide_parser::parse(&allocator, js).expect("parse failed");
        let hash = compiled_module_hash(&program);
        kernel
            .code_forge()
            .get_or_insert_with(hash, || Compiler::new().compile(&program))
            .expect("compile failed")
    };
    let heavy = compile_one(heavy_js);
    let light = compile_one(light_js);

    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
    if let Err(e) = vm.run(&heavy) {
        eprintln!("[pool_high_water] heavy run failed: {e}");
        return ExitCode::FAILURE;
    }
    vm.full_reset();
    let anchor = vm.arena_retained_bytes();
    eprintln!("[pool_high_water] anchor after full_reset: {anchor} bytes (expect 0)");
    if anchor != 0 {
        return ExitCode::FAILURE;
    }

    let mut sampler = LeakSampler::new(20);
    let mut series: Vec<(usize, f64)> = Vec::new();
    for i in 0..LIGHT_ROUNDS {
        if let Err(e) = vm.run(&light) {
            eprintln!("[pool_high_water] round {i} run failed: {e}");
            return ExitCode::FAILURE;
        }
        vm.full_reset();
        if let Some(kb) = read_vmrss_kb() {
            let v = kb as f64;
            series.push((i, v));
            if let Some(verdict) = sampler.add_sample(i, v) {
                eprintln!(
                    "[LEAK] pool_high_water rss_kb: slope={:.6} R²={:.4} over {} samples",
                    verdict.slope, verdict.r2, 20
                );
            }
        }
    }
    report_series("pool_high_water", "round", &series)
}

/// 校准用例：单次 run（无 full_reset）跑 N 轮「闭包 + 对象 + 数组」churn
/// 源，度量执行期死对象滞留的峰值。主锚 = in-engine 分配包络高水位
/// （`run_alloc_peak`，dispatch 循环顶 O(1) 采样）；次锚 = RSS 序列
/// （块边界采样，单 run 内上升即 churn 本体，仅记录形态不做泄漏判定）；
/// 参考 = session/epoch 对象计数与留存账目。
///
/// # 注意事项
/// 不走 VM 池、不接 full_reset：churn 峰值是 run 内量。源分 CHUNKS 块连续
/// run（块间无 reset，arena/对象表/账目跨块累积，等价单次 run），RSS 在
/// 块边界采样以获得序列形态。降幅验收走宿内 pre/post 对照（宿主安静
/// 时点），跨宿主绝对比较禁止。
pub fn run_mem_object_churn_peak(kernel: &Arc<KernelCore>) -> ExitCode {
    const N: usize = 500_000;
    const CHUNKS: usize = 8;
    const M: usize = N / CHUNKS;
    // 每轮对 t 的贡献 = f() + o.a + o.b.length = 2i + 2；Σ(i=0..M) = M² + M。
    const EXPECTED: u64 = (M as u64) * (M as u64) + (M as u64);

    let js = format!(
        "var t = 0; for (var i = 0; i < {M}; i++) {{ var f = function() {{ return i; }}; var o = {{ a: i, b: [i, i + 1] }}; t += f() + o.a + o.b.length; }} t === {EXPECTED}"
    );
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, &js) {
        Ok(p) => p,
        Err(_) => return ExitCode::FAILURE,
    };
    let hash = compiled_module_hash(&program);
    let module = match kernel
        .code_forge()
        .get_or_insert_with(hash, || Compiler::new().compile(&program))
    {
        Ok(m) => m,
        Err(_) => return ExitCode::FAILURE,
    };

    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
    let mut series: Vec<(usize, f64)> = Vec::new();
    if let Some(kb) = read_vmrss_kb() {
        series.push((0, kb as f64));
    }
    for c in 0..CHUNKS {
        let result = match vm.run(&module) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[object_churn_peak] chunk {c} run failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        if result != JsValue::bool(true) {
            eprintln!("[object_churn_peak] chunk {c} result mismatch: {result:?}");
            return ExitCode::FAILURE;
        }
        if let Some(kb) = read_vmrss_kb() {
            series.push((c + 1, kb as f64));
        }
    }

    let (slope, r2) = linreg(&series);
    let (rss_first, rss_last) = (series.first().map(|(_, v)| *v), series.last().map(|(_, v)| *v));
    eprintln!(
        "[object_churn_peak] N={} chunks={} run_alloc_peak={} bytes (主锚)",
        N,
        CHUNKS,
        vm.run_alloc_peak()
    );
    eprintln!(
        "[object_churn_peak] session_bytes_allocated={} session_bytes_peak={} session_objects={} epoch_objects={} gc_cycles={}",
        vm.session_bytes_allocated(),
        vm.session_bytes_peak(),
        vm.session_object_count(),
        vm.epoch_object_count(),
        vm.session_gc_stats().total_collections
    );
    eprintln!(
        "[object_churn_peak] rss_kb={}..{} (delta {:+.0}) slope={:.4} kB/chunk R²={:.4} (单 run 内上升 = churn 本体，不做泄漏判定)",
        rss_first.unwrap_or(0.0),
        rss_last.unwrap_or(0.0),
        rss_last.unwrap_or(0.0) - rss_first.unwrap_or(0.0),
        slope,
        r2
    );
    ExitCode::SUCCESS
}

/// 校准用例：长寿命 kernel 寿命旋钮端到端 + PermInterner 增长斜率首测
/// 基线。每复跑一轮：专用 kernel（重建阈值 `Some(200_000 / 2)`，不污染
/// bench 共享 kernel）→ JS 固定段 8192 对象（计算键写入，仅 intern 不
/// 物化）→ 191_704 个唯一 14 字节键 intern（每 4 次物化一次，模拟
/// `perm_string` = intern + 物化同式，物化比例 ≈1/4；键长 14 字节贴合
/// 生产字面量短串面）→ 11 采样点 [10k,15k,20k,30k,40k,60k,80k,100k,
/// 131072,160k,200k] 采 VmRSS。增长相斜率只记录不判定（append-only
/// 增长，正斜率即预期签名，"斜率>0 即 fail" 契约不适用，独立打印逻辑，
/// 不进 `report_series`）；重建相验"无存活 VM"边界重建：键数超阈值 →
/// `should_rebuild_perm` 返回 `Some(建议上限)`（阈值取 2 的幂后加倍）→
/// drop 旧 kernel → 新 kernel 严格主锚 `entry_count` 恰 0 + 旋钮 `None`。
///
/// # 注意事项
/// 不传 bench 共享 kernel：20 万条目会改变后续 case 的 CodeForge/harness
/// 面；本 case 全程无存活 VM（JS 段 VM 出作用域即 drop），drop 旧 Arc 即
/// 整体释放。重建后 RSS 可能不回落至增长前之下（glibc 小对象 free 滞留
/// 分配器自由链不回 OS），RSS 为次锚仅参考、不进 exit 判据。exit 0/1
/// 仅由重建相确定性断言驱动。
pub fn run_mem_kernel_lifetime() -> ExitCode {
    const JS_OBJECTS: u32 = 8_192; // JS 固定段对象数（每对象一个唯一计算键）
    const TOTAL_STRINGS: u32 = 191_704; // 增长循环唯一键数（与 JS 固定段键不相交）
    const RERUNS: usize = 2;
    const REBUILD_THRESHOLD: u32 = 200_000 / 2;
    const TARGETS: [u32; 11] =
        [10_000, 15_000, 20_000, 30_000, 40_000, 60_000, 80_000, 100_000, 131_072, 160_000, 200_000];

    let mut config = KernelConfig::minimal();
    config.perm_interner_max_entries = Some(REBUILD_THRESHOLD);
    let expected_suggestion = REBUILD_THRESHOLD.next_power_of_two().saturating_mul(2);

    let js_source = format!(
        "var arr = []; for (var i = 0; i < {JS_OBJECTS}; i++) {{ var o = {{}}; o['s_' + i] = i; arr.push(o); }} arr.length === {JS_OBJECTS}"
    );
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, &js_source) {
        Ok(p) => p,
        Err(_) => return ExitCode::FAILURE,
    };
    let js_module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[kernel_lifetime] js phase compile failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut slopes: Vec<f64> = Vec::new();
    for run in 0..RERUNS {
        let kernel = KernelCore::new(config.clone());

        // JS 固定段：建对象 + 计算键写入（仅 intern 不物化）。
        let rss_pre = read_vmrss_kb().unwrap_or(0);
        {
            let mut vm = Vm::with_kernel_core(Arc::clone(&kernel));
            let result = match vm.run(&js_module) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[kernel_lifetime] run {run} js phase failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if result != JsValue::bool(true) {
                eprintln!("[kernel_lifetime] run {run} js phase result mismatch: {result:?}");
                return ExitCode::FAILURE;
            }
        }
        let baseline = kernel.perm_interner().entry_count();

        // 增长相：唯一键 intern + 1/4 物化，11 采样点采 VmRSS。
        let mut series: Vec<(usize, f64)> = Vec::new();
        let mut next_target = 0usize;
        for k in 0..TOTAL_STRINGS {
            let key = format!("s_{k:012x}");
            let (id, _) = kernel.perm_interner().intern(&key);
            if k % 4 == 0 {
                let _ = kernel.perm_interner().string_ptr(id);
            }
            let count = baseline + k + 1;
            while next_target < TARGETS.len() && count >= TARGETS[next_target] {
                if let Some(kb) = read_vmrss_kb() {
                    series.push((count as usize, kb as f64));
                }
                next_target += 1;
            }
        }
        // 尾部采样点（200k）可能未达：按最终唯一键数补采。
        while next_target < TARGETS.len() {
            if let Some(kb) = read_vmrss_kb() {
                series.push((kernel.perm_interner().entry_count() as usize, kb as f64));
            }
            next_target += 1;
        }
        let entries = kernel.perm_interner().entry_count();
        let rss_post = read_vmrss_kb().unwrap_or(0);
        let (slope, r2) = linreg(&series);
        slopes.push(slope);
        eprintln!(
            "[kernel_lifetime] run={run} threshold={REBUILD_THRESHOLD} pre={rss_pre} post={rss_post} samples={} entries={entries} slope={:.4} kB/entry (~{:.1} B/entry) R²={:.4} materialization≈1/4 (B143) [info: 探索期，不进 report_series 判据]",
            series.len(),
            slope,
            slope * 1024.0,
            r2
        );

        // 确定性断言：增长相总量 + 旋钮端到端（Some = 建议上限）。
        if entries != baseline + TOTAL_STRINGS {
            eprintln!("[kernel_lifetime] run {run} growth mismatch: baseline={baseline} entries={entries}");
            return ExitCode::FAILURE;
        }
        let knob = kernel.should_rebuild_perm();
        if knob != Some(expected_suggestion) {
            eprintln!("[kernel_lifetime] run {run} knob not armed: {knob:?} (expect Some({expected_suggestion}))");
            return ExitCode::FAILURE;
        }

        // 重建相：drop 旧 Arc 整体释放，新 kernel 严格主锚。
        drop(kernel);
        let rebuilt = KernelCore::new(config.clone());
        let rebuilt_entries = rebuilt.perm_interner().entry_count();
        let rebuilt_knob = rebuilt.should_rebuild_perm();
        eprintln!(
            "[kernel_lifetime] rebuild run={run}: threshold={REBUILD_THRESHOLD} suggestion={expected_suggestion} entries {entries} -> {rebuilt_entries} knob Some({expected_suggestion}) -> {rebuilt_knob:?} rss_kb={:?}",
            read_vmrss_kb()
        );
        if rebuilt_entries != 0 || rebuilt_knob.is_some() {
            return ExitCode::FAILURE;
        }
    }

    // 复跑一致性（仅记录不判定）：跨轮斜率差 >10% 先查 harness 污染
    // （分配器归还 OS 延迟），再疑用例。
    if slopes.len() == RERUNS {
        eprintln!(
            "[kernel_lifetime] rerun drift: {:.1}% (仅记录)",
            (slopes[0] - slopes[1]).abs() / slopes[0].max(1e-9) * 100.0
        );
    }
    ExitCode::SUCCESS
}

/// 全序列回归报告：打印首末 RSS、全序列斜率/R² 与窗口（末 20 样本）总
/// 增量；斜率显著为正（R² > 0.9）返回失败，表示泄漏签名。
fn report_series(case: &str, unit: &str, series: &[(usize, f64)]) -> ExitCode {
    let Some(&(first_i, first_kb)) = series.first() else {
        eprintln!("[{case}] no samples");
        return ExitCode::FAILURE;
    };
    let &(last_i, last_kb) = series.last().expect("首样本存在则末样本存在");
    let (slope, r2) = linreg(series);
    let window_delta = series
        .get(series.len().saturating_sub(20))
        .map(|(_, a)| last_kb - *a)
        .unwrap_or(0.0);
    eprintln!(
        "[{case}] samples={} {}={}..{} rss_kb={}..{} (delta {:+.0}) slope={:.4} kB/{unit} R²={:.4} window_delta={:+.0} kB",
        series.len(),
        unit,
        first_i,
        last_i,
        first_kb,
        last_kb,
        last_kb - first_kb,
        slope,
        r2,
        window_delta
    );
    if slope > 0.0 && r2 > 0.9 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linux 宿主 VmRSS 可读且大于 0。
    #[test]
    fn vmrss_readable() {
        assert!(matches!(read_vmrss_kb(), Some(kb) if kb > 0));
    }

    /// 已知斜率直线：斜率精确、R² 完美拟合。
    #[test]
    fn linreg_exact_line() {
        let pts: Vec<(usize, f64)> = (0..10).map(|i| (i, 2.0 * i as f64 + 1.0)).collect();
        let (slope, r2) = linreg(&pts);
        assert!((slope - 2.0).abs() < 1e-9);
        assert!((r2 - 1.0).abs() < 1e-9);
    }

    /// 零方差序列：斜率 0、R² 记完美拟合，不触发泄漏判定。
    #[test]
    fn linreg_constant_series() {
        let pts = vec![(0, 100.0), (1, 100.0), (2, 100.0)];
        let (slope, r2) = linreg(&pts);
        assert_eq!(slope, 0.0);
        assert_eq!(r2, 1.0);
    }

    /// 窗口采样器：正斜率序列出判定且斜率还原正确。
    #[test]
    fn sampler_verdict_on_rising_series() {
        let mut s = LeakSampler::new(20);
        let mut verdict = None;
        for i in 0..30 {
            verdict = s.add_sample(i, 1000.0 + 10.0 * i as f64);
        }
        let v = verdict.expect("正斜率高拟合序列应出判定");
        assert!((v.slope - 10.0).abs() < 1e-6);
        assert!(v.r2 > 0.9);
    }
}
