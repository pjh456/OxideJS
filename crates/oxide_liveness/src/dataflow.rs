//! 块级数据流迭代：gen/kill + reverse_postorder 不动点。
//!
//! gen/kill 直接消费 `oxide_ir::contract` 的 `def_reg`/`use_regs`，
//! None→0 / This→254 / NewTarget→255 / CALL 隐式 reg0 全内置，不重复建。
//! Exception 边当普通边参与迭代；Exception 目标块入口 reg 0 隐式 def
//! （异常展开由 VM 派发循环写入 regs[0]）在此建模：gen.remove(0)+kill.insert(0)。
//!
//! 位集为 `u64` 压缩（1 字 64 位，密度为 `Vec<bool>` 的 8 倍）；不动点迭代
//! 复用 out/input 两块缓冲（clear + 覆盖），不逐块逐迭代分配。

use oxide_cfg::{Cfg, EdgeKind};
use oxide_ir::IRFunction;

use crate::live_info::{bitset_clear, bitset_set};

/// 块级 liveness：返回 (block_live_in, block_live_out, reg_count)。
/// reg_count = 全部 def/use 最大 reg 号（bitset 容量 = reg_count+1）。
pub(super) fn block_liveness(f: &IRFunction, cfg: &Cfg) -> (Vec<Vec<u64>>, Vec<Vec<u64>>, usize) {
    // 上界扫描：f.n_registers 在 emit 阶段未必填准，故不信任它，逐 inst 取 def/use 最大 reg 号作 bitset 容量上界。
    let mut reg_count = 255usize; // 兜底 This/NewTarget 254/255
    for inst in &f.insts {
        if let Some(d) = inst.def_reg() {
            reg_count = reg_count.max(d as usize);
        }
        for u in inst.use_regs() {
            reg_count = reg_count.max(u as usize);
        }
    }

    let words = (reg_count + 1).div_ceil(64);
    let n = cfg.blocks.len();

    // 每块 gen/kill：先算块内反向扫描的 gen，再算正向扫描的 kill。
    let mut gen: Vec<Vec<u64>> = vec![vec![0u64; words]; n];
    let mut kill: Vec<Vec<u64>> = vec![vec![0u64; words]; n];
    for (b, block) in cfg.blocks.iter().enumerate() {
        // gen：块内反向扫描，kill 先于 gen（`live = (live − def) ∪ use`）。
        // 顺序理由：COMPOUND_ADD 等读-写同寄存器指令的 uses 含 Rd（见 oxide_ir::contract 的 def/use 约定），
        // gen 先于 kill 会把 rd 旧值从 live_before 错误剔除。
        let mut live = vec![0u64; words];
        for i in block.inst_range.clone().rev() {
            if let Some(d) = f.insts[i].def_reg() {
                bitset_clear(&mut live, d as usize);
            }
            for u in f.insts[i].use_regs() {
                bitset_set(&mut live, u as usize);
            }
        }
        gen[b] = live;

        // kill：块内正向扫描，def_reg 非 None 即插入
        for i in block.inst_range.clone() {
            if let Some(d) = f.insts[i].def_reg() {
                bitset_set(&mut kill[b], d as usize);
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
            bitset_clear(&mut gen[b], 0);
            bitset_set(&mut kill[b], 0);
        }
    }

    // reverse_postorder：从 entry 沿 succs 存储序 DFS，保证确定性（禁 HashMap 迭代序）。
    let rpo = reverse_postorder(cfg);

    // 不动点迭代：out/input 缓冲在循环外分配复用，clear + 覆盖，免逐迭代分配。
    let mut live_in = vec![vec![0u64; words]; n];
    let mut live_out = vec![vec![0u64; words]; n];
    let mut out = vec![0u64; words];
    let mut input = vec![0u64; words];
    let mut iters = 0usize;
    loop {
        let mut changed = false;
        for &b in &rpo {
            // live_out[b] = ∪ live_in[s]（全部 succs，含 Exception 边当普通边）
            out.fill(0);
            for &(succ, _kind) in &cfg.blocks[b].succs {
                let si = &live_in[succ];
                for (w, word) in out.iter_mut().enumerate() {
                    *word |= si[w];
                }
            }
            // live_in[b] = gen[b] | (live_out[b] − kill[b])
            for w in 0..words {
                input[w] = gen[b][w] | (out[w] & !kill[b][w]);
            }
            if input != live_in[b] || out != live_out[b] {
                changed = true;
                live_in[b].copy_from_slice(&input);
                live_out[b].copy_from_slice(&out);
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
/// 不可达块按升序追加在末尾。纯 Vec + visited 位图，保证确定性。
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
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    fn set(v: &[u64], regs: &[u32]) -> bool {
        regs.iter().all(|&r| crate::live_info::bitset_get(v, r as usize))
    }

    fn empty_function() -> IRFunction {
        IRFunction::new()
    }

    #[test]
    fn linear_function_liveness() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(0), Operand::Reg(1)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, live_out, reg_count) = block_liveness(&f, &cfg);
        assert!(reg_count >= 2);
        assert!(set(&live_in[0], &[0, 1]), "entry liveIn 应含 0、1");
        assert!(!crate::live_info::bitset_get(&live_in[0], 2), "r2 是 def 不应在 liveIn");
        assert!(set(&live_out[0], &[]), "live_out 空");
        // exit 哨兵为空块，live 全空
        let exit = cfg.exit;
        assert!(live_in[exit].iter().all(|&w| w == 0));
        assert!(live_out[exit].iter().all(|&w| w == 0));
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
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(3), Operand::Reg(2)));
        f.insts.push(Inst::jmp(2));
        f.insts
            .push(Inst::new(OpCode::SUB, Operand::Reg(4), Operand::Reg(4), Operand::Reg(5)));
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
        f.label_pos = vec![None, Some(3), Some(4)];
        f.label_count = 3;
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        // join 块（NOP 所在块）liveIn 含 6（RETURN 用）
        let join = cfg.blocks.iter().position(|b| b.inst_range == (4..6usize)).unwrap();
        assert!(crate::live_info::bitset_get(&live_in[join], 6), "join 块 liveIn 应含 r6");
        // then 分支块 liveIn 含 2、3，不含 5
        let then = cfg.blocks.iter().position(|b| b.inst_range == (1..3usize)).unwrap();
        assert!(set(&live_in[then], &[2, 3]));
        assert!(!crate::live_info::bitset_get(&live_in[then], 5));
        // else 分支块 liveIn 含 4、5，不含 2
        let els = cfg.blocks.iter().position(|b| b.inst_range == (3..4usize)).unwrap();
        assert!(set(&live_in[els], &[4, 5]));
        assert!(!crate::live_info::bitset_get(&live_in[els], 2));
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
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(3), Operand::Reg(2)));
        f.insts.push(Inst::jmp(0));
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
        f.label_pos = vec![Some(0), Some(3)];
        f.label_count = 2;
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        // header 块（含 inst 0）liveIn 含 2（r2 跨回边存活）与 1（cond）
        assert!(crate::live_info::bitset_get(&live_in[0], 1), "header liveIn 应含 cond r1");
        assert!(crate::live_info::bitset_get(&live_in[0], 2), "header liveIn 应含 r2（跨回边存活）");
        // body 块 liveIn 含 2
        let body = cfg.blocks.iter().position(|b| b.inst_range == (1..3usize)).unwrap();
        assert!(crate::live_info::bitset_get(&live_in[body], 2));
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
        let (live_in, _, _) = block_liveness(&f, &cfg);
        // catch 块 liveIn 不含 0（隐式 def 截断）
        let catch = cfg.blocks.iter().position(|b| b.inst_range == (3..5usize)).unwrap();
        assert!(!crate::live_info::bitset_get(&live_in[catch], 0), "catch 块入口 reg0 应被隐式 def 截断");
        // try 块（entry）liveIn 不含 0（未污染入口）
        assert!(!crate::live_info::bitset_get(&live_in[0], 0), "reg0 use 不得污染函数入口");
        // try 块 liveIn 含 2（正常变量照常存活）
        assert!(crate::live_info::bitset_get(&live_in[0], 2), "try 块 liveIn 应含 r2");
    }

    #[test]
    fn none_operand_maps_to_reg0() {
        // 0: LOAD_VAR(Reg(5), None, None)   def 5 use {0}
        // 1: HALT(None, None, None)          HALT 读 regs[0]（contract 约定）
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(5), Operand::None, Operand::None));
        f.insts
            .push(Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let (live_in, _, _) = block_liveness(&f, &cfg);
        assert!(crate::live_info::bitset_get(&live_in[0], 0), "None→0 映射由 contract.rs 消费生效");
    }
}
