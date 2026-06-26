use criterion::{black_box, criterion_group, criterion_main, Criterion};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use std::sync::Arc;

fn bench_gc(c: &mut Criterion) {
    let mut config = KernelConfig::minimal();
    config.session_gc_threshold = 1024 * 1024;
    let kernel = KernelCore::new(config);
    let js = "var g = {}; for (var i = 0; i < 500; i++) { var obj = { x: i, y: i * 2 }; g[i] = obj; } g[\"x\"]";
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, js).expect("parse");
    let module = Compiler.compile(&program).expect("compile");

    c.bench_function("gc_mark_sweep", |b| {
        b.iter_batched(
            || Vm::with_kernel_core(Arc::clone(&kernel)),
            |mut vm| {
                vm.run(&module).ok();
                black_box(vm)
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_gc);
criterion_main!(benches);
