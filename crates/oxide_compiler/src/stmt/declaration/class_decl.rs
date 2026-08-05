use crate::compiler::{CompileCtx, Compiler};
use crate::ir::inst::Inst;
use crate::ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{Statement, VariableDeclarationKind};

impl Compiler {
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
        ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(ctor_reg as u32), Operand::None));
        Ok(None)
    }
}
