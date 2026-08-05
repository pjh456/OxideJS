use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{Expression, ObjectPropertyKind, PropertyKey, PropertyKind};

impl Compiler {
    pub(crate) fn emit_object_expression(
        &self, obj: &oxide_parser::ObjectExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_OBJECT, Operand::Reg(obj_reg as u32), Operand::None, Operand::None));
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
                    if let Some(sub_mod) = ctx.nested.last_mut() {
                        sub_mod.function_name = Some(prop_name.to_string());
                    }
                    let undef_reg = self.emit_undefined(ctx);
                    let (get_reg, set_reg) = if p.kind == PropertyKind::Get {
                        (accessor_reg, undef_reg)
                    } else {
                        (undef_reg, accessor_reg)
                    };
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    ctx.inst(Inst::define_accessor(
                        Operand::Reg(obj_reg as u32),
                        Operand::Reg(get_reg as u32),
                        Operand::Reg(set_reg as u32),
                        idx as u32,
                    ));
                    ctx.restore_reg_checkpoint(prop_checkpoint);
                }
                _ => {
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.inst(Inst::load_const(Operand::Reg(key_reg as u32), idx));
                    let val_reg = self.emit_expression(&p.value, ctx)?;
                    if matches!(&p.value, Expression::ArrowFunctionExpression(_)) {
                        if let Some(sub_mod) = ctx.nested.last_mut() {
                            sub_mod.function_name = Some(prop_name.to_string());
                        }
                    }
                    ctx.inst(Inst::new(OpCode::SET_PROP, Operand::Reg(obj_reg as u32), Operand::Reg(val_reg as u32), Operand::Reg(key_reg as u32)));
                    ctx.restore_reg_checkpoint(prop_checkpoint);
                }
            }
        }
        Ok(obj_reg)
    }
}
