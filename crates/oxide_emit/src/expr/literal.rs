//! 字面量表达式 emit：数字/字符串/布尔/null/正则 → 常量池或立即数。
//! 整数走立即数编码（`is_int_literal`），其余入常量池。

use crate::{is_int_literal, CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Expression;

impl Emitter {
    pub(crate) fn emit_literal(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::NumericLiteral(n) => self.emit_numeric_literal_expression(n, ctx),
            Expression::StringLiteral(s) => self.emit_string_literal_expression(s, ctx),
            Expression::BooleanLiteral(b) => self.emit_boolean_literal_expression(b, ctx),
            Expression::NullLiteral(_) => self.emit_null_literal_expression(ctx),
            Expression::RegExpLiteral(lit) => self.emit_reg_exp_literal_expression(lit, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }

    fn emit_numeric_literal_expression(
        &self, n: &oxide_parser::NumericLiteral, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let idx = if is_int_literal(n.value) {
            ctx.add_constant(Constant::Int(n.value as i32))
        } else {
            ctx.add_constant(Constant::Number(n.value))
        };
        let r = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(r), idx));
        Ok(r)
    }

    fn emit_string_literal_expression(
        &self, s: &oxide_parser::StringLiteral, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        eprintln!("[DBG] string literal value={:?}", s.value.as_str());
        let idx = ctx.add_constant(Constant::String(s.value.to_string()));
        let r = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(r), idx));
        Ok(r)
    }

    fn emit_boolean_literal_expression(
        &self, b: &oxide_parser::BooleanLiteral, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let idx = ctx.add_constant(Constant::Boolean(b.value));
        let r = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(r), idx));
        Ok(r)
    }

    fn emit_null_literal_expression(&self, ctx: &mut CompileCtx) -> Result<u32, String> {
        let idx = ctx.add_constant(Constant::Null);
        let r = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(r), idx));
        Ok(r)
    }

    fn emit_reg_exp_literal_expression(
        &self, lit: &oxide_parser::RegExpLiteral, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        if let Some(raw) = &lit.raw {
            let raw_str = raw.to_string();
            if raw_str.len() >= 2 && raw_str.starts_with('/') {
                let last_slash = raw_str.rfind('/').unwrap_or(raw_str.len() - 1);
                let pattern = raw_str[1..last_slash].to_string();
                let flags = raw_str[last_slash + 1..].to_string();
                // 非法正则字面量须在编译期报 SyntaxError（负面测试期望编译失败）。
                // 用 regress（ECMAScript 语法）校验，避免误报 backreference/lookaround 等合法模式。
                if regress::Regex::with_flags(&pattern, flags.as_str()).is_err() {
                    return Err(format!(
                        "SyntaxError: Invalid regular expression: /{pattern}/{flags}"
                    ));
                }
                let pat_ci = ctx.add_constant(Constant::String(pattern));
                let pat_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(pat_reg), pat_ci));
                let flags_ci = ctx.add_constant(Constant::String(flags));
                let flags_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(flags_reg), flags_ci));
                let r = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::CREATE_REGEXP,
                    Operand::Reg(r),
                    Operand::Reg(pat_reg),
                    Operand::Reg(flags_reg),
                ));
                Ok(r)
            } else {
                Err(format!("unsupported regexp literal: {:?}", lit))
            }
        } else {
            Err(format!("unsupported regexp literal: {:?}", lit))
        }
    }
}


