//! 块级数据流迭代：gen/kill + reverse_postorder 不动点。
//!
//! gen/kill 直接消费 `oxide_ir::contract` 的 `def_reg`/`use_regs`（D-22），
//! None→0 / This→254 / NewTarget→255 / CALL 隐式 reg0 全内置，不重复建。
//! Exception 边当普通边参与迭代（D-13）；Exception 目标块入口 reg 0 隐式 def
//! （异常展开写 regs[0]，vm_runtime.rs:204-205）在此建模：gen.remove(0)+kill.insert(0)。

use oxide_cfg::{Cfg, EdgeKind};
use oxide_ir::IRFunction;

/// 块级 liveness：返回 (block_live_in, block_live_out, reg_count)。
/// reg_count = 全部 def/use 最大 reg 号（bitset 长度 = reg_count+1）。
pub(super) fn block_liveness(f: &IRFunction, cfg: &Cfg) -> (Vec<Vec<bool>>, Vec<Vec<bool>>, usize) {
    // 1. reg_count 上界扫描（不信任 f.n_registers，动态求上界，照 iter_sweep 先例）
    let mut reg_count = 255usize; // 兜底 This/NewTarget 254/255
    for inst in &f.insts {
        if let Some(d) = inst.def_reg() {
            reg_count = reg_count.max(d as usize);
        }
        for u in inst.use_regs() {
            reg_count = reg_count.max(u as usize);
        }
    }

    let bits = reg_count + 1;
    let n = cfg.blocks.len();

    // 2. 每块 gen/kill
    let mut gen: Vec<Vec<bool>> = vec![vec![false; bits]; n];
    let mut kill: Vec<Vec<bool>> = vec![vec![false; bits]; n];
    for (b, block) in cfg.blocks.iter().enumerate() {
        // gen：块内反向扫描，kill 先于 gen（`live = (live − def) ∪ use`）。
        // 顺序理由：COMPOUND_ADD 等读-写同寄存器指令 use 含 rd（contract.rs:137），
        // gen 先于 kill 会把 rd 旧值从 live_before 错误剔除。
        let mut live = vec![false; bits];
        for i in block.inst_range.clone().rev() {
            if let Some(d) = f.insts[i].def_reg() {
                live[d as usize] = false;
            }
            for u in f.insts[i].use_regs() {
                live[u as usize] = true;
            }
        }
        gen[b] = live;

        // kill：块内正向扫描，def_reg 非 None 即插入
        for i in block.inst_range.clone() {
            if let Some(d) = f.insts[i].def_reg() {
                kill[b][d as usize] = true;
            }
        }

        // Exception 目标块：入口 reg 0 隐式 def（异常展开在块外写 regs[0]）。
        // gen 去 0 防"无 def use"把活度污染到函数入口；kill 插 0 防 0 从任何 pred 流入。
        // 注：preds 是 Vec<BBId>（无 EdgeKind），须从源块 succs 侧检测 Exception 边。
        let is_exception_target = cfg
            .blocks
            .iter()
            .any(|src| src.succs.iter().any(|&(dst, k)| dst == b && k == EdgeKind::Exception));
        if is_exception_target {
            gen[b][0] = false;
            kill[b][0] = true;
        }
    }

    // 3. reverse_postorder（确定性，禁 HashMap 迭代序——B010）
    let rpo = reverse_postorder(cfg);

    // 4. 不动点迭代
    let mut live_in = vec![vec![false; bits]; n];
    let mut live_out = vec![vec![false; bits]; n];
    let mut iters = 0usize;
    loop {
        let mut changed = false;
        for &b in &rpo {
            // live_out[b] = ∪ live_in[s]（全部 succs，含 Exception 边——D-13 当普通边）
            let mut out = vec![false; bits];
            for &(succ, _kind) in &cfg.blocks[b].succs {
                for (r, &v) in live_in[succ].iter().enumerate() {
                    if v {
                        out[r] = true;
                    }
                }
            }
            // live_in[b] = gen[b] | (live_out[b] − kill[b])
            let mut input = vec![false; bits];
            for r in 0..bits {
                input[r] = gen[b][r] || (out[r] && !kill[b][r]);
            }
            if input != live_in[b] || out != live_out[b] {
                changed = true;
                live_in[b] = input;
                live_out[b] = out;
            }
        }
        if !changed {
            break;
        }
        iters += 1;
        debug_assert!(iters < cfg.blocks.len() * 2 + 16, "liveness 不动点疑似不收敛");
    }

    (live_in, live_out, reg_count)
}

/// 从 entry 出发 DFS 沿 succs 存储序（确定序）收集 postorder 后反转；
/// 不可达块按升序追加在末尾。纯 Vec + visited 位图（B010）。
fn reverse_postorder(cfg: &Cfg) -> Vec<usize> {
    let n = cfg.blocks.len();
    let mut visited = vec![false; n];
    let mut postorder = Vec::with_capacity(n);
    fn dfs(cfg: &Cfg, visited: &mut [bool], postorder: &mut Vec<usize>, b: usize) {
        visited[b] = true;
        for &(succ, _) in &cfg.blocks[b].succs {
            if !visited[succ] {
                dfs(cfg, visited, postorder, succ);
            }
        }
        postorder.push(b);
    }
    dfs(cfg, &mut visited, &mut postorder, cfg.entry);
    postorder.reverse();
    for (b, &vis) in visited.iter().enumerate() {
        if !vis {
            postorder.push(b);
        }
    }
    postorder
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;
    use oxide_bytecode::opcode::OpCode;

    fn set(v: &[bool], regs: &[u32]) -> bool {
        regs.iter().all(|&r| v[r as usize])
    }

    fn empty_function() -> IRFunction {
        IRFunction::new()
    }

    #[test]
    fn linear_function_liveness() {
        let mut f = empty_function();
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(0), Operand::Reg(1)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, live_out, reg_count) = block_liveness(&f, &cfg);
        assert!(reg_count >= 2);
        assert!(set(&live_in[0], &[0, 1]), "entry liveIn 应含 0、1");
        assert!(!live_in[0][2], "r2 是 def 不应在 liveIn");
        assert!(set(&live_out[0], &[]), "live_out 空");
        // exit 哨兵为空块，live 全空
        let exit = cfg.exit;
        assert!(live_in[exit].iter().all(|&v| !v));
        assert!(live_out[exit].iter().all(|&v| !v));
    }

    #[test]
    fn if_else_join_unions_live_sets() {
        // 0: jmp_if_false(1, L1)   cond=Reg(1)
        // 1: ADD r3=r3+r2           def 3 use {3,2}
        // 2: jmp L2
        // 3: SUB r4=r4+r5           def 4 use {4,5}
        // 4: NOP
        // 5: RETURN Reg(6)
        let mut f = empty_function();
        f.insts.push(Inst::jmp_if_false(1, 1));
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(3), Operand::Reg(2)));
        f.insts.push(Inst::jmp(2));
        f.insts.push(Inst::new(OpCode::SUB, Operand::Reg(4), Operand::Reg(4), Operand::Reg(5)));
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
        f.label_pos = vec![None, Some(3), Some(4)];
        f.label_count = 3;
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        // join 块（NOP 所在块）liveIn 含 6（RETURN 用）
        let join = cfg.blocks.iter().position(|b| b.inst_range == (4..6usize)).unwrap();
        assert!(live_in[join][6], "join 块 liveIn 应含 r6");
        // then 分支块 liveIn 含 2、3，不含 5
        let then = cfg.blocks.iter().position(|b| b.inst_range == (1..3usize)).unwrap();
        assert!(set(&live_in[then], &[2, 3]));
        assert!(!live_in[then][5]);
        // else 分支块 liveIn 含 4、5，不含 2
        let els = cfg.blocks.iter().position(|b| b.inst_range == (3..4usize)).unwrap();
        assert!(set(&live_in[els], &[4, 5]));
        assert!(!live_in[els][2]);
    }

    #[test]
    fn loop_back_edge_keeps_value_alive() {
        // 0: jmp_if_false(1, L1)
        // 1: ADD r3=r3+r2
        // 2: jmp L0
        // 3: NOP
        // 4: RETURN Reg(6)
        let mut f = empty_function();
        f.insts.push(Inst::jmp_if_false(1, 1));
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(3), Operand::Reg(2)));
        f.insts.push(Inst::jmp(0));
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
        f.label_pos = vec![Some(0), Some(3)];
        f.label_count = 2;
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        // header 块（含 inst 0）liveIn 含 2（r2 跨回边存活）与 1（cond）
        assert!(live_in[0][1], "header liveIn 应含 cond r1");
        assert!(live_in[0][2], "header liveIn 应含 r2（跨回边存活）");
        // body 块 liveIn 含 2
        let body = cfg.blocks.iter().position(|b| b.inst_range == (1..3usize)).unwrap();
        assert!(live_in[body][2]);
        // 测试通过本身即不动点收敛证明
    }

    #[test]
    fn try_catch_reg0_not_polluting_entry() {
        // 0: try_begin(L_catch)
        // 1: ADD r3=r3+r2
        // 2: RETURN Reg(3)
        // 3: STORE_VAR(Reg(5), None, None)   catch 入口：a=None→0 读 reg 0
        // 4: RETURN Reg(5)
        let mut f = empty_function();
        f.insts.push(Inst::try_begin(3));
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(3), Operand::Reg(2)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::STORE_VAR, Operand::Reg(5), Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        f.label_pos = vec![None, None, None, Some(3)];
        f.label_count = 1;
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        // catch 块 liveIn 不含 0（隐式 def 截断）
        let catch = cfg.blocks.iter().position(|b| b.inst_range == (3..5usize)).unwrap();
        assert!(!live_in[catch][0], "catch 块入口 reg0 应被隐式 def 截断");
        // try 块（entry）liveIn 不含 0（未污染入口）
        assert!(!live_in[0][0], "reg0 use 不得污染函数入口");
        // try 块 liveIn 含 2（正常变量照常存活）
        assert!(live_in[0][2], "try 块 liveIn 应含 r2");
    }

    #[test]
    fn none_operand_maps_to_reg0() {
        // 0: LOAD_VAR(Reg(5), None, None)   def 5 use {0}
        // 1: HALT(None, None, None)          contract.rs HALT 读 regs[0]
        let mut f = empty_function();
        f.insts.push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(5), Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        assert!(live_in[0][0], "None→0 映射由 contract.rs 消费生效");
    }
}
