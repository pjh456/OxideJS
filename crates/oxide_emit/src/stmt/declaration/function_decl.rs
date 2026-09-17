//! 函数声明语句 emit：`emit_function_declaration_statement` 创建闭包并绑定函数名。

use crate::symbol_table::ScopeKind;
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
        // 块级函数声明按入口物化状态三分（块槽预声明 Let 绑定）：
        // - 本声明已入口物化（块直接子/标签直接子体）：块槽持有本声明闭包，
        //   复用槽位保闭包同一，不重编；
        // - 同名直接子已入口物化（支臂与直接子共享块槽）：支臂声明只更新外层
        //   var 绑定、块绑定不动（V8 双绑定面，引擎以块槽 + 外层 var 两槽模拟），
        //   闭包落 fresh 寄存器写回外层 var；
        // - 其余（支臂首次求值）：执行期自物化闭包写入块槽。
        // 非块路径（顶层/函数作用域声明点）恒自物化。
        let binding_scope_kind = ctx
            .scopes
            .symbols
            .lookup_any_binding(&name)
            .map(|(_, scope_idx)| ctx.scopes.symbols.scopes[scope_idx].kind);
        let node = fd as *const _ as *const ();
        let var_reg = if binding_scope_kind == Some(ScopeKind::BlockScope) {
            match ctx.block_fn_entry_mats.last().and_then(|m| m.get(&name)) {
                Some(&m) if m == node => ctx.lookup(&name)?,
                Some(_) => self.materialize_function_declaration_var_only(fd, &name, ctx)?,
                None => self.materialize_function_declaration(fd, &name, ctx)?,
            }
        } else {
            self.materialize_function_declaration(fd, &name, ctx)?
        };
        // 块内函数声明求值后按三种情形处理：
        //
        // sloppy 模式下按浏览器/web 实现惯例为块级函数声明建外层 var 绑定并写回：
        // 预声明期为该名实例化的函数作用域 var 绑定在此写入块槽位里的函数对象，
        // 求值一次写回一次（循环体内每迭代覆写头绑定）。
        //
        // 形参或函数作用域树内同名词法声明（抑制集）不写回：该名被同名声明遮蔽。
        //
        // 顶层不可写内置名不写回：sloppy 下对其赋值（put）永不成功，跳过与执行等价；
        // 顶层情形另补全局对象属性写（与 for-in 循环头的顶层写同形）。
        if !ctx.is_strict
            && ctx.scopes.symbols.scopes.len() > 1
            && !ctx.block_fn_suppressed.contains(&name)
            && !(ctx.is_global_scope && CompileCtx::is_non_writable_global_builtin(&name))
        {
            let var_target = ctx.scopes.symbols.find_var_target_scope();
            if let Some(outer) = ctx.scopes.symbols.scopes[var_target].bindings.get(&name) {
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(outer.reg),
                    Operand::Reg(var_reg),
                    Operand::None,
                ));
                // 全局对象属性写以「外层绑定的作用域层级是否为 0（全局作用域）」判定，
                // 不按名字解析：此处块内 Let 绑定已占内层作用域，按名解析
                // （`is_global_tier_name`）会命中块绑定误判为非顶层；嵌套函数作用域
                // 的同名局部 var 不在全局 ctx 上，经 `is_global_scope` 门禁不走全局对象写。
                if ctx.is_global_scope && var_target == 0 && ctx.global_tier_names.contains(&name) {
                    self.emit_tier_global_write(&name, var_reg, ctx);
                }
            }
        }
        // 脚本顶层（非块内）函数声明：同步写全局对象，使 globalThis 可反射函数名。
        // 块内声明不走本臂（其全局可见性只经上方外层 var 写回面建立）。
        // 严格 eval 代码的函数声明绑定 eval 自身词法环境、不落全局对象：抑制全局对象属性写
        // （局部绑定读面未暴露），同时避免不可写全局内置名在 `DEFINE_GLOBAL_PROP_C`（`0x98`，
        // 严格 eval 路径经此指令）上误抛 strict `TypeError`。
        if ctx.is_global_scope && ctx.scopes.symbols.scopes.len() == 1 && !(ctx.is_eval_script && ctx.is_strict) {
            self.emit_global_func_bind_write(&name, var_reg, ctx);
        }
        Ok(None)
    }

    /// 函数声明物化：建形参 spec、编译体为子模块、创建闭包并把函数对象写入
    /// 声明绑定槽（名被捕获时同步建 cell）。顶层声明点与块级块入口初始化共用。
    ///
    /// # 前提
    /// - 声明名已有已初始化绑定（顶层为 var 预声明、块为 Let 预声明）；
    ///   `lookup` 对未声明名失败。
    ///
    /// # 副作用
    /// - 子模块登记进嵌套模块表；当前指令流发 CREATE_CLOSURE（及 MAKE_CELL /
    ///   STORE_VAR）。
    ///
    /// # 注意事项
    /// - 闭包的 upvalue/cell 捕获在函数入口的捕获分析期固定，本调用点的时序
    ///   只决定 CREATE_CLOSURE 在指令流中的落点。
    pub(crate) fn materialize_function_declaration(
        &self, fd: &oxide_parser::Function, name: &str, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let var_reg = ctx.lookup(name)?;
        ctx.reserve_reg(var_reg);
        self.materialize_function_closure(fd, name, var_reg, ctx, true)?;
        Ok(var_reg)
    }

    /// G 形支臂物化变体：闭包落 fresh 寄存器，不写块槽、不建 cell（同名
    /// 直接子占用块槽，嵌套捕获读槽 cell 与 V8 块绑定一致）。调用点经返回
    /// 寄存器把闭包写回外层 var 绑定。
    pub(crate) fn materialize_function_declaration_var_only(
        &self, fd: &oxide_parser::Function, name: &str, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let reg = ctx.alloc_reg();
        self.materialize_function_closure(fd, name, reg, ctx, false)?;
        Ok(reg)
    }

    fn materialize_function_closure(
        &self, fd: &oxide_parser::Function, name: &str, reg: u32, ctx: &mut CompileCtx, write_slot: bool,
    ) -> Result<(), String> {
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
        sub_module.function_name = Some(name.to_string());
        ctx.nested.push(sub_module);
        ctx.inst(Inst::create_closure(Operand::Reg(reg), ctx.nested.len() as u16));
        if write_slot {
            if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                ctx.inst(Inst::new(
                    OpCode::MAKE_CELL,
                    Operand::Reg(reg),
                    Operand::Imm(cell_idx as u16),
                    Operand::None,
                ));
            } else {
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(reg), Operand::Reg(reg), Operand::None));
            }
        }
        Ok(())
    }
}
