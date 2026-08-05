use crate::{CompileCtx, Emitter, ParamSpec};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_function_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::FunctionDeclaration(fd) = stmt else {
            return Err("FunctionDeclaration without name".into());
        };
        let name = if let Some(id) = &fd.id {
            id.name.to_string()
        } else {
            return Err("FunctionDeclaration without name".into());
        };
        let mut param_names = Vec::new();
        for (idx, param) in fd.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_names.push(ParamSpec::Identifier(bi.name.to_string()))
                }
                pattern => param_names.push(ParamSpec::Pattern {
                    synthetic_name: format!("@@param_{idx}"),
                    pattern,
                }),
            }
        }
        let body_stmts: &[Statement] = if let Some(body) = &fd.body { &body.statements } else { &[] };
        let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
        sub_module.function_name = Some(name.clone());
        ctx.nested.push(sub_module);
        let var_reg = ctx.lookup(&name)?;
        ctx.reserve_reg(var_reg);
        ctx.inst(Inst::create_closure(Operand::Reg(var_reg as u32), ctx.nested.len() as u16));
        if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
            ctx.inst(Inst::new(OpCode::MAKE_CELL, Operand::Reg(var_reg as u32), Operand::Imm(cell_idx as u16), Operand::None));
        } else {
            ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(var_reg as u32), Operand::None));
        }
        Ok(None)
    }
}
