//! Pass A：块级可达性。
//!
//! 消费 `oxide_cfg::build_cfg` 的 Cfg，从 entry 沿 succs DFS（**遍历含 EdgeKind::Exception 边**，
//! catch/finally 入口强制可达），不可达块的全部指令标记删除。

use oxide_cfg::build_cfg;
use oxide_ir::IRFunction;

/// 从 entry 可达的指令位图（keep 前置：不可达指令为 false）。
pub(super) fn pass_a_reachable(f: &IRFunction) -> Vec<bool> {
    let cfg = build_cfg(f);
    let mut reachable = vec![false; cfg.blocks.len()];
    let mut stack = vec![cfg.entry];
    while let Some(b) = stack.pop() {
        if reachable[b] {
            continue;
        }
        reachable[b] = true;
        // succs 三边全遍历：Jump / Fallthrough / Exception（保守，异常入口强制可达）
        for &(succ, _kind) in &cfg.blocks[b].succs {
            stack.push(succ);
        }
    }
    // 块可达 → 展开到指令粒度 keep
    let mut keep = vec![false; f.insts.len()];
    for (b, r) in reachable.iter().enumerate() {
        if *r {
            for i in cfg.blocks[b].inst_range.clone() {
                keep[i] = true;
            }
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    /// 线性函数全可达：所有指令 keep=true。
    #[test]
    fn linear_all_insts_reachable() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
        let keep = pass_a_reachable(&f);
        assert!(keep.iter().all(|k| *k), "线性函数全指令可达");
    }

    /// return 后死块不可达：标记 keep=false（return 后紧跟的自指 JMP 块）。
    #[test]
    fn unreachable_block_marked_dead() {
        let mut f = IRFunction::new();
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(0), Operand::None, Operand::None)); // 0
        f.insts.push(Inst::jmp(0)); // 1: 死块（L0 目标），自指 JMP
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(1)];
        f.label_count = 1;

        let keep = pass_a_reachable(&f);
        assert!(keep[0], "RETURN 指令可达");
        assert!(!keep[1] && !keep[2], "return 后死块不可达");
    }

    /// Exception 边保守：TRY_BEGIN 的 Exception 边使 catch 入口强制可达，
    /// 即使没有正常边指向它。
    #[test]
    fn exception_edge_keeps_catch_entry() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → catch 入口（label 0 → inst 3）
        f.insts.push(Inst::load_const(Operand::Reg(5), 1)); // 1
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        f.insts.push(Inst::load_const(Operand::Reg(6), 2)); // 3: catch 入口
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3)];
        f.label_count = 1;

        let keep = pass_a_reachable(&f);
        assert!(keep[3] && keep[4], "catch 入口经 Exception 边强制可达");
    }
}
