use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::ClassElement;

impl Compiler {
    pub(crate) fn count_class_static_blocks(&self, elements: &[ClassElement], ctx: &mut CompileCtx) {
        for element in elements {
            if let ClassElement::StaticBlock(block) = element {
                for stmt in &block.body {
                    self.count_statement(stmt, ctx);
                }
            }
        }
    }

    pub(crate) fn emit_class_static_blocks(
        &self, elements: &[ClassElement], ctor_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        for element in elements {
            if let ClassElement::StaticBlock(block) = element {
                let saved_static_this = ctx.static_block_this_reg;
                ctx.static_block_this_reg = Some(ctor_reg);
                ctx.push_scope();
                for stmt in &block.body {
                    self.emit_statement(stmt, ctx)?;
                }
                ctx.pop_scope();
                ctx.static_block_this_reg = saved_static_this;
            }
        }
        Ok(())
    }
}
