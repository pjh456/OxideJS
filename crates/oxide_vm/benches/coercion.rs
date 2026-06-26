use criterion::{black_box, criterion_group, criterion_main, Criterion};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use std::sync::Arc;

fn bench_coercion(c: &mut Criterion) {
    let kernel = KernelCore::new(KernelConfig::minimal());
    let js = "var x = \"42\"; var y = \"3.14\"; var r = 0; for (var i = 0; i < 5000; i++) { r += (+x) + (+y); } r";
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, js).expect("parse");
    let module = Compiler.compile(&program).expect("compile");

    c.bench_function("coercion", |b| {
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

criterion_group!(benches, bench_coercion);
criterion_main!(benches);
