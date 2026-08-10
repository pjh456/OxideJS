//! 字节码操作码表与指令编解码。
//!
//! [`OpCode`] 由 `define_opcodes!` 宏从单张表生成（每行含语义字段
//! def/uses/pure/jump/term/ic）；[`Instr`] 是 4 字节指令（opcode + rd + a + b，
//! imm16/offset16 复用 a、b 两字节）。本模块提供编译器发射（`encode*`）与 VM
//! 解码（`opcode` / `rd` / `a` / `b` 等）全套函数。

use std::fmt;

/// 操作数槽位：指令 rd/a/b 三槽之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// 目标寄存器槽。
    Rd,
    /// 第一操作数槽。
    A,
    /// 第二操作数槽。
    B,
}

/// 槽位规范：声明指令如何读写寄存器。解析器在 `oxide_ir::contract`。
///
/// # 语义约定
/// - `Slot(Rd|A|B)`：读/写对应操作数槽；Const/Imm/Label 非寄存器则跳过；
///   None→0、This→254、NewTarget→255 由操作数层 reg_of 映射（不进表）。
/// - `Reg0`：常量寄存器 0（CALL 隐式写、HALT 隐式读）。
/// - `Range(Slot)`：从指定槽起连续 nargs 个寄存器，nargs=ext[0]（调用实参区）。
/// - `SpreadArgs`：ext[1..] 每个字 `& 0x7FFF_FFFF` 是实参寄存器（spread 源同低 31 位）。
/// - `TemplateExprs`：TEMPLATE_STR 的 ext[1..]，`seg>>31==1` 时低 8 位是表达式寄存器。
/// - `BrandReg`：ext[0] 是 brand 对象寄存器，0 表示跳过检查（不产生 use）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSpec {
    /// 读写对应操作数槽。
    Slot(Slot),
    /// 常量寄存器 0。
    Reg0,
    /// 从指定槽起连续 nargs 个寄存器（nargs=ext[0]）。
    Range(Slot),
    /// spread 调用系 ext[1..] 的有序实参字。
    SpreadArgs,
    /// TEMPLATE_STR 的 ext[1..] 表达式寄存器。
    TemplateExprs,
    /// ext[0] 的 brand 对象寄存器（0 跳过检查）。
    BrandReg,
}

/// 单张语义表行：一个 opcode 的 def/uses/pure/跳转/终结/IC 扩展字声明。
///
/// 字段语义与各消费方历史硬编码集合一致：`is_jump` = lower.rs 跳转族（label
/// 回填），`is_terminator` = CFG 块尾终结，`ic_ext` = VM 的 IC 扩展字。`pure`
/// 是静态可删判定，LOAD_VAR 的上下文例外留在 `oxide_ir::contract::is_pure`。
pub struct OpSemantics {
    /// 写入的寄存器（恒为单个，无 Range）。
    pub def: Option<SlotSpec>,
    /// 读取的寄存器（按序，序即 use_regs 输出序）。
    pub uses: &'static [SlotSpec],
    /// 静态可删判定（LOAD_VAR 例外在 oxide_ir 侧）。
    pub pure: bool,
    /// 带 Label 槽、需偏移回填（= lower.rs is_jump_op）。
    pub is_jump: bool,
    /// CFG 块尾终结（= cfg is_terminator）。
    pub is_terminator: bool,
    /// 3 个 IC 扩展字（= has_ic_ext_words）。
    pub ic_ext: bool,
}

/// 从单张表生成 [`OpCode`] 枚举、`TryFrom<u8>`、`Display` 与语义访问器。
///
/// 每行同时提供枚举判别值、`TryFrom` 分支、`Display` 名称与语义字段
/// （def/uses/pure/jump/term/ic）七份信息，保证多表永不漂移；新增操作码只需
/// 加一行。`semantics()` 的 match 无 `_` 臂——漏填语义即编译错误。
///
/// # 注意事项
/// - 分组注释（`// ── 算术 ──`）在宏展开前被词法器剥离，可自由插入行间。
macro_rules! define_opcodes {
    ( $(
        $name:ident = $val:literal => $disp:literal,
        def = $def:expr, uses = [$($u:expr),*],
        pure = $pure:literal, jump = $jump:literal, term = $term:literal, ic = $ic:literal
    ),+ $(,)? ) => {
        /// 寄存器字节码虚拟机的操作码。
        ///
        /// 按 16 个一组组织便于阅读。已实现的操作码在编译器中有发射支持；
        /// 占位符操作码为后续阶段预留（IC、profiling、并行化）。
        #[repr(u8)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[allow(non_camel_case_types)]
        pub enum OpCode {
            $( $name = $val, )+
        }

        impl TryFrom<u8> for OpCode {
            type Error = ();

            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $( $val => Ok(OpCode::$name), )+
                    _ => Err(()),
                }
            }
        }

        impl fmt::Display for OpCode {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let name = match self {
                    $( OpCode::$name => $disp, )+
                };
                write!(f, "{name}")
            }
        }

        impl OpCode {
            /// 单张语义表访问器。match 无 `_` 臂：新 opcode 漏填语义即编译错误。
            ///
            /// # 注意事项
            /// - 返回 `&'static`：match 臂内 `&OpSemantics { .. }` 是常量表达式，
            ///   由常量提升保证零运行时开销。
            pub fn semantics(&self) -> &'static OpSemantics {
                match self {
                    $( OpCode::$name => &OpSemantics {
                        def: $def,
                        uses: &[$($u),*],
                        pure: $pure,
                        is_jump: $jump,
                        is_terminator: $term,
                        ic_ext: $ic,
                    }, )+
                }
            }

            /// 便捷访问器：跳转族（label 偏移回填）。
            pub fn is_jump(&self) -> bool {
                self.semantics().is_jump
            }

            /// 便捷访问器：CFG 块尾终结。
            pub fn is_terminator(&self) -> bool {
                self.semantics().is_terminator
            }

            /// 便捷访问器：IC 扩展字。
            pub fn has_ic_ext_words(&self) -> bool {
                self.semantics().ic_ext
            }
        }
    };
}

define_opcodes! {
    // ── Arithmetic (0x00-0x0F) ──
    ADD = 0x00 => "ADD",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    SUB = 0x01 => "SUB",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    MUL = 0x02 => "MUL",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    DIV = 0x03 => "DIV",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    MOD = 0x04 => "MOD",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    NEG = 0x05 => "NEG",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    COMPOUND_ADD = 0x06 => "COMPOUND_ADD",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_SUB = 0x07 => "COMPOUND_SUB",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_MUL = 0x08 => "COMPOUND_MUL",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_DIV = 0x09 => "COMPOUND_DIV",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_MOD = 0x0A => "COMPOUND_MOD",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_EXP = 0x0B => "COMPOUND_EXP",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    MOV = 0x0C => "MOV", // rd=dst, a=src（寄存器复制，区间拆分搬值）
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    SPILL = 0x0D => "SPILL", // rd=src, ext=[slot u16]（寄存器值写 VM spill 栈）
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    UNSPILL = 0x0E => "UNSPILL", // rd=dst, ext=[slot u16]（从 VM spill 栈恢复寄存器）
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    SPREAD_OBJECT = 0x0F => "SPREAD_OBJECT", // rd=目标对象, a=源（对象字面量 ... 展开，原地写目标）
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,

    // ── 比较 (0x10-0x1F) ──
    EQ = 0x10 => "EQ",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    NEQ = 0x11 => "NEQ",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    LT = 0x12 => "LT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    GT = 0x13 => "GT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    LTE = 0x14 => "LTE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    GTE = 0x15 => "GTE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    IN = 0x16 => "IN",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    NOT = 0x17 => "NOT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    AND = 0x18 => "AND",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    OR = 0x19 => "OR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    STRICT_EQ = 0x1A => "STRICT_EQ",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    STRICT_NEQ = 0x1C => "STRICT_NEQ",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    UNARY_PLUS = 0x1D => "UNARY_PLUS",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,

    // ── 控制流 (0x1E-0x2F) ──
    BREAK = 0x1E => "BREAK",
        def = None, uses = [],
        pure = false, jump = true, term = true, ic = false,
    CONTINUE = 0x1F => "CONTINUE",
        def = None, uses = [],
        pure = false, jump = true, term = true, ic = false,
    JMP = 0x20 => "JMP",
        def = None, uses = [],
        pure = false, jump = true, term = true, ic = false,
    JMP_IF_FALSE = 0x21 => "JMP_IF_FALSE",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = true, term = true, ic = false,
    JMP_IF_TRUE = 0x22 => "JMP_IF_TRUE",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = true, term = true, ic = false,
    FOR_OF_INIT = 0x23 => "FOR_OF_INIT",
        def = None, uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    FOR_OF_NEXT = 0x24 => "FOR_OF_NEXT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,

    // ── 更新 (0x25-0x28) ──
    INC_PRE = 0x25 => "INC_PRE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    INC_POST = 0x26 => "INC_POST",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    DEC_PRE = 0x27 => "DEC_PRE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    DEC_POST = 0x28 => "DEC_POST",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,

    FOR_IN_INIT = 0x29 => "FOR_IN_INIT",
        def = None, uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    FOR_IN_NEXT = 0x2A => "FOR_IN_NEXT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    FOR_IN_DONE = 0x2B => "FOR_IN_DONE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    SWITCH_TABLE = 0x2C => "SWITCH_TABLE", // 占位：emit 不产，按 catch-all 保守 def
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    FOR_IN_CLEANUP = 0x2D => "FOR_IN_CLEANUP",
        def = None, uses = [],
        pure = false, jump = false, term = false, ic = false,

    // ── 异常 (0x2E-0x2F, 0x33-0x35) ──
    THROW = 0x2E => "THROW",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = true, ic = false,
    TRY_BEGIN = 0x2F => "TRY_BEGIN",
        def = None, uses = [],
        pure = false, jump = true, term = false, ic = false,
    TRY_END = 0x33 => "TRY_END",
        def = None, uses = [],
        pure = false, jump = false, term = false, ic = false,
    TRY_FINALLY_BEGIN = 0x34 => "TRY_FINALLY_BEGIN",
        def = None, uses = [],
        pure = false, jump = true, term = false, ic = false,
    TRY_FINALLY_END = 0x35 => "TRY_FINALLY_END",
        def = None, uses = [],
        pure = false, jump = false, term = false, ic = false,
    FOR_OF_DONE = 0x36 => "FOR_OF_DONE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    FOR_OF_CLOSE = 0x37 => "FOR_OF_CLOSE",
        def = None, uses = [],
        pure = false, jump = false, term = false, ic = false,

    // ── 模板字符串 (0x38) ──
    TEMPLATE_STR = 0x38 => "TEMPLATE_STR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::TemplateExprs],
        pure = true, jump = false, term = false, ic = false,

    // ── 小语言特性 (0x39-0x3B) ──
    DELETE_PROP_STATIC = 0x39 => "DELETE_PROP_STATIC",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    DELETE_PROP_DYNAMIC = 0x3A => "DELETE_PROP_DYNAMIC",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    INSTANCEOF = 0x3B => "INSTANCEOF",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    MAKE_CELL = 0x3C => "MAKE_CELL",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    CELL_GET = 0x3D => "CELL_GET",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    REST_OBJECT = 0x3E => "REST_OBJECT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    CELL_SET = 0x3F => "CELL_SET",
        def = None, uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,

    // ── 变量 (0x30-0x32) ──
    LOAD_VAR = 0x30 => "LOAD_VAR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    STORE_VAR = 0x31 => "STORE_VAR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    LOAD_CONST = 0x32 => "LOAD_CONST",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,

    // ── 调用 (0x40-0x4F) ──
    CALL = 0x40 => "CALL", // 结果隐式写 reg 0；rd=callee, a=this, b=首参, ext[0]=nargs
        def = Some(SlotSpec::Reg0), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Range(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    RETURN = 0x41 => "RETURN",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = true, ic = false,
    CALL_NATIVE = 0x42 => "CALL_NATIVE",
        def = Some(SlotSpec::Reg0), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Range(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    NEW_EXPRESSION = 0x43 => "NEW_EXPRESSION", // a=ctor, b..b+nargs
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Range(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    SUPER_CALL = 0x44 => "SUPER_CALL", // a..a+nargs，无 callee/this 槽
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Range(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    SUPER_GET_PROP = 0x45 => "SUPER_GET_PROP",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    SUPER_STATIC_GET_PROP = 0x46 => "SUPER_STATIC_GET_PROP",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    SET_HOME_OBJECT = 0x47 => "SET_HOME_OBJECT",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    DEFINE_ACCESSOR = 0x48 => "DEFINE_ACCESSOR",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    LOAD_UPVALUE = 0x49 => "LOAD_UPVALUE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,
    CREATE_CLOSURE = 0x4A => "CREATE_CLOSURE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,
    CREATE_REGEXP = 0x4B => "CREATE_REGEXP",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    STORE_UPVALUE = 0x4C => "STORE_UPVALUE",
        def = None, uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    CALL_SPREAD = 0x4D => "CALL_SPREAD", // 结果经 reg 0；ext[1..] 有序实参字
        def = Some(SlotSpec::Reg0), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::SpreadArgs],
        pure = false, jump = false, term = false, ic = false,
    NEW_EXPRESSION_SPREAD = 0x4E => "NEW_EXPRESSION_SPREAD",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::SpreadArgs],
        pure = false, jump = false, term = false, ic = false,
    SUPER_CALL_SPREAD = 0x4F => "SUPER_CALL_SPREAD",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::SpreadArgs],
        pure = false, jump = false, term = false, ic = false,

    // ── Object Property (0x50-0x5F) ──
    IC_GET_PROP = 0x50 => "IC_GET_PROP", // a 槽既是对象 use 又写回结果，rd 忽略
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    IC_SET_PROP = 0x51 => "IC_SET_PROP",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    GET_PROP = 0x52 => "GET_PROP", // 结果写 a 槽，rd=obj 是 use
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    GET_PROP_DYNAMIC = 0x53 => "GET_PROP_DYNAMIC", // 结果写 b 槽
        def = Some(SlotSpec::Slot(Slot::B)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    SET_PROP = 0x54 => "SET_PROP",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    SET_PROP_DYNAMIC = 0x55 => "SET_PROP_DYNAMIC",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    NEW_OBJECT = 0x56 => "NEW_OBJECT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,
    NEW_ARRAY = 0x57 => "NEW_ARRAY",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,
    SET_ELEM = 0x58 => "SET_ELEM",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,

    // ── 成员更新 (0x59-0x62) ──
    MEMBER_INC = 0x59 => "MEMBER_INC", // 结果写 a 槽（val 槽原地更新）
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    MEMBER_DEC = 0x5A => "MEMBER_DEC",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    DYN_MEMBER_INC = 0x5B => "DYN_MEMBER_INC", // 结果写 b 槽
        def = Some(SlotSpec::Slot(Slot::B)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    DYN_MEMBER_DEC = 0x5C => "DYN_MEMBER_DEC",
        def = Some(SlotSpec::Slot(Slot::B)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_MEMBER_ADD = 0x5D => "COMPOUND_MEMBER_ADD",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_SUB = 0x5E => "COMPOUND_MEMBER_SUB",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_MUL = 0x5F => "COMPOUND_MEMBER_MUL",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_DIV = 0x60 => "COMPOUND_MEMBER_DIV",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_MOD = 0x61 => "COMPOUND_MEMBER_MOD",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_EXP = 0x62 => "COMPOUND_MEMBER_EXP",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,

    // ── 对象 (0x63) ──
    CREATE_ARGUMENTS = 0x63 => "CREATE_ARGUMENTS",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    // rest 参数数组：rd=目标寄存器，b=固定形参数（rest 之前的形参个数）。
    CREATE_REST_ARRAY = 0x6E => "CREATE_REST_ARRAY",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,

    // ── define 语义属性写入 (0x6D, 0x6F) ──
    DEFINE_PROP = 0x6D => "DEFINE_PROP",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    // 全局 var 绑定属性定义：rd=目标对象、a=值、b=键，数据属性可写/可枚举/不可配置
    // （脚本顶层 var/function 声明同步到 globalThis 用，属性描述符与规范一致）。
    DEFINE_GLOBAL_PROP = 0x6F => "DEFINE_GLOBAL_PROP",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,

    // ── 成员复合赋值：位/移位 (0x64-0x69) ──
    COMPOUND_MEMBER_BIT_AND = 0x64 => "COMPOUND_MEMBER_BIT_AND",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_BIT_OR = 0x65 => "COMPOUND_MEMBER_BIT_OR",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_BIT_XOR = 0x66 => "COMPOUND_MEMBER_BIT_XOR",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_SHL = 0x67 => "COMPOUND_MEMBER_SHL",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_SHR = 0x68 => "COMPOUND_MEMBER_SHR",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,
    COMPOUND_MEMBER_USHR = 0x69 => "COMPOUND_MEMBER_USHR",
        def = Some(SlotSpec::Slot(Slot::A)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = true,

    // ── Profiling — 占位符 (0x6A-0x6F) ──
    PROFILE_SHAPE = 0x6A => "PROFILE_SHAPE", // 占位：catch-all 保守 def
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    PROFILE_BRANCH = 0x6B => "PROFILE_BRANCH",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    PROFILE_CALL = 0x6C => "PROFILE_CALL",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,

    // ── 并行 — 占位符 (0x70-0x75) ──
    FORK = 0x70 => "FORK",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    JOIN = 0x71 => "JOIN",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    GET_PRIVATE = 0x72 => "GET_PRIVATE", // ext[0]=brand_reg，非 0 才产生 use
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B), SlotSpec::BrandReg],
        pure = false, jump = false, term = false, ic = false,
    SET_PRIVATE = 0x73 => "SET_PRIVATE",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B), SlotSpec::BrandReg],
        pure = false, jump = false, term = false, ic = false,
    INIT_PRIVATE = 0x74 => "INIT_PRIVATE",
        def = None, uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,
    PRIVATE_BRAND_IN = 0x75 => "PRIVATE_BRAND_IN",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = false, jump = false, term = false, ic = false,

    // ── 生成器 (0x76-0x78) ──
    YIELD = 0x76 => "YIELD", // rd=被让出的值（结果经 reg 0 交付，见 contract.rs）
        def = Some(SlotSpec::Reg0), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    // 生成器 body 起点标记：调用时参数初始化完成后挂起于此，首次 next() 从这继续。
    SUSPEND_BODY = 0x77 => "SUSPEND_BODY",
        def = None, uses = [],
        pure = false, jump = false, term = false, ic = false,
    // `yield*` 委托：rd=内层可迭代对象；委托完成值经 reg 0 交付（与 YIELD 同协议）。
    YIELD_STAR = 0x78 => "YIELD_STAR",
        def = Some(SlotSpec::Reg0), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    // `await`：rd=被等待的值（PromiseResolve 包装）；挂起异步帧，恢复值经 reg 0 交付。
    AWAIT = 0x79 => "AWAIT",
        def = Some(SlotSpec::Reg0), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    // for-await-of 步进：INIT(a=可迭代值) 取异步迭代器；NEXT 调用迭代器 next()，
    // 结果写 rd（随后由 AWAIT 等待）；DONE 读取 AWAIT 恢复值（a 槽）的 done 写 rd；
    // CLOSE 执行异步 IteratorClose（return() 结果 await 后继续）。
    FOR_AWAIT_OF_INIT = 0x7A => "FOR_AWAIT_OF_INIT",
        def = None, uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    FOR_AWAIT_OF_NEXT = 0x7B => "FOR_AWAIT_OF_NEXT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = false, jump = false, term = false, ic = false,
    FOR_AWAIT_OF_DONE = 0x7C => "FOR_AWAIT_OF_DONE",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    FOR_AWAIT_OF_CLOSE = 0x7D => "FOR_AWAIT_OF_CLOSE",
        def = None, uses = [],
        pure = false, jump = false, term = false, ic = false,
    // 无条件新建 cell 并替换 cell_stack[cell_idx]，值为 regs[rd]——循环每迭代绑定
    // 把当前值拷入新 cell，本迭代闭包捕获新 cell（旧闭包仍指向旧 cell）。
    MAKE_CELL_FRESH = 0x7E => "MAKE_CELL_FRESH",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,

    // ── 位运算 (0x80-0x8F) ──
    BIT_AND = 0x80 => "BIT_AND",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    BIT_OR = 0x81 => "BIT_OR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    BIT_XOR = 0x82 => "BIT_XOR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    SHL = 0x83 => "SHL",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    SHR = 0x84 => "SHR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    USHR = 0x85 => "USHR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    BIT_NOT = 0x86 => "BIT_NOT",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    TO_OBJECT = 0x87 => "TO_OBJECT", // rd 原地转换（读且写）
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_AND = 0x88 => "COMPOUND_AND",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_OR = 0x89 => "COMPOUND_OR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_XOR = 0x8A => "COMPOUND_XOR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_SHL = 0x8B => "COMPOUND_SHL",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_SHR = 0x8C => "COMPOUND_SHR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    COMPOUND_USHR = 0x8D => "COMPOUND_USHR",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::Rd), SlotSpec::Slot(Slot::A)],
        pure = false, jump = false, term = false, ic = false,
    NULLISH = 0x8E => "NULLISH",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A), SlotSpec::Slot(Slot::B)],
        pure = true, jump = false, term = false, ic = false,
    JMP_IF_NULLISH = 0x8F => "JMP_IF_NULLISH",
        def = None, uses = [SlotSpec::Slot(Slot::Rd)],
        pure = false, jump = true, term = true, ic = false,

    // ── 杂项 (0xF0-0xFF) ──
    NOP = 0xF0 => "NOP", // 保持 def=Rd：catch-all 使 NOP(rd=None) 的 def_reg=Some(0)
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,
    HALT = 0xF1 => "HALT", // 隐式读 reg 0（顶层返回值）
        def = None, uses = [SlotSpec::Reg0],
        pure = false, jump = false, term = true, ic = false,
    TYPEOF = 0xF2 => "TYPEOF",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [SlotSpec::Slot(Slot::A)],
        pure = true, jump = false, term = false, ic = false,
    VOID = 0xF3 => "VOID",
        def = Some(SlotSpec::Slot(Slot::Rd)), uses = [],
        pure = true, jump = false, term = false, ic = false,
}

/// 4 字节定长指令。
///
/// 布局 `[opcode: u8] [rd: u8] [a: u8] [b: u8]`：
/// - `rd` — 目标寄存器
/// - `a` — 第一个源寄存器，或 imm16 低字节
/// - `b` — 第二个源寄存器，或 imm16 高字节
pub type Instr = u32;

/// 把操作码与三个操作数字节编码为一条 [`Instr`]。
pub fn encode(op: OpCode, rd: u8, a: u8, b: u8) -> Instr {
    ((b as Instr) << 24) | ((a as Instr) << 16) | ((rd as Instr) << 8) | (op as Instr)
}

/// 解码指令的低 8 位操作码；未知字节返回 [`OpCode::NOP`]。
pub fn opcode(instr: Instr) -> OpCode {
    OpCode::try_from((instr & 0xFF) as u8).unwrap_or(OpCode::NOP)
}

/// 解码目标寄存器（bits 8-15）。
pub fn rd(instr: Instr) -> u8 {
    ((instr >> 8) & 0xFF) as u8
}

/// 解码第一个操作数（bits 16-23）。
pub fn a(instr: Instr) -> u8 {
    ((instr >> 16) & 0xFF) as u8
}

/// 解码第二个操作数（bits 24-31）。
pub fn b(instr: Instr) -> u8 {
    ((instr >> 24) & 0xFF) as u8
}

/// 解码 16 位立即数（bits 16-31，由 a、b 两字节拼成）。
pub fn imm16(instr: Instr) -> u16 {
    ((instr >> 16) & 0xFFFF) as u16
}

/// 解码 16 位有符号跳转偏移（bits 16-31）。
pub fn offset16(instr: Instr) -> i16 {
    ((instr >> 16) & 0xFFFF) as i16
}

/// 发射无条件跳转指令（`JMP`，偏移以 16 位补码编码）。
pub fn encode_jmp(offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP, 0, lo, hi)
}

/// 发射 `JMP_IF_FALSE`：当 `rd` 寄存器为 falsy 时跳转。
pub fn encode_jmp_if_false(rd: u8, offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP_IF_FALSE, rd, lo, hi)
}

/// 发射 `JMP_IF_TRUE`：当 `rd` 寄存器为 truthy 时跳转。
pub fn encode_jmp_if_true(rd: u8, offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP_IF_TRUE, rd, lo, hi)
}

/// 发射 `JMP_IF_NULLISH`：当 `rd` 为 `null` / `undefined` 时跳转（`??` 短路）。
pub fn encode_jmp_if_nullish(rd: u8, offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::JMP_IF_NULLISH, rd, lo, hi)
}

/// 发射 `TRY_BEGIN`：标记 try 块开始，偏移指向 catch 处理器。
pub fn encode_try_begin(offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::TRY_BEGIN, 0, lo, hi)
}

/// 发射 `TRY_FINALLY_BEGIN`：标记 try/finally 块开始，偏移指向 finally 处理器。
pub fn encode_try_finally_begin(offset: i16) -> Instr {
    let lo = (offset as u16 & 0xFF) as u8;
    let hi = ((offset as u16 >> 8) & 0xFF) as u8;
    encode(OpCode::TRY_FINALLY_BEGIN, 0, lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 遍历全部有效 opcode（枚举字节序）。
    fn all_opcodes() -> impl Iterator<Item = OpCode> {
        (0u8..=u8::MAX).filter_map(|b| OpCode::try_from(b).ok())
    }

    #[test]
    fn opcode_table_is_byte_identical() {
        for byte in 0u8..=u8::MAX {
            if let Ok(op) = OpCode::try_from(byte) {
                assert_eq!(op as u8, byte, "discriminant mismatch for 0x{byte:02X}");
                assert!(!op.to_string().is_empty(), "empty Display for 0x{byte:02X}");
            }
        }

        assert_eq!(OpCode::ADD as u8, 0x00);
        assert_eq!(OpCode::STRICT_EQ as u8, 0x1A);
        assert_eq!(OpCode::JMP_IF_NULLISH as u8, 0x8F);
        assert_eq!(OpCode::VOID as u8, 0xF3);
        assert_eq!(OpCode::MOV as u8, 0x0C);
        assert_eq!(OpCode::SPILL as u8, 0x0D);
        assert_eq!(OpCode::UNSPILL as u8, 0x0E);
        assert_eq!(OpCode::SPREAD_OBJECT as u8, 0x0F);
        assert_eq!(OpCode::CREATE_ARGUMENTS as u8, 0x63);
        assert_eq!(OpCode::CREATE_REST_ARRAY as u8, 0x6E);
        assert_eq!(OpCode::ADD.to_string(), "ADD");
        assert_eq!(OpCode::COMPOUND_MEMBER_EXP.to_string(), "COMPOUND_MEMBER_EXP");
        assert_eq!(OpCode::MOV.to_string(), "MOV");
        assert_eq!(OpCode::SPILL.to_string(), "SPILL");
        assert_eq!(OpCode::UNSPILL.to_string(), "UNSPILL");
        assert_eq!(OpCode::SPREAD_OBJECT.to_string(), "SPREAD_OBJECT");

        assert!(OpCode::try_from(0x1B).is_err());
        assert!(OpCode::try_from(0xFF).is_err());
    }

    /// 全 opcode 表遍历：jump/terminator/ic 三集合与 lower/cfg/vm 硬编码集合逐字节一致。
    #[test]
    fn semantics_golden_sets() {
        let jump_set: Vec<u8> = all_opcodes().filter(|op| op.is_jump()).map(|op| op as u8).collect();
        assert_eq!(
            jump_set,
            [0x1E, 0x1F, 0x20, 0x21, 0x22, 0x2F, 0x34, 0x8F],
            "is_jump 集合（8 个跳转族，含 TRY_BEGIN/TRY_FINALLY_BEGIN label 回填）"
        );

        let term_set: Vec<u8> = all_opcodes().filter(|op| op.is_terminator()).map(|op| op as u8).collect();
        assert_eq!(
            term_set,
            [0x1E, 0x1F, 0x20, 0x21, 0x22, 0x2E, 0x41, 0x8F, 0xF1],
            "is_terminator 集合（跳转族去 TRY_* 加 RETURN/HALT/THROW）"
        );

        let ic_set: Vec<u8> = all_opcodes()
            .filter(|op| op.has_ic_ext_words())
            .map(|op| op as u8)
            .collect();
        assert_eq!(ic_set.len(), 16, "ic_ext 恰 16 个");
    }

    /// 表驱动 ic_ext 与迁移前硬编码 16 集合逐项相等（旧表并存期的迁移校验）。
    #[test]
    fn semantics_ic_ext_matches_legacy_list() {
        let legacy = [
            0x50, 0x51, 0x59, 0x5A, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x64, 0x65, 0x66, 0x67, 0x68,
            0x69,
        ];
        let tbl: Vec<u8> = all_opcodes()
            .filter(|op| op.has_ic_ext_words())
            .map(|op| op as u8)
            .collect();
        assert_eq!(tbl, legacy);
    }
}
