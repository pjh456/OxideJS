use super::*;

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
            ctx.emit_load_const(key_reg, idx);
            let this_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, this_reg, 254, 0));
            let result_reg = ctx.alloc_reg();
            let op = if ctx.in_static_method {
                OpCode::SUPER_STATIC_GET_PROP
            } else {
                OpCode::SUPER_GET_PROP
            };
            ctx.emit(opcode::encode(op, result_reg, this_reg, key_reg));
            return Ok(result_reg);
        }
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let prop_name = member.property.name.as_str();
        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_CONST, key_reg, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8));
        ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, obj_reg, key_reg));
        ctx.emit(0);
        ctx.emit(0);
        ctx.emit(0);
        Ok(obj_reg)
    }

    fn emit_computed_member_expression(
        &self, member: &oxide_parser::ComputedMemberExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let key_reg = self.emit_expression(&member.expression, ctx)?;
        let r = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::GET_PROP_DYNAMIC, obj_reg, key_reg, r));
        Ok(r)
    }

    fn emit_private_field_expression(
        &self, member: &oxide_parser::PrivateFieldExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
        let r = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::GET_PRIVATE, r, obj_reg, key_reg));
        Ok(r)
    }

    fn emit_chain_expression(&self, chain: &oxide_parser::ChainExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
        let id = ctx.next_label_id();
        let short_label = Label::TernaryElse(id);
        let end_label = Label::TernaryEnd(id);
        let value_reg = self.emit_chain_element(&chain.expression, Some(short_label), ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, value_reg, 0));
        let end_pos = ctx.resolve_label(end_label)?;
        let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));
        let undefined_idx = ctx.add_constant(Constant::Undefined);
        ctx.emit_load_const(result_reg, undefined_idx);
        Ok(result_reg)
    }

    pub(in crate::emitter) fn emit_member_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::StaticMemberExpression(member) => self.emit_static_member_expression(member, ctx),
            Expression::ComputedMemberExpression(member) => self.emit_computed_member_expression(member, ctx),
            Expression::PrivateFieldExpression(member) => self.emit_private_field_expression(member, ctx),
            Expression::ChainExpression(chain) => self.emit_chain_expression(chain, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
