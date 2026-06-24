use criterion::{black_box, criterion_group, criterion_main, Criterion};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use std::sync::Arc;

fn bench_call(c: &mut Criterion) {
    let kernel = KernelCore::new(KernelConfig::minimal());
    let js = "function fib(n) { if (n < 2) return n; return fib(n-1) + fib(n-2); } fib(15);";
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, js).expect("parse");
    let module = Compiler.compile(&program).expect("compile");

    c.bench_function("call", |b| {
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

criterion_group!(benches, bench_call);
criterion_main!(benches);
