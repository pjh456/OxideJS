//! 标识符表达式 emit：`emit_identifier_expression` 按绑定/builtin/全局解析寄存器。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;

impl Emitter {
    pub(crate) fn emit_identifier_expression(
        &self, ident: &oxide_parser::IdentifierReference, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let name = ident.name.as_str();

        for (uv_idx, up) in ctx.current_upvalue_captures.iter().enumerate() {
            if up.name == name {
                let r = ctx.alloc_reg();
                let idx = uv_idx as u8;
                ctx.inst(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(r), Operand::Reg(idx as u32), Operand::None));
                return Ok(r);
            }
        }

        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            let r = ctx.alloc_reg();
            if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(r), Operand::Reg(binding.reg), Operand::Imm(cell_idx as u16)));
            } else {
                ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(r), Operand::None, Operand::Imm(cell_idx as u16)));
            }
            return Ok(r);
        }

        let var_reg = ctx.lookup_or_builtin(name)?;
        let r = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r), Operand::Reg(var_reg), Operand::None));
        Ok(r)
    }
}
