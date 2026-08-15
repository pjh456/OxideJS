//! 集成冒烟：真实编译产物（parse→emit→build_cfg→liveness）跑不崩 + 维度一致性 + try/catch reg0 断言 + 确定性。

use oxide_cfg::build_cfg;
use oxide_liveness::{liveness, LiveInfo};

fn run_liveness(src: &str) -> (LiveInfo, oxide_cfg::Cfg, usize) {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse");
    let ir = oxide_emit::Emitter::new().emit_program(&program).expect("emit");
    let cfg = build_cfg(&ir);
    let info = liveness(&ir, &cfg);
    (info, cfg, ir.insts.len())
}

fn smoke(src: &str) {
    let (info, cfg, inst_len) = run_liveness(src);
    assert_eq!(info.block_live_in.len(), cfg.blocks.len(), "block 维度一致");
    assert_eq!(info.block_live_out.len(), cfg.blocks.len(), "block 维度一致");
    assert_eq!(info.inst_live_before.len(), inst_len, "inst 维度一致");
    assert_eq!(info.inst_live_after.len(), inst_len, "inst 维度一致");
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
fn smoke_loop_with_variables() {
    smoke("function f(n) { var s = 0; for (var i = 0; i < n; i++) { s = s + i; } return s; } f(10)");
}

#[test]
fn smoke_try_catch() {
    let (info, cfg, _) =
        run_liveness("function x() {} function y(e) {} function f() { try { x(); } catch (e) { y(e); } } f()");
    // entry 块 liveIn 不含 reg 0（异常边 reg0 截断在真实产物上成立）
    assert!(!oxide_liveness::bitset_get(&info.block_live_in[cfg.entry], 0), "entry 块 liveIn 不得含 reg0");
}

#[test]
fn smoke_nested_calls() {
    smoke("function g(x) { return x + 1; } function f(x) { return g(x) + g(x + 1); } f(1)");
}

#[test]
fn liveness_is_deterministic() {
    let src = "function f(n) { var s = 0; for (var i = 0; i < n; i++) { s = s + i; } return s; } f(10)";
    let (a, _, _) = run_liveness(src);
    let (b, _, _) = run_liveness(src);
    assert_eq!(a.block_live_in, b.block_live_in);
    assert_eq!(a.block_live_out, b.block_live_out);
    assert_eq!(a.inst_live_before, b.inst_live_before);
    assert_eq!(a.inst_live_after, b.inst_live_after);
}
