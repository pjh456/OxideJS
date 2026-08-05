use crate::compiler::{CompileCtx, Compiler};
use crate::ir::inst::Inst;
use crate::ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{ForStatementLeft, Statement};

impl Compiler {
    pub(crate) fn emit_for_of_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ForOfStatement(fo) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let iter_src_reg = self.emit_expression(&fo.right, ctx)?;
        ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(iter_src_reg as u32), Operand::None));
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg as u32), Operand::None, Operand::None));
        ctx.inst(Inst::jmp_if_false(has_reg, end_label));
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg as u32), Operand::None, Operand::None));
        match &fo.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    self.emit_binding_pattern(&d.id, val_reg, decl.kind, false, ctx)?;
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(val_reg as u32), Operand::None));
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
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
