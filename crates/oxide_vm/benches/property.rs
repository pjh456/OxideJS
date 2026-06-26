use criterion::{black_box, criterion_group, criterion_main, Criterion};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use std::sync::Arc;

fn bench_property(c: &mut Criterion) {
    let kernel = KernelCore::new(KernelConfig::minimal());
    let js = "var obj = { a: 1, b: 2, c: 3, d: 4, e: 5 }; var sum = 0; for (var i = 0; i < 10000; i++) { sum += obj.a + obj.b + obj.c + obj.d + obj.e; } sum";
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, js).expect("parse");
    let module = Compiler.compile(&program).expect("compile");

    c.bench_function("property", |b| {
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

criterion_group!(benches, bench_property);
criterion_main!(benches);
