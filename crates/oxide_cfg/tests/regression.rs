//! 集成回归锚（D-12 第 2 层）：真实 JS 编译产物 parse→emit→build_cfg。
//!
//! 验证 CFG 不崩且结构合理。仅新增本文件，不修改既有测试文件（D-13）。

use oxide_cfg::build_cfg;
use oxide_cfg::EdgeKind;

/// parse→emit→build_cfg 全链路：真实编译产物构建 CFG。
fn cfg_from_source(src: &str) -> oxide_cfg::Cfg {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse failed");
    let ir = oxide_emit::Emitter::new().emit_program(&program).expect("emit failed");
    build_cfg(&ir)
}

/// succs/preds 对称性断言（Pitfall 4）。
fn assert_symmetric(cfg: &oxide_cfg::Cfg) {
    let succs_total: usize = cfg.blocks.iter().map(|b| b.succs.len()).sum();
    let preds_total: usize = cfg.blocks.iter().map(|b| b.preds.len()).sum();
    assert_eq!(succs_total, preds_total, "succs/preds 不对称");
}

/// 全部边目标块 id 可解析（target < blocks.len()）。
fn assert_targets_resolvable(cfg: &oxide_cfg::Cfg) {
    for (i, b) in cfg.blocks.iter().enumerate() {
        for &(target, _) in &b.succs {
            assert!(target < cfg.blocks.len(), "块 {i} 的边目标 {target} 越界");
        }
    }
}

/// try/catch 真实编译产物：Exception 边存在、exit 有 pred、目标可解析、对称。
#[test]
fn try_catch_regression_anchor() {
    let cfg = cfg_from_source("try { var x = 1; } catch (e) { var y = 2; }");
    // (a) TRY_BEGIN 出发的异常边至少一条（D-07/D-08）
    let has_exception = cfg.blocks.iter().any(|b| b.succs.iter().any(|&(_, k)| k == EdgeKind::Exception));
    assert!(has_exception, "期望存在 Exception 边");
    // (b) 顶层 HALT 收尾 → exit 哨兵 preds 非空（D-05）
    assert!(!cfg.blocks[cfg.exit].preds.is_empty(), "exit 哨兵应有 pred（HALT 汇入）");
    // (c) 全部边目标可解析
    assert_targets_resolvable(&cfg);
    // (d) succs/preds 对称
    assert_symmetric(&cfg);
}

/// 每个 nested 子函数各自独立可建 CFG（D-11：不递归，但各函数可建）。
#[test]
fn nested_function_cfgs_are_independent() {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "function outer(){ var x = 0; while(x<3){x=x+1;} }").expect("parse failed");
    let ir = oxide_emit::Emitter::new().emit_program(&program).expect("emit failed");
    for nested in &ir.nested {
        let cfg = build_cfg(nested);
        assert!(!cfg.blocks.is_empty());
    }
}

/// 线性程序：1 实块 + exit 哨兵（CFG-01 线性函数单块）。
#[test]
fn linear_program_single_block_plus_exit() {
    let cfg = cfg_from_source("var x = 1;");
    assert_eq!(cfg.blocks.len(), 2);
    assert_eq!(cfg.exit, 1);
}

/// 顶层 if/else：条件分裂块 + entry 可达覆盖全部块 + 对称（CFG-01 对 if/else）。
#[test]
fn if_else_conditional_edges() {
    let cfg = cfg_from_source("var x = 1; if(x){ var a = 1; } else { var b = 2; }");
    // (a) 条件分裂：某块 succs 同时含 Jump 与 Fallthrough 两条出边
    let has_split = cfg.blocks.iter().any(|b| {
        b.succs.iter().any(|&(_, k)| k == EdgeKind::Jump) && b.succs.iter().any(|&(_, k)| k == EdgeKind::Fallthrough)
    });
    assert!(has_split, "期望条件分裂块（Jump + Fallthrough 双出边）");
    // (b) 从 entry 沿 succs DFS 覆盖全部块 id（含 exit——顶层以 HALT Fallthrough 汇入）
    let mut visited = vec![false; cfg.blocks.len()];
    let mut stack = vec![cfg.entry];
    visited[cfg.entry] = true;
    while let Some(b) = stack.pop() {
        for &(t, _) in &cfg.blocks[b].succs {
            if !visited[t] {
                visited[t] = true;
                stack.push(t);
            }
        }
    }
    for (i, v) in visited.iter().enumerate() {
        assert!(v, "块 {i} 从 entry 不可达");
    }
    // (c) succs/preds 对称
    assert_symmetric(&cfg);
}
