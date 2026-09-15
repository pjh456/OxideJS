//! 函数声明语句 emit：`emit_function_declaration_statement` 创建闭包并绑定函数名。

use crate::{CompileCtx, Emitter, ParamSpec};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_function_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let Statement::FunctionDeclaration(fd) = stmt else {
            return Err("FunctionDeclaration without name".into());
        };
        self.emit_function_declaration(fd, ctx)
    }

    /// 函数声明本体：供 export 声明复用（导出声明先按普通声明 emit，再就地注册导出值）。
    pub(crate) fn emit_function_declaration(
        &self, fd: &oxide_parser::Function, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let name = if let Some(id) = &fd.id {
            id.name.to_string()
        } else {
            return Err("FunctionDeclaration without name".into());
        };
        let mut param_names = Vec::new();
        for (idx, param) in fd.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => param_names.push(ParamSpec::Identifier {
                    name: bi.name.to_string(),
                    initializer: param.initializer.as_deref(),
                }),
                pattern => param_names.push(ParamSpec::Pattern {
                    synthetic_name: format!("@@param_{idx}"),
                    pattern,
                    initializer: param.initializer.as_deref(),
                }),
            }
        }
        if let Some(rest) = &fd.params.rest {
            self.push_rest_param(&rest.rest.argument, &mut param_names)?;
        }
        let body_stmts: &[Statement] = if let Some(body) = &fd.body { &body.statements } else { &[] };
        let own_strict = fd.has_use_strict_directive();
        let mut sub_module = if fd.generator && fd.r#async {
            self.compile_async_generator_body(&param_names, body_stmts, ctx, own_strict)?
        } else if fd.generator {
            self.compile_generator_body(&param_names, body_stmts, ctx, own_strict)?
        } else if fd.r#async {
            self.compile_async_body(&param_names, body_stmts, ctx, false, false, own_strict)?
        } else {
            self.compile_function_body(&param_names, body_stmts, ctx, false, false, own_strict)?
        };
        sub_module.function_name = Some(name.clone());
        ctx.nested.push(sub_module);
        let var_reg = ctx.lookup(&name)?;
        ctx.reserve_reg(var_reg);
        ctx.inst(Inst::create_closure(Operand::Reg(var_reg), ctx.nested.len() as u16));
        if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
            ctx.inst(Inst::new(
                OpCode::MAKE_CELL,
                Operand::Reg(var_reg),
                Operand::Imm(cell_idx as u16),
                Operand::None,
            ));
        } else {
            ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg), Operand::Reg(var_reg), Operand::None));
        }
        // 块内函数声明求值写回外层 var 绑定（sloppy web-compat）：预声明期为该
        // 名实例化的外层 var 绑定（函数作用域）在此写入块槽位里的函数对象，
        // 求值一次写回一次（循环体内每迭代覆写头绑定）。形参/词法声明同名
        // （抑制集）与顶层 builtin 名不写回；顶层 A 侧补全局对象属性写（与
        // for 头写同形）。
        if !ctx.is_strict
            && ctx.scopes.symbols.scopes.len() > 1
            && !ctx.block_fn_suppressed.contains(&name)
            && !(ctx.is_global_scope && CompileCtx::is_known_builtin(&name))
        {
            let var_target = ctx.scopes.symbols.find_var_target_scope();
            if let Some(outer) = ctx.scopes.symbols.scopes[var_target].bindings.get(&name) {
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(outer.reg),
                    Operand::Reg(var_reg),
                    Operand::None,
                ));
                // 顶层 A 侧同步写以外层绑定是否落全局作用域为准：此处块内
                // Let 绑定已占内层作用域，按名解析（is_global_tier_name）会
                // 命中块绑定误判为非顶层；嵌套函数作用域同名局部 var 不在
                // 全局 ctx 上，经 is_global_scope 门禁不走全局对象写。
                if ctx.is_global_scope && var_target == 0 && ctx.global_tier_names.contains(&name) {
                    self.emit_tier_global_write(&name, var_reg, ctx);
                }
            }
        }
        // 脚本顶层（非块内）函数声明：同步写全局对象，使 globalThis 可反射函数名。
        // 块内声明不走本臂（其全局可见性只经上方外层 var 写回面建立）。
        // 严格 eval 代码函数声明绑定 eval 自身 lexical 环境、不落全局对象：抑制 A
        // 侧写（局部绑定读面未暴露），同时避免不可写全局内置在 0x98 上误抛 strict。
        if ctx.is_global_scope && ctx.scopes.symbols.scopes.len() == 1 && !(ctx.is_eval_script && ctx.is_strict) {
            self.emit_global_func_bind_write(&name, var_reg, ctx);
        }
        Ok(None)
    }
}
