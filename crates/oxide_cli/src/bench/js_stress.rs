use std::fs;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_kernel::kernel::KernelCore;
use oxide_parser::Allocator;
use oxide_vm::vm_pool::VmPool;

use crate::bench::baseline::{compare_baseline, save_baseline};
use crate::bench::metrics::MetricCollection;
use crate::bench::output::{format_json, format_text_table};
use crate::bench::BenchConfig;

pub fn run_js_stress_bench(config: &BenchConfig, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>) -> ExitCode {
    let mut results: Vec<MetricCollection> = Vec::new();
    let compiler = Compiler::new();

    let test_dir = "tests/stress";
    let mut files: Vec<_> = match fs::read_dir(test_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().is_some_and(|ext| ext == "js")
                    && config
                        .filter
                        .as_ref()
                        .map_or(true, |f| e.file_name().to_string_lossy().contains(f.as_str()))
            })
            .map(|e| e.path())
            .collect(),
        Err(e) => {
            eprintln!("Could not read {}: {}", test_dir, e);
            return ExitCode::FAILURE;
        }
    };
    files.sort();

    if files.is_empty() {
        eprintln!("No matching stress tests found in {}", test_dir);
        return ExitCode::FAILURE;
    }

    for path in &files {
        let test_name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut js = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to read {:?}: {}", path, e);
                continue;
            }
        };

        if config.iterations > 0 {
            js = override_iterations(&js, config.iterations);
        }

        let allocator = Allocator::default();
        let program = match oxide_parser::parse(&allocator, &js) {
            Ok(p) => p,
            Err(errors) => {
                eprintln!("Parse error in {}: {:?}", test_name, errors.first());
                continue;
            }
        };

        let hash = compiled_module_hash(&program);
        let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Compile error in {}: {}", test_name, e);
                continue;
            }
        };

        for _ in 0..config.warmup {
            let mut guard = pool.spawn();
            let _ = guard.vm_mut().run(&module);
        }

        let mut metrics = Vec::new();
        for _ in 0..config.iterations {
            let compile_start = Instant::now();
            let mut guard = pool.spawn();
            let vm = guard.vm_mut();
            let compile_time = compile_start.elapsed();

            let pre_session = vm.session_object_count();
            let pre_epoch = vm.epoch_object_count();

            let exec_start = Instant::now();
            let _result = vm.run(&module);
            let exec_time = exec_start.elapsed();

            let gc_stats = vm.session_gc_stats();

            metrics.push(MetricCollection {
                test_name: test_name.clone(),
                wall_time_us: (compile_time + exec_time).as_micros() as u64,
                session_objects: vm.session_object_count().saturating_sub(pre_session),
                session_bytes: vm.session_bytes_allocated(),
                epoch_objects: vm.epoch_object_count().saturating_sub(pre_epoch),
                epoch_bytes: 0,
                gc_trigger_count: gc_stats.total_collections,
                gc_bytes_freed: gc_stats.last_collection_bytes_freed,
                gc_objects_scanned: gc_stats.last_collection_objects_scanned,
                gc_collection_us: gc_stats.last_collection_duration_us,
                instruction_count: vm.instruction_count(),
                compile_time_us: compile_time.as_micros() as u64,
                exec_time_us: exec_time.as_micros() as u64,
                ic_hit_rate: vm.ic_hit_rate(),
                ic_hits: vm.ic_hit_count(),
                ic_misses: vm.ic_miss_count(),
            });
        }

        let avg = average_metrics(&metrics);
        results.push(avg);
    }

    let json = format_json(&results);
    let table = format_text_table(&results);
    eprintln!("{}", table);
    println!("{}", json);

    if config.update_baseline {
        if let Err(e) = save_baseline(&results) {
            eprintln!("Failed to save baseline: {}", e);
            return ExitCode::FAILURE;
        }
        eprintln!("Baseline saved to BENCHMARK_BASELINE.md + benchmark_baseline.json");
    } else {
        let regressions = compare_baseline(&results);
        if !regressions.is_empty() {
            eprintln!("\nRegression detected:");
            for r in &regressions {
                eprintln!(
                    "  {}: {} baseline={} current={} ratio={:.2}% tolerance={:.0}%",
                    r.test_name,
                    r.metric,
                    r.baseline,
                    r.current,
                    (r.ratio - 1.0) * 100.0,
                    r.tolerance * 100.0,
                );
            }
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

fn average_metrics(metrics: &[MetricCollection]) -> MetricCollection {
    let n = metrics.len() as f64;
    if n == 0.0 || metrics.is_empty() {
        return metrics.first().cloned().unwrap_or(MetricCollection {
            test_name: String::new(),
            wall_time_us: 0,
            session_objects: 0,
            session_bytes: 0,
            epoch_objects: 0,
            epoch_bytes: 0,
            gc_trigger_count: 0,
            gc_bytes_freed: 0,
            gc_objects_scanned: 0,
            gc_collection_us: 0,
            instruction_count: 0,
            compile_time_us: 0,
            exec_time_us: 0,
            ic_hit_rate: 0.0,
            ic_hits: 0,
            ic_misses: 0,
        });
    }
    MetricCollection {
        test_name: metrics[0].test_name.clone(),
        wall_time_us: (metrics.iter().map(|m| m.wall_time_us as f64).sum::<f64>() / n) as u64,
        session_objects: (metrics.iter().map(|m| m.session_objects as f64).sum::<f64>() / n) as usize,
        session_bytes: (metrics.iter().map(|m| m.session_bytes as f64).sum::<f64>() / n) as usize,
        epoch_objects: (metrics.iter().map(|m| m.epoch_objects as f64).sum::<f64>() / n) as usize,
        epoch_bytes: (metrics.iter().map(|m| m.epoch_bytes as f64).sum::<f64>() / n) as u64,
        gc_trigger_count: (metrics.iter().map(|m| m.gc_trigger_count as f64).sum::<f64>() / n) as u64,
        gc_bytes_freed: (metrics.iter().map(|m| m.gc_bytes_freed as f64).sum::<f64>() / n) as u64,
        gc_objects_scanned: (metrics.iter().map(|m| m.gc_objects_scanned as f64).sum::<f64>() / n) as u64,
        gc_collection_us: (metrics.iter().map(|m| m.gc_collection_us as f64).sum::<f64>() / n) as u64,
        instruction_count: (metrics.iter().map(|m| m.instruction_count as f64).sum::<f64>() / n) as u64,
        compile_time_us: (metrics.iter().map(|m| m.compile_time_us as f64).sum::<f64>() / n) as u64,
        exec_time_us: (metrics.iter().map(|m| m.exec_time_us as f64).sum::<f64>() / n) as u64,
        ic_hit_rate: metrics.iter().map(|m| m.ic_hit_rate).sum::<f64>() / n,
        ic_hits: (metrics.iter().map(|m| m.ic_hits as f64).sum::<f64>() / n) as u64,
        ic_misses: (metrics.iter().map(|m| m.ic_misses as f64).sum::<f64>() / n) as u64,
    }
}

fn override_iterations(js: &str, iters: u32) -> String {
    js.lines()
        .map(|line| {
            if line.trim_start().starts_with("var ITERATIONS ") {
                format!("var ITERATIONS = {};", iters)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
