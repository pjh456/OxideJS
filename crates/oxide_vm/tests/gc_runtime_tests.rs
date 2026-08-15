use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

fn vm_with_threshold(bytes: usize) -> Vm {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(bytes);
    Vm::with_kernel_core(KernelCore::new(config))
}

/// 长循环拼接产生海量 session 字符串：执行期自动触发仅字符串 GC，
/// 结果正确且 session 字节有界（不触发则累计 ~20 MiB 远超低阈值）。
#[test]
fn long_string_loop_triggers_runtime_gc_and_bounds_session() {
    let mut vm = vm_with_threshold(4096);
    let module = compile("var s; for (var i = 0; i < 500000; i++) { s = 'str' + i; } s.length");
    let result = vm.run(&module).expect("run");

    assert_eq!(format!("{}", result), "9");
    assert!(vm.session_gc_stats().total_collections > 0, "执行期应触发字符串 GC");
    assert!(
        vm.session_bytes_allocated() < 1_000_000,
        "session 字节应受阈值约束，实际 {}",
        vm.session_bytes_allocated()
    );
}

/// 拼接结果写 global（逃逸根），跨 run 后触发执行期 GC 仍可读——
/// 验证非会话存活串不因执行期回收丢失。
#[test]
fn session_string_survives_across_runs_with_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("globalThis.kept = 'he' + 'llo'; 0");
    vm.run(&first).expect("run1");

    let second = compile("var s; for (var i = 0; i < 500000; i++) { s = 'y' + i; } globalThis.kept");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("kept 应为字符串").to_string();

    assert_eq!(text, "hello");
    assert!(vm.session_gc_stats().total_collections > 0, "run2 应触发执行期字符串 GC");
}

/// reset（轻量重置）保留 session 字符串：跨 eval 存活语义不被执行期 GC 破坏。
#[test]
fn reset_preserves_session_strings_after_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("globalThis.kept = 'x' + 'y'; 0");
    vm.run(&first).expect("run1");
    vm.reset();

    let second = compile("globalThis.kept");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("kept 应为字符串").to_string();
    assert_eq!(text, "xy");
}

/// strings-only 收集后 reset 完整收集：global 子树中 session 对象属性里的串
/// 跨两次收集存活（strings-only 残留 mark 位不得短路完整收集的 mark DFS）。
#[test]
fn strings_only_then_reset_keeps_global_subtree_strings() {
    let mut vm = vm_with_threshold(4096);
    let first = compile(
        "globalThis.kept = { s: 'he' + 'llo' }; var t; for (var i = 0; i < 500000; i++) { t = 'x' + i; } 0",
    );
    vm.run(&first).expect("run1");
    assert!(vm.session_gc_stats().total_collections > 0, "run1 应触发执行期字符串 GC");
    vm.reset();

    let second = compile("globalThis.kept.s");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("kept.s 应为字符串").to_string();
    assert_eq!(text, "hello");
}

/// full_reset 清空 session 内存：执行期回收释放过的死串不与完全重置的
/// 整体释放路径重复释放（无双重释放崩溃），重置后引擎可继续运行。
#[test]
fn full_reset_clears_session_after_runtime_gc() {
    let mut vm = vm_with_threshold(4096);
    let first = compile("var s; for (var i = 0; i < 200000; i++) { s = 'a' + i; } s.length");
    vm.run(&first).expect("run1");
    assert!(vm.session_gc_stats().total_collections > 0, "run1 应触发执行期字符串 GC");

    vm.full_reset();

    assert_eq!(vm.session_bytes_allocated(), 0, "full_reset 后 session 字节应为 0");
    let second = compile("'ok' + '!'");
    let result = vm.run(&second).expect("run2");
    let text = vm.lookup_str(result).expect("拼接结果应为字符串").to_string();
    assert_eq!(text, "ok!");
}
