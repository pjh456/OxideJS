use crate::compiler::{CompileCtx, Compiler, ParamSpec};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn count_function_declaration(&self, decl: &oxide_parser::Function<'_>, ctx: &mut CompileCtx) {
        let name = if let Some(id) = &decl.id {
            id.name.to_string()
        } else {
            return;
        };
        let func_reg = ctx.alloc_reg();
        let _ = ctx.declare_initialized(&name, func_reg, VariableDeclarationKind::Var, false);
        ctx.count_words(2);
    }

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
        ctx.sub_modules.push(sub_module);
        let sub_idx = ctx.sub_modules.len() as u32;
        let var_reg = ctx.lookup(&name)?;
        ctx.reserve_reg(var_reg);
        ctx.emit_create_closure(var_reg, sub_idx);
        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, var_reg, 0));
        Ok(None)
    }
}
