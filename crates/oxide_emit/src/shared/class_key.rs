//! 类属性键/私有名 emit：私有名 id 分配、类字段键寄存器与 undefined 常量。
//! 函数：`private_name_id`、`emit_private_id_reg`、`emit_class_key_reg` 等。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::PropertyKey;

impl Emitter {
    pub(crate) fn private_name_id(&self, name: &str, ctx: &CompileCtx) -> Result<u32, String> {
        ctx.scopes
            .private_name_map
            .iter()
            .find_map(|(n, id)| (n == name).then_some(*id))
            .ok_or_else(|| format!("private name #{name} is not defined"))
    }

    pub(crate) fn emit_private_id_reg(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        let id = self.private_name_id(name, ctx)?;
        let idx = ctx.add_constant(Constant::Int(id as i32));
        let reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
        Ok(reg)
    }

    /// 私有成员访问的 brand 编码：返回 `(brand_reg, brand_id)`，供 GET_PRIVATE/SET_PRIVATE
    /// dispatch 做 brand 检查。
    ///
    /// instance 字段走 PrivateFieldFind 原型链查找（允许 `Object.create` 链穿透），
    /// 不检查 brand（返回 `(0,0)`）；方法/访问器/静态字段须验证接收者属于当前类
    /// （brand 对象同一性）。brand 对象由方法函数捕获的 `@@class_brand` upvalue 提供；
    /// 嵌套函数未捕获该 upvalue 时跳过检查（保持语法合法，brand 语义受限）。
    pub(crate) fn private_access_brand(
        &self, _obj_reg: u32, name: &str, ctx: &mut CompileCtx,
    ) -> Result<(u32, u32), String> {
        let Some(brand_id) = ctx.scopes.private_brand_id else { return Ok((0, 0)) };
        let Some((_, kind, is_static)) = ctx.scopes.private_element_kinds.iter().find(|(n, _, _)| n == name) else {
            return Ok((0, 0));
        };
        if kind.is_none() && !is_static {
            return Ok((0, 0));
        }
        let Some(uv_idx) = ctx.current_upvalue_captures.iter().position(|u| u.name == "@@class_brand") else {
            return Ok((0, 0));
        };
        let brand_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::LOAD_UPVALUE,
            Operand::Reg(brand_reg),
            Operand::Imm(uv_idx as u16),
            Operand::None,
        ));
        Ok((brand_reg, brand_id))
    }

    pub(crate) fn emit_class_key_reg(
        &self, key: &PropertyKey, computed: bool, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        if !computed {
            let name = self.class_property_name(key)?;
            let idx = ctx.add_constant(Constant::String(name));
            let reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
            return Ok(reg);
        }

        if matches!(key, PropertyKey::PrivateIdentifier(_)) {
            return Err("private class elements not yet supported".into());
        }
        self.emit_expression(key.to_expression(), ctx)
    }

    pub(crate) fn static_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::Identifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::NumericLiteral(n) => Ok(n.value.to_string()),
            _ => Err("computed destructuring keys not yet supported".into()),
        }
    }

    /// 从类定义期 computed key 数组按 slot 取键（方法/静态字段在类定义期使用）。
    pub(crate) fn emit_class_array_key_reg(&self, slot: u8, ctx: &mut CompileCtx) -> Result<u32, String> {
        let arr_reg = ctx.class_keys_reg.expect("class computed keys array missing");
        let idx_reg = ctx.alloc_reg();
        let idx = ctx.add_constant(Constant::Int(slot as i32));
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

    pub(crate) fn class_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::Identifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::NumericLiteral(n) => Ok(n.value.to_string()),
            PropertyKey::PrivateIdentifier(_) => Err("private class elements not yet supported".into()),
            _ => Err("unsupported class property key type".into()),
        }
    }

    pub(crate) fn emit_undefined(&self, ctx: &mut CompileCtx) -> u32 {
        let idx = ctx.add_constant(Constant::Undefined);
        let reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
        reg
    }
}
