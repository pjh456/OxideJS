//! 块内反向扫描：块级 liveOut → 逐指令 live_before/live_after。
//!
//! 起点是 **block_live_out**（非 liveIn——从 liveIn 出发会得到错误放大的逐指令集）。
//! 反向扫描 **kill 先于 gen**（`live = (live − def) ∪ use`）：COMPOUND_ADD 等读-写
//! 同寄存器指令 use 含 rd（contract.rs:137），gen 先于 kill 会把 rd 旧值从
//! live_before 错误剔除。Exception 目标块入口 reg 0 隐式 def 在此截断，并以
//! `debug_assert_eq!(live, block_live_in[b])` 校验与 dataflow 一致性。
//!
//! 位集为 `u64` 压缩（与 dataflow 同 words 布局），逐指令集经 `copy_from_slice` 落地。

use oxide_cfg::{Cfg, EdgeKind};
use oxide_ir::IRFunction;

use crate::live_info::{bitset_clear, bitset_set};

/// 逐指令 liveness：返回 (inst_live_before, inst_live_after)，长度 = insts.len()。
pub(super) fn inst_liveness(
    f: &IRFunction, cfg: &Cfg, block_live_out: &[Vec<u64>], block_live_in: &[Vec<u64>], reg_count: usize,
) -> (Vec<Vec<u64>>, Vec<Vec<u64>>) {
    let words = (reg_count + 1).div_ceil(64);
    let mut before = vec![vec![0u64; words]; f.insts.len()];
    let mut after = vec![vec![0u64; words]; f.insts.len()];
    for (b, block) in cfg.blocks.iter().enumerate() {
        let mut live = block_live_out[b].clone(); // 从块级 liveOut 出发（非 liveIn）
        for i in block.inst_range.clone().rev() {
            after[i].copy_from_slice(&live);
            // kill 先于 gen：本指令先写后读（COMPOUND_ADD 等读-写同寄存器，contract.rs:137）
            if let Some(d) = f.insts[i].def_reg() {
                bitset_clear(&mut live, d as usize);
            }
            for u in f.insts[i].use_regs() {
                bitset_set(&mut live, u as usize);
            }
            before[i].copy_from_slice(&live);
        }
        // Exception 目标块：入口 reg 0 隐式 def（异常展开写 regs[0]）截断无 def use。
        // preds 是 Vec<BBId>（无 EdgeKind），从源块 succs 侧检测 Exception 边。
        let is_exception_target = cfg
            .blocks
            .iter()
            .any(|src| src.succs.iter().any(|&(dst, k)| dst == b && k == EdgeKind::Exception));
        if is_exception_target {
            bitset_clear(&mut live, 0);
        }
        debug_assert_eq!(live, block_live_in[b], "ranges 反向扫描与 dataflow liveIn 不一致，块 {b}");
    }
    (before, after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataflow::block_liveness;
    use crate::live_info::bitset_get;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    fn empty_function() -> IRFunction {
        IRFunction::new()
    }

    #[test]
    fn linear_block_instruction_sets() {
        // 0: ADD r2=r0+r1    1: RETURN r2
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(0), Operand::Reg(1)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, live_out, rc) = block_liveness(&f, &cfg);
        let (before, after) = inst_liveness(&f, &cfg, &live_out, &live_in, rc);
        assert!(bitset_get(&after[0], 2), "ADD 后 r2 存活（RETURN 用）");
        assert!(bitset_get(&before[0], 0) && bitset_get(&before[0], 1), "ADD 前 r0、r1 存活");
        assert!(!bitset_get(&before[0], 2), "ADD 前 r2 不死");
        assert!(after[1].iter().all(|&w| w == 0), "RETURN 后无存活");
        assert!(bitset_get(&before[1], 2), "RETURN 前 r2 存活");
    }

    #[test]
    fn read_modify_write_kills_before_gen() {
        // 0: COMPOUND_ADD rd=1, a=2    def {1} use {1,2}
        // 1: RETURN r1
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::COMPOUND_ADD, Operand::Reg(1), Operand::Reg(2), Operand::None));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, live_out, rc) = block_liveness(&f, &cfg);
        let (before, _) = inst_liveness(&f, &cfg, &live_out, &live_in, rc);
        assert!(bitset_get(&before[0], 1), "COMPOUND_ADD 前 r1 旧值在读（kill 先于 gen）");
        assert!(bitset_get(&before[0], 2), "COMPOUND_ADD 前 r2 存活");
        assert!(bitset_get(&live_in[0], 1) && bitset_get(&live_in[0], 2), "块级 liveIn 含 r1、r2");
    }

    #[test]
    fn call_result_reg0_range_is_short() {
        // 0: call(callee=1, this=2, first_arg=3, nargs=1)   def {0} use {1,2,3}
        // 1: LOAD_VAR(Reg(4), None, None)                   def {4} use {0}
        // 2: RETURN r4
        let mut f = empty_function();
        f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
        f.insts
            .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(4), Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(4), Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, live_out, rc) = block_liveness(&f, &cfg);
        let (before, after) = inst_liveness(&f, &cfg, &live_out, &live_in, rc);
        assert!(bitset_get(&before[1], 0), "LOAD_VAR 前读 CALL 结果 reg0");
        assert!(bitset_get(&after[0], 0), "CALL 后 reg0 存活（结果）");
        assert!(!bitset_get(&before[0], 0), "CALL 前 reg0 不死（区间仅 [CALL, LOAD_VAR)）");
        assert!(!bitset_get(&live_in[0], 0), "reg0 不污染块入口");
    }

    #[test]
    fn catch_entry_reg0_not_live_before_block() {
        // 0: try_begin(3)   1: ADD r3=r3+r2   2: RETURN r3
        // 3: STORE_VAR(r5, None, None)   4: RETURN r5
        let mut f = empty_function();
        f.insts.push(Inst::try_begin(3));
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(3), Operand::Reg(2)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::STORE_VAR, Operand::Reg(5), Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        f.label_pos = vec![None, None, None, Some(3)];
        f.label_count = 1;
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, live_out, rc) = block_liveness(&f, &cfg);
        let (before, _) = inst_liveness(&f, &cfg, &live_out, &live_in, rc);
        assert!(bitset_get(&before[3], 0), "catch 块内 STORE_VAR 读 reg0（异常值，正确）");
        let catch_block = cfg.blocks.iter().position(|b| b.inst_range == (3..5usize)).unwrap();
        assert!(!bitset_get(&live_in[catch_block], 0), "catch 块入口 reg0 被隐式 def 截断");
        assert!(!bitset_get(&live_in[0], 0), "try 块（entry）reg0 不污染入口");
    }
}
