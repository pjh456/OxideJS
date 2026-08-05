//! CFG 分析 pass：IRFunction → 基本块划分 + 控制流图。
//!
//! 消费 `oxide_ir::IRFunction`（只读，D-06 索引关联，不拷贝指令），产出一次性
//! 分析快照（D-01：pass 输出视图，不住进 IR）。本 crate 只管 CFG（D-02）：
//! 不建通用分析框架、不递归 nested（D-11）。无生命周期参数——CFG 只存指令
//! 下标区间，使用方同时持有 `&IRFunction` 与 `&Cfg` 联合访问。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 基本块 id：blocks 向量下标（D-03）。
pub type BBId = usize;

/// 边三分类（D-04）：无条件/条件跳转、顺序落入、异常处理入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Jump,
    Fallthrough,
    Exception,
}

/// 基本块：`inst_range` 直接索引 `IRFunction.insts` 切片（D-03，不拷贝指令），
/// preds/succs 双向就位，liveness 数据流迭代可直接用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub inst_range: std::ops::Range<usize>,
    pub preds: Vec<BBId>,
    pub succs: Vec<(BBId, EdgeKind)>,
}

/// 控制流图快照：实块按指令序编号，exit 哨兵（空块 `0..0`）恒在 blocks 末尾（D-05）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfg {
    pub blocks: Vec<BasicBlock>,
    pub entry: BBId,
    pub exit: BBId,
}

impl Cfg {
    /// 构造空 CFG：blocks 空、entry/exit 置 0。
    pub fn new() -> Self {
        Self { blocks: Vec::new(), entry: 0, exit: 0 }
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Self::new()
    }
}

/// terminator 判定（D-09）：块尾指令决定出边。
///
/// 注意与 `oxide_ir::lower.rs::is_jump_op` 的差异：lowering 需要为 TRY_BEGIN /
/// TRY_FINALLY_BEGIN 回填偏移所以把它们并列进 is_jump_op；但 CFG 里它们是**块内
/// 标记**（异常边起点，见 Pass 3），不是控制流终结——抄用 is_jump_op 会导致 try 体
/// 被切块、异常边爆炸（RESEARCH Pitfall 1）。
fn is_terminator(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::JMP
            | OpCode::JMP_IF_TRUE
            | OpCode::JMP_IF_FALSE
            | OpCode::JMP_IF_NULLISH
            | OpCode::RETURN
            | OpCode::HALT
            | OpCode::THROW
    )
}

/// 构建 CFG：`&IRFunction → Cfg`。纯函数，只读 IR（D-06 方案 A）。
///
/// 四 pass：块头识别（Pass 1）→ 线性切块 + 出边（Pass 2）→ 异常边（Pass 3）
/// → preds 反推 + exit 哨兵收尾（Pass 4）。幂等（CFG-03）。
pub fn build_cfg(f: &IRFunction) -> Cfg {
    let len = f.insts.len();

    // 空 IRFunction 退化形态：仅 exit 哨兵一块（Pitfall 5）。
    if len == 0 {
        return Cfg {
            blocks: vec![BasicBlock { inst_range: 0..0, preds: Vec::new(), succs: Vec::new() }],
            entry: 0,
            exit: 0,
        };
    }

    // ── Pass 1：块头识别 ──
    // 块头 = {0} ∪ label 位置 ∪ 跳转目标 ∪ 条件跳转 fallthrough 后继。
    // 长度 len+1 容纳指向末尾的空尾块（Pitfall 3）。
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
        // 条件跳转的 fallthrough 后继是块头（否则 cond+then 融块、条件边丢失）。
        if matches!(inst.op, OpCode::JMP_IF_TRUE | OpCode::JMP_IF_FALSE | OpCode::JMP_IF_NULLISH) {
            heads[i + 1] = true;
        }
    }

    // ── Pass 2：线性切块 + 出边 ──
    let head_positions: Vec<usize> = heads.iter().enumerate().filter(|(_, &h)| h).map(|(i, _)| i).collect();
    let mut blocks: Vec<BasicBlock> = Vec::with_capacity(head_positions.len());
    for (idx, &start) in head_positions.iter().enumerate() {
        let end = head_positions.get(idx + 1).copied().unwrap_or(len);
        blocks.push(BasicBlock { inst_range: start..end, preds: Vec::new(), succs: Vec::new() });
    }
    let exit_id = blocks.len(); // exit 哨兵块号 = 实块数

    for i in 0..blocks.len() {
        if blocks[i].inst_range.is_empty() {
            continue; // 空尾块（label 指向 len）：无指令可判出边
        }
        let last = &f.insts[blocks[i].inst_range.end - 1];
        if is_terminator(last.op) {
            match last.op {
                OpCode::JMP => {
                    if let Operand::Label(l) = last.b {
                        match f.label_pos.get(l as usize).and_then(|p| *p) {
                            Some(p) => {
                                let target = block_id_of(p, &blocks);
                                blocks[i].succs.push((target, EdgeKind::Jump));
                            }
                            None => debug_assert!(false, "unresolved label {l}"),
                        }
                    }
                }
                OpCode::JMP_IF_TRUE | OpCode::JMP_IF_FALSE | OpCode::JMP_IF_NULLISH => {
                    if let Operand::Label(l) = last.b {
                        match f.label_pos.get(l as usize).and_then(|p| *p) {
                            Some(p) => {
                                let target = block_id_of(p, &blocks);
                                blocks[i].succs.push((target, EdgeKind::Jump));
                            }
                            None => debug_assert!(false, "unresolved label {l}"),
                        }
                    }
                    // fallthrough 后继：下一块（Pass 1 置 heads[i+1] 保证存在）。
                    blocks[i].succs.push((i + 1, EdgeKind::Fallthrough));
                }
                OpCode::RETURN | OpCode::HALT => {
                    // RETURN/HALT 汇入 exit 哨兵，用 Fallthrough 表达正常流出口（A1 语义归属）。
                    blocks[i].succs.push((exit_id, EdgeKind::Fallthrough));
                }
                OpCode::THROW => { /* 无出边：异常传播，D-07 不建恢复路径 */ }
                _ => unreachable!("is_terminator 已穷尽其余分支"),
            }
        } else {
            // 非 terminator：顺序落入下一块；已是最后一块则无出边。
            if i + 1 < blocks.len() {
                blocks[i].succs.push((i + 1, EdgeKind::Fallthrough));
            }
        }
    }

    // ── Pass 3：异常边 ──
    // 块内扫 TRY_BEGIN / TRY_FINALLY_BEGIN（块内标记，非 terminator，Pitfall 1），
    // 从所在 BB 连 Exception 边到处理入口（D-07/D-08：仅起始 BB，不扩散）。
    for i in 0..blocks.len() {
        let range = blocks[i].inst_range.clone();
        for inst_idx in range.start..range.end {
            let inst = &f.insts[inst_idx];
            if !matches!(inst.op, OpCode::TRY_BEGIN | OpCode::TRY_FINALLY_BEGIN) {
                continue;
            }
            if let Operand::Label(l) = inst.b {
                match f.label_pos.get(l as usize).and_then(|p| *p) {
                    Some(p) => {
                        let target = block_id_of(p, &blocks);
                        // push 前去重（Pitfall 4：块内多个 TRY 标记可指向同一目标）。
                        if !blocks[i].succs.contains(&(target, EdgeKind::Exception)) {
                            blocks[i].succs.push((target, EdgeKind::Exception));
                        }
                    }
                    None => debug_assert!(false, "unresolved label {l}"),
                }
            }
        }
    }

    // ── Pass 4：exit 哨兵收尾 + succs 反推 preds ──
    blocks.push(BasicBlock { inst_range: 0..0, preds: Vec::new(), succs: Vec::new() });
    let mut succs_total = 0usize;
    let mut preds_total = 0usize;
    for i in 0..blocks.len() {
        succs_total += blocks[i].succs.len();
        let targets: Vec<BBId> = blocks[i].succs.iter().map(|&(t, _)| t).collect();
        for target in targets {
            blocks[target].preds.push(i);
            preds_total += 1;
        }
    }
    debug_assert_eq!(succs_total, preds_total, "succs/preds 不对称（Pitfall 4）");

    Cfg { blocks, entry: 0, exit: exit_id }
}

/// label 目标指令位置 → 块 id。blocks 按 `inst_range.start` 升序，p 必为某块块头
/// （Pass 1 已把所有 label 目标置为块头）。
fn block_id_of(p: usize, blocks: &[BasicBlock]) -> usize {
    blocks.partition_point(|b| b.inst_range.start <= p) - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::inst::Inst;

    fn succs_total(cfg: &Cfg) -> usize {
        cfg.blocks.iter().map(|b| b.succs.len()).sum()
    }

    fn preds_total(cfg: &Cfg) -> usize {
        cfg.blocks.iter().map(|b| b.preds.len()).sum()
    }

    /// 无跳转线性函数：1 实块 + exit 哨兵，块 0 Fallthrough→exit，preds/succs 对称。
    #[test]
    fn linear_function_is_single_block_plus_exit() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));

        let cfg = build_cfg(&f);
        assert_eq!(cfg.blocks.len(), 2); // 1 实块 + exit 哨兵
        assert_eq!(cfg.entry, 0);
        assert_eq!(cfg.exit, 1);
        assert_eq!(cfg.blocks[0].inst_range, 0..3);
        assert_eq!(cfg.blocks[0].succs, vec![(1, EdgeKind::Fallthrough)]);
        assert!(cfg.blocks[1].succs.is_empty()); // exit 哨兵无出边
        assert_eq!(cfg.blocks[1].preds, vec![0]);
        assert_eq!(succs_total(&cfg), preds_total(&cfg));
    }

    /// if/else 典型形状：条件分裂块（Jump + Fallthrough 双出边），4 实块 + exit 哨兵。
    #[test]
    fn if_else_conditional_jump_splits_block_with_dual_edges() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(1, 0)); // 0: cond → L0(else)
        f.insts.push(Inst::call(Operand::Reg(2), Operand::Reg(0), Operand::Reg(3), 1)); // 1: then a()
        f.insts.push(Inst::jmp(1)); // 2: → L1(end)
        f.insts.push(Inst::call(Operand::Reg(4), Operand::Reg(0), Operand::Reg(3), 1)); // 3: else b()
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3), Some(4)]; // L0→else 块头(3), L1→end(4)
        f.label_count = 2;

        let cfg = build_cfg(&f);
        assert_eq!(cfg.blocks.len(), 5); // 4 实块 + exit 哨兵
        assert_eq!(cfg.entry, 0);
        assert_eq!(cfg.exit, 4);

        // 块 0（cond）：条件分裂，Jump→else + Fallthrough→then 两条出边都在
        assert_eq!(cfg.blocks[0].inst_range, 0..1);
        assert_eq!(cfg.blocks[0].succs, vec![(2, EdgeKind::Jump), (1, EdgeKind::Fallthrough)]);
        assert!(cfg.blocks[0].preds.is_empty());
        // 块 1（then）：Jump→end
        assert_eq!(cfg.blocks[1].inst_range, 1..3);
        assert_eq!(cfg.blocks[1].succs, vec![(3, EdgeKind::Jump)]);
        assert_eq!(cfg.blocks[1].preds, vec![0]);
        // 块 2（else）：Fallthrough→end
        assert_eq!(cfg.blocks[2].inst_range, 3..4);
        assert_eq!(cfg.blocks[2].succs, vec![(3, EdgeKind::Fallthrough)]);
        assert_eq!(cfg.blocks[2].preds, vec![0]);
        // 块 3（end）：RETURN → Fallthrough→exit
        assert_eq!(cfg.blocks[3].inst_range, 4..5);
        assert_eq!(cfg.blocks[3].succs, vec![(4, EdgeKind::Fallthrough)]);
        assert_eq!(cfg.blocks[3].preds, vec![1, 2]);
        // 块 4（exit 哨兵）：空块 0..0，preds 含块 3，无出边
        assert_eq!(cfg.blocks[4].inst_range, 0..0);
        assert!(cfg.blocks[4].succs.is_empty());
        assert_eq!(cfg.blocks[4].preds, vec![3]);
        assert_eq!(succs_total(&cfg), preds_total(&cfg));
    }

    /// try/catch：TRY_BEGIN 是块内标记不切块（Pitfall 1），所在 BB 出 Exception 边到 catch 入口。
    #[test]
    fn try_begin_does_not_split_block_and_emits_exception_edge() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN（块内标记）
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: catch 入口
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3)];
        f.label_count = 1;

        let cfg = build_cfg(&f);
        assert_eq!(cfg.blocks.len(), 3); // 2 实块 + exit 哨兵（TRY_BEGIN 不切块）
        assert_eq!(cfg.blocks[0].inst_range, 0..3); // try 体未被切块
        // Exception 边从 TRY_BEGIN 所在 BB 出发指向 catch 入口块（D-07/D-08）
        assert!(cfg.blocks[0].succs.contains(&(1, EdgeKind::Exception)));
        assert_eq!(cfg.blocks[1].succs, vec![(2, EdgeKind::Fallthrough)]);
        assert_eq!(cfg.blocks[2].preds, vec![0, 1]);
        assert_eq!(succs_total(&cfg), preds_total(&cfg));
    }

    /// try/finally：TRY_FINALLY_BEGIN 同上，Exception 边到 finally 入口块。
    #[test]
    fn try_finally_begin_emits_exception_edge() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_finally_begin(0)); // 0
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: finally 入口
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 4
        f.label_pos = vec![Some(3)];
        f.label_count = 1;

        let cfg = build_cfg(&f);
        assert_eq!(cfg.blocks.len(), 3);
        assert!(cfg.blocks[0].succs.contains(&(1, EdgeKind::Exception)));
        assert_eq!(cfg.blocks[2].preds, vec![0, 1]);
        assert_eq!(succs_total(&cfg), preds_total(&cfg));
    }

    /// 尾部 RETURN：该块 Fallthrough 到 exit。
    #[test]
    fn trailing_return_flows_to_exit() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));

        let cfg = build_cfg(&f);
        assert_eq!(cfg.blocks.len(), 2);
        assert_eq!(cfg.blocks[0].succs, vec![(1, EdgeKind::Fallthrough)]);
        assert_eq!(cfg.blocks[1].preds, vec![0]);
    }

    /// 空 IRFunction：退化 CFG，entry == exit == 0，blocks 长度 1（Pitfall 5）。
    #[test]
    fn empty_function_yields_degenerate_cfg() {
        let f = IRFunction::new();
        let cfg = build_cfg(&f);
        assert_eq!(cfg.blocks.len(), 1);
        assert_eq!(cfg.entry, 0);
        assert_eq!(cfg.exit, 0);
        assert_eq!(cfg.blocks[0].inst_range, 0..0);
        assert!(cfg.blocks[0].succs.is_empty());
        assert!(cfg.blocks[0].preds.is_empty());
    }

    /// 幂等性（CFG-03）：同一 IR 两次构建结果相等。
    #[test]
    fn build_cfg_is_idempotent() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(1, 0)); // 0: → L0（回环目标）
        f.insts.push(Inst::jmp(0)); // 1: → L0
        f.insts.push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(2), Some(0)]; // L0→2（尾部）, L1→0（回环）
        f.label_count = 2;

        let cfg1 = build_cfg(&f);
        let cfg2 = build_cfg(&f);
        assert_eq!(cfg1, cfg2);
    }
}
