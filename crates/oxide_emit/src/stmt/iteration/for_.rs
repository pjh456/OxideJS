//! for 语句 emit：`emit_for_statement` 含 init/test/update 三段与循环跳转。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{BindingPattern, ForStatementInit, Statement, VariableDeclarationKind};

impl Emitter {
    /// 递归收集解构 pattern 内全部绑定标识符名（用于循环头声明被捕获时的 fresh 判定）。
    fn collect_pattern_binding_names(&self, pattern: &BindingPattern, out: &mut Vec<String>) {
        match pattern {
            BindingPattern::BindingIdentifier(bi) => out.push(bi.name.as_str().to_string()),
            BindingPattern::ArrayPattern(ap) => {
                for p in ap.elements.iter().flatten() {
                    self.collect_pattern_binding_names(p, out);
                }
                if let Some(rest) = &ap.rest {
                    self.collect_pattern_binding_names(&rest.argument, out);
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.collect_pattern_binding_names(&prop.value, out);
                }
                if let Some(rest) = &op.rest {
                    self.collect_pattern_binding_names(&rest.argument, out);
                }
            }
            BindingPattern::AssignmentPattern(ap) => self.collect_pattern_binding_names(&ap.left, out),
        }
    }

    pub(crate) fn emit_for_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ForStatement(fr) = stmt else {
            return Ok(None);
        };
        ctx.push_scope();
        let start_label = ctx.next_label_id();
        let update_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.push_loop(end_label, update_label, crate::emit_ctx::LoopKind::Plain);
        let n_labeled = ctx.take_pending_loop_labels(end_label, update_label);
        // 循环头声明中被嵌套函数捕获的 let/const 绑定：每迭代 fresh cell。
        let mut fresh_bindings: Vec<(String, u8)> = Vec::new();
        // 循环头 let/const 声明名：update 段是 per-iteration 可变绑定
        // （规范 §14.7.4.4 CreatePerIterationEnvironment 用 CreateMutableBinding），
        // 编译期 const 写检查对 update 段豁免，故记录全部声明名（含未被捕获者）。
        let mut update_names: Vec<String> = Vec::new();
        if let Some(init) = &fr.init {
            if let Some(expr) = init.as_expression() {
                self.emit_expression(expr, ctx)?;
            } else if let ForStatementInit::VariableDeclaration(decl) = init {
                for d in &decl.declarations {
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    if let Some(init_expr) = &d.init {
                        let val_reg = self.emit_expression(init_expr, ctx)?;
                        self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, false, ctx)?;
                        // 被捕获绑定：fresh 从寄存器读当前值，须把 init 值写入寄存器槽。
                        // emit_binding_pattern 对捕获绑定只建 cell，不写寄存器。
                        let mut names = Vec::new();
                        self.collect_pattern_binding_names(&d.id, &mut names);
                        for name in &names {
                            if ctx.captured_bindings.contains_key(name) {
                                if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                                    // 全局不可写内置的捕获写被声明路径拦截，同步写一并跳过。
                                    if !ctx.targets_readonly_builtin(name, reg) {
                                        ctx.inst(Inst::new(
                                            OpCode::STORE_VAR,
                                            Operand::Reg(reg),
                                            Operand::Reg(val_reg),
                                            Operand::None,
                                        ));
                                    }
                                }
                            }
                        }
                    } else if let BindingPattern::BindingIdentifier(bi) = &d.id {
                        let idx = ctx.add_constant(Constant::Undefined);
                        let tmp = ctx.alloc_reg();
                        ctx.inst(Inst::load_const(Operand::Reg(tmp), idx));
                        let var_reg = ctx.alloc_reg();
                        // 无初始化 var 是纯声明而非赋值：绑定已预先存在（提升引用或
                        // 先前写入）时保留槽值，仅首次声明把 undefined 物化进槽。
                        let (target_reg, already_bound) = if matches!(decl.kind, VariableDeclarationKind::Var) {
                            match ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const) {
                                Ok(()) => (var_reg, false),
                                Err(_) => (ctx.lookup(bi.name.as_str()).unwrap_or(var_reg), true),
                            }
                        } else {
                            ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                            (var_reg, false)
                        };
                        if !already_bound {
                            ctx.inst(Inst::new(
                                OpCode::STORE_VAR,
                                Operand::Reg(target_reg),
                                Operand::Reg(tmp),
                                Operand::None,
                            ));
                        }
                        ctx.init_var(bi.name.as_str());
                    }
                    // 记录 let/const 循环头声明名（update 段写豁免所需）与被捕获的绑定
                    // （解构 pattern 递归收集绑定名，被捕获者每迭代 fresh cell）。
                    if !matches!(decl.kind, VariableDeclarationKind::Var) {
                        let mut names = Vec::new();
                        self.collect_pattern_binding_names(&d.id, &mut names);
                        for name in &names {
                            update_names.push(name.clone());
                            if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                                fresh_bindings.push((name.clone(), cell_idx));
                            }
                        }
                    }
                }
            }
        }
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        // 每迭代 fresh cell：被捕获的 let/const 循环变量在迭代开始拷贝当前值到新 cell，
        // 本迭代闭包捕获新 cell（规范 CreatePerIterationEnvironment）。
        // 值源是循环变量的寄存器（update 段写寄存器而非 cell），首迭代即 init 值。
        for (name, cell_idx) in &fresh_bindings {
            let reg = ctx.scopes.symbols.lookup_any(name).expect("declared loop binding");
            ctx.inst(Inst::new(
                OpCode::MAKE_CELL_FRESH,
                Operand::Reg(reg),
                Operand::Imm(*cell_idx as u16),
                Operand::None,
            ));
        }
        if let Some(test) = &fr.test {
            let test_reg = self.emit_expression(test, ctx)?;
            ctx.inst(Inst::jmp_if_false(test_reg, end_label));
        }
        self.emit_statement(&fr.body, ctx)?;
        ctx.labels.set_label_pos(update_label, ctx.insts.len());
        if let Some(update) = &fr.update {
            // update 段写寄存器（而非 cell）：被捕获的 let/const 循环变量每迭代 fresh，
            // update 写寄存器供下一迭代 fresh 拷贝——否则 CELL_SET 会污染本迭代闭包
            // 捕获的 cell。未捕获者本就走寄存器，一并登记以豁免 update 段的 const 检查。
            let prev = std::mem::take(&mut ctx.register_update_names);
            ctx.register_update_names = update_names;
            self.emit_expression(update, ctx)?;
            ctx.register_update_names = prev;
        }
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        ctx.pop_scope();
        Ok(None)
    }
}
