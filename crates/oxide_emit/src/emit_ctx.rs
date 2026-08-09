//! `CompileCtx` 的成组子上下文。
//!
//! 把编译器中心 `CompileCtx` 拆开，语法域 worker 只借用所需切片：
//! `LabelCtx` 负责跳转目标解析，`ScopeCtx` 负责标识符绑定。
//! `PatternCtx` 为解构工作预留。执行流字段（insts/registers/pc 等）仍
//! 平铺在 `CompileCtx` 上。

use crate::symbol_table::SymbolTable;
use crate::LabelScope;
use oxide_ir::operand::LabelId;
use oxide_parser::MethodDefinitionKind;

/// 跳转目标 / 标签语句解析状态。
pub(crate) struct LabelCtx {
    /// label id → 指令下标。id 连续递增，Vec 索引即 id；写入前须扩容。
    pub(crate) label_pos: Vec<Option<usize>>,
    /// 每个条目记录循环打开时嵌套的 finally 域数（break/continue 跨越 finally 计数用）。
    pub(crate) loop_stack: Vec<(LabelId, LabelId, usize)>,
    /// 每个条目记录 switch 打开时嵌套的 finally 域数。
    pub(crate) switch_stack: Vec<(LabelId, usize)>,
    /// 活动标签语句作用域（解析 `break label` / `continue label`）。
    pub(crate) label_scopes: Vec<LabelScope>,
    /// 等待绑定到下一个循环 continue 目标的标签名。
    pub(crate) pending_loop_labels: Vec<String>,
    /// 当前打开（正在 emit）的 try/finally 域数。
    pub(crate) finally_depth: usize,
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
