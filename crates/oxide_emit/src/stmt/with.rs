//! with 语句 emit：求值对象压入 with 作用域栈，动态解析体内自由标识符。
//!
//! with 自身不创建声明式作用域；body 为块时由块语句建立块作用域，
//! 块内 let/const/函数声明优先于 with 对象属性（对象环境在块环境外层）。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Statement;

impl Emitter {
    fn emit_with_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::WithStatement(with) = stmt else {
            return Ok(None);
        };

        // 求值 with 对象并 ToObject（对象环境记录要求）；压入动态作用域栈，
        // body 编译完成后弹出。
        let obj_reg = self.emit_expression(&with.object, ctx)?;
        ctx.inst(Inst::new(OpCode::TO_OBJECT, Operand::Reg(obj_reg), Operand::None, Operand::None));
        ctx.push_with(obj_reg);
        let result = self.emit_statement(&with.body, ctx)?;
        ctx.pop_with();
        Ok(result)
    }

    pub(crate) fn emit_with_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        match stmt {
            Statement::WithStatement(_) => self.emit_with_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
