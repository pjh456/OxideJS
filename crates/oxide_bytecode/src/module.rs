//! 编译产物（compiled module）与常量池 ABI。
//!
//! [`CompiledModule`] 是编译器输出的字节码函数单元：指令序列、常量池、
//! 寄存器布局元信息、子函数（嵌套函数）与 upvalue 捕获描述；随 VM 解释执行
//! 或由其它模块克隆复制。`Display` 输出可读的反汇编文本，供调试用。

use std::fmt;

use crate::opcode::{self, OpCode};

/// 常量池条目：编译期折叠的不可变值。
///
/// 变体覆盖 ECMAScript 顶层字面量类型；`Display` 未实现，调试输出走 `Debug`。
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    Int(i32),
    String(String),
    Boolean(bool),
    Null,
    Undefined,
}

/// 闭包对上层作用域一个变量的捕获描述。
///
/// `enclosing_reg` 是外层函数中该变量的寄存器位；若外层变量本身就是 upvalue
/// （多级闭包），`cell_idx` 指向链式捕获的 cell。
#[derive(Debug, Clone)]
pub struct UpvalueCapture {
    pub name: String,
    pub enclosing_reg: u32,
    pub cell_idx: u8,
}

/// 一个函数单元（或顶层脚本）的编译产物。
///
/// 字段说明：
/// - `bytecode` / `constants` — 指令序列与常量池；
/// - `n_registers` / `n_args` / `param_base` — 寄存器窗口布局；
/// - `builtin_reg_map` — 内置对象到寄存器的预绑定；
/// - `sub_modules` — 嵌套函数（闭包体）的编译产物；
/// - `is_arrow` / `captured_this_const_idx` — 箭头函数词法 `this`；
/// - `is_class_constructor` / `is_derived_constructor` / `needs_home_object` — 类相关；
/// - `upvalue_captures` / `cells_needed` — 闭包捕获描述。
pub struct CompiledModule {
    pub bytecode: Vec<opcode::Instr>,
    pub constants: Vec<Constant>,
    pub n_registers: u8,
    pub n_args: u8,
    pub param_base: u8,
    pub builtin_reg_map: Vec<(String, u32)>,
    pub sub_modules: Vec<CompiledModule>,
    /// True when this module is an arrow function body.
    /// Arrow functions capture lexical `this` from the enclosing scope.
    pub is_arrow: bool,
    /// Index into `constants` holding the captured `this` JsValue.
    /// 0 means "not captured - use standard this binding".
    pub captured_this_const_idx: u16,
    /// Function name inferred from assignment context.
    /// Set at the VariableDeclaration / ObjectProperty assignment site.
    pub function_name: Option<String>,
    /// True when this bytecode function is a class constructor.
    /// Ordinary CALL must reject it, while NEW_EXPRESSION may construct through it.
    pub is_class_constructor: bool,
    /// True when this class constructor has an `extends` clause.
    /// `this` stays uninitialized until SUPER_CALL completes.
    pub is_derived_constructor: bool,
    /// True for prototype methods whose function object needs a runtime home_object.
    pub needs_home_object: bool,
    pub upvalue_captures: Vec<UpvalueCapture>,
    pub cells_needed: u8,
}

impl CompiledModule {
    /// 构造空模块：空字节码、空常量池、零寄存器与全部标志默认关闭。
    pub fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            constants: Vec::new(),
            n_registers: 0,
            n_args: 0,
            param_base: 0,
            builtin_reg_map: Vec::new(),
            sub_modules: Vec::new(),
            is_arrow: false,
            captured_this_const_idx: 0,
            function_name: None,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            upvalue_captures: Vec::new(),
            cells_needed: 0,
        }
    }
}

impl Default for CompiledModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CompiledModule {
    fn clone(&self) -> Self {
        Self {
            bytecode: self.bytecode.clone(),
            constants: self.constants.clone(),
            n_registers: self.n_registers,
            n_args: self.n_args,
            param_base: self.param_base,
            builtin_reg_map: self.builtin_reg_map.clone(),
            sub_modules: self.sub_modules.clone(),
            is_arrow: self.is_arrow,
            captured_this_const_idx: self.captured_this_const_idx,
            function_name: self.function_name.clone(),
            is_class_constructor: self.is_class_constructor,
            is_derived_constructor: self.is_derived_constructor,
            needs_home_object: self.needs_home_object,
            upvalue_captures: self.upvalue_captures.clone(),
            cells_needed: self.cells_needed,
        }
    }
}

impl fmt::Display for CompiledModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "; n_registers = {}", self.n_registers)?;
        writeln!(f, "; constants:")?;
        for (i, c) in self.constants.iter().enumerate() {
            writeln!(f, ";   [{i}] = {c:?}")?;
        }
        writeln!(f)?;
        writeln!(f, "; upvalue_captures: {:?}", self.upvalue_captures)?;
        writeln!(f)?;
        for (offset, &instr) in self.bytecode.iter().enumerate() {
            let op = opcode::opcode(instr);
            let rd = opcode::rd(instr);
            let a = opcode::a(instr);
            let b = opcode::b(instr);
            write!(f, "  {offset:04}  {op}")?;
            match op {
                OpCode::LOAD_CONST => {
                    write!(f, " r{rd}, const[{}]", opcode::imm16(instr))?;
                }
                OpCode::JMP | OpCode::JMP_IF_FALSE | OpCode::JMP_IF_TRUE => {
                    write!(f, " r{rd}, {offset:+}", offset = opcode::offset16(instr))?;
                }
                OpCode::SWITCH_TABLE => {
                    let n_cases = rd as u16 | ((b as u16) << 8);
                    write!(f, " r{disc_reg}={a}, {n_cases} cases", disc_reg = a)?;
                }
                OpCode::RETURN | OpCode::HALT | OpCode::NOP => {
                    write!(f, " r{rd}")?;
                }
                OpCode::NEG => {
                    write!(f, " r{rd}, r{a}")?;
                }
                _ => {
                    write!(f, " r{rd}, r{a}, r{b}")?;
                }
            }
            writeln!(f)?;
        }
        Ok(())
    }
}
