//! 类字段初始化 emit：公有/私有实例字段与私有方法的实例化初始化。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Expression, PropertyKey};

impl Emitter {
    pub(crate) fn emit_public_field_init(
        &self, target: Operand, key: &PropertyKey, computed: bool, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_class_key_reg(key, computed, ctx)?;
        let value_reg = if let Some(expr) = value {
            self.emit_expression(expr, ctx)?
        } else {
            self.emit_undefined(ctx)
        };
        if computed {
            ctx.inst(Inst::new(
                OpCode::SET_PROP_DYNAMIC,
                target,
                Operand::Reg(key_reg),
                Operand::Reg(value_reg),
            ));
        } else {
            ctx.inst(Inst::new(OpCode::SET_PROP, target, Operand::Reg(value_reg), Operand::Reg(key_reg)));
        }
        Ok(())
    }

    pub(crate) fn emit_private_field_init(
        &self, target: Operand, name: &str, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let value_reg = if let Some(expr) = value {
            self.emit_expression(expr, ctx)?
        } else {
            self.emit_undefined(ctx)
        };
        ctx.inst(Inst::new(OpCode::INIT_PRIVATE, target, Operand::Reg(value_reg), Operand::Reg(key_reg)));
        Ok(())
    }

    pub(crate) fn emit_private_method_init(
        &self, target: Operand, method: &oxide_parser::MethodDefinition, home_reg: Operand, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let PropertyKey::PrivateIdentifier(private) = &method.key else {
            return Err("expected private method key".into());
        };
        let name = private.name.as_str();
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let method_reg = self.emit_class_method_function(method, name, home_reg, ctx, &[])?;
        ctx.inst(Inst::new(OpCode::INIT_PRIVATE, target, Operand::Reg(method_reg), Operand::Reg(key_reg)));
        Ok(())
    }
}
