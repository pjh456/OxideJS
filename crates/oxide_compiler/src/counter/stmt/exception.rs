use super::*;

impl Compiler {
    pub(in crate::counter) fn count_throw_statement(
        &self, stmt: &oxide_parser::ThrowStatement<'_>, ctx: &mut CompileCtx,
    ) {
        self.count_expression(&stmt.argument, ctx);
        ctx.projected_pc += 1; // THROW
    }

    pub(in crate::counter) fn count_try_statement(&self, stmt: &oxide_parser::TryStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let catch_label = Label::CatchBody(id);
        let try_end_label = Label::TryEnd(id);
        let has_catch = stmt.handler.is_some();
        let has_finally = stmt.finalizer.is_some();

        ctx.alloc_reg(); // result_reg

        if has_finally {
            ctx.projected_pc += 1; // TRY_FINALLY_BEGIN (before try body)
        }

        if has_catch {
            ctx.projected_pc += 1; // TRY_BEGIN (before try body)
        }

        for s in &stmt.block.body {
            self.count_statement(s, ctx);
        }
        ctx.projected_pc += 1; // LOAD_VAR result_reg (if try body has result)

        if has_catch {
            ctx.projected_pc += 1; // TRY_END
        }

        let jmp_needed = has_catch || has_finally;
        if jmp_needed {
            ctx.projected_pc += 1; // JMP
        }

        ctx.label_map.insert(catch_label, ctx.projected_pc);
        if let Some(catch) = &stmt.handler {
            ctx.push_scope();
            if let Some(_param) = &catch.param {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // STORE_VAR
            }
            for cs in &catch.body.body {
                self.count_statement(cs, ctx);
            }
            ctx.projected_pc += 1; // LOAD_VAR result_reg (if catch body has result)
            ctx.pop_scope();
        }

        if let Some(finally) = &stmt.finalizer {
            let finally_label = Label::FinallyBody(id);
            ctx.label_map.insert(finally_label, ctx.projected_pc);
            for fs in &finally.body {
                self.count_statement(fs, ctx);
            }
            ctx.projected_pc += 1; // LOAD_VAR result_reg (if finally has result)
            ctx.projected_pc += 1; // TRY_FINALLY_END
        }

        ctx.label_map.insert(try_end_label, ctx.projected_pc);
    }
}
