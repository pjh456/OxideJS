//! 类字段初始化 emit：公有/私有实例字段与私有方法的实例化初始化。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Expression, MethodDefinitionKind, PropertyKey};

impl Emitter {
    /// 实例字段初始化：define 语义（不触发原型 setter）。
    /// 非计算键用字符串常量；计算键从 `@@field_keys` upvalue 数组按 `key_slot` 取（类定义期求值一次）。
    pub(crate) fn emit_public_field_init(
        &self, target: Operand, key: &PropertyKey, computed: bool, value: Option<&Expression>, key_slot: Option<u8>,
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = if computed {
            self.emit_instance_field_key(key_slot, ctx)?
        } else {
            self.emit_class_key_reg(key, false, ctx)?
        };
        let value_reg = self.emit_field_value(value, ctx)?;
        ctx.inst(Inst::define_prop(target, Operand::Reg(value_reg), Operand::Reg(key_reg)));
        Ok(())
    }

    /// 类定义期求值一次后存入 upvalue 数组的计算键读取：`arr[slot]`。
    fn emit_instance_field_key(&self, key_slot: Option<u8>, ctx: &mut CompileCtx) -> Result<u32, String> {
        let uv = ctx.field_keys_uv.expect("computed instance field without field keys upvalue");
        let arr_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::LOAD_UPVALUE,
            Operand::Reg(arr_reg),
            Operand::Imm(uv as u16),
            Operand::None,
        ));
        let idx_reg = ctx.alloc_reg();
        let idx = ctx.add_constant(Constant::Int(key_slot.unwrap_or(0) as i32));
        ctx.inst(Inst::load_const(Operand::Reg(idx_reg), idx));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::GET_PROP_DYNAMIC,
            Operand::Reg(arr_reg),
            Operand::Reg(idx_reg),
            Operand::Reg(key_reg),
        ));
        Ok(key_reg)
    }

    /// 字段值求值：有值求表达式，无值 define undefined。
    pub(crate) fn emit_field_value(&self, value: Option<&Expression>, ctx: &mut CompileCtx) -> Result<u32, String> {
        if let Some(expr) = value {
            self.emit_expression(expr, ctx)
        } else {
            Ok(self.emit_undefined(ctx))
        }
    }

    pub(crate) fn emit_private_field_init(
        &self, target: Operand, name: &str, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let value_reg = self.emit_field_value(value, ctx)?;
        ctx.inst(Inst::init_private(target, Operand::Reg(value_reg), Operand::Reg(key_reg), false));
        Ok(())
    }

    pub(crate) fn emit_private_method_init(
        &self, target: Operand, method: &oxide_parser::MethodDefinition, home_reg: Operand, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let PropertyKey::PrivateIdentifier(private) = &method.key else {
            return Err("expected private method key".into());
        };
        let name = private.name.as_str();
        let method_reg = self.emit_class_method_function(method, name, home_reg, ctx, &[])?;
        match method.kind {
            MethodDefinitionKind::Method => {
                let key_reg = self.emit_private_id_reg(name, ctx)?;
                ctx.inst(Inst::init_private(target, Operand::Reg(method_reg), Operand::Reg(key_reg), true));
            }
            MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                let id = self.private_name_id(name, ctx)?;
                let key_idx = ctx.add_constant(Constant::Int(id as i32));
                let undef_reg = self.emit_undefined(ctx);
                let (get_reg, set_reg) = if method.kind == MethodDefinitionKind::Get {
                    (method_reg, undef_reg)
                } else {
                    (undef_reg, method_reg)
                };
                ctx.inst(Inst::define_accessor(
                    target,
                    Operand::Reg(get_reg),
                    Operand::Reg(set_reg),
                    key_idx as u32,
                ));
            }
            MethodDefinitionKind::Constructor => return Err("constructor cannot be private".into()),
        }
        Ok(())
    }
}
