use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode::{self, OpCode};

impl Compiler {
    pub(crate) fn emit_identifier_expression(
        &self, ident: &oxide_parser::IdentifierReference, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let name = ident.name.as_str();

        for (uv_idx, up) in ctx.current_upvalue_captures.iter().enumerate() {
            if up.name == name {
                let r = ctx.alloc_reg();
                let idx = uv_idx as u8;
                ctx.emit(opcode::encode(OpCode::LOAD_UPVALUE, r, idx, 0));
                return Ok(r);
            }
        }

        if ctx.scopes.symbols.lookup_is_captured(name) {
            let cell_idx = ctx
                .scopes
                .cell_registry
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, idx)| *idx)
                .unwrap_or(0);
            let r = ctx.alloc_reg();
            if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                ctx.emit(opcode::encode(OpCode::CELL_GET, r, binding.reg, cell_idx));
            } else {
                ctx.emit(opcode::encode(OpCode::CELL_GET, r, 0, cell_idx));
            }
            return Ok(r);
        }

        let var_reg = ctx.lookup_or_builtin(name)?;
        let r = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, var_reg, 0));
        Ok(r)
    }
}
