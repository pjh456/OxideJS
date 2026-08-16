//! `new.target` / `import.meta` 元属性表达式 emit。
//!
//! - `new.target` → `LOAD_VAR` 读物理寄存器 255（VM 帧 `saved_new_target`，构造时传
//!   constructor、普通调用传 undefined）。
//! - `import.meta` 需模块命名空间对象，当前未支持：显式报错分类 Skip，不留静默错误。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::MetaProperty;

impl Emitter {
    /// 元属性表达式 emit：`new.target` 读当前构造目标寄存器，`import.meta` 显式报错。
    ///
    /// # 边界与前提
    /// - `new.target` 语法上仅出现在函数体内，顶层用法由 oxc 早期错误拦截。
    /// - 非 `new.target` 的元属性（`import.meta`）返回编译错误，错误文本含
    ///   "not supported" 供 test262 分类为 Skip。
    ///
    /// # 副作用
    /// - 分配一个结果寄存器（`ctx.alloc_reg`）。
    pub(crate) fn emit_meta_property_expression(&self, mp: &MetaProperty, ctx: &mut CompileCtx) -> Result<u32, String> {
        if mp.meta.name == "new" && mp.property.name == "target" {
            let r = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r), Operand::NewTarget, Operand::None));
            return Ok(r);
        }

        // import.meta 需要模块命名空间对象，未支持：显式报错避免静默错误结果。
        Err("import.meta not yet supported".into())
    }
}
