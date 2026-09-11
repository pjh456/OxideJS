//! for-of 与 for-await-of 语句 emit。
//!
//! 同步 for-of 用 FOR_OF_* 指令；`for await` 用 FOR_AWAIT_OF_*：初始化取异步
//! 迭代器（缺失 `@@asyncIterator` 时回退 AsyncFromSyncIterator），每步
//! `next()` 结果经 `AWAIT` 挂起后检查 done。体内 await 语义由 `AWAIT` 指令
//! 原生提供（每次迭代都 await）。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ForOfStatement, ForStatementLeft, Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_for_of_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ForOfStatement(fo) = stmt else {
            return Ok(None);
        };
        if fo.r#await {
            return self.emit_for_await_of_statement(fo, ctx);
        }
        self.emit_sync_for_of_statement(fo, ctx)
    }

    fn emit_sync_for_of_statement(&self, fo: &ForOfStatement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        ctx.push_scope();
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let iter_src_reg = self.emit_expression(&fo.right, ctx)?;
        ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(iter_src_reg), Operand::None));
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label, crate::emit_ctx::LoopKind::ForOf);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg), Operand::None, Operand::None));
        ctx.inst(Inst::jmp_if_false(has_reg, end_label));
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg), Operand::None, Operand::None));
        self.emit_for_of_left_assignment(&fo.left, val_reg, ctx)?;
        self.emit_statement(&fo.body, ctx)?;
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        ctx.pop_scope();
        Ok(None)
    }

    /// for-await-of：异步迭代器协议遍历，每步经 `AWAIT` 挂起等待 next() 结果。
    ///
    /// 指令序列：INIT 取异步迭代器 → 循环顶 NEXT 调 next() 写 promise → `AWAIT`
    /// 挂起（恢复值经 reg 0 交付）→ LOAD_VAR 拷贝为结果对象 → DONE 检查 done →
    /// FOR_OF_NEXT 读 value → 绑定左侧 → 循环体 → CLOSE 异步收尾。
    fn emit_for_await_of_statement(&self, fo: &ForOfStatement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        ctx.push_scope();
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let iter_src_reg = self.emit_expression(&fo.right, ctx)?;
        ctx.inst(Inst::new(
            OpCode::FOR_AWAIT_OF_INIT,
            Operand::None,
            Operand::Reg(iter_src_reg),
            Operand::None,
        ));
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label, crate::emit_ctx::LoopKind::ForAwaitOf);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let next_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_AWAIT_OF_NEXT, Operand::Reg(next_reg), Operand::None, Operand::None));
        ctx.inst(Inst::await_expr(Operand::Reg(next_reg)));
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::FOR_AWAIT_OF_DONE,
            Operand::Reg(has_reg),
            Operand::Reg(result_reg),
            Operand::None,
        ));
        ctx.inst(Inst::jmp_if_false(has_reg, end_label));
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg), Operand::None, Operand::None));
        self.emit_for_of_left_assignment(&fo.left, val_reg, ctx)?;
        self.emit_statement(&fo.body, ctx)?;
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.inst(Inst::new(OpCode::FOR_AWAIT_OF_CLOSE, Operand::None, Operand::None, Operand::None));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        ctx.pop_scope();
        Ok(None)
    }

    /// for-of/for-await-of 左侧绑定：把当前迭代值 `val_reg` 写入声明/赋值目标。
    /// let/const 声明对被捕获绑定用 fresh cell（每迭代新 cell，规范 per-iteration 绑定）；
    /// var 保持单绑定。
    fn emit_for_of_left_assignment(
        &self, left: &ForStatementLeft, val_reg: u32, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match left {
            ForStatementLeft::VariableDeclaration(decl) => {
                let fresh_cell = !matches!(decl.kind, VariableDeclarationKind::Var);
                for d in &decl.declarations {
                    self.emit_binding_pattern(&d.id, val_reg, decl.kind, false, fresh_cell, ctx)?;
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                // 写仅在本迭代实际产生值后发生（for-of NEXT 之后），空集合不抛。
                if ctx.targets_readonly_builtin(name, var_reg) {
                    // 全局不可写内置：sloppy 静默跳过写（槽保留入口预载原值）；
                    // strict 在本迭代抛 TypeError（put 失败）。
                    if ctx.is_strict {
                        self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx)?;
                    }
                } else {
                    let is_implicit = ctx.is_implicit_global_reg(var_reg);
                    if is_implicit && ctx.is_strict {
                        // 严格模式未声明写：抛 ReferenceError，跳过寄存器写（值无关）。
                        self.emit_strict_undeclared_write(name, ctx)?;
                    } else {
                        ctx.inst(Inst::new(
                            OpCode::STORE_VAR,
                            Operand::Reg(var_reg),
                            Operand::Reg(val_reg),
                            Operand::None,
                        ));
                        if is_implicit {
                            self.emit_implicit_global_write(name, var_reg, ctx);
                        }
                    }
                }
            }
            ForStatementLeft::ArrayAssignmentTarget(ap) => {
                self.emit_array_assignment(ap, val_reg, ctx)?;
            }
            ForStatementLeft::ObjectAssignmentTarget(op) => {
                self.emit_object_assignment(op, val_reg, ctx)?;
            }
            _ => return Err("unsupported for-of left-hand side".into()),
        }
        Ok(())
    }
}
