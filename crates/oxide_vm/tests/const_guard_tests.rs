//! B011 const guard 回归测试：const 声明路径不依赖寄存器槽新鲜度。
//!
//! 背景：`dispatch_store_var` 的 const guard（b!=0）读 `regs[rd]` 判断目标槽是否已初始化。
//! 声明路径带 b=1 时，checkpoint/寄存器复用残留陈旧值会误抛 "Assignment to constant variable"。
//! Phase 5 RegAlloc 复用寄存器后此 bug 必现——本测试锁死声明路径恒 b=0 的行为。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// B011 复现样例：switch 语句（checkpoint 复用槽）后跟 const 声明。
// 修复前误抛 "Assignment to constant variable"，修复后输出 5。
#[test]
fn switch_then_const_declaration_no_false_positive() {
    assert_eq!(eval("switch (1) { case 1: break; } const x = 5; x"), "5");
}

// 锚：对已声明 const 再赋值仍必须抛错（guard 保留在赋值路径）。
#[test]
fn const_reassignment_still_throws() {
    let out = eval("const x = 5; x = 6");
    assert!(out.contains("Assignment to constant variable"), "got: {out}");
}

// 锚：块级 const 遮蔽互不影响（内层 const 不污染外层槽）。
#[test]
fn block_scoped_const_shadowing_independent() {
    assert_eq!(eval("const x = 5; { const x = 1; } x"), "5");
}

// 锚：解构 const 声明正常（emit_object_binding → emit_bind_target）。
#[test]
fn const_destructuring_declaration_works() {
    assert_eq!(eval("const { a } = { a: 7 }; a"), "7");
}
