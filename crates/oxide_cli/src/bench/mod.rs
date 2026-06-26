pub mod baseline;
pub mod js_stress;
pub mod leak_detect;
pub mod metrics;
pub mod output;
pub mod rust_bench;

use std::process::ExitCode;
use std::sync::Arc;

use oxide_kernel::kernel::KernelCore;
use oxide_vm::vm_pool::VmPool;

pub struct BenchConfig {
    pub mode: String,
    pub filter: Option<String>,
    pub warmup: u32,
    pub iterations: u32,
    pub process: bool,
    pub update_baseline: bool,
    pub leak_check_interval: usize,
}

pub fn run_benchmarks(config: BenchConfig, kernel: Arc<KernelCore>, pool: Arc<VmPool>) -> ExitCode {
    match config.mode.as_str() {
        "js" => js_stress::run_js_stress_bench(&config, &kernel, &pool),
        "rust" => rust_bench::run_rust_bench(config.filter.as_deref()),
        "leak" => leak_detect::run_leak_detect(&config, &kernel, &pool),
        _ => {
            eprintln!("Unknown bench mode: {}. Use: js, rust, or leak", config.mode);
            ExitCode::FAILURE
        }
    }
}
