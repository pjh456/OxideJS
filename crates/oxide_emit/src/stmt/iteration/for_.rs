//! for 语句 emit：`emit_for_statement` 含 init/test/update 三段与循环跳转。
//!
//! C 风格 for 的词法头按规范建模为独立环境：init 求值前头名绑定未初始化 cell
//! （直读与捕获均抛 ReferenceError），被闭包引用或与顶层 var/函数同名的头名保留
//! 独立 cell 贯穿循环，其余 init 后撤出覆盖回退寄存器循环。

use std::collections::HashSet;

use super::for_in::ForHeadEnv;
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
        // 循环头 let/const 声明名：update 段是 per-iteration 可变绑定（规范
        // §14.7.4.4 CreatePerIterationEnvironment 用 CreateMutableBinding）——
        // 规范允许 update 段写 let/const 循环变量，每迭代新建一个可变绑定；
        // 编译期 const 写检查对 update 段豁免，故记录全部声明名（含未被捕获者）。
        let mut update_names: Vec<String> = Vec::new();
        // 词法头独立环境：init 求值前把头名全部覆盖为未初始化 cell，使 init 内的
        // 直读与该区创建闭包的捕获命中 TDZ cell（不穿透外层同名已初始化绑定）。
        // init 后仅「被闭包引用」或「与顶层 var/函数同名」的头名保留覆盖贯穿循环
        // （体/测试/update 走 cell），其余撤出覆盖回退寄存器循环——普通 for-let
        // 不付每迭代 cell 代价。循环收尾恢复捕获映射，兄弟语句仍见外层同名绑定的
        // 原 cell。
        let mut head_keep: HashSet<String> = HashSet::new();
        let mut head_names: Option<Vec<String>> = None;
        let mut head_env: Option<ForHeadEnv> = None;
        if let Some(init) = &fr.init {
            if let Some(names) = self.collect_for_init_lexical_names(init) {
                for name in &names {
                    if ctx.captured_bindings.contains_key(name) || self.is_global_tier_name(ctx, name) {
                        head_keep.insert(name.clone());
                    }
                }
                head_env = Some(self.begin_for_head_env(names.clone(), ctx)?);
                ctx.for_head_store_registers = names.iter().cloned().collect();
                head_names = Some(names);
            }
        }
        if let Some(init) = &fr.init {
            if let Some(expr) = init.as_expression() {
                self.emit_expression(expr, ctx)?;
            } else if let ForStatementInit::VariableDeclaration(decl) = init {
                // C 风格 for 头 let/const 名在 init 表达式发射期间视为可见：头名不像
                // 块级 let/const 那样在块入口预声明，init 内闭包创建点头名尚不在符号表，
                // 可见性过滤会误删其前向捕获（见 `CompileCtx::pending_for_head_names`）。
                let saved_pending = ctx.pending_for_head_names.clone();
                if !matches!(decl.kind, VariableDeclarationKind::Var) {
                    for d in &decl.declarations {
                        let mut head_names = Vec::new();
                        self.collect_pattern_binding_names(&d.id, &mut head_names);
                        ctx.pending_for_head_names.extend(head_names);
                    }
                }
                for d in &decl.declarations {
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    if let Some(init_expr) = &d.init {
                        let val_reg = self.emit_expression(init_expr, ctx)?;
                        // 捕获头名的寄存器补写在 `emit_bind_target` 经
                        // `for_head_store_registers` 完成（解构叶用各自的叶值，不用整头 RHS）。
                        self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, false, ctx)?;
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
                        // 词法头无初始化声明：覆盖 cell 须置为已初始化（undefined），
                        // 否则后续 init 表达式内创建、捕获该头名的闭包读时误报 TDZ。
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            if let Some(&cell_idx) = ctx.captured_bindings.get(bi.name.as_str()) {
                                ctx.inst(Inst::new(
                                    OpCode::MAKE_CELL,
                                    Operand::Reg(tmp),
                                    Operand::Imm(cell_idx as u16),
                                    Operand::None,
                                ));
                            }
                        }
                        ctx.init_var(bi.name.as_str());
                    }
                    // 记录 let/const 循环头声明名（update 段写豁免所需）；fresh cell 判定
                    // 在 init 后按保留集统一重算（见下方）。
                    if !matches!(decl.kind, VariableDeclarationKind::Var) {
                        let mut names = Vec::new();
                        self.collect_pattern_binding_names(&d.id, &mut names);
                        for name in &names {
                            update_names.push(name.clone());
                        }
                    }
                }
                ctx.pending_for_head_names = saved_pending;
            }
        }
        ctx.for_head_store_registers.clear();
        // 撤出非保留头名的覆盖：普通 for-let 回退寄存器循环，只有被闭包引用或与顶层
        // var/函数同名的头名保留独立 cell 贯穿循环。
        for name in head_names.iter().flatten() {
            if !head_keep.contains(name) {
                ctx.captured_bindings.remove(name);
            }
        }
        // 每迭代 fresh cell 的绑定：保留覆盖者（仍含 cell 下标）；已撤出覆盖的头名
        // 走寄存器，不含在内。
        let mut fresh_bindings: Vec<(String, u8)> = Vec::new();
        for name in &update_names {
            if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                fresh_bindings.push((name.clone(), cell_idx));
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
        let body_result = self.emit_statement(&fr.body, ctx)?;
        ctx.labels.set_label_pos(update_label, ctx.insts.len());
        if let Some(update) = &fr.update {
            // update 段写循环变量寄存器、不写 cell：被捕获的 let/const 循环变量每迭代
            // 新分配 cell，寄存器值供其拷入（否则 CELL_SET 会污染本迭代闭包捕获的
            // cell）；未捕获者本就走寄存器，一并登记以豁免 update 段的 const 检查。
            // 机制见 `compile_ctx.rs` 的 `register_update_names` 字段文档。
            let prev = std::mem::take(&mut ctx.register_update_names);
            ctx.register_update_names = update_names;
            self.emit_expression(update, ctx)?;
            ctx.register_update_names = prev;
        }
        // 恢复捕获映射：循环后的兄弟语句仍见外层同名绑定的原 cell（词法头环境
        // 作用是语句局部的）。
        if let Some(env) = head_env {
            self.restore_for_head_env(env, ctx);
        }
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
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
