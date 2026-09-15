//! for-in 语句 emit：`emit_for_in_statement` 遍历可枚举属性名。
//!
//! 词法头（let/const/using）按规范建模为独立环境：右值区头名绑定未初始化
//! cell（读与捕获均抛 ReferenceError），体区绑定每迭代 fresh cell；捕获映射
//! 于语句结束后恢复，for-of 经同一套覆盖复用。

use std::collections::HashSet;

use crate::capture::collect_binding_pattern_names;
use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{ForStatementLeft, Statement, VariableDeclarationKind};

/// for-in/for-of 词法头的捕获映射覆盖：每头名一条记录，依次为名字、旧条目
/// （无旧条目为 None）、右值区 TDZ cell、体区 fresh cell（切换后填入）。
pub(crate) struct ForHeadEnv {
    entries: Vec<(String, Option<u8>, u8, u8)>,
}

impl Emitter {
    /// 收集 for-in/for-of 词法声明头（let/const/using，含解构 pattern 叶）的
    /// 头名（排序保证稳定）；var 头与赋值头无 TDZ 环境，返回 None。
    pub(crate) fn collect_for_head_lexical_names(&self, left: &ForStatementLeft) -> Option<Vec<String>> {
        let ForStatementLeft::VariableDeclaration(decl) = left else {
            return None;
        };
        if matches!(decl.kind, VariableDeclarationKind::Var) {
            return None;
        }
        let mut names = HashSet::new();
        for d in &decl.declarations {
            collect_binding_pattern_names(&d.id, &mut names);
        }
        let mut v: Vec<String> = names.into_iter().collect();
        v.sort();
        Some(v)
    }

    /// 把词法头名覆盖到捕获映射（右值区阶段）：保存旧条目、分配 TDZ cell 并
    /// 发射未初始化 MAKE_CELL、头名映射 TDZ cell。右值区内对头名的直读与
    /// 该区内创建闭包的捕获均命中未初始化 cell（读抛 ReferenceError）。
    ///
    /// # 步骤
    /// 1. 逐头名：移除旧条目并记入恢复清单。
    /// 2. 按现有最大 cell 索引顺序分配 TDZ cell（与类 brand 合成 cell 同口径）。
    /// 3. 发射未初始化 MAKE_CELL 并覆盖映射。
    ///
    /// # 副作用
    /// - 捕获映射被改动；须经 `switch_for_head_env_to_body` 与
    ///   `restore_for_head_env` 成对收尾，防头名泄漏进兄弟语句的捕获映射。
    pub(crate) fn begin_for_head_env(&self, names: Vec<String>, ctx: &mut CompileCtx) -> Result<ForHeadEnv, String> {
        let mut entries = Vec::with_capacity(names.len());
        let mut next = ctx.captured_bindings.values().copied().max().map_or(0, |m| m.saturating_add(1));
        for name in names {
            let saved = ctx.captured_bindings.remove(&name);
            let tdz_idx = next;
            next = next.saturating_add(1);
            ctx.captured_bindings.insert(name.clone(), tdz_idx);
            // 未初始化标志走 b 槽：a 槽 Imm 的高字节会覆写 b 槽，故标志并入
            // 立即数高字节，dispatch 侧按 a/b 分槽取回。
            let undef_reg = self.emit_undefined(ctx);
            ctx.inst(Inst::new(
                OpCode::MAKE_CELL,
                Operand::Reg(undef_reg),
                Operand::Imm(tdz_idx as u16 | 0x0100),
                Operand::None,
            ));
            entries.push((name, saved, tdz_idx, 0));
        }
        Ok(ForHeadEnv { entries })
    }

    /// 右值区求值完毕后把覆盖从 TDZ cell 切到体区 fresh cell：每头名分配新
    /// cell 并改写映射，头发射捕获臂与体区读/捕获自然命中（每迭代
    /// MAKE_CELL_FRESH 建新 cell，本迭代闭包捕获新 cell）。
    pub(crate) fn switch_for_head_env_to_body(&self, env: &mut ForHeadEnv, ctx: &mut CompileCtx) {
        let mut next = ctx.captured_bindings.values().copied().max().map_or(0, |m| m.saturating_add(1));
        for (name, _, _, body_idx) in &mut env.entries {
            let idx = next;
            next = next.saturating_add(1);
            *body_idx = idx;
            ctx.captured_bindings.insert(name.clone(), idx);
        }
    }

    /// 语句收尾恢复捕获映射：旧条目放回（无旧条目则移除），头名不进入后续
    /// 兄弟语句的捕获判定。
    pub(crate) fn restore_for_head_env(&self, env: ForHeadEnv, ctx: &mut CompileCtx) {
        for (name, saved, _, _) in env.entries {
            match saved {
                Some(old) => {
                    ctx.captured_bindings.insert(name, old);
                }
                None => {
                    ctx.captured_bindings.remove(&name);
                }
            }
        }
    }

    pub(crate) fn emit_for_in_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ForInStatement(fi) = stmt else {
            return Ok(None);
        };
        ctx.push_scope();
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        // 词法头 TDZ 环境：右值区头名绑定未初始化 cell，右值区求值后切到
        // 每迭代 fresh cell，体发射后恢复捕获映射。
        let mut head_env = self
            .collect_for_head_lexical_names(&fi.left)
            .map(|names| self.begin_for_head_env(names, ctx))
            .transpose()?;
        let obj_reg = self.emit_expression(&fi.right, ctx)?;
        if let Some(env) = &mut head_env {
            self.switch_for_head_env_to_body(env, ctx);
        }
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
                                // 顶层 for-in var 头：迭代值落全局对象属性（A 侧单一真值）。
                                // 只读三常量已在上方拦截臂跳过；其余 builtin 名属性可写，
                                // 迭代键照规范覆写既有全局属性。
                                if self.is_global_tier_name(ctx, name) {
                                    self.emit_tier_global_write(name, key_reg, ctx);
                                }
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
                    let is_tier = self.is_global_tier_name(ctx, name);
                    let is_implicit = ctx.is_implicit_global_reg(var_reg);
                    if is_implicit && ctx.is_strict {
                        // 严格模式未声明写：抛 ReferenceError，跳过寄存器写（值无关）。
                        self.emit_strict_undeclared_write(name, ctx)?;
                    } else if is_tier {
                        // 顶层 for-in 赋值头：迭代值落全局对象属性（A 侧单一真值）。
                        // 只读三常量已在上方拦截臂跳过；其余 builtin 名属性可写，
                        // 迭代键照规范覆写既有全局属性。
                        self.emit_tier_global_write(name, key_reg, ctx);
                    } else {
                        ctx.inst(Inst::new(
                            OpCode::STORE_VAR,
                            Operand::Reg(var_reg),
                            Operand::Reg(key_reg),
                            Operand::None,
                        ));
                        if is_implicit {
                            self.emit_implicit_global_write(name, var_reg, ctx);
                        } else if ctx.targets_writable_builtin(name, var_reg) {
                            // 可写内置名：迭代键同步落全局对象属性。
                            self.emit_global_put_write(name, var_reg, ctx);
                        }
                    }
                }
            }
            _ => return Err("unsupported for-in left-hand side".into()),
        }
        let body_result = self.emit_statement(&fi.body, ctx)?;
        if let Some(env) = head_env {
            self.restore_for_head_env(env, ctx);
        }
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.inst(Inst::new(OpCode::FOR_IN_CLEANUP, Operand::None, Operand::None, Operand::None));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        ctx.pop_scope();
        // 循环完成值 = 循环体最后一次非空完成值；空体物化 undefined 作为带值完成
        // 返回，不沿用循环前的值。
        let result = match body_result {
            Some(r) => r,
            None => self.emit_undefined(ctx),
        };
        Ok(Some(result))
    }
}
