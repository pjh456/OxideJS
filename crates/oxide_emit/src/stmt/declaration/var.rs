//! var/let/const 声明语句 emit：声明 + 初始化表达式，处理提升与解构 pattern。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{BindingPattern, Statement, VariableDeclaration, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_variable_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let Statement::VariableDeclaration(decl) = stmt else {
            return Ok(None);
        };
        self.emit_variable_declaration(decl, ctx)
    }

    /// 变量声明本体：供 export 声明复用（导出声明需先按普通声明 emit，再就地注册导出值）。
    pub(crate) fn emit_variable_declaration(
        &self, decl: &VariableDeclaration, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let mut r = None;
        for d in &decl.declarations {
            let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
            if is_const && d.init.is_none() {
                return Err("const declaration must have an initializer".into());
            }
            if let Some(init) = &d.init {
                let val_reg = self.emit_expression(init, ctx)?;
                self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, false, ctx)?;
                if let BindingPattern::BindingIdentifier(bi) = &d.id {
                    if crate::is_anonymous_function_definition(init) {
                        if let Some(sub_mod) = ctx.nested.last_mut() {
                            if sub_mod.function_name.is_none() {
                                sub_mod.function_name = Some(bi.name.to_string());
                            }
                        }
                    }
                }
                r = Some(val_reg);
            } else {
                let BindingPattern::BindingIdentifier(bi) = &d.id else {
                    return Err("destructuring declaration requires an initializer".into());
                };
                let idx = ctx.add_constant(Constant::Undefined);
                let tmp = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(tmp), idx));
                let var_reg = ctx.alloc_reg();
                // 无初始化 var 是纯声明而非赋值：绑定已预先存在（提升引用或先前写入）
                // 时保留槽值，仅首次声明把 undefined 物化进槽。
                let (target_reg, already_bound) = if matches!(decl.kind, VariableDeclarationKind::Var) {
                    match ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const) {
                        Ok(()) => (var_reg, false),
                        Err(_) => (ctx.lookup(bi.name.as_str()).unwrap_or(var_reg), true),
                    }
                } else {
                    // let/const 无初始化器：复用本块预声明槽位，否则新声明
                    // （for 头等未预声明路径）。
                    let target = if let Some(reg) = ctx.scopes.symbols.consume_predeclared_slot(bi.name.as_str()) {
                        reg
                    } else {
                        let r = ctx.alloc_reg();
                        ctx.declare(bi.name.as_str(), r, decl.kind, is_const)?;
                        r
                    };
                    (target, false)
                };
                if let Some(&cell_idx) = ctx.captured_bindings.get(bi.name.as_str()) {
                    // 被捕获绑定的 cell 在函数入口已实例化（undefined）；声明语句
                    // 不赋值，仅首次声明刷新初值，已绑定的 cell 保留现值。
                    if !already_bound {
                        ctx.inst(Inst::new(
                            OpCode::MAKE_CELL,
                            Operand::Reg(tmp),
                            Operand::Imm(cell_idx as u16),
                            Operand::None,
                        ));
                    }
                } else if !already_bound {
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(target_reg), Operand::Reg(tmp), Operand::None));
                }
                ctx.init_var(bi.name.as_str());
                // 脚本顶层 var：全局对象属性同步绑定当前值（首次声明即 undefined，
                // 先前已写入则保留写入值，声明不是覆盖性赋值）。被捕获绑定的值
                // 存在 cell 而非槽，从 cell 同步。builtin 名不同步：描述符翻
                // writable 已由 GDI 序言以镜像值完成，未初始化的槽/cell 是
                // undefined，再同步会把现存值（NaN 等）抹掉。
                if ctx.is_global_scope
                    && matches!(decl.kind, VariableDeclarationKind::Var)
                    && !CompileCtx::is_known_builtin(bi.name.as_str())
                {
                    let src = if let Some(&cell_idx) = ctx.captured_bindings.get(bi.name.as_str()) {
                        let r = ctx.alloc_reg();
                        if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(bi.name.as_str()) {
                            ctx.inst(Inst::new(
                                OpCode::CELL_GET,
                                Operand::Reg(r),
                                Operand::Reg(binding.reg),
                                Operand::Imm(cell_idx as u16),
                            ));
                        } else {
                            ctx.inst(Inst::new(
                                OpCode::CELL_GET,
                                Operand::Reg(r),
                                Operand::None,
                                Operand::Imm(cell_idx as u16),
                            ));
                        }
                        r
                    } else {
                        target_reg
                    };
                    self.emit_global_prop_write(bi.name.as_str(), src, ctx);
                }
                r = Some(var_reg);
            }
        }
        Ok(r)
    }
}
