use crate::compiler::{CompileCtx, Compiler};
use crate::ir::operand::Operand;
use oxide_parser::{ClassElement, PropertyKey};

impl Compiler {
    /// Emit static field initializers and static blocks in source order.
    /// JS class semantics interleave them (e.g. `static x = 1; static {
    /// this.y = 1 } static z = this.y + 1` must run in that order). Emitting
    /// all fields before all blocks would make `z` read `y` before it is set.
    pub(crate) fn emit_class_static_elements(
        &self, elements: &[ClassElement], ctor_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let saved_static_this = ctx.static_block_this_reg;
        ctx.static_block_this_reg = Some(ctor_reg);
        for element in elements {
            match element {
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if prop.r#static {
                        if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                            self.emit_private_field_init(Operand::Reg(ctor_reg as u32), private.name.as_str(), prop.value.as_ref(), ctx)?;
                        } else {
                            self.emit_public_field_init(Operand::Reg(ctor_reg as u32), &prop.key, prop.computed, prop.value.as_ref(), ctx)?;
                        }
                    }
                }
                ClassElement::StaticBlock(block) => {
                    ctx.push_scope();
                    for stmt in &block.body {
                        self.emit_statement(stmt, ctx)?;
                    }
                    ctx.pop_scope();
                }
                _ => {}
            }
        }
        ctx.static_block_this_reg = saved_static_this;
        Ok(())
    }
}
