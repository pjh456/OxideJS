//! Pass C：mark-sweep 一次性重建 insts + label_pos 重映射（不用就地逐删平移——
//! 多删点叠加偏移易错）。sweep：keep 序 compact 重建；label_pos 逐 id 重写，
//! 目标被删的 label 置 None 保留 id 槽位（lowering 只读 label_pos 不读 label_count）。

use oxide_ir::inst::Inst;
use oxide_ir::IRFunction;

/// mark-sweep 重建：按 `keep` 序 compact `f.insts`，同步重映射 `f.label_pos`。
pub(super) fn pass_c_sweep(f: &mut IRFunction, keep: &[bool]) {
    // old_to_new：存活位置记新下标，被删记 usize::MAX
    let mut old_to_new = vec![usize::MAX; f.insts.len()];
    let mut new_insts: Vec<Inst> = Vec::with_capacity(keep.iter().filter(|k| **k).count());
    for (old, k) in keep.iter().enumerate() {
        if *k {
            old_to_new[old] = new_insts.len();
            new_insts.push(f.insts[old].clone());
        }
    }
    f.insts = new_insts;
    // label_pos 逐 id 重写：get + usize::MAX 过滤做越界守卫（不信任 label_pos 长度）
    for pos in f.label_pos.iter_mut() {
        if let Some(old) = *pos {
            *pos = old_to_new.get(old).filter(|n| **n != usize::MAX).copied();
        }
    }
    // 防御性校验：存活 label 新下标不越界（空尾块映射 new insts.len() 合法透传）
    let new_len = f.insts.len();
    debug_assert!(f.label_pos.iter().all(|p| p.map_or(true, |np| np <= new_len)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_bytecode::module::Constant;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    /// 存活 label 新旧下标对应：删除前置死指令后，label 目标重映射到新下标。
    #[test]
    fn live_label_remapped_to_new_index() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(5), 1)); // 0: 存活
        f.insts.push(Inst::load_const(Operand::Reg(9), 2)); // 1: 死（删）
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(2)]; // label 0 → inst 2（RETURN）
        f.label_count = 1;

        pass_c_sweep(&mut f, &[true, false, true]);
        assert_eq!(f.insts.len(), 2, "死 LOAD_CONST 删除");
        assert_eq!(f.insts[0].op, OpCode::LOAD_CONST);
        assert_eq!(f.insts[1].op, OpCode::RETURN);
        assert_eq!(f.label_pos, vec![Some(1)], "存活 label 2 → 新下标 1");
    }

    /// 不动域断言：constants / n_registers / nested / label_count 一律不动；
    /// 死 label 置 None、存活 label 重映射、原 None 保持。
    #[test]
    fn untouched_domains_asserted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0)); // 0: label 0 目标（死值，保守保留）
        f.insts.push(Inst::load_const(Operand::Reg(2), 1)); // 1: 非 label 目标的死指令（删）
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(0), Some(2), None];
        f.label_count = 3;
        f.n_registers = 10;
        f.constants = vec![Constant::Number(1.0)];
        f.nested.push(IRFunction::new());

        pass_c_sweep(&mut f, &[true, false, true]);
        assert_eq!(f.insts.len(), 2, "非目标死指令删、label 目标死指令保守保留");
        assert_eq!(f.insts[0].op, OpCode::LOAD_CONST);
        assert_eq!(f.insts[1].op, OpCode::RETURN);
        assert_eq!(f.label_pos, vec![Some(0), Some(1), None], "存活 label→新下标、原 None 保持");
        assert_eq!(f.label_count, 3, "label_count 保持原值，不参与重建");
        assert_eq!(f.n_registers, 10, "n_registers 不收缩");
        assert_eq!(f.constants, vec![Constant::Number(1.0)], "常量池不清理");
        assert_eq!(f.nested.len(), 1, "nested 不递归不回收");
    }
}
