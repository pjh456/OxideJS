//! 类声明语句 emit：`emit_class_declaration_statement` 声明类名并初始化类对象。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_class_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ClassDeclaration(class) = stmt else {
            return Err("ClassDeclaration without name".into());
        };
        let name = class
            .id
            .as_ref()
            .map(|id| id.name.to_string())
            .ok_or_else(|| "ClassDeclaration without name".to_string())?;
        let var_reg = ctx.alloc_reg();
        ctx.declare(&name, var_reg, VariableDeclarationKind::Let, false)?;
        ctx.init_var(&name);
        let ctor_reg = self.emit_class(class, ctx)?;
        if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
            ctx.inst(Inst::new(OpCode::MAKE_CELL, Operand::Reg(var_reg as u32), Operand::Imm(cell_idx as u16), Operand::None));
        } else {
            ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(ctor_reg as u32), Operand::None));
        }
        Ok(None)
    }
}
