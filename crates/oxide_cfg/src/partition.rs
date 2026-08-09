//! Pass 1：块头识别。
//!
//! 块头集合 = `{0}` ∪ label 目标位置 ∪ 跳转目标 ∪ 条件跳转 fallthrough 后继。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 识别块头位置：返回 `heads`，长度 `insts.len()+1`（+1 容纳指向末尾的空尾块）。
pub(super) fn partition_blocks(f: &IRFunction) -> Vec<bool> {
    let len = f.insts.len();
    let mut heads = vec![false; len + 1];
    heads[0] = true;
    for p in f.label_pos.iter().flatten() {
        heads[*p] = true;
    }
    for (i, inst) in f.insts.iter().enumerate() {
        // label 恒在 b 槽（inst.rs 构造 API 保证，不查 rd/a 槽）。
        if let Operand::Label(l) = inst.b {
            match f.label_pos.get(l as usize).and_then(|p| *p) {
                Some(p) => heads[p] = true,
                None => debug_assert!(false, "unresolved label {l}"),
            }
        }
        // 跳转的 fallthrough 后继是块头：条件跳转否则 cond+then 融块、条件边丢失；
        // 无条件 JMP 同样必须切块头——JMP 不 fallthrough，但其后继位置需成块边界，
        // 否则 JMP 与后续指令融块、块尾判定错、Jump 边丢失（DCE 可达性误删目标块）。
        if matches!(
            inst.op,
            OpCode::JMP
                | OpCode::BREAK
                | OpCode::CONTINUE
                | OpCode::JMP_IF_TRUE
                | OpCode::JMP_IF_FALSE
                | OpCode::JMP_IF_NULLISH
        ) {
            heads[i + 1] = true;
        }
    }
    heads
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::inst::Inst;

    /// 线性无跳转函数：仅 entry 为块头，其余位置非块头。
    #[test]
    fn linear_function_only_entry_is_head() {
        let mut f = IRFunction::new();
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));

        let heads = partition_blocks(&f);
        assert_eq!(heads, vec![true, false, false, false], "仅 inst 0 是块头");
    }

    /// label 目标是块头：`label_pos` 指向的指令位置置 heads。
    #[test]
    fn label_target_is_head() {
        let mut f = IRFunction::new();
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 0
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1: label 0 目标
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(1)];
        f.label_count = 1;

        let heads = partition_blocks(&f);
        assert!(heads[0] && heads[1], "entry 与 label 目标都是块头");
        assert!(!heads[2], "非块头位置保持 false");
    }

    /// 条件跳转的 fallthrough 后继是块头（否则 cond+then 融块、条件边丢失）。
    #[test]
    fn conditional_jump_fallthrough_successor_is_head() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(1, 0)); // 0: cond → L0(else)
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1: then
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(2)];
        f.label_count = 1;

        let heads = partition_blocks(&f);
        assert!(heads[1], "条件跳转 fallthrough 后继是块头");
    }

    /// 无条件 JMP 的后继位置成块头（JMP 不 fallthrough，后续代码需独立成块，修复 Jump 边丢失）。
    #[test]
    fn unconditional_jmp_successor_is_head() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp(0)); // 0: → L0
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1: 独立块头
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(2)];
        f.label_count = 1;

        let heads = partition_blocks(&f);
        assert!(heads[1], "无条件 JMP 后继位置是块头");
    }
}
