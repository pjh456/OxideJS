//! 生成器 × session GC 回归：promote 后 full_reset 释放 epoch 原对象与 session 克隆
//! 的堆数据（各单所有权），不 double-free、不悬垂，VM 继续可用。
//! sweep 变体与闭包捕获变体见 `vm_support.rs` 内部测试（需 crate 内 `resume_generator`
//! 在模块表未重建时恢复执行，integration 层无法触达）。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

/// promote + full_reset 变体：global 根持有生成器克隆，full_reset 释放 epoch 原对象
/// 与 session 克隆的堆数据（各单所有权），不 double-free、不悬垂，VM 继续可用。
#[test]
fn generator_promoted_to_global_survives_full_reset_cleanly() {
    let mut vm = Vm::new();
    vm.run(&compile("function* g(){ yield 1; yield 2; } globalThis.it = g(); globalThis.it.next(); 0"))
        .expect("run1");
    assert!(vm.session_object_count() > 0, "生成器挂 global 应被 promote 进 session");

    // global 含 session 对象即强制重建：旧 global 丢弃、it 槽不存在，全程无悬垂访问
    // （修复前 epoch 原对象释放共享状态盒 → 读已释放内存）。
    vm.full_reset();
    assert_eq!(vm.session_object_count(), 0, "full_reset 应清空 session 对象");

    let result = vm.run(&compile("typeof it")).expect("run2");
    let text = vm.lookup_str(result).expect("typeof 应返回字符串").to_string();
    assert_eq!(text, "undefined");
    // 新会话可正常创建并推进生成器。
    let ok = vm.run(&compile("function* h(){ yield 9; } var it2 = h(); it2.next().value")).expect("run3");
    assert_eq!(format!("{ok}"), "9");
}
