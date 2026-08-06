//! DCE 改写 pass：IRFunction → 死代码消除后 compact 重建。
//!
//! 改写 pass（D-09）：`&mut IRFunction` 就地 compact（D-06 方案 A 索引关联保证无借用冲突），
//! 三 Pass——Pass A 块级可达性（消费 `oxide_cfg::build_cfg`，Exception 边保守，D-07/Pitfall 4）、
//! Pass B 全函数 use 计数迭代删除死指令到不动点（D-01/D-02 连锁语义）、
//! Pass C mark-sweep 一次性重建 insts + label_pos 重映射（D-13，不用就地逐删平移）。
//! 本 pass 不碰 nested（D-03 不递归）、常量池（D-14 不清理）、n_registers（D-04 不收缩）、
//! label_count（D-13 不读不改）。中间产物（keep/use_count）用局部 Vec 显式传参，
//! 不做 struct 状态持有（无共享可变状态约定）。零 unsafe。

use oxide_cfg::build_cfg;
use oxide_ir::inst::Inst;
use oxide_ir::IRFunction;

/// 死代码消除：块级不可达删除 + 全函数 use 计数迭代删除到不动点 + mark-sweep 重建。
pub fn dce(f: &mut IRFunction) {
    // 空 IRFunction 退化形态：无指令可删（照 oxide_cfg empty_function 先例）。
    if f.insts.is_empty() {
        return;
    }
    // Pass A：块级可达性（消费 build_cfg，Exception 边保守）→ 指令粒度 keep。
    let mut keep = pass_a_reachable(f);
    // Pass B：可达存活指令上全函数 use 计数迭代删除到不动点（D-02 连锁语义）。
    pass_b_dead_code(f, &mut keep);
    // Pass C：mark-sweep 一次性重建 insts + label_pos 重映射（D-13）。
    pass_c_sweep(f, &keep);
}

/// Pass A：块级可达性。从 entry 沿 succs DFS（**遍历含 EdgeKind::Exception 边**，
/// D-07/Pitfall 4——catch/finally 入口强制可达），不可达块的全部指令标记删除。
fn pass_a_reachable(f: &IRFunction) -> Vec<bool> {
    let cfg = build_cfg(f);
    let mut reachable = vec![false; cfg.blocks.len()];
    let mut stack = vec![cfg.entry];
    while let Some(b) = stack.pop() {
        if reachable[b] {
            continue;
        }
        reachable[b] = true;
        // succs 三边全遍历：Jump / Fallthrough / Exception（D-07 保守）
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

/// Pass B：全函数 use 计数迭代删除到不动点（D-02 连锁语义）。
///
/// 只对可达存活指令聚合 use 计数（逐条 `inst.use_regs()` 累加）→ 收集 `def_reg` 为
/// Some(r) 且 use_count[r]==0 且 `is_pure(f)` 的指令 → 删除并减去其 use_regs → 迭代。
/// CALL 隐式 def reg0 不参与死删（CALL 本身 is_pure=false 永不删，Pitfall 1），但 CALL
/// 的 use 计数正常聚合——喂活上游写其参数的纯指令。
fn pass_b_dead_code(f: &IRFunction, keep: &mut [bool]) {
    // 寄存器号上界：覆盖可达指令全部 def/use（手工 IR 可能超 n_registers，动态取上界）
    let mut max_reg: u32 = 0;
    for (i, k) in keep.iter().enumerate() {
        if *k {
            if let Some(d) = f.insts[i].def_reg() {
                max_reg = max_reg.max(d);
            }
            for u in f.insts[i].use_regs() {
                max_reg = max_reg.max(u);
            }
        }
    }
    let mut use_count = vec![0usize; max_reg as usize + 1];
    for (i, k) in keep.iter().enumerate() {
        if *k {
            for u in f.insts[i].use_regs() {
                use_count[u as usize] += 1;
            }
        }
    }
    // 迭代删除至不动点：删一条纯指令 → 减其 use_regs → 上游写者可能连锁变死（D-02）
    loop {
        let mut deleted = false;
        for (i, k) in keep.iter_mut().enumerate() {
            if !*k {
                continue;
            }
            let inst = &f.insts[i];
            // CALL 系隐式 def reg0 是注册写入不参与死删收集；is_pure=false 已保证永不删（Pitfall 1）
            if let Some(r) = inst.def_reg() {
                if use_count[r as usize] == 0 && inst.is_pure(f) {
                    *k = false;
                    for u in inst.use_regs() {
                        use_count[u as usize] -= 1;
                    }
                    deleted = true;
                }
            }
        }
        if !deleted {
            break;
        }
    }
}

/// Pass C：mark-sweep 一次性重建 insts + label_pos 重映射（D-13，不用就地逐删平移——
/// 多删点叠加偏移易错）。sweep：keep 序 compact 重建；label_pos 逐 id 重写，
/// 目标被删的 label 置 None 保留 id 槽位（lowering 只读 label_pos 不读 label_count）。
fn pass_c_sweep(f: &mut IRFunction, keep: &[bool]) {
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
    // label_pos 逐 id 重写：get + usize::MAX 过滤做越界守卫（T-04-03，不信任 label_pos 长度）
    for pos in f.label_pos.iter_mut() {
        if let Some(old) = *pos {
            *pos = old_to_new.get(old).filter(|n| **n != usize::MAX).copied();
        }
    }
    // 防御性校验（T-04-03）：存活 label 新下标不越界（空尾块映射 new insts.len() 合法透传）
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

    /// 线性死链：LOAD_CONST→ADD→结果丢弃，全删（剩 RETURN）。
    #[test]
    fn linear_dead_chain_fully_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0: r0 = const
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(1), Operand::Reg(0), Operand::Reg(0))); // 1: r1 = r0 + r0
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None)); // 2: return r2

        dce(&mut f);
        assert_eq!(f.insts.len(), 1);
        assert_eq!(f.insts[0].op, OpCode::RETURN);
    }

    /// 有副作用负例：CALL / STORE_VAR(b=Imm(1)) / GET_PROP / SET_PROP / INC_POST 永不删。
    #[test]
    fn impure_ops_never_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0
        f.insts.push(Inst::call(Operand::Reg(0), Operand::Reg(0), Operand::Reg(0), 0)); // 1: CALL
        f.insts.push(Inst::load_const(Operand::Reg(1), 1)); // 2
        f.insts.push(Inst::load_const(Operand::Reg(2), 2)); // 3
        f.insts.push(Inst::new(OpCode::STORE_VAR, Operand::Reg(1), Operand::Reg(2), Operand::Imm(1))); // 4
        f.insts.push(Inst::new(OpCode::GET_PROP, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2))); // 5
        f.insts.push(Inst::new(OpCode::SET_PROP, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2))); // 6
        f.insts.push(Inst::new(OpCode::INC_POST, Operand::Reg(4), Operand::Reg(5), Operand::None)); // 7

        dce(&mut f);
        assert_eq!(f.insts.len(), 8, "有副作用指令一律保留");
    }

    /// STORE_VAR b=None 纯拷贝死删（A2）；b=Imm(1) const guard 保留（Pitfall 5）。
    #[test]
    fn store_var_pure_copy_deleted_guard_kept() {
        // 纯拷贝死删 + 连锁：STORE_VAR 删后其源 LOAD_CONST 也死
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 1)); // 0
        f.insts.push(Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None)); // 1
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        dce(&mut f);
        assert_eq!(f.insts.len(), 1);
        assert_eq!(f.insts[0].op, OpCode::RETURN);

        // b=Imm(1) const guard：不纯保留，源寄存器保活
        let mut g = IRFunction::new();
        g.insts.push(Inst::load_const(Operand::Reg(1), 1)); // 0
        g.insts.push(Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(1))); // 1
        g.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        dce(&mut g);
        assert_eq!(g.insts.len(), 3, "const guard 的 STORE_VAR 及其源保留");
    }

    /// if/else 双分支：Jump + Fallthrough 双出边全可达，指令全保留，label 重映射正确。
    #[test]
    fn if_else_both_branches_reachable_kept() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(0, 0)); // 0: cond → L0(else)
        f.insts.push(Inst::call(Operand::Reg(2), Operand::Reg(0), Operand::Reg(3), 1)); // 1: then
        f.insts.push(Inst::jmp(1)); // 2: → L1(end)
        f.insts.push(Inst::call(Operand::Reg(4), Operand::Reg(0), Operand::Reg(3), 1)); // 3: else
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3), Some(4)];
        f.label_count = 2;

        dce(&mut f);
        assert_eq!(f.insts.len(), 5, "双分支全保留");
        assert_eq!(f.label_pos, vec![Some(3), Some(4)], "存活 label 重映射到新下标");
        assert_eq!(f.label_count, 2, "label_count 不动（D-13）");
    }

    /// return 后死块：入口块以 RETURN 结尾汇 exit；label 指向 RETURN 后紧跟的指令
    /// 使其成独立块头，该死块仅含自指 JMP（块中部不出边）→ 无外部入边不可达，
    /// 整体删除，死 label 置 None 保留 id（D-13）。
    #[test]
    fn unreachable_block_after_return_deleted_with_dead_label() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(0), Operand::None, Operand::None)); // 0
        f.insts.push(Inst::jmp(0)); // 1: 死块（L0 目标），自指 JMP
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(1)];
        f.label_count = 1;

        dce(&mut f);
        assert_eq!(f.insts.len(), 1, "死块全部删除");
        assert_eq!(f.insts[0].op, OpCode::RETURN);
        assert_eq!(f.insts[0].rd, Operand::Reg(0));
        assert_eq!(f.label_pos, vec![None], "死 label 置 None 保留 id（D-13）");
    }

    /// Exception 边保守（Pitfall 4）：TRY_BEGIN 的 Exception 边使 catch 入口强制可达，
    /// 即使没有正常边指向它；catch 入口与 TRY_BEGIN 均保留。
    #[test]
    fn exception_edge_keeps_catch_entry() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → catch 入口（label 0 → inst 3）
        f.insts.push(Inst::load_const(Operand::Reg(5), 1)); // 1
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        f.insts.push(Inst::load_const(Operand::Reg(6), 2)); // 3: catch 入口
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3)];
        f.label_count = 1;

        dce(&mut f);
        assert_eq!(f.insts.len(), 5, "catch 入口经 Exception 边保留");
        assert_eq!(f.insts[0].op, OpCode::TRY_BEGIN);
        assert_eq!(f.insts[3].op, OpCode::LOAD_CONST, "catch 入口指令保留");
        assert_eq!(f.label_pos, vec![Some(3)], "catch 入口 label 重映射正确");
    }

    /// 死闭包：CREATE_CLOSURE 结果未用 → 删除；nested 槽位不动（D-03 不递归不回收）。
    #[test]
    fn dead_create_closure_deleted_nested_untouched() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::create_closure(Operand::Reg(1), 0)); // 0: r1 = nested[0]
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 1
        f.nested.push(IRFunction::new());

        dce(&mut f);
        assert_eq!(f.insts.len(), 1);
        assert_eq!(f.insts[0].op, OpCode::RETURN);
        assert_eq!(f.nested.len(), 1, "nested 槽位不动（D-03）");
    }

    /// 连锁迭代（D-02）：写链 LOAD_CONST→ADD→LOAD_CONST 全死，逐轮删除至不动点。
    #[test]
    fn chained_dead_writes_deleted_iteratively() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0: r0（ADD 读，轮 2 删）
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(1), Operand::Reg(0), Operand::Reg(0))); // 1: r1（轮 1 删）
        f.insts.push(Inst::load_const(Operand::Reg(2), 2)); // 2: r2 无人读（轮 1 删）
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 3

        dce(&mut f);
        assert_eq!(f.insts.len(), 1);
        assert_eq!(f.insts[0].op, OpCode::RETURN);
    }

    /// 存活 label 新旧下标对应：删除前置死指令后，label 目标重映射到新下标。
    #[test]
    fn live_label_remapped_to_new_index() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(5), 1)); // 0: 存活（RETURN 保活 r5）
        f.insts.push(Inst::load_const(Operand::Reg(9), 2)); // 1: 死（r9 无人读，删）
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(2)]; // label 0 → inst 2（RETURN）
        f.label_count = 1;

        dce(&mut f);
        assert_eq!(f.insts.len(), 2, "死 LOAD_CONST 删除");
        assert_eq!(f.insts[0].op, OpCode::LOAD_CONST);
        assert_eq!(f.insts[1].op, OpCode::RETURN);
        assert_eq!(f.label_pos, vec![Some(1)], "存活 label 2 → 新下标 1");
    }

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
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(1), Operand::Reg(1))); // 1: 死
        f.insts.push(Inst::load_const(Operand::Reg(0), 1)); // 2: 存活（CALL 保活 r0）
        f.insts.push(Inst::call(Operand::Reg(0), Operand::Reg(0), Operand::Reg(0), 0)); // 3
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(0)];

        dce(&mut f);
        let once_insts = f.insts.clone();
        let once_labels = f.label_pos.clone();
        dce(&mut f);
        assert_eq!(f.insts, once_insts, "二次 dce 后 insts 不变");
        assert_eq!(f.label_pos, once_labels, "二次 dce 后 label_pos 不变");
    }

    /// 不动域断言（D-14/D-04/D-03/D-13）：constants / n_registers / nested / label_count
    /// 一律不动；死 label 置 None、存活 label 重映射、原 None 保持。
    #[test]
    fn untouched_domains_asserted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0)); // 0: 死（label 0 目标 → None）
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 1
        f.label_pos = vec![Some(0), Some(1), None];
        f.label_count = 3;
        f.n_registers = 10;
        f.constants = vec![Constant::Number(1.0)];
        f.nested.push(IRFunction::new());

        dce(&mut f);
        assert_eq!(f.insts.len(), 1);
        assert_eq!(f.insts[0].op, OpCode::RETURN);
        assert_eq!(f.label_pos, vec![None, Some(0), None], "死 label→None、存活→新下标、原 None 保持");
        assert_eq!(f.label_count, 3, "label_count 不动（D-13）");
        assert_eq!(f.n_registers, 10, "n_registers 不收缩（D-04）");
        assert_eq!(f.constants, vec![Constant::Number(1.0)], "常量池不清理（D-14）");
        assert_eq!(f.nested.len(), 1, "nested 不递归不回收（D-03）");
    }
}
