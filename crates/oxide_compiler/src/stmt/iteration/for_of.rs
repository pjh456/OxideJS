use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{ForStatementLeft, Statement};

impl Compiler {
    pub(crate) fn count_for_of_statement(&self, stmt: &oxide_parser::ForOfStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let start_label = Label::ForOfStart(id);
        let end_label = Label::ForOfEnd(id);
        self.count_expression(&stmt.right, ctx);
        ctx.count_instr();
        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        ctx.alloc_reg();
        ctx.count_instr();
        ctx.count_jump();
        ctx.alloc_reg();
        ctx.count_instr();
        match &stmt.left {
            oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    self.count_binding_pattern(&d.id, ctx);
                }
            }
            oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr();
            }
            oxide_parser::ForStatementLeft::ArrayAssignmentTarget(ap) => {
                self.count_array_assignment(ap, ctx);
            }
            oxide_parser::ForStatementLeft::ObjectAssignmentTarget(op) => {
                self.count_object_assignment(op, ctx);
            }
            _ => {}
        }
        self.count_statement(&stmt.body, ctx);
        ctx.count_jump();
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
        ctx.count_instr();
    }

    pub(crate) fn emit_for_of_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ForOfStatement(fo) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::ForOfStart(id);
        let end_label = Label::ForOfEnd(id);
        let iter_src_reg = self.emit_expression(&fo.right, ctx)?;
        ctx.emit(opcode::encode(OpCode::FOR_OF_INIT, 0, iter_src_reg, 0));
        ctx.labels.label_map.insert(start_label, ctx.bytecode.len());
        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let has_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_OF_DONE, has_reg, 0, 0));
        ctx.emit_jmp_if_false_labeled(has_reg, end_label);
        let val_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_OF_NEXT, val_reg, 0, 0));
        match &fo.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    self.emit_binding_pattern(&d.id, val_reg, decl.kind, false, ctx)?;
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
            }
            ForStatementLeft::ArrayAssignmentTarget(ap) => {
                self.emit_array_assignment(ap, val_reg, ctx)?;
            }
            ForStatementLeft::ObjectAssignmentTarget(op) => {
                self.emit_object_assignment(op, val_reg, ctx)?;
            }
            _ => return Err("unsupported for-of left-hand side".into()),
        }
        self.emit_statement(&fo.body, ctx)?;
        ctx.emit_jmp_labeled(start_label);
        ctx.labels.label_map.insert(end_label, ctx.bytecode.len());
        ctx.emit(opcode::encode(OpCode::FOR_OF_CLOSE, 0, 0, 0));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
