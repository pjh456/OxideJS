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
    /// 变量声明语句完成值按规范为空记录（对外 undefined），不回填结果寄存器。
    pub(crate) fn emit_variable_declaration(
        &self, decl: &VariableDeclaration, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        // 循环体内的块级 let/const 声明每迭代重执行：被捕获绑定每迭代实例化新
        // cell，本迭代闭包捕获新 cell、旧闭包保留旧值。var 是规范单绑定，
        // 保持原位更新；函数体/嵌套块经子 ctx 进入时循环栈为空，不命中。
        let reexec = !matches!(decl.kind, VariableDeclarationKind::Var) && !ctx.labels.loop_stack.is_empty();
        for d in &decl.declarations {
            let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
            if is_const && d.init.is_none() {
                return Err("const declaration must have an initializer".into());
            }
            if let Some(init) = &d.init {
                let val_reg = self.emit_expression(init, ctx)?;
                self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, reexec, ctx)?;
                if let BindingPattern::BindingIdentifier(bi) = &d.id {
                    if crate::is_anonymous_function_definition(init) {
                        if let Some(sub_mod) = ctx.nested.last_mut() {
                            if sub_mod.function_name.is_none() {
                                sub_mod.function_name = Some(bi.name.to_string());
                            }
                        }
                    }
                }
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
                        // 重执行迭代取新 cell，与 with-init 臂同一门控。
                        let op = if reexec { OpCode::MAKE_CELL_FRESH } else { OpCode::MAKE_CELL };
                        ctx.inst(Inst::new(op, Operand::Reg(tmp), Operand::Imm(cell_idx as u16), Operand::None));
                    }
                } else if !already_bound {
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(target_reg), Operand::Reg(tmp), Operand::None));
                }
                ctx.init_var(bi.name.as_str());
            }
        }
        Ok(None)
    }
}
