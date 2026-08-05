//! 调用表达式 emit：普通/原生/`new`/`super` 调用与调用域分发。
//! 函数：`emit_call_expression`、`emit_call_domain`。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::Expression;

impl Emitter {
    fn emit_call_expression(&self, call: &oxide_parser::CallExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
        if matches!(&call.callee, Expression::Super(_)) {
            if !ctx.in_derived_constructor {
                return Err("super() only supported in derived constructors".into());
            }
            let mut arg_regs = Vec::new();
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    arg_regs.push(self.emit_expression(expr, ctx)?);
                }
            }
            let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
            let result_reg = ctx.alloc_reg();
            ctx.inst(Inst::super_call(
                Operand::Reg(result_reg as u32),
                Operand::Reg(first_arg_reg as u32),
                arg_regs.len() as u8,
            ));
            if let Some(mut field_buffer) = ctx.field_buffer.take() {
                let insert_inst = ctx.insts.len();
                ctx.insts.append(&mut field_buffer.insts);
                for (label, relative) in field_buffer.labels {
                    ctx.labels.set_label_pos(label, insert_inst + relative);
                }
            }
            return Ok(result_reg);
        }
        let (callee_reg, this_reg) = match &call.callee {
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let callee_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::GET_PRIVATE, Operand::Reg(callee_reg as u32), Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32)));
                (callee_reg, obj_reg)
            }
            Expression::StaticMemberExpression(member) => {
                let is_super_member = matches!(&member.object, Expression::Super(_));
                let obj_reg = if is_super_member {
                    let this_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(this_reg as u32), Operand::This, Operand::None));
                    this_reg
                } else {
                    self.emit_expression(&member.object, ctx)?
                };
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg as u32), idx));
                let callee_reg = ctx.alloc_reg();
                if is_super_member {
                    if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                        return Err("super property only supported in class methods".into());
                    }
                    let op = if ctx.in_static_method {
                        OpCode::SUPER_STATIC_GET_PROP
                    } else {
                        OpCode::SUPER_GET_PROP
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(callee_reg as u32), Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32)));
                } else {
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(callee_reg as u32), Operand::Reg(obj_reg as u32), Operand::None));
                    ctx.inst(Inst::ic_get(Operand::Reg(callee_reg as u32), Operand::Reg(key_reg as u32)));
                }
                (callee_reg, obj_reg)
            }
            _ => {
                let callee_reg = self.emit_expression(&call.callee, ctx)?;
                let this_idx = ctx.add_constant(Constant::Undefined);
                let this_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(this_reg as u32), this_idx));
                (callee_reg, this_reg)
            }
        };
        let mut arg_regs = Vec::new();
        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                arg_regs.push(self.emit_expression(expr, ctx)?);
            }
        }
        let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
        let op = match &call.callee {
            Expression::Identifier(ident) if ctx.is_builtin(ident.name.as_str()) => OpCode::CALL_NATIVE,
            _ => OpCode::CALL,
        };
        match op {
            OpCode::CALL => {
                ctx.inst(Inst::call(
                    Operand::Reg(callee_reg as u32),
                    Operand::Reg(this_reg as u32),
                    Operand::Reg(first_arg_reg as u32),
                    arg_regs.len() as u8,
                ));
            }
            OpCode::CALL_NATIVE => {
                ctx.inst(Inst::call_native(
                    Operand::Reg(callee_reg as u32),
                    Operand::Reg(this_reg as u32),
                    Operand::Reg(first_arg_reg as u32),
                    arg_regs.len() as u8,
                ));
            }
            _ => unreachable!(),
        }
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::None, Operand::None));
        Ok(result_reg)
    }

    pub(crate) fn emit_call_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::CallExpression(call) => self.emit_call_expression(call, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
