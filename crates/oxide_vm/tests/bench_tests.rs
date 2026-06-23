use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    Compiler::new().compile(&program).expect("compile failed")
}

#[test]
fn bench_1m_property_reads() {
    use std::time::Instant;

    let source = "({a: 1, b: 2, c: 3, d: 4, e: 5}).e";
    let module = compile(source);

    let mut vm = Vm::new();
    vm.run(&module).expect("vm run failed");
    for _ in 0..100 {
        vm.rerun().ok();
    }

    const N: usize = 1_000_000;
    let start = Instant::now();
    for _ in 0..N {
        vm.rerun().ok();
    }
    let elapsed = start.elapsed();
    let ns_per = elapsed.as_nanos() as f64 / N as f64;
    println!("1M IC property reads: {:.2}ms ({:.0} ns/read)", elapsed.as_secs_f64() * 1000.0, ns_per);
    if cfg!(debug_assertions) {
        println!("  (debug build -- skipping timing assertion)");
    } else if cfg!(windows) {
        println!("  (windows release build -- skipping absolute timing assertion)");
    } else {
        assert!(ns_per < 500.0, "{} ns/read exceeds 500ns target", ns_per);
    }
}

#[test]
fn bench_clean_reset_cost() {
    use std::time::Instant;

    const N: usize = 1_000;

    let mut selective_vm = Vm::new();
    selective_vm.full_reset();
    let selective_start = Instant::now();
    for _ in 0..N {
        selective_vm.full_reset();
    }
    let selective_elapsed = selective_start.elapsed();

    let mut legacy_vm = Vm::new();
    legacy_vm.full_reset_legacy_for_bench();
    let legacy_start = Instant::now();
    for _ in 0..N {
        legacy_vm.full_reset_legacy_for_bench();
    }
    let legacy_elapsed = legacy_start.elapsed();

    let selective_ns = selective_elapsed.as_nanos() as f64 / N as f64;
    let legacy_ns = legacy_elapsed.as_nanos() as f64 / N as f64;
    println!(
        "clean full_reset: selective {:.2}ms ({:.0} ns/reset), legacy {:.2}ms ({:.0} ns/reset)",
        selective_elapsed.as_secs_f64() * 1000.0,
        selective_ns,
        legacy_elapsed.as_secs_f64() * 1000.0,
        legacy_ns
    );

    if cfg!(debug_assertions) {
        println!("  (debug build -- skipping timing assertion)");
    } else {
        assert!(
            selective_elapsed.as_nanos() * 5 <= legacy_elapsed.as_nanos(),
            "selective reset must be >=80% faster; selective={selective_ns:.0}ns legacy={legacy_ns:.0}ns"
        );
    }
}

#[test]
fn bench_dirty_reset_cost() {
    use std::time::{Duration, Instant};

    const N: usize = 1_000;

    let dirty_array_proto = compile("Array.prototype.__resetBench = 1");
    let mut vm = Vm::new();
    let mut elapsed = Duration::ZERO;

    for _ in 0..N {
        vm.run(&dirty_array_proto).expect("dirty Array.prototype");
        let reset_start = Instant::now();
        vm.full_reset();
        elapsed += reset_start.elapsed();
    }

    let ns_per = elapsed.as_nanos() as f64 / N as f64;
    println!("dirty array full_reset: {:.2}ms ({:.0} ns/reset)", elapsed.as_secs_f64() * 1000.0, ns_per);
}
