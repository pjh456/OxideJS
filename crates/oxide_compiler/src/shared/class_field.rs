use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{Expression, PropertyKey};

impl Compiler {
    pub(crate) fn emit_public_field_init(
        &self, target_reg: u8, key: &PropertyKey, computed: bool, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_class_key_reg(key, computed, ctx)?;
        let value_reg = if let Some(expr) = value {
            self.emit_expression(expr, ctx)?
        } else {
            self.emit_undefined(ctx)
        };
        if computed {
            ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, target_reg, key_reg, value_reg));
        } else {
            ctx.emit(opcode::encode(OpCode::SET_PROP, target_reg, value_reg, key_reg));
        }
        Ok(())
    }

    pub(crate) fn emit_private_field_init(
        &self, target_reg: u8, name: &str, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let value_reg = if let Some(expr) = value {
            self.emit_expression(expr, ctx)?
        } else {
            self.emit_undefined(ctx)
        };
        ctx.emit(opcode::encode(OpCode::INIT_PRIVATE, target_reg, value_reg, key_reg));
        Ok(())
    }

    pub(crate) fn emit_private_method_init(
        &self, target_reg: u8, method: &oxide_parser::MethodDefinition, home_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let PropertyKey::PrivateIdentifier(private) = &method.key else {
            return Err("expected private method key".into());
        };
        let name = private.name.as_str();
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let method_reg = self.emit_class_method_function(method, name, home_reg, ctx, &[])?;
        ctx.emit(opcode::encode(OpCode::INIT_PRIVATE, target_reg, method_reg, key_reg));
        Ok(())
    }
}
