use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_block_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
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
}
