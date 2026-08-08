//! 调用表达式 emit：普通/原生/`new`/`super` 调用与调用域分发。
//! 函数：`emit_call_expression`、`emit_call_domain`。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Expression;

impl Emitter {
    fn emit_call_expression(&self, call: &oxide_parser::CallExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        if matches!(&call.callee, Expression::Super(_)) {
            if !ctx.in_derived_constructor {
                return Err("super() only supported in derived constructors".into());
            }
            let words = self.emit_call_args(&call.arguments, ctx)?;
            let result_reg = ctx.alloc_reg();
            if words.iter().any(|w| w >> 31 == 1) {
                ctx.inst(Inst::super_call_spread(Operand::Reg(result_reg), &words));
            } else {
                let mut static_regs = words;
                let first_arg_reg = if static_regs.is_empty() { 0u32 } else { pack_arg_regs(&mut static_regs, ctx) };
                ctx.inst(Inst::super_call(
                    Operand::Reg(result_reg),
                    Operand::Reg(first_arg_reg),
                    static_regs.len() as u8,
                ));
            }
            if let Some(mut field_buffer) = ctx.field_buffer.take() {
                let insert_inst = ctx.insts.len();
                ctx.insts.append(&mut field_buffer.insts);
                for (label, relative) in field_buffer.labels {
                    ctx.labels.set_label_pos(label, insert_inst + relative);
                }
            }
            return Ok(result_reg);
        }
        let (callee_reg, this_reg) = match &call.callee {
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let callee_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::GET_PRIVATE,
                    Operand::Reg(callee_reg),
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                ));
                (callee_reg, obj_reg)
            }
            Expression::StaticMemberExpression(member) => {
                let is_super_member = matches!(&member.object, Expression::Super(_));
                let obj_reg = if is_super_member {
                    let this_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(this_reg), Operand::This, Operand::None));
                    this_reg
                } else {
                    self.emit_expression(&member.object, ctx)?
                };
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                let callee_reg = ctx.alloc_reg();
                if is_super_member {
                    if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                        return Err("super property only supported in class methods".into());
                    }
                    let op = if ctx.in_static_method {
                        OpCode::SUPER_STATIC_GET_PROP
                    } else {
                        OpCode::SUPER_GET_PROP
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(callee_reg), Operand::Reg(obj_reg), Operand::Reg(key_reg)));
                } else {
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(callee_reg),
                        Operand::Reg(obj_reg),
                        Operand::None,
                    ));
                    ctx.inst(Inst::ic_get(Operand::Reg(callee_reg), Operand::Reg(key_reg)));
                }
                (callee_reg, obj_reg)
            }
            _ => {
                let callee_reg = self.emit_expression(&call.callee, ctx)?;
                let this_idx = ctx.add_constant(Constant::Undefined);
                let this_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(this_reg), this_idx));
                (callee_reg, this_reg)
            }
        };
        let words = self.emit_call_args(&call.arguments, ctx)?;
        if words.iter().any(|w| w >> 31 == 1) {
            // 任一实参是 spread → 运行期物化完整实参（有序字逐个读寄存器 / 迭代展开）。
            ctx.inst(Inst::call_spread(Operand::Reg(callee_reg), Operand::Reg(this_reg), &words));
        } else {
            let mut static_regs = words;
            let first_arg_reg = if static_regs.is_empty() { 0u32 } else { pack_arg_regs(&mut static_regs, ctx) };
            let op = match &call.callee {
                Expression::Identifier(ident) if ctx.is_builtin(ident.name.as_str()) => OpCode::CALL_NATIVE,
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

    pub(crate) fn emit_call_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::CallExpression(call) => self.emit_call_expression(call, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}

/// 把调用参数打包到连续寄存器块（VM 按 `regs[first_arg + i]` 连续读参数）。
/// 各参数由独立 vreg 承载，复杂表达式（对象/数组字面量、嵌套调用）的临时寄存器
/// 会使参数 vreg 不连续——检测到不连续时用 MOV 打包到新连续块。
/// 返回首参寄存器；参数为空时返回 0。
pub(crate) fn pack_arg_regs(arg_regs: &mut [u32], ctx: &mut CompileCtx) -> u32 {
    let consecutive = arg_regs.windows(2).all(|w| w[1] == w[0] + 1);
    if consecutive {
        return arg_regs[0];
    }
    // 预留连续块（单调 alloc_reg 保证 base..base+n 连续）
    let base = ctx.alloc_reg();
    for _ in 1..arg_regs.len() {
        ctx.alloc_reg();
    }
    for (i, &reg) in arg_regs.iter().enumerate() {
        ctx.inst(Inst::inst_mov(Operand::Reg(base + i as u32), Operand::Reg(reg)));
    }
    base
}

impl Emitter {
    /// 收集调用实参为有序实参字（保持源码求值序）：静态实参 = 寄存器号，
    /// spread 源 = `0x8000_0000 | 寄存器号`（高位标记区分）。
    pub(crate) fn emit_call_args(
        &self, args: &[oxide_parser::Argument], ctx: &mut CompileCtx,
    ) -> Result<Vec<u32>, String> {
        let mut words = Vec::new();
        for arg in args {
            if let Some(expr) = arg.as_expression() {
                words.push(self.emit_expression(expr, ctx)?);
            } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
                words.push(0x8000_0000 | self.emit_expression(&sp.argument, ctx)?);
            }
        }
        Ok(words)
    }
}
