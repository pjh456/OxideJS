//! `CompileCtx` 的成组子上下文。
//!
//! 把编译器中心 `CompileCtx` 拆开，语法域 worker 只借用所需切片：
//! `LabelCtx` 负责跳转目标解析，`ScopeCtx` 负责标识符绑定。
//! `PatternCtx` 为解构工作预留。执行流字段（insts/registers/pc 等）仍
//! 平铺在 `CompileCtx` 上。

use crate::symbol_table::SymbolTable;
use oxide_ir::operand::LabelId;
use oxide_parser::MethodDefinitionKind;

/// 循环语句类别：决定逃出计数是否计入 for-of / for-in 迭代器关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopKind {
    /// while / do-while / C 风格 for：无迭代器关闭语义。
    Plain,
    ForOf,
    ForAwaitOf,
    ForIn,
}

impl LoopKind {
    /// 是否计入 for-of 逃出关闭计数。for-await-of 的逃出（labeled break /
    /// continue / return）需要异步 await 迭代器 return() 的 promise，走
    /// 异步挂起机制另行实现——此处只计同步 for-of，避免运行时同步调用异步
    /// 迭代器的 return()。
    pub(crate) fn is_for_of(self) -> bool {
        matches!(self, LoopKind::ForOf)
    }

    pub(crate) fn is_for_in(self) -> bool {
        matches!(self, LoopKind::ForIn)
    }
}

/// 循环打开时的词法快照：break/continue/return 逃出时据此计算需关闭的迭代器层数。
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopEntry {
    pub(crate) break_label: LabelId,
    pub(crate) continue_label: LabelId,
    /// 循环打开时嵌套的 finally 域数（finally 逃出计数用）。
    pub(crate) finally_depth_at_open: usize,
    /// 循环打开时已打开的 for-of/for-await-of 循环数。
    pub(crate) for_of_depth_at_open: usize,
    /// 循环打开时已打开的 for-in 循环数。
    pub(crate) for_in_depth_at_open: usize,
    pub(crate) kind: LoopKind,
    /// 循环出口结果寄存器：入口初始 undefined，体正常完成回写，break/continue
    /// 携值写入（空携值不写，保持前次累积值）。
    pub(crate) v_reg: u32,
}

/// 完成值帧：编译期帧栈元素，承载「每语句列表独立累积」语义。
/// `List` 的 `last` 记源序最后非空语句的寄存器（空语句不覆写）；`Boundary`
/// 是 `UpdateEmpty(_, undefined)` 站点（if 支臂 / with 体），携值解析撞它物化
/// undefined；`Target` 是循环 / switch / 非迭代标签的出口结果寄存器。
#[derive(Debug, Clone, Copy)]
pub(crate) enum CompletionFrame {
    List { last: Option<u32> },
    Boundary,
    Target { v_reg: u32 },
}

/// break/continue 的携值：`Value(r)` 拷贝寄存器 `r`，`Undefined` 物化 undefined，
/// `Empty` 不写（出口目标寄存器保持既有累积值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionCarry {
    Value(u32),
    Undefined,
    Empty,
}

/// 标签语句作用域：编译期内登记 `break label` / `continue label` 的跳转目标。
/// `continue_label` 仅在标签直接包裹迭代语句时存在。
#[derive(Debug, Clone)]
pub struct LabelScope {
    pub(crate) name: String,
    pub(crate) break_label: LabelId,
    pub(crate) continue_label: Option<LabelId>,
    /// 标签打开时嵌套的 finally 域数：break/continue 跨越 finally 的计数依据。
    pub(crate) finally_depth_at_open: usize,
    /// 标签打开时已打开的 for-of 循环数：break/continue 逃出计数依据。
    pub(crate) for_of_depth_at_open: usize,
    /// 标签打开时已打开的 for-in 循环数。
    pub(crate) for_in_depth_at_open: usize,
    /// 标签目标出口结果寄存器：迭代标签取所包裹循环的 `v_reg`；非迭代标签仅当
    /// 体含指向本标签的 break 时分配（体正常完成回写、break 携值写入）。
    pub(crate) completion_reg: Option<u32>,
}

/// switch 打开时的词法快照：switch 内 break 的逃出计数以打开点为准，switch 之前
/// 已打开的循环不属于本次逃出；另记 CaseBlock 出口结果寄存器（`resultValue` 累积槽）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwitchEntry {
    pub(crate) break_label: LabelId,
    pub(crate) finally_depth_at_open: usize,
    pub(crate) for_of_depth_at_open: usize,
    pub(crate) for_in_depth_at_open: usize,
    pub(crate) result_reg: u32,
}

/// 跳转目标 / 标签语句解析状态。
pub(crate) struct LabelCtx {
    /// label id → 指令下标。id 连续递增，Vec 索引即 id；写入前须扩容。
    pub(crate) label_pos: Vec<Option<usize>>,
    /// 每个条目记录循环打开时的词法快照（finally/for-of/for-in 深度）。
    pub(crate) loop_stack: Vec<LoopEntry>,
    /// 每个条目记录 switch 打开时的词法快照与 CaseBlock 出口结果寄存器：
    /// switch 内 break 的逃出计数以打开点为准，switch 之前已打开的迭代器
    /// 不属于本次逃出。
    pub(crate) switch_stack: Vec<SwitchEntry>,
    /// 活动标签语句作用域（解析 `break label` / `continue label`）。
    pub(crate) label_scopes: Vec<LabelScope>,
    /// 等待绑定到下一个循环 continue 目标的标签名。
    pub(crate) pending_loop_labels: Vec<String>,
    /// 当前打开（正在 emit）的 try/finally 域数。
    pub(crate) finally_depth: usize,
    /// 当前打开的 for-of/for-await-of 循环数（逃出计数基数）。
    pub(crate) for_of_depth: usize,
    /// 当前打开的 for-in 循环数（逃出计数基数）。
    pub(crate) for_in_depth: usize,
    pub(crate) label_counter: u32,
}

impl LabelCtx {
    /// 按 id 扩容后写入 label 定义位置。
    pub(crate) fn set_label_pos(&mut self, id: LabelId, pos: usize) {
        if id as usize >= self.label_pos.len() {
            self.label_pos.resize(id as usize + 1, None);
        }
        self.label_pos[id as usize] = Some(pos);
    }
}

/// 标识符绑定状态：符号表、builtin 寄存器、私有名。
pub(crate) struct ScopeCtx {
    pub(crate) symbols: SymbolTable,
    pub(crate) builtin_reg_map: Vec<(String, u32)>,
    pub(crate) private_name_map: Vec<(String, u32)>,
    /// 私有元素类型（name, kind，static 标志）。kind=None 表示字段；
    /// instance 字段的私有访问走 PrivateFieldFind 原型链查找，不加 brand 检查。
    pub(crate) private_element_kinds: Vec<(String, Option<MethodDefinitionKind>, bool)>,
    /// 当前类的私有 brand 私有名 id：私有方法/访问器访问时对实例做 brand 检查。
    pub(crate) private_brand_id: Option<u32>,
    pub(crate) next_private_name_id: u32,
}

/// 预留的解构 pattern 状态。当前为空；为后续解构工作预留归属地，避免改动 `ScopeCtx`。
#[allow(dead_code)]
pub(crate) struct PatternCtx;
