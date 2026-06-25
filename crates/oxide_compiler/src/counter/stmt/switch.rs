use super::*;

impl Compiler {
    pub(in crate::counter) fn count_switch_statement(
        &self, stmt: &oxide_parser::SwitchStatement<'_>, ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        let end_label = Label::SwitchEnd(id);
        ctx.push_switch(end_label);

        self.count_expression(&stmt.discriminant, ctx);
        let compare_reg_checkpoint = ctx.reg_checkpoint();

        let cases = &stmt.cases;
        for case in cases.iter() {
            if let Some(test) = &case.test {
                self.count_expression(test, ctx);
                ctx.projected_pc += 1; // EQ
                ctx.alloc_reg(); // eq result
                ctx.projected_pc += 1; // JMP_IF_TRUE
                ctx.restore_reg_checkpoint(compare_reg_checkpoint);
            }
        }

        let has_default = cases.iter().any(|c| c.test.is_none());
        if !has_default {
            ctx.projected_pc += 1; // JMP to SwitchEnd (no match)
        }

        for (case_idx, case) in cases.iter().enumerate() {
            let case_label = Label::SwitchCase(id, case_idx as u32);
            ctx.labels.label_map.insert(case_label, ctx.projected_pc);
            for s in &case.consequent {
                self.count_statement(s, ctx);
            }
        }

        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
        ctx.pop_switch();
    }
}
