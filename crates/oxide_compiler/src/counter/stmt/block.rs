use super::*;

impl Compiler {
    fn count_block_statement(&self, block: &oxide_parser::BlockStatement<'_>, ctx: &mut CompileCtx) {
        for s in &block.body {
            self.count_statement(s, ctx);
        }
    }

    pub(in crate::counter) fn count_block_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        if let Statement::BlockStatement(block) = stmt {
            self.count_block_statement(block, ctx);
        }
    }
}
