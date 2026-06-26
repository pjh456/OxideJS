use std::collections::VecDeque;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_kernel::kernel::KernelCore;
use oxide_parser::Allocator;
use oxide_vm::vm_pool::VmPool;

use crate::bench::metrics::MetricCollection;
use crate::bench::output::format_json;
use crate::bench::BenchConfig;

pub struct LeakSampler {
    window: VecDeque<(usize, f64)>,
    window_size: usize,
}

impl LeakSampler {
    pub fn new(window_size: usize) -> Self {
        Self {
            window: VecDeque::new(),
            window_size,
        }
    }

    pub fn add_sample(&mut self, iteration: usize, value: f64) -> Option<LeakVerdict> {
        self.window.push_back((iteration, value));
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        if self.window.len() < 10 {
            return None;
        }
        let n = self.window.len() as f64;
        let sum_x: f64 = self.window.iter().map(|(i, _)| *i as f64).sum();
        let sum_y: f64 = self.window.iter().map(|(_, v)| v).sum();
        let sum_xy: f64 = self.window.iter().map(|(i, v)| *i as f64 * v).sum();
        let sum_x2: f64 = self.window.iter().map(|(i, _)| (*i as f64).powi(2)).sum();
        let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x * sum_x);
        let intercept = (sum_y - slope * sum_x) / n;
        let y_mean = sum_y / n;
        let ss_res: f64 = self
            .window
            .iter()
            .map(|(i, v)| (v - (slope * *i as f64 + intercept)).powi(2))
            .sum();
        let ss_tot: f64 = self.window.iter().map(|(_, v)| (v - y_mean).powi(2)).sum();
        let r2 = if ss_tot == 0.0 { 1.0 } else { 1.0 - ss_res / ss_tot };
        if r2 > 0.9 && slope > 0.0 {
            Some(LeakVerdict { slope, r2 })
        } else {
            None
        }
    }
}

pub struct LeakVerdict {
    pub slope: f64,
    pub r2: f64,
}

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
            });
        }
    }

    let json = format_json(&results);
    println!("{}", json);
    ExitCode::SUCCESS
}
