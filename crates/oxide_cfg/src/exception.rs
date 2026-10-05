//! Pass 3：异常边。
//!
//! TRY_BEGIN：匹配 TRY_BEGIN/TRY_END 对（栈匹配，处理嵌套），从 try 体
//! [p_begin, p_end) 内所有重叠块连 Exception 边到 catch 入口。try 体内任意点
//! 可抛异常，catch 须读到的活变量在整段 try 体保持存活，故异常边须覆盖整段。
//!
//! TRY_FINALLY_BEGIN：从所在 BB 连 Exception 边到 finally 入口（仅起始 BB）。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

use crate::{BasicBlock, EdgeKind};

/// 补异常边：TRY_BEGIN 从 try 体全部重叠块出发，TRY_FINALLY_BEGIN 从所在块出发。
pub(super) fn add_exception_edges(f: &IRFunction, blocks: &mut [BasicBlock]) {
    // TRY_BEGIN/TRY_END 栈匹配（处理嵌套），收集 (p_begin, p_end, catch_label)。
    let mut try_spans: Vec<(usize, usize, u32)> = Vec::new();
    let mut stack: Vec<(usize, u32)> = Vec::new();
    for (idx, inst) in f.insts.iter().enumerate() {
        match inst.op {
            OpCode::TRY_BEGIN => {
                if let Operand::Label(l) = inst.b {
                    stack.push((idx, l));
                }
            }
            OpCode::TRY_END => {
                if let Some((p_begin, catch_label)) = stack.pop() {
                    try_spans.push((p_begin, idx, catch_label));
                }
            }
            _ => {}
        }
    }
    // 每个 try 体：从 [p_begin, p_end) 内所有重叠块连 Exception 边到 catch 入口。
    for (p_begin, p_end, catch_label) in try_spans {
        if let Some(p) = f.label_pos.get(catch_label as usize).and_then(|p| *p) {
            let target = crate::split::block_id_of(p, blocks);
            for block in blocks.iter_mut() {
                if block.inst_range.start < p_end
                    && block.inst_range.end > p_begin
                    && !block.succs.contains(&(target, EdgeKind::Exception))
                {
                    block.succs.push((target, EdgeKind::Exception));
                }
            }
        }
    }
    // TRY_FINALLY_BEGIN：从所在块连 Exception 边到 finally 入口（仅起始块）。
    // 先收集 (block_index, target) 对，再统一加边（避免 iter_mut 与 block_id_of 借用冲突）。
    let mut finally_edges: Vec<(usize, usize)> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let range = block.inst_range.clone();
        for inst_idx in range.start..range.end {
            if f.insts[inst_idx].op != OpCode::TRY_FINALLY_BEGIN {
                continue;
            }
            if let Operand::Label(l) = f.insts[inst_idx].b {
                match f.label_pos.get(l as usize).and_then(|p| *p) {
                    Some(p) => finally_edges.push((i, crate::split::block_id_of(p, blocks))),
                    None => debug_assert!(false, "unresolved label {l}"),
                }
            }
        }
    }
    for (i, target) in finally_edges {
        if !blocks[i].succs.contains(&(target, EdgeKind::Exception)) {
            blocks[i].succs.push((target, EdgeKind::Exception));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BasicBlock;
    use oxide_ir::inst::Inst;

    /// TRY_BEGIN 从 try 体全部重叠块出发 Exception 边 → catch 入口块。
    #[test]
    fn try_begin_emits_exception_edge_to_handler() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → catch（label 0 → inst 3）
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
        f.insts
            .push(Inst::new(OpCode::TRY_END, Operand::None, Operand::None, Operand::None)); // 2
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: catch 入口
        f.label_pos = vec![Some(3)];
        f.label_count = 1;

        let mut blocks = vec![
            BasicBlock {
                inst_range: 0..3,
                preds: Vec::new(),
                succs: Vec::new(),
            },
            BasicBlock {
                inst_range: 3..4,
                preds: Vec::new(),
                succs: Vec::new(),
            },
        ];
        add_exception_edges(&f, &mut blocks);
        assert_eq!(blocks[0].succs, vec![(1, EdgeKind::Exception)], "try 体块出 Exception 边到 catch 入口");
        assert!(blocks[1].succs.is_empty(), "catch 入口块无出边");
    }

    /// 块内多个 TRY 标记指向同一目标：Exception 边去重。
    #[test]
    fn duplicate_exception_targets_deduplicated() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → L0
        f.insts.push(Inst::try_finally_begin(0)); // 1: TRY_FINALLY_BEGIN → 同一 L0
        f.insts
            .push(Inst::new(OpCode::TRY_END, Operand::None, Operand::None, Operand::None)); // 2
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: L0 目标
        f.label_pos = vec![Some(3)];
        f.label_count = 1;

        let mut blocks = vec![
            BasicBlock {
                inst_range: 0..3,
                preds: Vec::new(),
                succs: Vec::new(),
            },
            BasicBlock {
                inst_range: 3..4,
                preds: Vec::new(),
                succs: Vec::new(),
            },
        ];
        add_exception_edges(&f, &mut blocks);
        assert_eq!(
            blocks[0].succs,
            vec![(1, EdgeKind::Exception)],
            "两个 TRY 标记指向同一目标只保留一条 Exception 边"
        );
    }

    /// try 体跨多块时，每块都出 Exception 边到 catch 入口（非仅 TRY_BEGIN 所在块）。
    #[test]
    fn exception_edges_from_all_try_body_blocks() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → catch（label 0 → inst 6）
        f.insts.push(Inst::jmp_if_false(1, 3)); // 1: 条件分支
        f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 0)); // 2: CALL（then）
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: false 分支
        f.insts
            .push(Inst::new(OpCode::TRY_END, Operand::None, Operand::None, Operand::None)); // 4
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 5: 汇合
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 6: catch 入口
        f.label_pos = vec![Some(6), None, None, Some(3)];
        f.label_count = 4;

        let cfg = crate::build_cfg(&f);
        // 块：[0..2) TRY_BEGIN+JMP_IF_FALSE, [2..3) CALL, [3..6) false+TRY_END+NOP,
        //     [6..7) catch, [0..0) exit
        let catch_block = cfg.blocks.iter().position(|b| b.inst_range == (6..7)).unwrap();
        // CALL 块（then 分支）须有 Exception 边到 catch 入口
        let call_block = cfg.blocks.iter().position(|b| b.inst_range == (2..3)).unwrap();
        assert!(
            cfg.blocks[call_block].succs.contains(&(catch_block, EdgeKind::Exception)),
            "CALL 块（try 体深处）应有 Exception 边到 catch 入口"
        );
        // false 分支块也须有 Exception 边到 catch 入口
        let false_block = cfg.blocks.iter().position(|b| b.inst_range == (3..6)).unwrap();
        assert!(
            cfg.blocks[false_block].succs.contains(&(catch_block, EdgeKind::Exception)),
            "false 分支块（try 体深处）应有 Exception 边到 catch 入口"
        );
    }
}
