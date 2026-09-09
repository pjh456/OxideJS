//! for-in 语句 emit：`emit_for_in_statement` 遍历可枚举属性名。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ForStatementLeft, Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_for_in_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ForInStatement(fi) = stmt else {
            return Ok(None);
        };
        ctx.push_scope();
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let obj_reg = self.emit_expression(&fi.right, ctx)?;
        ctx.inst(Inst::new(OpCode::FOR_IN_INIT, Operand::None, Operand::Reg(obj_reg), Operand::None));
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label, crate::emit_ctx::LoopKind::ForIn);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let done_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_IN_DONE, Operand::Reg(done_reg), Operand::None, Operand::None));
        // done=true 表示迭代结束：done=false 时跳过 end jmp 继续迭代
        let continue_label = ctx.next_label_id();
        ctx.inst(Inst::jmp_if_false(done_reg, continue_label));
        ctx.inst(Inst::jmp(end_label));
        ctx.labels.set_label_pos(continue_label, ctx.insts.len());
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_IN_NEXT, Operand::Reg(key_reg), Operand::None, Operand::None));
        match &fi.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                // let/const 声明对被捕获绑定用 fresh cell（每迭代新 cell）；var 单绑定。
                let fresh_cell = !matches!(decl.kind, VariableDeclarationKind::Var);
                for d in &decl.declarations {
                    match &d.id {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                            let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                            let name = bi.name.as_str();
                            let var_reg = ctx.alloc_reg();
                            // var 声明：绑定已存在（顶层 var 预声明、GDI 序言预登记
                            // builtin 名、同 scope 先前声明）时复用既有槽位，不重复
                            // 声明；let/const 同 scope 重复声明仍报错。
                            let target_reg = if matches!(decl.kind, VariableDeclarationKind::Var) {
                                match ctx.declare(name, var_reg, decl.kind, is_const) {
                                    Ok(()) => var_reg,
                                    Err(_) => ctx.lookup(name).unwrap_or(var_reg),
                                }
                            } else {
                                ctx.declare(name, var_reg, decl.kind, is_const)?;
                                var_reg
                            };
                            if ctx.targets_readonly_builtin(name, target_reg) {
                                // 声明撞全局不可写内置：声明不更新既有全局绑定——sloppy
                                // 静默跳过写（槽保留入口预载原值），strict 抛 TypeError。
                                if ctx.is_strict {
                                    self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx)?;
                                }
                            } else if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                                let op = if fresh_cell { OpCode::MAKE_CELL_FRESH } else { OpCode::MAKE_CELL };
                                ctx.inst(Inst::new(
                                    op,
                                    Operand::Reg(key_reg),
                                    Operand::Imm(cell_idx as u16),
                                    Operand::None,
                                ));
                            } else {
                                ctx.inst(Inst::new(
                                    OpCode::STORE_VAR,
                                    Operand::Reg(target_reg),
                                    Operand::Reg(key_reg),
                                    Operand::None,
                                ));
                            }
                            ctx.init_var(name);
                        }
                        _ => return Err("destructuring not supported".into()),
                    }
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                // 写仅在本迭代实际产生键后发生（for-in NEXT 之后），空集合不抛。
                if ctx.targets_readonly_builtin(name, var_reg) {
                    // 全局不可写内置：sloppy 静默跳过写（槽保留入口预载原值）；
                    // strict 在本迭代抛 TypeError（put 失败）。
                    if ctx.is_strict {
                        self.emit_throw_error("TypeError", "cannot assign to read-only property", ctx)?;
                    }
                } else {
                    let is_implicit = ctx.implicit_global_writes.contains(&var_reg);
                    if is_implicit && ctx.is_strict {
                        // 严格模式未声明写：抛 ReferenceError，跳过寄存器写（值无关）。
                        self.emit_strict_undeclared_write(name, ctx)?;
                    } else {
                        ctx.inst(Inst::new(
                            OpCode::STORE_VAR,
                            Operand::Reg(var_reg),
                            Operand::Reg(key_reg),
                            Operand::None,
                        ));
                        if is_implicit {
                            self.emit_implicit_global_write(name, var_reg, ctx);
                        }
                    }
                }
            }
            _ => return Err("unsupported for-in left-hand side".into()),
        }
        self.emit_statement(&fi.body, ctx)?;
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.inst(Inst::new(OpCode::FOR_IN_CLEANUP, Operand::None, Operand::None, Operand::None));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        ctx.pop_scope();
        Ok(None)
    }
}
