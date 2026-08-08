//! 函数表达式域：箭头/普通函数/类表达式/`new` 表达式的 emit 与分发。
//! 函数：`emit_arrow_function_expression`、`emit_function_expression`、
//! `emit_class_expression`、`emit_new_expression`、`emit_function_domain`。

use crate::expr::call::pack_arg_regs;
use crate::{CompileCtx, Emitter, ParamSpec};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Class, Expression, Statement};

impl Emitter {
    fn emit_arrow_function_expression(
        &self, arrow: &oxide_parser::ArrowFunctionExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        // 未支持：箭头函数 rest 参数
        if let Some(_rest) = &arrow.params.rest {
            return Err("rest params in arrow functions not yet supported".into());
        }

        // 提取形参名（与函数表达式相同的形态）
        let mut param_names = Vec::new();
        for (idx, param) in arrow.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_names.push(ParamSpec::Identifier {
                        name: bi.name.to_string(),
                        initializer: param.initializer.as_deref(),
                    });
                }
                pattern => {
                    param_names.push(ParamSpec::Pattern {
                        synthetic_name: format!("@@param_{idx}"),
                        pattern,
                        initializer: param.initializer.as_deref(),
                    });
                }
            }
        }

        // 表达式体：以 is_expression_body=true 编译；语句体：以 false 编译。
        let body_stmts = &arrow.body.statements;
        let is_expr_body = arrow.expression;

        let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, is_expr_body, true)?;
        sub_module.is_arrow = true;

        ctx.nested.push(sub_module);
        // 子模块下标 1 起始：0 保留为无子模块哨兵
        let sub_idx = ctx.nested.len() as u16;

        let r = ctx.alloc_reg();
        ctx.inst(Inst::create_closure(Operand::Reg(r), sub_idx));
        Ok(r)
    }

    fn emit_function_expression(&self, fe: &oxide_parser::Function, ctx: &mut CompileCtx) -> Result<u32, String> {
        // 函数表达式：编译函数体为子模块，create_closure 实例化闭包
        let mut param_names = Vec::new();
        for (idx, param) in fe.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_names.push(ParamSpec::Identifier {
                        name: bi.name.to_string(),
                        initializer: param.initializer.as_deref(),
                    });
                }
                pattern => {
                    param_names.push(ParamSpec::Pattern {
                        synthetic_name: format!("@@param_{idx}"),
                        pattern,
                        initializer: param.initializer.as_deref(),
                    });
                }
            }
        }

        let body_stmts: &[Statement] = if let Some(body) = &fe.body { &body.statements } else { &[] };

        let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
        if let Some(id) = &fe.id {
            sub_module.function_name = Some(id.name.to_string());
        }
        ctx.nested.push(sub_module);
        // 子模块下标 1 起始：0 保留为无子模块哨兵
        let sub_idx = ctx.nested.len() as u16;

        let r = ctx.alloc_reg();
        ctx.inst(Inst::create_closure(Operand::Reg(r), sub_idx));
        Ok(r)
    }

    fn emit_class_expression(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u32, String> {
        self.emit_class(class, ctx)
    }

    fn emit_new_expression(&self, ne: &oxide_parser::NewExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        let constructor_reg = self.emit_expression(&ne.callee, ctx)?;
        let words = self.emit_call_args(&ne.arguments, ctx)?;
        let r = ctx.alloc_reg();
        if words.iter().any(|w| w >> 31 == 1) {
            ctx.inst(Inst::new_expression_spread(Operand::Reg(r), Operand::Reg(constructor_reg), &words));
        } else {
            let mut static_regs = words;
            let first_arg_reg = if static_regs.is_empty() { 0u32 } else { pack_arg_regs(&mut static_regs, ctx) };
            ctx.inst(Inst::new_expression(
                Operand::Reg(r),
                Operand::Reg(constructor_reg),
                Operand::Reg(first_arg_reg),
                static_regs.len() as u8,
            ));
        }
        Ok(r)
    }

    pub(crate) fn emit_function_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::ArrowFunctionExpression(arrow) => self.emit_arrow_function_expression(arrow, ctx),
            Expression::FunctionExpression(fe) => self.emit_function_expression(fe, ctx),
            Expression::ClassExpression(class) => self.emit_class_expression(class, ctx),
            Expression::NewExpression(ne) => self.emit_new_expression(ne, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
