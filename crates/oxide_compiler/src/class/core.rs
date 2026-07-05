use crate::compiler::{CompileCtx, Compiler};

impl Compiler {
    pub(crate) fn count_class(&self, class: &oxide_parser::Class, ctx: &mut CompileCtx) {
        self.count_class_header(class, ctx);
        self.count_class_prototype(class.super_class.is_some(), ctx);
        self.count_class_methods(&class.body.body, ctx);
        self.count_class_static_fields(&class.body.body, ctx);
        self.count_class_static_blocks(&class.body.body, ctx);
    }
}
