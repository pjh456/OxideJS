//! DCE 改写 pass：IRFunction → 死代码消除后 compact 重建。
//!
//! 改写 pass：`&mut IRFunction` 就地 compact（以指令下标索引 keep 位图，无借用冲突），
//! 三 Pass 各居一模块（照 oxide_cfg 四阶段拆文件先例）：
//! - `reachability`（Pass A）：块级可达性，消费 `oxide_cfg::build_cfg`，Exception 边保守遍历
//! - `iter_sweep`（Pass B）：全函数 use 计数迭代删除死指令到不动点（连锁删上游写者）
//! - `rebuild`（Pass C）：mark-sweep 一次性重建 insts + label_pos 重映射（不用就地逐删平移）
//!
//! 本 pass 不碰 nested（不递归）、常量池（不清理）、n_registers（不收缩）、
//! label_count（不读不改）。中间产物（keep/use_count）用局部 Vec 显式传参，
//! 不做 struct 状态持有（无共享可变状态约定）。零 unsafe。
//!
//! 精确二轮 `dce_precise`：liveness 驱动的死指令 + 局部死 STORE_VAR 删除，
//! 消费 `oxide_liveness::LiveInfo`，mark-sweep 重建复用 Pass C。

mod iter_sweep;
mod precise_sweep;
mod reachability;
mod rebuild;

use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 死代码消除：块级不可达删除 + 全函数 use 计数迭代删除到不动点 + mark-sweep 重建。
pub fn dce(f: &mut IRFunction) {
    // 空 IRFunction 退化形态：无指令可删（照 oxide_cfg empty_function 先例）。
    if f.insts.is_empty() {
        return;
    }
    // Pass A：块级可达性（消费 build_cfg，Exception 边保守）→ 指令粒度 keep。
    let mut keep = reachability::pass_a_reachable(f);
    // Pass B：可达存活指令上全函数 use 计数迭代删除到不动点（连锁删上游写者）。
    iter_sweep::pass_b_dead_code(f, &mut keep);
    // Pass C：mark-sweep 一次性重建 insts + label_pos 重映射。
    rebuild::pass_c_sweep(f, &keep);
}

/// 精确二轮：liveness 驱动的死指令 + 局部死 STORE_VAR 删除。
///
/// 消费 oxide_liveness::LiveInfo（RegAlloc 管线传入；若 LiveInfo 已过期，重建 liveness
/// 是调用方 RegAlloc 的职责，本 pass 不重复建分析引擎）。mark-sweep 重建复用既有 Pass C。
pub fn dce_precise(f: &mut IRFunction, live: &LiveInfo) {
    // 空函数退化（照 dce 先例）
    if f.insts.is_empty() {
        return;
    }
    // 维度守卫：live 与 insts 对齐才可信（删改后的过期 LiveInfo 索引错位会误删活指令）
    if live.inst_live_after.len() != f.insts.len() {
        return;
    }
    let mut keep = vec![true; f.insts.len()];
    precise_sweep::pass_dead_with_liveness(f, live, &mut keep);
    // mark-sweep 一次性重建
    rebuild::pass_c_sweep(f, &keep);
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    /// 空函数退化：insts 空时 dce 直接返回，状态不变。
    #[test]
    fn empty_function_unchanged() {
        let mut f = IRFunction::new();
        dce(&mut f);
        assert!(f.insts.is_empty());
        assert!(f.label_pos.is_empty());
        assert_eq!(f.label_count, 0);
    }

    /// 幂等性：dce 两次结果一致（不动点后无二次改写）。
    #[test]
    fn dce_is_idempotent() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0)); // 0: 死
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(1), Operand::Reg(1))); // 1: 死
        f.insts.push(Inst::load_const(Operand::Reg(0), 1)); // 2: 存活（CALL 保活 r0）
        f.insts.push(Inst::call(Operand::Reg(0), Operand::Reg(0), Operand::Reg(0), 0)); // 3
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(0)];

        dce(&mut f);
        let once_insts = f.insts.clone();
        let once_labels = f.label_pos.clone();
        dce(&mut f);
        assert_eq!(f.insts, once_insts, "二次 dce 后 insts 不变");
        assert_eq!(f.label_pos, once_labels, "二次 dce 后 label_pos 不变");
    }

    /// if/else 双分支完整链路：Jump + Fallthrough 双出边全可达，指令全保留，label 重映射正确。
    #[test]
    fn if_else_both_branches_reachable_kept() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(0, 0)); // 0: cond → L0(else)
        f.insts.push(Inst::call(Operand::Reg(2), Operand::Reg(0), Operand::Reg(3), 1)); // 1: then
        f.insts.push(Inst::jmp(1)); // 2: → L1(end)
        f.insts.push(Inst::call(Operand::Reg(4), Operand::Reg(0), Operand::Reg(3), 1)); // 3: else
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3), Some(4)];
        f.label_count = 2;

        dce(&mut f);
        assert_eq!(f.insts.len(), 5, "双分支全保留");
        assert_eq!(f.label_pos, vec![Some(3), Some(4)], "存活 label 重映射到新下标");
        assert_eq!(f.label_count, 2, "label_count 保持原值，不参与重建");
    }
}
