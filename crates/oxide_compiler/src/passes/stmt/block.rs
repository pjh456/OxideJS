use super::*;

impl Compiler {
    fn count_block_statement(&self, block: &oxide_parser::BlockStatement<'_>, ctx: &mut CompileCtx) {
        for s in &block.body {
            self.count_statement(s, ctx);
        }
    }

    fn emit_block_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
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

    pub(crate) fn count_block_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        if let Statement::BlockStatement(block) = stmt {
            self.count_block_statement(block, ctx);
        }
    }

    pub(crate) fn emit_block_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::BlockStatement(_) => self.emit_block_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
