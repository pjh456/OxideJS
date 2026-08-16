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
                let target_reg = if matches!(decl.kind, VariableDeclarationKind::Var) {
                    match ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const) {
                        Ok(()) => var_reg,
                        Err(_) => ctx.lookup(bi.name.as_str()).unwrap_or(var_reg),
                    }
                } else {
                    // let/const 无初始化器：复用本块预声明槽位，否则新声明
                    // （for 头等未预声明路径）。
                    let var_reg = if let Some(reg) = ctx.scopes.symbols.consume_predeclared_slot(bi.name.as_str()) {
                        reg
                    } else {
                        let r = ctx.alloc_reg();
                        ctx.declare(bi.name.as_str(), r, decl.kind, is_const)?;
                        r
                    };
                    var_reg
                };
                if let Some(&cell_idx) = ctx.captured_bindings.get(bi.name.as_str()) {
                    ctx.inst(Inst::new(
                        OpCode::MAKE_CELL,
                        Operand::Reg(tmp),
                        Operand::Imm(cell_idx as u16),
                        Operand::None,
                    ));
                } else {
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(target_reg), Operand::Reg(tmp), Operand::None));
                }
                ctx.init_var(bi.name.as_str());
                // 脚本顶层 var 无初始化：仍须写全局对象属性（值为 undefined）。
                if ctx.is_global_scope && matches!(decl.kind, VariableDeclarationKind::Var) {
                    self.emit_global_prop_write(bi.name.as_str(), tmp, ctx);
                }
                r = Some(var_reg);
            }
        }
        Ok(r)
    }
}
