//! 成员访问域：静态/计算/私有字段成员与可选链表达式 emit 及分发。
//! 函数：`emit_static_member_expression`、`emit_computed_member_expression`、
//! `emit_private_field_expression`、`emit_chain_expression`、`emit_member_domain`。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Expression;

impl Emitter {
    fn emit_static_member_expression(
        &self, member: &oxide_parser::StaticMemberExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        if matches!(&member.object, Expression::Super(_)) {
            if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                return Err("super property only supported in class methods".into());
            }
            let prop_name = member.property.name.as_str();
            let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            let this_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(this_reg), Operand::This, Operand::None));
            let result_reg = ctx.alloc_reg();
            let op = if ctx.in_static_method {
                OpCode::SUPER_STATIC_GET_PROP
            } else {
                OpCode::SUPER_GET_PROP
            };
            ctx.inst(Inst::new(op, Operand::Reg(result_reg), Operand::Reg(this_reg), Operand::Reg(key_reg)));
            return Ok(result_reg);
        }
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let prop_name = member.property.name.as_str();
        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
        ctx.inst(Inst::ic_get(Operand::Reg(obj_reg), Operand::Reg(key_reg)));
        Ok(obj_reg)
    }

    fn emit_computed_member_expression(
        &self, member: &oxide_parser::ComputedMemberExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        // 常量字符串键折叠为 IC 静态路径：免去运行期键 interning 与慢路径 ordinary_get。
        if let Some(key) = computed_const_key(&member.expression) {
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let idx = ctx.add_constant(Constant::String(key));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::ic_get(Operand::Reg(obj_reg), Operand::Reg(key_reg)));
            return Ok(obj_reg);
        }
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let key_reg = self.emit_expression(&member.expression, ctx)?;
        let r = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::GET_PROP_DYNAMIC,
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            Operand::Reg(r),
        ));
        Ok(r)
    }

    fn emit_private_field_expression(
        &self, member: &oxide_parser::PrivateFieldExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let obj_reg = self.emit_expression(&member.object, ctx)?;
        let name = member.field.name.as_str();
        let (brand_reg, brand_id) = self.private_access_brand(obj_reg, name, ctx)?;
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let r = ctx.alloc_reg();
        ctx.inst(Inst::get_private(
            Operand::Reg(r),
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            brand_reg,
            brand_id,
        ));
        Ok(r)
    }

    fn emit_chain_expression(
        &self, chain: &oxide_parser::ChainExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let short_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let value_reg = self.emit_chain_element(&chain.expression, Some(short_label), ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(result_reg),
            Operand::Reg(value_reg),
            Operand::None,
        ));
        ctx.inst(Inst::jmp(end_label));
        ctx.labels.set_label_pos(short_label, ctx.insts.len());
        let undefined_idx = ctx.add_constant(Constant::Undefined);
        ctx.inst(Inst::load_const(Operand::Reg(result_reg), undefined_idx));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        Ok(result_reg)
    }

    pub(crate) fn emit_member_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::StaticMemberExpression(member) => self.emit_static_member_expression(member, ctx),
            Expression::ComputedMemberExpression(member) => self.emit_computed_member_expression(member, ctx),
            Expression::PrivateFieldExpression(member) => self.emit_private_field_expression(member, ctx),
            Expression::ChainExpression(chain) => self.emit_chain_expression(chain, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}

/// 判断字符串是否为规范数组下标键（无前导零的纯数字串，可解析为 u32）。
///
/// 数组元素区独立于 shape 槽：此类键经 `set_or_create_prop_value` 写入元素区，
/// 不进 shape 链，IC 判定 `slot < prop_vec_len` 永不命中——折叠零收益且多付探试开销，
/// 故排除。规则与 VM `array_index_from_property_key` 一致。
pub(crate) fn is_array_index_str(s: &str) -> bool {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) {
        return false;
    }
    s.parse::<u32>().is_ok()
}

/// 提取计算成员表达式的常量字符串键（可折叠为 IC 静态路径）。
///
/// 只接受字符串字面量与无插值模板（单段、无表达式）；数字字面量、含插值模板
/// （运行期非常量 / 键求值有副作用）与规范数组下标串返回 `None`，维持 DYNAMIC 路径。
pub(crate) fn computed_const_key(expr: &Expression) -> Option<String> {
    let key = match expr {
        Expression::StringLiteral(s) => s.value.to_string(),
        Expression::TemplateLiteral(tl) if tl.expressions.is_empty() && tl.quasis.len() == 1 => {
            tl.quasis[0].value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default()
        }
        _ => return None,
    };
    if is_array_index_str(&key) {
        None
    } else {
        Some(key)
    }
}
