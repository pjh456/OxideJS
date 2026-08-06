//! IR 中间表示：AST→IR→字节码 流水线的程序表示。
//!
//! 单一程序表示（D-01），分域组合（D-02）：
//! - code: 指令流 + label 位置
//! - data: 常量池
//! - bindings: 参数布局
//! - builtins: 内置全局 → 寄存器
//! - closures: 闭包捕获 + cell
//! - meta: 函数元信息 + 溢出证据
//! - nested: 子函数（递归 IR，替代 sub_modules）
//!
//! 分析视图（CFG/LiveInfo/AllocMap）是 pass 输出，不住进 IR。

pub mod contract;
pub mod inst;
pub mod lower;
pub mod operand;

use oxide_bytecode::module::{Constant, UpvalueCapture};

use crate::inst::Inst;

/// 参数段布局：base 起 count 个寄存器连续（VM 调用契约），RegAlloc 输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamLayout {
    pub base: u32,
    pub count: u32,
}

/// 单函数 IR。分域组合，嵌套封顶两层。
#[derive(Debug, Clone)]
pub struct IRFunction {
    // code 域
    pub insts: Vec<Inst>,
    /// label id → 指令下标。id 连续递增，Vec 索引即 id；写入前须扩容。
    pub label_pos: Vec<Option<usize>>,
    pub label_count: u32,
    // data 域
    pub constants: Vec<Constant>,
    // bindings 域
    pub param_layout: ParamLayout,
    // builtins 域
    pub builtin_reg_map: Vec<(String, u8)>,
    // closures 域
    pub upvalue_captures: Vec<UpvalueCapture>,
    pub cells_needed: u8,
    // meta 域
    pub n_registers: u8,
    pub is_arrow: bool,
    pub is_class_constructor: bool,
    pub is_derived_constructor: bool,
    pub needs_home_object: bool,
    pub captured_this_const_idx: u16,
    pub function_name: Option<String>,
    /// emit 侧溢出证据，lowering 读标志报错（D-20）。
    pub reg_overflow: bool,
    pub const_overflow: bool,
    // nested 域
    pub nested: Vec<IRFunction>,
}

impl IRFunction {
    /// 构造空 IRFunction：全域置空/置零，等价于 Default。
    pub fn new() -> Self {
        Self {
            insts: Vec::new(),
            label_pos: Vec::new(),
            label_count: 0,
            constants: Vec::new(),
            param_layout: ParamLayout { base: 0, count: 0 },
            builtin_reg_map: Vec::new(),
            upvalue_captures: Vec::new(),
            cells_needed: 0,
            n_registers: 0,
            is_arrow: false,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            captured_this_const_idx: 0,
            function_name: None,
            reg_overflow: false,
            const_overflow: false,
            nested: Vec::new(),
        }
    }
}

impl Default for IRFunction {
    fn default() -> Self {
        Self::new()
    }
}
