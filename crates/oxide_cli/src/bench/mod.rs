//! Benchmark 子系统：支持 JS 压力测试（`js`）、Rust 原生 bench（`rust`）
//! 与内存泄漏检测（`leak`），并附带基线保存/回归对比功能。

/// 基准持久化与回归对比。
pub mod baseline;
/// JS 压力测试执行。
pub mod js_stress;
/// 内存泄漏检测。
pub mod leak_detect;
/// 指标采集结构。
pub mod metrics;
/// 结果格式化输出。
pub mod output;
/// Rust 原生 benchmark 委托。
pub mod rust_bench;

use std::process::ExitCode;
use std::sync::Arc;

use oxide_kernel::kernel::KernelCore;
use oxide_vm::vm_pool::VmPool;

/// 一次 bench 运行的全部配置（来自 CLI `bench` 子命令参数）。
pub struct BenchConfig {
    pub mode: String,
    pub filter: Option<String>,
    pub warmup: u32,
    pub iterations: u32,
    pub process: bool,
    pub update_baseline: bool,
    pub leak_check_interval: usize,
}

/// 按 mode 分派到具体 benchmark 实现（js/rust/leak）。
pub fn run_benchmarks(config: BenchConfig, kernel: Arc<KernelCore>, pool: Arc<VmPool>) -> ExitCode {
    match config.mode.as_str() {
        "js" => js_stress::run_js_stress_bench(&config, &kernel, &pool),
        "rust" => rust_bench::run_rust_bench(config.filter.as_deref()),
        // leak 模式内按 case 过滤参数分派校准用例；缺省保持原泄漏检测行为。
        "leak" => match config.filter.as_deref() {
            Some("vm_creation") => leak_detect::run_mem_vm_creation_leak(&kernel),
            Some("dirty_rebuild") => leak_detect::run_mem_dirty_rebuild_leak(&kernel),
            Some("closure_dead_leak") => leak_detect::run_mem_closure_dead_leak(&kernel),
            Some("pool_high_water") => leak_detect::run_mem_pool_high_water(&kernel),
            Some("object_churn_peak") => leak_detect::run_mem_object_churn_peak(&kernel),
            Some("kernel_lifetime") => leak_detect::run_mem_kernel_lifetime(),
            _ => leak_detect::run_leak_detect(&config, &kernel, &pool),
        },
        _ => {
            eprintln!("Unknown bench mode: {}. Use: js, rust, or leak", config.mode);
            ExitCode::FAILURE
        }
    }
}
