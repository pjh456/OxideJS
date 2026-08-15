//! 精确轮（Pass D）：liveness 驱动的死指令 + 局部死 STORE_VAR 删除。
//!
//! 消费 `oxide_liveness::LiveInfo::inst_live_after` 判定（契约复用 contract.rs，不重复建分析引擎）：
//! - 通用规则：纯指令 def_reg 不在 live_after → 删（含寄存器复用死写，保守 use 计数抓不到）
//! - STORE_VAR 特例（def_reg=None 不命中通用规则）：非顶层 && b==Imm(0) && 槽非 escaped && 槽不在 live_after → 删
//!
//! label 目标保守保留；单遍不级联（保守轮已做连锁）；nested 不递归。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 单遍死指令标记：就地改写 `keep`（保活/删除标记），f 只读。
pub(super) fn pass_dead_with_liveness(f: &IRFunction, live: &LiveInfo, keep: &mut [bool]) {
    // label 目标位图：label_pos 指向的 Inst 下标（照 iter_sweep:36-42，越界防御性忽略）
    let mut label_target = vec![false; f.insts.len()];
    for pos in f.label_pos.iter().flatten() {
        if let Some(t) = label_target.get_mut(*pos) {
            *t = true;
        }
    }
    // escaped 槽位图：nested 树 LOAD_VAR.a / STORE_VAR.rd 直引的父槽（父 liveness 看不到
    // 跨函数读，删 STORE_VAR 后子模块会读陈旧/undefined 槽）
    let mut max_reg: usize = 0;
    for inst in &f.insts {
        if let Some(d) = inst.def_reg() {
            max_reg = max_reg.max(d as usize);
        }
        for u in inst.use_regs() {
            max_reg = max_reg.max(u as usize);
        }
    }
    let mut escaped_slot = vec![false; max_reg + 1];
    super::iter_sweep::collect_escaped_slots(&f.nested, &mut escaped_slot);
    // 单遍扫描：不级联（删 STORE_VAR 后 a 源残留死纯代码无害，保守轮已做连锁）
    for (i, inst) in f.insts.iter().enumerate() {
        if !keep[i] || label_target[i] {
            continue;
        }
        let after = &live.inst_live_after[i];
        let not_live_after = |r: u32| !oxide_liveness::bitset_get(after, r as usize);
        // 通用规则：纯指令结果不被后续使用 → 删（is_pure=false 已防 CALL/SPILL/UNSPILL）
        if let Some(r) = inst.def_reg() {
            if not_live_after(r) && inst.is_pure(f) {
                keep[i] = false;
                continue;
            }
        }
        // STORE_VAR 特例：def_reg=None 不命中通用规则，单独判定（四条件全满足才删）。
        // 槽活度条件是必须的：`var y=1; return y` 删掉 STORE_VAR 会让 LOAD_VAR 读到
        // 陈旧/undefined 槽（vreg 化 + RegAlloc 复用后脏槽）。
        if inst.op == OpCode::STORE_VAR {
            let slot = match inst.rd {
                Operand::Reg(s) => s as usize,
                _ => continue,
            };
            let deletable = !f.is_top_level // 顶层赋值全局可观察，不可删
                && matches!(inst.b, Operand::Imm(0)) // const 赋值 b=1 抛 TypeError，不可删
                && !escaped_slot.get(slot).copied().unwrap_or(false) // nested 直读槽不可动
                && not_live_after(slot as u32); // 槽无后续读（vreg 复用后脏槽风险）
            if deletable {
                keep[i] = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dce_precise;
    use oxide_ir::inst::Inst;

    /// helper：稀疏号集 → u64 位集 live_after。
    fn live_sets(after: &[&[u32]], reg_count: usize) -> Vec<Vec<u64>> {
        let words = (reg_count + 1).div_ceil(64);
        after
            .iter()
            .map(|set| {
                let mut v = vec![0u64; words];
                for &r in *set {
                    v[(r as usize) >> 6] |= 1u64 << ((r as usize) & 63);
                }
                v
            })
            .collect()
    }

    fn run_precise(f: &mut IRFunction, after: &[&[u32]], reg_count: usize) {
        let mut live = LiveInfo::new();
        live.inst_live_after = live_sets(after, reg_count);
        dce_precise(f, &live);
    }

    /// 精确轮标志性测试：寄存器复用后被后写杀死的死写（保守 use 计数抓不到）。
    #[test]
    fn reuse_killed_dead_write_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::ADD,
            Operand::Reg(5),
            Operand::Reg(1),
            Operand::Reg(2),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::ADD,
            Operand::Reg(5),
            Operand::Reg(3),
            Operand::Reg(4),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        // inst_live_after = [{3,4}, {5}, {}]：inst0 def 5 在 live_after[0] 无 5（后写杀死）
        run_precise(&mut f, &[&[3, 4], &[5], &[]], 5);
        assert_eq!(f.insts.len(), 2, "死写（前一个 ADD）应被删");
    }

    /// 局部死 STORE_VAR 删除（四条件全满足）。
    #[test]
    fn local_dead_store_var_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::STORE_VAR,
            Operand::Reg(2),
            Operand::Reg(1),
            Operand::Imm(0),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        // is_top_level 默认 false；inst_live_after = [{1,5}, {5}, {}]
        run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
        assert_eq!(f.insts.len(), 2, "局部死 STORE_VAR 应被删（LOAD_CONST 单遍不级联保留）");
    }

    /// 顶层 STORE_VAR 永不删（顶层赋值全局可观察）。
    #[test]
    fn top_level_store_var_kept() {
        let mut f = IRFunction::new();
        f.is_top_level = true;
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::STORE_VAR,
            Operand::Reg(2),
            Operand::Reg(1),
            Operand::Imm(0),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
        assert_eq!(f.insts.len(), 3, "顶层 STORE_VAR 不可删");
    }

    /// escaped 槽 STORE_VAR 永不删（nested 直读）。
    #[test]
    fn escaped_slot_store_var_kept() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::STORE_VAR,
            Operand::Reg(2),
            Operand::Reg(1),
            Operand::Imm(0),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        let mut sub = IRFunction::new();
        sub.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::LOAD_VAR,
            Operand::Reg(5),
            Operand::Reg(2),
            Operand::None,
        ));
        f.nested.push(sub);
        run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
        assert_eq!(f.insts.len(), 3, "被嵌套函数直引的 escaped 槽 STORE_VAR 不可删");
    }

    /// const 赋值路径 b=1 保留（运行时抛 TypeError 可观察）。
    #[test]
    fn const_guard_store_var_kept() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::STORE_VAR,
            Operand::Reg(2),
            Operand::Reg(1),
            Operand::Imm(1),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
        assert_eq!(f.insts.len(), 3, "b=1 const 赋值路径不可删");
    }

    /// label 目标指令永不删。
    #[test]
    fn label_target_inst_never_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0)); // label 0 目标，纯且结果未用
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        f.label_pos = vec![Some(0)];
        run_precise(&mut f, &[&[5], &[]], 5);
        assert_eq!(f.insts.len(), 2, "label 目标指令不可删");
    }

    /// SPILL/UNSPILL 永不删（is_pure=false）；活 MOV 保留。
    #[test]
    fn spill_unspill_never_deleted_mov_kept_when_live() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::inst_spill(Operand::Reg(1), 0));
        f.insts.push(Inst::inst_unspill(Operand::Reg(2), 0));
        f.insts.push(Inst::inst_mov(Operand::Reg(3), Operand::Reg(2)));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(3),
            Operand::None,
            Operand::None,
        ));
        run_precise(&mut f, &[&[], &[2], &[3], &[]], 5);
        assert_eq!(f.insts.len(), 4, "SPILL/UNSPILL 永不删，活 MOV 保留");
    }

    /// 过期 LiveInfo 维度守卫：直接 return 零删除。
    #[test]
    fn stale_liveinfo_no_op() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::STORE_VAR,
            Operand::Reg(2),
            Operand::Reg(1),
            Operand::Imm(0),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        let live = LiveInfo::new(); // 空 LiveInfo：维度不符
        dce_precise(&mut f, &live);
        assert_eq!(f.insts.len(), 3, "过期 LiveInfo 不得删除任何指令");
    }

    /// 收敛性：单遍不级联设计下，重复跑会逐轮收敛（删 STORE_VAR 后其源 LOAD_CONST 变死），
    /// 收敛到不动点后不再改变。
    #[test]
    fn dce_precise_converges() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::STORE_VAR,
            Operand::Reg(2),
            Operand::Reg(1),
            Operand::Imm(0),
        ));
        f.insts.push(Inst::new(
            oxide_bytecode::opcode::OpCode::RETURN,
            Operand::Reg(5),
            Operand::None,
            Operand::None,
        ));
        // 第一遍：手填 live（STORE_VAR 死）
        run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
        assert_eq!(f.insts.len(), 2, "第一遍删 STORE_VAR");
        // 第二遍：重算 live → LOAD_CONST 变死
        let cfg = oxide_cfg::build_cfg(&f);
        let live = oxide_liveness::liveness(&f, &cfg);
        dce_precise(&mut f, &live);
        assert_eq!(f.insts.len(), 1, "第二遍删 LOAD_CONST（级联收敛）");
        let after2 = f.insts.clone();
        // 第三遍：不动点，不再改变
        let cfg = oxide_cfg::build_cfg(&f);
        let live = oxide_liveness::liveness(&f, &cfg);
        dce_precise(&mut f, &live);
        assert_eq!(f.insts, after2, "第三遍不再删除（收敛）");
    }

    /// 空函数退化。
    #[test]
    fn empty_function_unchanged() {
        let mut f = IRFunction::new();
        let live = LiveInfo::new();
        dce_precise(&mut f, &live);
        assert!(f.insts.is_empty());
    }
}
