use super::*;

impl Compiler {
    pub(in crate::counter) fn count_block_statement(
        &self, block: &oxide_parser::BlockStatement<'_>, ctx: &mut CompileCtx,
    ) {
        for s in &block.body {
            self.count_statement(s, ctx);
        }
    }
}
