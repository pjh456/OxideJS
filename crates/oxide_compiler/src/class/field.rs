use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::{ClassElement, PropertyKey};

impl Compiler {
    pub(crate) fn count_class_static_fields(&self, elements: &[ClassElement], ctx: &mut CompileCtx) {
        for element in elements {
            if let ClassElement::PropertyDefinition(prop) = element {
                let prop = prop.as_ref();
                if prop.r#static {
                    self.count_class_key(&prop.key, prop.computed, ctx);
                    if let Some(value) = &prop.value {
                        self.count_expression(value, ctx);
                    } else {
                        ctx.count_load_const();
                    }
                    ctx.count_instr();
                }
            }
        }
    }

    pub(crate) fn emit_class_static_fields(
        &self, elements: &[ClassElement], ctor_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        for element in elements {
            if let ClassElement::PropertyDefinition(prop) = element {
                let prop = prop.as_ref();
                if prop.r#static {
                    let saved_static_this = ctx.static_block_this_reg;
                    ctx.static_block_this_reg = Some(ctor_reg);
                    if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                        self.emit_private_field_init(ctor_reg, private.name.as_str(), prop.value.as_ref(), ctx)?;
                    } else {
                        self.emit_public_field_init(ctor_reg, &prop.key, prop.computed, prop.value.as_ref(), ctx)?;
                    }
                    ctx.static_block_this_reg = saved_static_this;
                }
            }
        }
        Ok(())
    }
}
