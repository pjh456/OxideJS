//! 集成冒烟：真实编译产物（parse→emit→build_cfg→liveness→color）不崩 + 完整性/确定性断言。

use oxide_regalloc::{Alloc, AllocMap};
use std::collections::BTreeSet;

fn run_color(src: &str) -> (AllocMap, oxide_ir::IRFunction) {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse");
    let ir = oxide_emit::Emitter::new().emit_program(&program, false, false).expect("emit");
    let cfg = oxide_cfg::build_cfg(&ir);
    let live = oxide_liveness::liveness(&ir, &cfg);
    let m = oxide_regalloc::color(&ir, &live).expect("color");
    (m, ir)
}

/// 收集函数内全部字面真实 vreg（对照 contract def/use，排除 0/254/255 保留位）。
fn literal_regs(ir: &oxide_ir::IRFunction) -> BTreeSet<u32> {
    let mut set = BTreeSet::new();
    for inst in &ir.insts {
        for o in [&inst.rd, &inst.a, &inst.b] {
            if let oxide_ir::operand::Operand::Reg(r) = o {
                if *r != 0 && *r != 254 && *r != 255 {
                    set.insert(*r);
                }
            }
        }
    }
    set
}

fn smoke(src: &str) {
    let (m, ir) = run_color(src);
    // 完整性：全部真实 vreg 有条目
    for r in literal_regs(&ir) {
        assert!(m.map.contains_key(&r), "vreg {r} 无分配");
    }
    // phys ≤ 253；非预着色色 < arg_window_base
    for a in m.map.values() {
        if let Alloc::Phys(c) = a {
            assert!(*c <= 253, "Phys 色 {c} 超 253");
            assert!(*c < m.arg_window_base || is_precolored(&ir, *c), "色 {c} 应在可分配集或为预着色");
        }
    }
    // spill slot 不重复
    let mut slots = BTreeSet::new();
    for s in &m.spills {
        assert!(slots.insert(s.slot), "spill slot {} 重复", s.slot);
    }
    assert!(m.phys_peak <= 254, "phys_peak {} 超 254", m.phys_peak);
}

fn is_precolored(ir: &oxide_ir::IRFunction, c: u32) -> bool {
    let pl = ir.param_layout;
    if c >= pl.base && c < pl.base + pl.count {
        return true;
    }
    // escaped 预着色（简化：允许参数段色）
    false
}

#[test]
fn smoke_simple_function() {
    smoke("function f(a, b) { return a + b; } f(1, 2)");
}

#[test]
fn smoke_if_else() {
    smoke("function f(x) { if (x > 0) { return 1; } else { return 2; } } f(5)");
}

#[test]
fn smoke_loop() {
    smoke("function f(n) { var s = 0; for (var i = 0; i < n; i++) { s = s + i; } return s; } f(10)");
}

#[test]
fn smoke_try_catch() {
    smoke("function x() {} function y(e) {} function f() { try { x(); } catch (e) { y(e); } } f()");
}

#[test]
fn smoke_nested_calls() {
    smoke("function g(x) { return x + 1; } function f(x) { return g(x) + g(x + 1); } f(1)");
}

#[test]
fn smoke_class_field() {
    smoke("var outerVar = 42; class C { p = outerVar; } new C()");
}

#[test]
fn smoke_destructuring() {
    smoke("function f() { var { a, b } = { a: 1, b: 2 }; return a + b; } f()");
}

#[test]
fn color_is_deterministic_on_real_ir() {
    let src = "function f(n) { var s = 0; for (var i = 0; i < n; i++) { s = s + i; } return s; } f(10)";
    let (a, _) = run_color(src);
    let (b, _) = run_color(src);
    assert_eq!(a, b);
}
