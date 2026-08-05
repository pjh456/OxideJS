use crate::compiler::{CompileCtx, Compiler};
use crate::ir::inst::Inst;
use crate::ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::Expression;

impl Compiler {
    fn emit_static_member_expression(
        &self, member: &oxide_parser::StaticMemberExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        if matches!(&member.object, Expression::Super(_)) {
            if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                return Err("super property only supported in class methods".into());
            }
            let prop_name = member.property.name.as_str();
            let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg as u32), idx));
            let this_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(this_reg as u32), Operand::This, Operand::None));
            let result_reg = ctx.alloc_reg();
            let op = if ctx.in_static_method {
                OpCode::SUPER_STATIC_GET_PROP
            } else {
                OpCode::SUPER_GET_PROP
            };
            ctx.inst(Inst::new(op, Operand::Reg(result_reg as u32), Operand::Reg(this_reg as u32), Operand::Reg(key_reg as u32)));
            return Ok(result_reg);
        }
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let prop_name = member.property.name.as_str();
        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg as u32), idx));
        ctx.inst(Inst::ic_get(Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32)));
        Ok(obj_reg)
    }

    fn emit_computed_member_expression(
        &self, member: &oxide_parser::ComputedMemberExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let key_reg = self.emit_expression(&member.expression, ctx)?;
        let r = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32), Operand::Reg(r as u32)));
        Ok(r)
    }

    fn emit_private_field_expression(
        &self, member: &oxide_parser::PrivateFieldExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
        let r = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::GET_PRIVATE, Operand::Reg(r as u32), Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32)));
        Ok(r)
    }

    fn emit_chain_expression(&self, chain: &oxide_parser::ChainExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
        let short_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let value_reg = self.emit_chain_element(&chain.expression, Some(short_label), ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(value_reg as u32), Operand::None));
        ctx.inst(Inst::jmp(end_label));
        ctx.labels.set_label_pos(short_label, ctx.insts.len());
        let undefined_idx = ctx.add_constant(Constant::Undefined);
        ctx.inst(Inst::load_const(Operand::Reg(result_reg as u32), undefined_idx));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        Ok(result_reg)
    }

    pub(crate) fn emit_member_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::StaticMemberExpression(member) => self.emit_static_member_expression(member, ctx),
            Expression::ComputedMemberExpression(member) => self.emit_computed_member_expression(member, ctx),
            Expression::PrivateFieldExpression(member) => self.emit_private_field_expression(member, ctx),
            Expression::ChainExpression(chain) => self.emit_chain_expression(chain, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
