//! 块语句 emit：`emit_block_statement` 压/弹作用域并顺序编译子语句（含域分发）。

use crate::{CompileCtx, Emitter};
use oxide_parser::Statement;

impl Emitter {
    fn emit_block_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::BlockStatement(block) = stmt else {
            return Ok(None);
        };
        ctx.push_scope();
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
