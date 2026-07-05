use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::{
    module::Constant,
    opcode::{self, OpCode},
};
use oxide_parser::{Expression, ObjectPropertyKind, PropertyKey, PropertyKind};

impl Compiler {
    pub(crate) fn count_object_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ObjectExpression(obj) = expr else {
            return;
        };
        ctx.alloc_reg();
        ctx.projected_pc += 1;
        let prop_checkpoint = ctx.reg_checkpoint();
        for prop in &obj.properties {
            if let ObjectPropertyKind::ObjectProperty(p) = prop {
                if matches!(p.kind, PropertyKind::Get | PropertyKind::Set) {
                    self.count_expression(&p.value, ctx);
                    ctx.count_load_const();
                    ctx.count_define_accessor();
                } else {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                    self.count_expression(&p.value, ctx);
                    ctx.projected_pc += 1;
                }
                ctx.restore_reg_checkpoint(prop_checkpoint);
            }
        }
    }

    pub(crate) fn emit_object_expression(
        &self, obj: &oxide_parser::ObjectExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::NEW_OBJECT, obj_reg, 0, 0));
        let prop_checkpoint = ctx.reg_checkpoint();
        for prop in &obj.properties {
            let ObjectPropertyKind::ObjectProperty(p) = prop else {
                return Err("spread properties not yet supported".into());
            };
            let prop_name = match &p.key {
                PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                PropertyKey::StringLiteral(s) => s.value.to_string(),
                _ => return Err("unsupported object property key type".into()),
            };
            match p.kind {
                PropertyKind::Get | PropertyKind::Set => {
                    let accessor_reg = self.emit_expression(&p.value, ctx)?;
                    if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                        sub_mod.function_name = Some(prop_name.to_string());
                    }
                    let undef_reg = self.emit_undefined(ctx);
                    let (get_reg, set_reg) = if p.kind == PropertyKind::Get {
                        (accessor_reg, undef_reg)
                    } else {
                        (undef_reg, accessor_reg)
                    };
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, obj_reg, get_reg, set_reg));
                    ctx.emit(idx as u32);
                    ctx.restore_reg_checkpoint(prop_checkpoint);
                }
                _ => {
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit_load_const(key_reg, idx);
                    let val_reg = self.emit_expression(&p.value, ctx)?;
                    if matches!(&p.value, Expression::ArrowFunctionExpression(_)) {
                        if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                            sub_mod.function_name = Some(prop_name.to_string());
                        }
                    }
                    ctx.emit(opcode::encode(OpCode::SET_PROP, obj_reg, val_reg, key_reg));
                    ctx.restore_reg_checkpoint(prop_checkpoint);
                }
            }
        }
        Ok(obj_reg)
    }
}
