//! 标签语句 emit：`emit_labeled_statement` 登记 break/continue 标签作用域。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Statement;

impl Emitter {
    fn is_iteration_statement(stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
                | Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
        )
    }

    /// 标签体是否含指向本标签的 `break <name>`：递归走语句树，不下钻嵌套
    /// 函数体（嵌套函数是独立编译单元，其 break 无法指向外层标签）。
    fn body_contains_break_to_label(stmt: &Statement, name: &str) -> bool {
        match stmt {
            Statement::BreakStatement(b) => b.label.as_ref().is_some_and(|l| l.name.as_str() == name),
            Statement::BlockStatement(b) => b.body.iter().any(|s| Self::body_contains_break_to_label(s, name)),
            Statement::IfStatement(i) => {
                Self::body_contains_break_to_label(&i.consequent, name)
                    || i.alternate
                        .as_ref()
                        .is_some_and(|a| Self::body_contains_break_to_label(a, name))
            }
            Statement::WhileStatement(w) => Self::body_contains_break_to_label(&w.body, name),
            Statement::DoWhileStatement(d) => Self::body_contains_break_to_label(&d.body, name),
            Statement::ForStatement(f) => Self::body_contains_break_to_label(&f.body, name),
            Statement::ForInStatement(f) => Self::body_contains_break_to_label(&f.body, name),
            Statement::ForOfStatement(f) => Self::body_contains_break_to_label(&f.body, name),
            Statement::LabeledStatement(l) => Self::body_contains_break_to_label(&l.body, name),
            Statement::SwitchStatement(s) => s
                .cases
                .iter()
                .any(|c| c.consequent.iter().any(|st| Self::body_contains_break_to_label(st, name))),
            Statement::TryStatement(t) => {
                t.block.body.iter().any(|s| Self::body_contains_break_to_label(s, name))
                    || t.handler
                        .as_ref()
                        .is_some_and(|h| h.body.body.iter().any(|s| Self::body_contains_break_to_label(s, name)))
                    || t.finalizer
                        .as_ref()
                        .is_some_and(|f| f.body.iter().any(|s| Self::body_contains_break_to_label(s, name)))
            }
            Statement::WithStatement(w) => Self::body_contains_break_to_label(&w.body, name),
            _ => false,
        }
    }

    pub(crate) fn emit_labeled_statement(
        &self, stmt: &oxide_parser::LabeledStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let name = stmt.label.name.as_str();
        // 标签语句完成值 = 体完成值（透传）：迭代体返回其循环值，块/表达式体返回其
        // 收敛值；标签本身不改变完成值。
        let body_result = if Self::is_iteration_statement(&stmt.body) {
            ctx.queue_loop_label(name)?;
            self.emit_statement(&stmt.body, ctx)?
        } else if Self::body_contains_break_to_label(&stmt.body, name) {
            // 体含指向本标签的 break：出口结果寄存器在此分配并初始 undefined，
            // 体正常完成回写、break 携值就地写入（break 后死语句的寄存器不得
            // 成为完成值）。
            let v_reg = ctx.alloc_reg();
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.inst(Inst::load_const(Operand::Reg(v_reg), undef_idx));
            let id = ctx.next_label_id();
            ctx.push_label_scope(name, id, None, Some(v_reg))?;
            ctx.push_completion_target(v_reg);
            let r = self.emit_statement(&stmt.body, ctx)?;
            if let Some(r) = r {
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(v_reg), Operand::Reg(r), Operand::None));
            }
            ctx.pop_completion_target();
            ctx.labels.set_label_pos(id, ctx.insts.len());
            ctx.pop_label_scope();
            Some(v_reg)
        } else {
            let id = ctx.next_label_id();
            ctx.push_label_scope(name, id, None, None)?;
            let r = self.emit_statement(&stmt.body, ctx)?;
            ctx.labels.set_label_pos(id, ctx.insts.len());
            ctx.pop_label_scope();
            r
        };
        Ok(body_result)
    }
}
