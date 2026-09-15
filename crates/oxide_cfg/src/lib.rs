//! CFG 分析 pass：IRFunction → 基本块划分 + 控制流图。
//!
//! 消费 `oxide_ir::IRFunction`（只读，不拷贝指令，仅存指令下标区间），产出一次性
//! 分析快照——pass 输出视图不住进 IR。本 crate 只管 CFG：不建通用分析框架、
//! 不递归 nested。无生命周期参数——CFG 与 `&IRFunction` 由使用方联合持有访问。
//!
//! 构建是四阶段分发：`partition`（块头）→ `split`（切块+出边）→ `exception`
//! （异常边）→ `finalize`（exit 哨兵 + preds 反推）。每阶段是独立文件的纯函数，
//! 中间产物（heads / blocks / exit_id）显式传参，无共享可变状态。

mod cfg_log;
mod exception;
mod finalize;
mod partition;
mod split;

use oxide_bytecode::opcode::OpCode;
use oxide_ir::IRFunction;

/// 基本块 id：blocks 向量下标。
pub type BBId = usize;

/// 边三分类：无条件/条件跳转、顺序落入、异常处理入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Jump,
    Fallthrough,
    Exception,
}

/// 基本块：`inst_range` 直接索引 `IRFunction.insts` 切片（不拷贝指令），
/// preds/succs 双向就位，供 liveness 数据流迭代直接消费。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub inst_range: std::ops::Range<usize>,
    pub preds: Vec<BBId>,
    pub succs: Vec<(BBId, EdgeKind)>,
}

/// 控制流图快照：实块按指令序编号，exit 哨兵（空块 `0..0`）恒在 blocks 末尾。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfg {
    pub blocks: Vec<BasicBlock>,
    pub entry: BBId,
    pub exit: BBId,
}

impl Cfg {
    /// 构造空 CFG：blocks 空、entry/exit 置 0。
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            entry: 0,
            exit: 0,
        }
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Self::new()
    }
}

/// terminator 判定：块尾指令决定出边。
///
/// 注意与 `oxide_ir::lower.rs::is_jump_op` 的差异：lowering 需要为 TRY_BEGIN /
/// TRY_FINALLY_BEGIN 回填偏移所以把它们并列进 is_jump_op；但 CFG 里它们是**块内
/// 标记**（异常边起点，见 Pass 3），不是控制流终结——抄用 is_jump_op 会导致 try 体
/// 被切块、异常边爆炸。
fn is_terminator(op: OpCode) -> bool {
    op.is_terminator()
}

/// 构建 CFG：`&IRFunction → Cfg`。纯函数，只读 IR。
///
/// 四阶段分发：`partition::partition_blocks`（Pass 1 块头）→ `split::split_and_edges`
/// （Pass 2 切块 + 出边）→ `exception::add_exception_edges`（Pass 3 异常边）
/// → `finalize::finalize`（Pass 4 exit 哨兵 + preds 反推）。幂等：同一 IR 两次构建结果相等。
pub fn build_cfg(f: &IRFunction) -> Cfg {
    // 空 IRFunction 退化形态：仅 exit 哨兵一块。
    if f.insts.is_empty() {
        return Cfg {
            blocks: vec![BasicBlock {
                inst_range: 0..0,
                preds: Vec::new(),
                succs: Vec::new(),
            }],
            entry: 0,
            exit: 0,
        };
    }

    let heads = partition::partition_blocks(f);
    let (mut blocks, exit_id) = split::split_and_edges(f, &heads);
    exception::add_exception_edges(f, &mut blocks);
    let cfg = finalize::finalize(blocks, exit_id);
    cfg_debug!("build_cfg: {} blocks", cfg.blocks.len());
    cfg
}
