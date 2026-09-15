//! 块语句 emit：`emit_block_statement` 压/弹作用域并顺序编译子语句（含域分发）。

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
        self.predeclare_block_function_declarations(&block.body, ctx);
        // 块 lexical 声明是局部绑定：不做受限全局名检查（重复声明错在 emit 期报）。
        let _ = self.predeclare_lexical_declarations(&block.body, ctx, false);
        // 块级函数声明的块入口初始化：闭包物化入预声明块槽，声明语句前读命中
        // 函数对象（sloppy 与 strict 同形）；声明点退化为其守语义。
        for s in &block.body {
            self.emit_block_fn_entry_init_stmt(s, ctx)?;
        }
        let mut r = None;
        for s in &block.body {
            if let Some(rr) = self.emit_statement(s, ctx)? {
                r = Some(rr);
            }
        }
        ctx.pop_scope();
        Ok(r)
    }

    /// 块入口初始化的单语句走法：函数声明就地物化（同名重复声明共享块绑定，
    /// 源序物化使末次声明天然成为入口值）；if/labeled/while/do-while/with 体
    /// 不推作用域、随本块绑定递归；嵌套块/循环/try 各自压作用域、由其自身块
    /// 入口处理，不进入。
    ///
    /// # 前提
    /// - 预声明已为各声明名建立块 Let 绑定；`materialize_function_declaration`
    ///   经 `lookup` 解析槽位。
    fn emit_block_fn_entry_init_stmt(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<(), String> {
        match stmt {
            Statement::FunctionDeclaration(fd) => {
                let Some(id) = &fd.id else {
                    return Ok(());
                };
                let name = id.name.to_string();
                self.materialize_function_declaration(fd, &name, ctx)?;
                Ok(())
            }
            Statement::IfStatement(is) => {
                self.emit_block_fn_entry_init_stmt(&is.consequent, ctx)?;
                if let Some(alt) = &is.alternate {
                    self.emit_block_fn_entry_init_stmt(alt, ctx)?;
                }
                Ok(())
            }
            Statement::WhileStatement(wh) => self.emit_block_fn_entry_init_stmt(&wh.body, ctx),
            Statement::DoWhileStatement(dw) => self.emit_block_fn_entry_init_stmt(&dw.body, ctx),
            Statement::LabeledStatement(ls) => self.emit_block_fn_entry_init_stmt(&ls.body, ctx),
            Statement::WithStatement(ws) => self.emit_block_fn_entry_init_stmt(&ws.body, ctx),
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
