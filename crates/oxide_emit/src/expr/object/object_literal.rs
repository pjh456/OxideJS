//! 对象字面量 emit：`emit_object_expression` 逐属性定义（含 getter/setter/展开）。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Expression, ObjectPropertyKind, PropertyKind};

impl Emitter {
    pub(crate) fn emit_object_expression(
        &self, obj: &oxide_parser::ObjectExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let obj_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_OBJECT, Operand::Reg(obj_reg), Operand::None, Operand::None));
        for prop in &obj.properties {
            let ObjectPropertyKind::ObjectProperty(p) = prop else {
                return Err("spread properties not yet supported".into());
            };
            let computed = p.computed;
            let prop_name = if computed {
                "<computed>".to_string()
            } else {
                self.class_property_name(&p.key)?
            };
            match p.kind {
                PropertyKind::Get | PropertyKind::Set => {
                    if computed {
                        return Err("computed object accessors not yet supported".into());
                    }
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
                        Operand::Reg(obj_reg),
                        Operand::Reg(get_reg),
                        Operand::Reg(set_reg),
                        idx as u32,
                    ));
                }
                _ => {
                    let key_reg = if computed {
                        self.emit_expression(p.key.to_expression(), ctx)?
                    } else {
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let reg = ctx.alloc_reg();
                        ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
                        reg
                    };
                    let val_reg = self.emit_expression(&p.value, ctx)?;
                    if matches!(
                        &p.value,
                        Expression::ArrowFunctionExpression(_)
                            | Expression::FunctionExpression(_)
                            | Expression::ClassExpression(_)
                    ) {
                        if let Some(sub_mod) = ctx.nested.last_mut() {
                            sub_mod.function_name = Some(prop_name.to_string());
                        }
                    }
                    let op = if computed { OpCode::SET_PROP_DYNAMIC } else { OpCode::SET_PROP };
                    let operands = if computed {
                        (Operand::Reg(key_reg), Operand::Reg(val_reg))
                    } else {
                        (Operand::Reg(val_reg), Operand::Reg(key_reg))
                    };
                    ctx.inst(Inst::new(
                        op,
                        Operand::Reg(obj_reg),
                        operands.0,
                        operands.1,
                    ));
                }
            }
        }
        Ok(obj_reg)
    }
}
