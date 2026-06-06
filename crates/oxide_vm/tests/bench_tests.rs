use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

#[test]
fn bench_1m_property_reads() {
    use std::time::Instant;

    let allocator = Allocator::default();
    let source = "({a: 1, b: 2, c: 3, d: 4, e: 5}).e";
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");

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
    println!(
        "1M IC property reads: {:.2}ms ({:.0} ns/read)",
        elapsed.as_secs_f64() * 1000.0,
        ns_per
    );
    if cfg!(debug_assertions) {
        println!("  (debug build -- skipping timing assertion)");
    } else {
        assert!(ns_per < 500.0, "{} ns/read exceeds 500ns target", ns_per);
    }
}
