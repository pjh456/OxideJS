//! 对象字面量 emit：`emit_object_expression` 逐属性定义（含 getter/setter/展开）。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ObjectPropertyKind, PropertyKind};

impl Emitter {
    pub(crate) fn emit_object_expression(
        &self, obj: &oxide_parser::ObjectExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let obj_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_OBJECT, Operand::Reg(obj_reg), Operand::None, Operand::None));
        for prop in &obj.properties {
            let ObjectPropertyKind::SpreadProperty(spread) = prop else {
                self.emit_object_property(obj_reg, prop, ctx)?;
                continue;
            };
            // spread 展开：把源表达式的可枚举自有属性写入目标对象（原地改）。
            // 顺序语义：{...b, a:1} 在 spread 之后定义 a，后者覆盖前者（从左到右求值）。
            let src_reg = self.emit_expression(&spread.argument, ctx)?;
            ctx.inst(Inst::spread_object(Operand::Reg(obj_reg), Operand::Reg(src_reg)));
        }
        Ok(obj_reg)
    }

    fn emit_object_property(
        &self, obj_reg: u32, prop: &oxide_parser::ObjectPropertyKind, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let ObjectPropertyKind::ObjectProperty(p) = prop else {
            return Err("unsupported object property kind".into());
        };
        let computed = p.computed;
        let prop_name = if computed {
            "<computed>".to_string()
        } else {
            self.class_property_name(&p.key)?
        };
        match p.kind {
            PropertyKind::Get | PropertyKind::Set => {
                // 计算键：键表达式先于访问器函数求值（规范求值序），键值运行时
                // 由 DEFINE_ACCESSOR_DYNAMIC 从 key_reg 读取。
                let key_reg = if computed {
                    Some(self.emit_expression(p.key.to_expression(), ctx)?)
                } else {
                    None
                };
                let accessor_reg = self.emit_expression(&p.value, ctx)?;
                if !computed {
                    if let Some(sub_mod) = ctx.nested.last_mut() {
                        // 访问器函数名带 "get "/"set " 前缀（SetFunctionName 语义）。
                        // 计算键的键值运行时才知，静态无法定名，保持匿名。
                        let fn_name = match p.kind {
                            PropertyKind::Get => format!("get {prop_name}"),
                            PropertyKind::Set => format!("set {prop_name}"),
                            _ => prop_name.clone(),
                        };
                        sub_mod.function_name = Some(fn_name);
                    }
                }
                let undef_reg = self.emit_undefined(ctx);
                let (get_reg, set_reg) = if p.kind == PropertyKind::Get {
                    (accessor_reg, undef_reg)
                } else {
                    (undef_reg, accessor_reg)
                };
                match key_reg {
                    Some(key_reg) => {
                        ctx.inst(Inst::define_accessor_dynamic(
                            Operand::Reg(obj_reg),
                            Operand::Reg(get_reg),
                            Operand::Reg(set_reg),
                            key_reg,
                        ));
                    }
                    None => {
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        ctx.inst(Inst::define_accessor(
                            Operand::Reg(obj_reg),
                            Operand::Reg(get_reg),
                            Operand::Reg(set_reg),
                            idx as u32,
                        ));
                    }
                }
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
                if crate::is_anonymous_function_definition(&p.value) {
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
                ctx.inst(Inst::new(op, Operand::Reg(obj_reg), operands.0, operands.1));
            }
        }
        Ok(())
    }
}
