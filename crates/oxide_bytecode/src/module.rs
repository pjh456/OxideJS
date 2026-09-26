//! 编译产物（compiled module）与常量池 ABI。
//!
//! [`CompiledModule`] 是编译器输出的字节码函数单元：指令序列、常量池、
//! 寄存器布局元信息、子函数（嵌套函数）与 upvalue 捕获描述；随 VM 解释执行
//! 或由其它模块克隆复制。`Display` 输出可读的反汇编文本，供调试用：
//! 扩展字并入其指令行不单独成行，offset 列为字序号（与 VM 的 pc 同单位），
//! 子模块（嵌套函数体）递归渲染。

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
        Self::render(f, self, 0, None)
    }
}

impl CompiledModule {
    /// 渲染一个模块单元：头部元信息 + 指令行 + 递归子模块。
    ///
    /// `depth` 控制指令行缩进（顶层 0，每层 +2 空格）；`sub_index` 为子模块序号
    /// （顶层 None）。指令行按 `1 + ext_word_count` 推进 offset，扩展字并入其
    /// 指令行，不单独成行；offset 列即字序号（与 VM 的 pc 同单位）。
    fn render(
        f: &mut fmt::Formatter<'_>, module: &CompiledModule, depth: usize, sub_index: Option<usize>,
    ) -> fmt::Result {
        // 子模块头：序号 + 全局 flat_id + 函数名（嵌套函数体调试入口）。
        if let Some(i) = sub_index {
            let name = module.function_name.as_deref().unwrap_or("");
            writeln!(f, "\n; sub_module[{i}] (flat_id={flat_id}, \"{name}\"):", flat_id = module.flat_id)?;
        }
        // 头部元信息（顶层 2 空格缩进，子模块 4 空格）。
        let p1 = if depth == 0 { "; " } else { ";   " };
        let p2 = if depth == 0 { ";   " } else { ";     " };
        writeln!(f, "{p1}n_registers = {}", module.n_registers)?;
        writeln!(f, "{p1}constants:")?;
        for (i, c) in module.constants.iter().enumerate() {
            writeln!(f, "{p2}[{i}] = {c:?}")?;
        }
        writeln!(f)?;
        writeln!(f, "{p1}upvalue_captures: {:?}", module.upvalue_captures)?;
        writeln!(f)?;
        // 指令行：逐指令按 ext 字数推进，扩展字并入本行。
        let indent = "  ".repeat(depth + 1);
        let mut offset = 0usize;
        while offset < module.bytecode.len() {
            let instr = module.bytecode[offset];
            let op = opcode::opcode(instr);
            write!(f, "{indent}{offset:04}  {op}")?;
            render_operands(f, op, instr, &module.bytecode, offset)?;
            writeln!(f)?;
            offset += 1 + opcode::ext_word_count(&module.bytecode, offset);
        }
        // 递归渲染子模块（嵌套函数体）。
        for (i, sub) in module.sub_modules.iter().enumerate() {
            Self::render(f, sub, depth + 1, Some(i))?;
        }
        Ok(())
    }
}

/// 指令操作数渲染：按 opcode 语义把 rd/a/b/imm16/ext 字渲染为可读文本。
///
/// 常量池下标族渲染 `const[N]`，upvalue 下标族 `uv[N]`，闭包模块下标
/// `module[N]`，跳转族渲染相对偏移（单位 = 指令字，含 ext 字）；ext 字并入
/// 本行。ext 字越界回退 0（Display 面不 panic）。
fn render_operands(
    f: &mut fmt::Formatter<'_>, op: OpCode, instr: opcode::Instr, bytecode: &[opcode::Instr], offset: usize,
) -> fmt::Result {
    let rd = opcode::rd(instr);
    let a = opcode::a(instr);
    let b = opcode::b(instr);
    // ext 字安全读取（越界回退 0）。
    let ext = |i: usize| bytecode.get(offset + 1 + i).copied().unwrap_or(0);
    match op {
        // 跳转族：offset16 相对偏移（单位 = 指令字，含 ext 字）。
        OpCode::JMP | OpCode::TRY_BEGIN | OpCode::TRY_FINALLY_BEGIN => {
            write!(f, " +{} (rel)", opcode::offset16(instr))
        }
        OpCode::JMP_IF_FALSE | OpCode::JMP_IF_TRUE | OpCode::JMP_IF_NULLISH | OpCode::BREAK | OpCode::CONTINUE => {
            write!(f, " r{rd}, +{} (rel)", opcode::offset16(instr))
        }
        // imm16 常量池/下标族。
        OpCode::LOAD_CONST | OpCode::LOAD_GLOBAL | OpCode::LOAD_GLOBAL_TYPEOF => {
            write!(f, " r{rd}, const[{}]", opcode::imm16(instr))
        }
        OpCode::LOAD_UPVALUE => write!(f, " r{rd}, uv[{}]", opcode::imm16(instr)),
        OpCode::CREATE_CLOSURE => write!(f, " r{rd}, module[{}]", opcode::imm16(instr)),
        OpCode::STORE_UPVALUE => write!(f, " r{a}, uv[{b}]"),
        // 单 ext 字族。
        OpCode::SPILL | OpCode::UNSPILL => write!(f, " r{rd}, slot[{}]", ext(0)),
        OpCode::CALL | OpCode::CALL_NATIVE | OpCode::NEW_EXPRESSION => {
            write!(f, " r{rd}, r{a}, r{b}, nargs={}", ext(0) & 0xFF)
        }
        OpCode::SUPER_CALL => write!(f, " r{rd}, r{a}, nargs={}", ext(0) & 0xFF),
        OpCode::DEFINE_ACCESSOR => write!(f, " r{rd}, r{a}, r{b}, const[{}]", ext(0)),
        OpCode::DEFINE_ACCESSOR_DYNAMIC => write!(f, " r{rd}, r{a}, r{b}, key=r{}", ext(0) & 0x7FFF_FFFF),
        OpCode::DEFINE_PROP_ATTRS => write!(f, " r{rd}, r{a}, r{b}, attrs={:#x}", ext(0)),
        OpCode::DEFINE_GLOBAL_PROP_C | OpCode::DEFINE_GLOBAL_PROP_C_IF_ABSENT => {
            write!(f, " r{a}, const[{}]", ext(0))
        }
        OpCode::DELETE_GLOBAL_PROP_C => write!(f, " r{rd}, r{a}, const[{}]", ext(0)),
        OpCode::DELETE_PROP_STATIC => write!(f, " r{rd}, r{a}, const[{}]", ext(0)),
        OpCode::REST_OBJECT => write!(f, " r{rd}, r{a}, r{b}, excl=const[{}]", ext(0)),
        OpCode::INIT_PRIVATE => write!(f, " r{rd}, r{a}, r{b}, method={}", ext(0)),
        // 双 ext 字族。
        OpCode::DEFINE_ACCESSOR_ATTRS => {
            write!(f, " r{rd}, r{a}, r{b}, const[{}], attrs={:#x}", ext(0), ext(1))
        }
        OpCode::DEFINE_ACCESSOR_ATTRS_DYNAMIC => {
            write!(f, " r{rd}, r{a}, r{b}, key=r{}, attrs={:#x}", ext(0) & 0x7FFF_FFFF, ext(1))
        }
        OpCode::GET_PRIVATE | OpCode::SET_PRIVATE | OpCode::PRIVATE_BRAND_IN => {
            write!(f, " r{rd}, r{a}, r{b}, brand=r{}, id={}", ext(0), ext(1))
        }
        // IC 族：8 个多态槽扩展字。
        _ if op.has_ic_ext_words() => write!(f, " r{rd}, r{a}, r{b}, ic x{}", opcode::IC_EXT_WORDS),
        // 变长 ext 族。
        OpCode::CALL_SPREAD | OpCode::NEW_EXPRESSION_SPREAD | OpCode::SUPER_CALL_SPREAD => {
            // 首字 = nstatic | (nspread<<8)；实参字：静态 = 寄存器号，spread 源带高位标记。
            let header = ext(0);
            let n = ((header & 0xFF) + ((header >> 8) & 0xFF)) as usize;
            write!(f, " r{rd}, r{a}, r{b}, args=[")?;
            for i in 0..n {
                if i > 0 {
                    write!(f, ", ")?;
                }
                let w = ext(i + 1);
                if w & 0x8000_0000 != 0 {
                    write!(f, "...r{}", w & 0x7FFF_FFFF)?;
                } else {
                    write!(f, "r{}", w)?;
                }
            }
            write!(f, "]")
        }
        OpCode::TEMPLATE_STR => {
            // 首字 = (segment_count<<16) | len_hint；段字：高位标记 = 表达式寄存器，
            // 否则常量池下标。
            let header = ext(0);
            let n = ((header >> 16) & 0xFFFF) as usize;
            write!(f, " r{rd}, segs=[")?;
            for i in 0..n {
                if i > 0 {
                    write!(f, ", ")?;
                }
                let w = ext(i + 1);
                if w & 0x8000_0000 != 0 {
                    write!(f, "r{}", w & 0x7FFF_FFFF)?;
                } else {
                    write!(f, "c{}", w & 0x7FFF_FFFF)?;
                }
            }
            write!(f, "]")
        }
        OpCode::GET_TEMPLATE_OBJECT => {
            // ext[0]=quasis 段数 n，随后 2n 个交错 cooked/raw 字，末尾 site 序号。
            let n = ext(0) as usize;
            write!(f, " r{rd}, {n} quasis, site={}", ext(1 + 2 * n))
        }
        OpCode::CONCAT_N => {
            // ext[0]=n=操作数总数，其余 n-1 字为操作数寄存器。
            let n = ext(0) as usize;
            write!(f, " r{rd}, r{a}, ops=[")?;
            for i in 0..n.saturating_sub(1) {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "r{}", ext(i + 1) & 0x7FFF_FFFF)?;
            }
            write!(f, "]")
        }
        OpCode::NEW_OBJECT => {
            // a 槽 = 属性数，ext = 每键常量池下标。
            let n = a as usize;
            write!(f, " r{rd}, {n} keys [")?;
            for i in 0..n {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "const[{}]", ext(i))?;
            }
            write!(f, "]")
        }
        // 单操作数族。
        OpCode::NEG => write!(f, " r{rd}, r{a}"),
        OpCode::RETURN | OpCode::HALT | OpCode::NOP => write!(f, " r{rd}"),
        _ => write!(f, " r{rd}, r{a}, r{b}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造反汇编夹具：顶层 5 条指令（NEW_OBJECT 1 字键表、CALL 1 ext 字、
    /// IC_GET_PROP 8 ext 字），子模块 1 个（3 条指令，RETURN 带 1 个逃出计数 ext 字）。
    fn fixture() -> CompiledModule {
        let sub = CompiledModule {
            function_name: Some("f".into()),
            flat_id: 1,
            n_registers: 2,
            constants: vec![Constant::Int(42)],
            bytecode: Arc::from([
                opcode::encode(OpCode::LOAD_CONST, 1, 0, 0),
                opcode::encode(OpCode::RETURN, 1, 0, 0),
                0, // RETURN 逃出计数 ext 字
                opcode::encode(OpCode::HALT, 0, 0, 0),
            ]),
            ..Default::default()
        };
        let mut m = CompiledModule::new();
        m.n_registers = 4;
        m.constants = vec![Constant::Int(1), Constant::String("a".into())];
        m.bytecode = Arc::from([
            opcode::encode(OpCode::LOAD_CONST, 1, 0, 0),
            opcode::encode(OpCode::NEW_OBJECT, 2, 1, 0),
            1, // NEW_OBJECT 键表：const[1]
            opcode::encode(OpCode::CALL, 3, 0, 1),
            1, // CALL ext：nargs=1
            opcode::encode(OpCode::IC_GET_PROP, 0, 2, 3),
            0xAAAA_AAAA,
            0xBBBB_BBBB,
            0xCCCC_CCCC,
            0xDDDD_DDDD,
            0xEEEE_EEEE,
            0xFFFF_FFFF,
            0x1111_1111,
            0x2222_2222,
            opcode::encode(OpCode::HALT, 0, 0, 0),
        ]);
        m.sub_modules = vec![Arc::new(sub)];
        m
    }

    #[test]
    fn disassembly_ext_words_merge_into_instruction_lines() {
        let text = fixture().to_string();
        let lines: Vec<&str> = text.lines().collect();
        // 指令行数 = 真实指令数（顶层 5 + 子模块 3），ext 字不单独成行。
        let instr_lines = lines
            .iter()
            .filter(|l| l.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit()));
        assert_eq!(instr_lines.count(), 8, "指令行数 = 真实指令数\n{text}");
        // 逐行形态：offset 列 = 字序号（与 VM pc 同单位），操作数按语义渲染。
        let expected = [
            "  0000  LOAD_CONST r1, const[0]",
            "  0001  NEW_OBJECT r2, 1 keys [const[1]]",
            "  0003  CALL r3, r0, r1, nargs=1",
            "  0005  IC_GET_PROP r0, r2, r3, ic x8",
            "  0014  HALT r0",
            "    0000  LOAD_CONST r1, const[0]",
            "    0001  RETURN r1",
            "    0003  HALT r0",
        ];
        for line in &expected {
            assert!(lines.contains(line), "missing line: {line}\n{text}");
        }
        // 子模块头：序号 + flat_id + 函数名。
        assert!(lines.contains(&"; sub_module[0] (flat_id=1, \"f\"):"));
    }

    #[test]
    fn disassembly_imm16_and_ext_operand_rendering() {
        let mut m = CompiledModule::new();
        m.constants = vec![Constant::String("x".into())];
        m.bytecode = Arc::from([
            opcode::encode(OpCode::LOAD_GLOBAL, 1, 0, 0),
            opcode::encode(OpCode::CREATE_CLOSURE, 2, 0, 0),
            opcode::encode(OpCode::DEFINE_GLOBAL_PROP_C, 0, 1, 0),
            3, // ext：键常量池下标
            opcode::encode(OpCode::JMP_IF_FALSE, 1, 12, 0),
        ]);
        let text = m.to_string();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.contains(&"  0000  LOAD_GLOBAL r1, const[0]"));
        assert!(lines.contains(&"  0001  CREATE_CLOSURE r2, module[0]"));
        assert!(lines.contains(&"  0002  DEFINE_GLOBAL_PROP_C r1, const[3]"));
        assert!(lines.contains(&"  0004  JMP_IF_FALSE r1, +12 (rel)"));
    }
}
