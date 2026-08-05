//! 类头 emit：类名绑定、继承链（`extends`）设置，见 `emit_class_header`。

use crate::{CompileCtx, Emitter};

impl Emitter {
    pub(crate) fn emit_class_header(
        &self, class: &oxide_parser::Class, ctx: &mut CompileCtx,
    ) -> Result<(u8, u8, Option<u8>), String> {
        let ctor_reg = ctx.alloc_reg();
        let proto_reg = ctx.alloc_reg();
        let super_reg = if let Some(super_expr) = &class.super_class {
            Some(self.emit_expression(super_expr, ctx)?)
        } else {
            None
        };
        Ok((ctor_reg, proto_reg, super_reg))
    }
}
