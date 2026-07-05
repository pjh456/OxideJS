use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{ForStatementLeft, Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn count_for_in_statement(&self, stmt: &oxide_parser::ForInStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let start_label = Label::ForInStart(id);
        let end_label = Label::ForInEnd(id);
        self.count_expression(&stmt.right, ctx);
        ctx.count_instr();
        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        ctx.count_instr();
        ctx.count_jump();
        ctx.count_jump();
        ctx.count_instr();
        match &stmt.left {
            oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                for _d in &decl.declarations {
                    ctx.alloc_reg();
                    ctx.count_instr();
                }
            }
            oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr();
            }
            _ => {}
        }
        self.count_statement(&stmt.body, ctx);
        ctx.count_jump();
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
        ctx.count_instr();
    }

    pub(crate) fn emit_for_in_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ForInStatement(fi) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::ForInStart(id);
        let end_label = Label::ForInEnd(id);
        let obj_reg = self.emit_expression(&fi.right, ctx)?;
        ctx.emit(opcode::encode(OpCode::FOR_IN_INIT, 0, obj_reg, 0));
        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let done_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_IN_DONE, done_reg, 0, 0));
        let end_pos = ctx.resolve_label(end_label)?;
        let cleanup_jmp_offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
        ctx.emit(opcode::encode_jmp_if_false(done_reg, 2));
        let cleanup_jmp_offset = ctx.checked_jump_offset(cleanup_jmp_offset);
        ctx.emit(opcode::encode_jmp(cleanup_jmp_offset));
        let key_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_IN_NEXT, key_reg, 0, 0));
        match &fi.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    let name = match &d.id {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                        _ => return Err("destructuring not supported".into()),
                    };
                    let var_reg = ctx.alloc_reg();
                    ctx.declare(name, var_reg, decl.kind, matches!(decl.kind, VariableDeclarationKind::Const))?;
                    ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                    ctx.init_var(name);
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
            }
            _ => return Err("unsupported for-in left-hand side".into()),
        }
        self.emit_statement(&fi.body, ctx)?;
        let start_pos = ctx.resolve_label(start_label)?;
        let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));
        ctx.emit(opcode::encode(OpCode::FOR_IN_CLEANUP, 0, 0, 0));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
