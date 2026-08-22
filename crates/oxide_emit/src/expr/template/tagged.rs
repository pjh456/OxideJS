//! 标签模板表达式 emit：`emit_tagged_template_expression` 按 GetTemplateObject 语义
//! 经 GET_TEMPLATE_OBJECT 指令取（缓存）模板对象并调用标签函数。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Expression;
impl Emitter {
    pub(crate) fn emit_tagged_template_expression(
        &self, tt: &oxide_parser::TaggedTemplateExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        // 标签求值 + receiver 绑定：成员表达式标签保留对象作 this（与 call.rs 同款
        // 约定），其余标签 this = undefined。
        let (tag_reg, this_reg) = self.emit_tagged_tag(&tt.tag, ctx)?;

        // 模板对象：GetTemplateObject 语义（cooked/raw 数组 + raw 属性，冻结 +
        // 按 site 缓存）由 GET_TEMPLATE_OBJECT 指令在运行时统一构建/复用。
        // cooked 为 None（非法转义）→ 元素 undefined；raw 恒为字符串。
        let mut cooked_words: Vec<u32> = Vec::with_capacity(tt.quasi.quasis.len());
        let mut raw_idxs: Vec<u16> = Vec::with_capacity(tt.quasi.quasis.len());
        for quasi in &tt.quasi.quasis {
            match quasi.value.cooked.as_ref() {
                Some(c) => {
                    let idx = ctx.add_constant(Constant::String(c.to_string()));
                    cooked_words.push(idx as u32);
                }
                None => cooked_words.push(0x8000_0000),
            }
            let raw_idx = ctx.add_constant(Constant::String(quasi.value.raw.to_string()));
            raw_idxs.push(raw_idx);
        }
        let site_no = ctx.next_template_site;
        ctx.next_template_site += 1;
        let template_reg = ctx.alloc_reg();
        ctx.inst(Inst::get_template_object(
            Operand::Reg(template_reg),
            site_no,
            &cooked_words,
            &raw_idxs,
        ));

        // 表达式实参：求值顺序在模板对象之后（规范 TaggedTemplate 求值序：
        // 标签引用 → GetTemplateObject → 实参表达式）。
        let mut expr_temps = Vec::new();
        for expr in &tt.quasi.expressions {
            expr_temps.push(self.emit_expression(expr, ctx)?);
        }

        let cooked_slot = ctx.alloc_reg();
        let mut expr_slots = Vec::new();
        for _ in &tt.quasi.expressions {
            expr_slots.push(ctx.alloc_reg());
        }

        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(cooked_slot),
            Operand::Reg(template_reg),
            Operand::None,
        ));
        for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(*slot), Operand::Reg(*temp), Operand::None));
        }

        let arg_count = 1 + tt.quasi.expressions.len();
        ctx.inst(Inst::call(
            Operand::Reg(tag_reg),
            Operand::Reg(this_reg),
            Operand::Reg(cooked_slot),
            arg_count as u8,
        ));

        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        Ok(result_reg)
    }

    /// 标签表达式的 `(callee, this)` 对：成员表达式（静态/计算/私有字段）保留
    /// 对象作 receiver（GetThisValue(tagRef)），其余标签 this = undefined。
    /// 与 call.rs 的 `emit_call_expression` 同款求值模式。
    fn emit_tagged_tag(&self, tag: &Expression, ctx: &mut CompileCtx) -> Result<(u32, u32), String> {
        let undefined_reg = {
            let idx = ctx.add_constant(Constant::Undefined);
            let r = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(r), idx));
            r
        };
        let pair = match tag {
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                        return Err("super property only supported in class methods".into());
                    }
                    let prop_name = member.property.name.as_str();
                    let key_reg = ctx.alloc_reg();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                    let this_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(this_reg), Operand::This, Operand::None));
                    let callee_reg = ctx.alloc_reg();
                    let op = if ctx.in_static_method {
                        OpCode::SUPER_STATIC_GET_PROP
                    } else {
                        OpCode::SUPER_GET_PROP
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(callee_reg), Operand::Reg(this_reg), Operand::Reg(key_reg)));
                    (callee_reg, this_reg)
                } else {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let callee_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(callee_reg),
                        Operand::Reg(obj_reg),
                        Operand::None,
                    ));
                    let prop_name = member.property.name.as_str();
                    let key_reg = ctx.alloc_reg();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                    ctx.inst(Inst::ic_get(Operand::Reg(callee_reg), Operand::Reg(key_reg)));
                    (callee_reg, obj_reg)
                }
            }
            Expression::ComputedMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                if let Some(key) = crate::expr::member::computed_const_key(&member.expression) {
                    let key_reg = ctx.alloc_reg();
                    let idx = ctx.add_constant(Constant::String(key));
                    ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                    let callee_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(callee_reg),
                        Operand::Reg(obj_reg),
                        Operand::None,
                    ));
                    ctx.inst(Inst::ic_get(Operand::Reg(callee_reg), Operand::Reg(key_reg)));
                    (callee_reg, obj_reg)
                } else {
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    let callee_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::GET_PROP_DYNAMIC,
                        Operand::Reg(obj_reg),
                        Operand::Reg(key_reg),
                        Operand::Reg(callee_reg),
                    ));
                    (callee_reg, obj_reg)
                }
            }
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let name = member.field.name.as_str();
                let (brand_reg, brand_id) = self.private_access_brand(obj_reg, name, ctx)?;
                let key_reg = self.emit_private_id_reg(name, ctx)?;
                let callee_reg = ctx.alloc_reg();
                ctx.inst(Inst::get_private(
                    Operand::Reg(callee_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                (callee_reg, obj_reg)
            }
            _ => {
                let tag_reg = self.emit_expression(tag, ctx)?;
                (tag_reg, undefined_reg)
            }
        };
        Ok(pair)
    }
}