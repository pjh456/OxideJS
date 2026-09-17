//! 可选链（`?.`）emit：短路守卫、链式成员访问/调用与逻辑赋值短路测试。
//! 函数：`emit_chainable_expression`、`emit_optional_guard`、`emit_chain_call` 等。

use crate::expr::call::pack_arg_regs;
use crate::expr::member::is_array_index_str;
use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::{LabelId, Operand};
use oxide_parser::{ChainElement, Expression, LogicalOperator, PropertyKey};

impl Emitter {
    /// 可选链短路守卫：`reg` 为 null 或 undefined 时跳转到短路标签 `short_label`。
    fn emit_optional_guard(&self, reg: u32, short_label: LabelId, ctx: &mut CompileCtx) -> Result<(), String> {
        let dup_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(reg), Operand::None));
        ctx.inst(Inst::jmp_if_nullish(dup_reg, short_label));
        Ok(())
    }

    /// 静态成员读取并保留 base 寄存器，返回 `(值寄存器, base 寄存器)` 供链上后续调用取 this。
    fn emit_static_member_get_preserve_base(
        &self, member: &oxide_parser::StaticMemberExpression, short_label: Option<LabelId>, ctx: &mut CompileCtx,
    ) -> Result<(u32, u32), String> {
        let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
        if member.optional {
            if let Some(label) = short_label {
                self.emit_optional_guard(obj_reg, label, ctx)?;
            }
        }
        let idx = ctx.add_constant(Constant::String(member.property.name.as_str().to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
        let value_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(value_reg), Operand::Reg(obj_reg), Operand::None));
        ctx.inst(Inst::ic_get(Operand::Reg(value_reg), Operand::Reg(key_reg)));
        Ok((value_reg, obj_reg))
    }

    /// 计算成员读取并保留 base 寄存器；常量字符串键折叠为 IC 静态路径，其余走动态属性读，返回 `(值寄存器, base 寄存器)`。
    fn emit_computed_member_get_preserve_base(
        &self, member: &oxide_parser::ComputedMemberExpression, short_label: Option<LabelId>, ctx: &mut CompileCtx,
    ) -> Result<(u32, u32), String> {
        let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
        if member.optional {
            if let Some(label) = short_label {
                self.emit_optional_guard(obj_reg, label, ctx)?;
            }
        }
        // 常量字符串键折叠为 IC 静态路径：value 独立寄存器，base 保留供 this 绑定。
        if let Some(key) = crate::expr::member::computed_const_key(&member.expression) {
            let idx = ctx.add_constant(Constant::String(key));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            let value_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(value_reg), Operand::Reg(obj_reg), Operand::None));
            ctx.inst(Inst::ic_get(Operand::Reg(value_reg), Operand::Reg(key_reg)));
            return Ok((value_reg, obj_reg));
        }
        let key_reg = self.emit_expression(&member.expression, ctx)?;
        let value_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::GET_PROP_DYNAMIC,
            Operand::Reg(obj_reg),
            Operand::Reg(key_reg),
            Operand::Reg(value_reg),
        ));
        Ok((value_reg, obj_reg))
    }

    /// 链上调用：成员 / 私有字段调用的 this 取链 base，其余调用为 undefined；实参打包与内置调用分派在此完成。
    fn emit_chain_call(
        &self, call: &oxide_parser::CallExpression, short_label: Option<LabelId>, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let (callee_reg, this_reg) = match &call.callee {
            Expression::StaticMemberExpression(member) => {
                self.emit_static_member_get_preserve_base(member, short_label, ctx)?
            }
            Expression::ComputedMemberExpression(member) => {
                self.emit_computed_member_get_preserve_base(member, short_label, ctx)?
            }
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
                if member.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(obj_reg, label, ctx)?;
                    }
                }
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
                let callee_reg = self.emit_chainable_expression(&call.callee, short_label, ctx)?;
                if call.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(callee_reg, label, ctx)?;
                    }
                }
                let this_idx = ctx.add_constant(Constant::Undefined);
                let this_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(this_reg), this_idx));
                (callee_reg, this_reg)
            }
        };
        if call.optional
            && matches!(
                &call.callee,
                Expression::StaticMemberExpression(_)
                    | Expression::ComputedMemberExpression(_)
                    | Expression::PrivateFieldExpression(_)
            )
        {
            if let Some(label) = short_label {
                self.emit_optional_guard(callee_reg, label, ctx)?;
            }
        }
        let words = self.emit_call_args(&call.arguments, ctx)?;
        if words.iter().any(|w| w >> 31 == 1) {
            ctx.inst(Inst::call_spread(Operand::Reg(callee_reg), Operand::Reg(this_reg), &words));
        } else {
            let mut static_regs = words;
            let first_arg_reg = if static_regs.is_empty() { 0u32 } else { pack_arg_regs(&mut static_regs, ctx) };
            let op = match &call.callee {
                Expression::Identifier(ident)
                    if ctx.is_builtin(ident.name.as_str()) && !ctx.is_local_shadowing_builtin(ident.name.as_str()) =>
                {
                    OpCode::CALL_NATIVE
                }
                _ => OpCode::CALL,
            };
            match op {
                OpCode::CALL => {
                    ctx.inst(Inst::call(
                        Operand::Reg(callee_reg),
                        Operand::Reg(this_reg),
                        Operand::Reg(first_arg_reg),
                        static_regs.len() as u8,
                    ));
                }
                OpCode::CALL_NATIVE => {
                    ctx.inst(Inst::call_native(
                        Operand::Reg(callee_reg),
                        Operand::Reg(this_reg),
                        Operand::Reg(first_arg_reg),
                        static_regs.len() as u8,
                    ));
                }
                _ => unreachable!(),
            }
        }
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        Ok(result_reg)
    }

    /// 递归发射链元素并返回结果寄存器；可短路点由 `short_label` 标记。
    fn emit_chainable_expression(
        &self, expr: &Expression, short_label: Option<LabelId>, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        match expr {
            Expression::StaticMemberExpression(member) => {
                let (value_reg, _) = self.emit_static_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            Expression::ComputedMemberExpression(member) => {
                let (value_reg, _) = self.emit_computed_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
                if member.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(obj_reg, label, ctx)?;
                    }
                }
                let name = member.field.name.as_str();
                let (brand_reg, brand_id) = self.private_access_brand(obj_reg, name, ctx)?;
                let key_reg = self.emit_private_id_reg(name, ctx)?;
                let value_reg = ctx.alloc_reg();
                ctx.inst(Inst::get_private(
                    Operand::Reg(value_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                Ok(value_reg)
            }
            Expression::CallExpression(call) => self.emit_chain_call(call, short_label, ctx),
            Expression::ChainExpression(chain) => self.emit_chain_element(&chain.expression, short_label, ctx),
            _ => self.emit_expression(expr, ctx),
        }
    }

    /// 发射单个 `ChainElement`，按变体分派成员读取 / 私有字段 / 调用；TS 非空断言不支持，直接报错。
    pub(crate) fn emit_chain_element(
        &self, element: &ChainElement, short_label: Option<LabelId>, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        match element {
            ChainElement::StaticMemberExpression(member) => {
                let (value_reg, _) = self.emit_static_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            ChainElement::ComputedMemberExpression(member) => {
                let (value_reg, _) = self.emit_computed_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            ChainElement::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
                if member.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(obj_reg, label, ctx)?;
                    }
                }
                let name = member.field.name.as_str();
                let (brand_reg, brand_id) = self.private_access_brand(obj_reg, name, ctx)?;
                let key_reg = self.emit_private_id_reg(name, ctx)?;
                let value_reg = ctx.alloc_reg();
                ctx.inst(Inst::get_private(
                    Operand::Reg(value_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    brand_reg,
                    brand_id,
                ));
                Ok(value_reg)
            }
            ChainElement::CallExpression(call) => self.emit_chain_call(call, short_label, ctx),
            ChainElement::TSNonNullExpression(_) => Err("TS non-null expressions are not supported in JS mode".into()),
        }
    }

    /// 逻辑赋值短路测试：按 And / Or / Coalesce 选择跳转条件，未短路则落入赋值路径。
    pub(crate) fn emit_logical_assign_test(
        &self, op: LogicalOperator, test_reg: u32, store_label: LabelId, end_label: LabelId, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match op {
            LogicalOperator::And => ctx.inst(Inst::jmp_if_false(test_reg, end_label)),
            LogicalOperator::Or => ctx.inst(Inst::jmp_if_true(test_reg, end_label)),
            LogicalOperator::Coalesce => {
                ctx.inst(Inst::jmp_if_nullish(test_reg, store_label));
                ctx.inst(Inst::jmp(end_label));
            }
        }
        Ok(())
    }

    /// 对象属性静态读：键已由调用方取为字符串，本函数按常量键走 IC 读并返回属性寄存器。
    pub(crate) fn emit_object_property_read(&self, src_reg: u32, key: &str, ctx: &mut CompileCtx) -> u32 {
        let prop_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(prop_reg), Operand::Reg(src_reg), Operand::None));
        let key_idx = ctx.add_constant(Constant::String(key.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
        ctx.inst(Inst::ic_get(Operand::Reg(prop_reg), Operand::Reg(key_reg)));
        prop_reg
    }

    /// 将属性键求值为寄存器：字面量键直接取常量，其余形态走通用表达式分发。
    fn emit_property_key_expression(&self, key: &PropertyKey, ctx: &mut CompileCtx) -> Result<u32, String> {
        match key {
            // 标识符计算键（`{[k]: a}`）走通用表达式分发：经 emit_static_identifier_read
            // 正确解析 upvalue/被捕获 cell/with 动态与未声明标识符读（LOAD_GLOBAL 抛
            // ReferenceError），与对象字面量/类字段计算键同口径；直接 lookup_or_builtin
            // 只读局部符号表，嵌套函数引用外层绑定会静默回退读全局。
            PropertyKey::Identifier(_) => self.emit_expression(key.to_expression(), ctx),
            PropertyKey::StringLiteral(s) => {
                let key_idx = ctx.add_constant(Constant::String(crate::shared::string_pool::pool_key_marker(
                    &s.value,
                    s.lone_surrogates,
                )));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
                Ok(key_reg)
            }
            PropertyKey::NumericLiteral(n) => {
                let key_idx = ctx.add_constant(Constant::Number(n.value));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
                Ok(key_reg)
            }
            PropertyKey::StaticIdentifier(ident) => {
                let key_idx = ctx.add_constant(Constant::String(ident.name.as_str().to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
                Ok(key_reg)
            }
            // 其余键形态（CallExpression/Template/BigInt/RegExp 等）：转回 Expression
            // 走通用 emit 分发，键值运行时求值后由 GET_PROP_DYNAMIC 做 ToPropertyKey。
            _ => self.emit_expression(key.to_expression(), ctx),
        }
    }

    /// 读取对象属性并产出键信息：非计算键走字符串常量 IC 路径，计算键在字面量可折叠时走 IC，否则动态求值。
    /// 返回 `(结果寄存器, 常量键名, 动态键寄存器)`，后两者互补为 None。
    pub(crate) fn emit_object_property_read_key(
        &self, src_reg: u32, key: &PropertyKey, computed: bool, ctx: &mut CompileCtx,
    ) -> Result<(u32, Option<String>, Option<u32>), String> {
        if !computed {
            let key_name = crate::shared::string_pool::pool_key_property(key)?;
            let prop_reg = self.emit_object_property_read(src_reg, &key_name, ctx);
            return Ok((prop_reg, Some(key_name), None));
        }
        let prop_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(prop_reg), Operand::Reg(src_reg), Operand::None));
        // 字符串字面量计算键折叠为 IC 静态路径；标识符/数字键走 DYNAMIC。
        if let PropertyKey::StringLiteral(s) = key {
            let key_str = crate::shared::string_pool::pool_key_marker(&s.value, s.lone_surrogates);
            if !is_array_index_str(&key_str) {
                let key_idx = ctx.add_constant(Constant::String(key_str));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
                ctx.inst(Inst::ic_get(Operand::Reg(prop_reg), Operand::Reg(key_reg)));
                return Ok((prop_reg, None, Some(key_reg)));
            }
        }
        let key_reg = self.emit_property_key_expression(key, ctx)?;
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::GET_PROP_DYNAMIC,
            Operand::Reg(prop_reg),
            Operand::Reg(key_reg),
            Operand::Reg(val_reg),
        ));
        Ok((val_reg, None, Some(key_reg)))
    }
}
