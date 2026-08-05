use crate::compiler::{CompileCtx, Compiler};
use crate::ir::inst::Inst;
use crate::ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;

impl Compiler {
    pub(crate) fn emit_identifier_expression(
        &self, ident: &oxide_parser::IdentifierReference, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let name = ident.name.as_str();

        for (uv_idx, up) in ctx.current_upvalue_captures.iter().enumerate() {
            if up.name == name {
                let r = ctx.alloc_reg();
                let idx = uv_idx as u8;
                ctx.inst(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(r as u32), Operand::Reg(idx as u32), Operand::None));
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
                ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(r as u32), Operand::Reg(binding.reg as u32), Operand::Reg(cell_idx as u32)));
            } else {
                ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(r as u32), Operand::None, Operand::Reg(cell_idx as u32)));
            }
            return Ok(r);
        }

        let var_reg = ctx.lookup_or_builtin(name)?;
        let r = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r as u32), Operand::Reg(var_reg as u32), Operand::None));
        Ok(r)
    }
}
