use super::*;

impl Compiler {
    pub(in crate::counter) fn count_variable_declaration(
        &self, decl: &oxide_parser::VariableDeclaration<'_>, ctx: &mut CompileCtx,
    ) {
        for d in &decl.declarations {
            if let Some(init) = &d.init {
                self.count_expression(init, ctx);
                self.count_binding_pattern(&d.id, ctx);
            } else {
                ctx.alloc_reg();
                ctx.count_words(2); // LOAD_CONST(undefined) + STORE_VAR
            }
        }
    }

    pub(in crate::counter) fn count_function_declaration(
        &self, decl: &oxide_parser::Function<'_>, ctx: &mut CompileCtx,
    ) {
        let name = if let Some(id) = &decl.id {
            id.name.to_string()
        } else {
            return;
        };

        let func_reg = ctx.alloc_reg();
        let _ = ctx.declare_initialized(&name, func_reg, VariableDeclarationKind::Var, false);

        // Body is compiled in the emit pass only.
        ctx.count_words(2); // LOAD_CONST(BytecodeFunc) + STORE_VAR
    }

    pub(in crate::counter) fn count_class_declaration(&self, class: &oxide_parser::Class<'_>, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // class binding reg
        self.count_class(class, ctx);
        ctx.projected_pc += 1; // STORE_VAR binding <- ctor
    }
}
