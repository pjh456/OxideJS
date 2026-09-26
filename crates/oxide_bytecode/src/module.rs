//! 编译产物（compiled module）与常量池 ABI。
//!
//! [`CompiledModule`] 是编译器输出的字节码函数单元：指令序列、常量池、
//! 寄存器布局元信息、子函数（嵌套函数）与 upvalue 捕获描述；随 VM 解释执行
//! 或由其它模块克隆复制。`Display` 输出可读的反汇编文本，供调试用。

use std::fmt;
use std::sync::Arc;

use crate::opcode::{self, OpCode};

/// 常量池条目：编译期折叠的不可变值。
///
/// 变体覆盖 ECMAScript 顶层字面量类型；`Display` 未实现，调试输出走 `Debug`。
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    Int(i32),
    BigInt(num_bigint::BigInt),
    String(String),
    Boolean(bool),
    Null,
    Undefined,
}

/// 闭包对上层作用域一个变量的捕获描述。
///
/// `enclosing_reg` 是外层函数中该变量的寄存器位；`cell_idx` 是父函数 own cell
/// 表下标（`parent_uv_idx` 为 None 时）。多级闭包（外层变量本身就是父函数从更
/// 外层捕获的 upvalue）时 `parent_uv_idx` 给出父闭包 `upvalues` 数组下标。
#[derive(Debug, Clone)]
pub struct UpvalueCapture {
    pub name: String,
    pub enclosing_reg: u32,
    pub cell_idx: u8,
    /// 链式捕获：None = 父 own cell（cell_idx 索引定义方 cell 表）；
    /// Some = 父函数自身 upvalue（运行时从父闭包 upvalues[parent_uv_idx] 取 cell）。
    pub parent_uv_idx: Option<u8>,
}

/// 一个函数单元（或顶层脚本）的编译产物。
///
/// 字段说明：
/// - `bytecode` / `constants` — 指令序列与常量池；
/// - `n_registers` / `n_args` / `param_base` — 寄存器窗口布局；
/// - `builtin_reg_map` — 内置对象到寄存器的预绑定；
/// - `sub_modules` — 嵌套函数（闭包体）的编译产物（`Arc` 共享，避免每次 run 深拷贝模块树）；
/// - `is_arrow` / `captured_this_const_idx` — 箭头函数词法 `this`；
/// - `is_class_constructor` / `is_derived_constructor` / `needs_home_object` — 类相关；
/// - `upvalue_captures` / `cells_needed` — 闭包捕获描述。
pub struct CompiledModule {
    pub bytecode: Arc<[opcode::Instr]>,
    pub constants: Vec<Constant>,
    pub n_registers: u8,
    pub n_args: u8,
    pub param_base: u8,
    pub builtin_reg_map: Vec<(String, u32)>,
    pub sub_modules: Vec<Arc<CompiledModule>>,
    /// 是否为箭头函数体（箭头函数从外围作用域词法捕获 `this`）。
    pub is_arrow: bool,
    /// 是否为严格模式函数体：VM 帧入口据其判定 sloppy `this` 替换。
    pub is_strict: bool,
    /// 捕获 `this` 的 JsValue 在常量池中的下标；0 表示未捕获，使用标准 this 绑定。
    pub captured_this_const_idx: u16,
    /// 由赋值上下文推断的函数名，在变量声明 / 对象属性赋值点设置。
    pub function_name: Option<String>,
    /// 函数 `length` 属性值：第一个带默认值/解构默认的形参之前的形参数（rest 不计）。
    pub function_length: u32,
    /// 是否为类构造函数（普通 CALL 必须拒绝它，仅 NEW_EXPRESSION 可经它构造）。
    pub is_class_constructor: bool,
    /// 类构造函数是否有 `extends` 子句（`this` 在 SUPER_CALL 完成前保持未初始化）。
    pub is_derived_constructor: bool,
    /// 原型方法是否需要运行时 home_object。
    pub needs_home_object: bool,
    /// 是否为生成器函数体（`function*`）：调用返回迭代器对象，body 挂起/恢复执行。
    pub is_generator: bool,
    /// 是否为异步函数体（`async function` / async 箭头）：调用返回 promise，body 挂起/恢复执行。
    pub is_async: bool,
    pub upvalue_captures: Vec<UpvalueCapture>,
    pub cells_needed: u8,
    /// 全局扁平模块 id：编译末端 flatten 阶段分配（顶层 0，子模块 DFS 递增）。
    /// `CREATE_CLOSURE` 的 imm16 在 flatten 后即此 id，运行时以它为平表下标。
    pub flat_id: u32,
    /// 是否为 ES module 顶层：VM 据此把顶层 `this` 绑定为 undefined
    /// （模块环境记录 GetThisBinding 返回 undefined，区别于脚本全局 this）。
    pub is_es_module: bool,
}

impl CompiledModule {
    /// 构造空模块：空字节码、空常量池、零寄存器与全部标志默认关闭。
    pub fn new() -> Self {
        Self {
            bytecode: Arc::from(Vec::new()),
            constants: Vec::new(),
            n_registers: 0,
            n_args: 0,
            param_base: 0,
            builtin_reg_map: Vec::new(),
            sub_modules: Vec::new(),
            is_arrow: false,
            is_strict: false,
            captured_this_const_idx: 0,
            function_name: None,
            function_length: 0,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            is_generator: false,
            is_async: false,
            upvalue_captures: Vec::new(),
            cells_needed: 0,
            flat_id: 0,
            is_es_module: false,
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
            is_strict: self.is_strict,
            captured_this_const_idx: self.captured_this_const_idx,
            function_name: self.function_name.clone(),
            function_length: self.function_length,
            is_class_constructor: self.is_class_constructor,
            is_derived_constructor: self.is_derived_constructor,
            needs_home_object: self.needs_home_object,
            is_generator: self.is_generator,
            is_async: self.is_async,
            upvalue_captures: self.upvalue_captures.clone(),
            cells_needed: self.cells_needed,
            flat_id: self.flat_id,
            is_es_module: self.is_es_module,
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
