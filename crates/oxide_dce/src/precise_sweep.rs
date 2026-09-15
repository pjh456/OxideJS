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
