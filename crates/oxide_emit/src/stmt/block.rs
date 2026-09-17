//! 块语句 emit：`emit_block_statement` 压/弹作用域并顺序编译子语句（含域分发）。

use std::collections::HashMap;

use crate::{CompileCtx, Emitter};
use oxide_parser::Statement;

impl Emitter {
    fn emit_block_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::BlockStatement(block) = stmt else {
            return Ok(None);
        };
        ctx.push_scope();
        // 函数预声明先于 lexical：`{ let g; function g(){} }` 时 lexical 的预声明
        // 命中已存在的函数绑定自然报重复声明错，不被静默覆盖破坏 TDZ。
        self.predeclare_block_function_declarations(&block.body, ctx, false);
        // 块 lexical 声明是局部绑定：不做受限全局名检查（重复声明错在 emit 期报）。
        let _ = self.predeclare_lexical_declarations(&block.body, ctx, false);
        // 块级函数声明的块入口初始化：直接子（含标签直接子体）闭包物化入预
        // 声明块槽，声明语句前读命中函数对象（sloppy 与 strict 同形）；声明
        // 点不再重编闭包，只处理外层 var 写回（见 emit_function_declaration）。
        ctx.block_fn_entry_mats.push(HashMap::new());
        for s in &block.body {
            self.emit_block_fn_entry_init_stmt(s, ctx)?;
        }
        let mut r = None;
        for s in &block.body {
            if let Some(rr) = self.emit_statement(s, ctx)? {
                r = Some(rr);
            }
        }
        ctx.block_fn_entry_mats.pop();
        ctx.pop_scope();
        Ok(r)
    }

    /// 块入口初始化的单语句走法：函数声明就地物化并登记（同名重复声明共享
    /// 块绑定，源序物化使末次声明天然成为入口值）；标签不改变执行流（V8 对
    /// 标签直接子的函数声明同样块入口可见），随标签体递归一层；if/while/
    /// do-while/with 体不进入——支臂声明的执行期才生效（仅被求值支物化，
    /// while/do-while/with 体直挂声明双侧均为语法错误不可达）；嵌套块/循环/
    /// try 各自压作用域、由其自身块入口处理，不进入。
    ///
    /// # 前提
    /// - 预声明已为各声明名建立块 Let 绑定；`materialize_function_declaration`
    ///   经 `lookup` 解析槽位。
    ///
    /// # 副作用
    /// - 当前指令流在块入口发 CREATE_CLOSURE（及 MAKE_CELL / STORE_VAR）；
    ///   物化登记写入 `block_fn_entry_mats` 栈顶帧。
    fn emit_block_fn_entry_init_stmt(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<(), String> {
        match stmt {
            Statement::FunctionDeclaration(fd) => {
                let Some(id) = &fd.id else {
                    return Ok(());
                };
                let name = id.name.to_string();
                self.materialize_function_declaration(fd, &name, ctx)?;
                // 登记声明节点 arena 地址（Box 解引到 arena 本体）：声明点
                // 以同一节点地址判同一，区分本声明入口物化与同名支臂。
                ctx.block_fn_entry_mats
                    .last_mut()
                    .unwrap()
                    .insert(name, &**fd as *const _ as *const ());
                Ok(())
            }
            Statement::LabeledStatement(ls) => self.emit_block_fn_entry_init_stmt(&ls.body, ctx),
            _ => Ok(()),
        }
    }

    pub(crate) fn emit_block_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        match stmt {
            Statement::BlockStatement(_) => self.emit_block_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
