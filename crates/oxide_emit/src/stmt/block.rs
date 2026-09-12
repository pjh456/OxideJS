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
        let mut r = None;
        for s in &block.body {
            if let Some(rr) = self.emit_statement(s, ctx)? {
                r = Some(rr);
            }
        }
        ctx.pop_scope();
        Ok(r)
    }

    pub(crate) fn emit_block_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        match stmt {
            Statement::BlockStatement(_) => self.emit_block_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
