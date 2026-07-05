use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn count_class_declaration(&self, class: &oxide_parser::Class<'_>, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        self.count_class(class, ctx);
        ctx.projected_pc += 1;
    }

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
        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, ctor_reg, 0));
        Ok(None)
    }
}
