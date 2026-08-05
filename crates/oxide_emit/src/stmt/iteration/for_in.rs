//! for-in 语句 emit：`emit_for_in_statement` 遍历可枚举属性名。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{ForStatementLeft, Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_for_in_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ForInStatement(fi) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        let obj_reg = self.emit_expression(&fi.right, ctx)?;
        ctx.inst(Inst::new(OpCode::FOR_IN_INIT, Operand::None, Operand::Reg(obj_reg as u32), Operand::None));
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let done_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_IN_DONE, Operand::Reg(done_reg as u32), Operand::None, Operand::None));
        // done=true 表示迭代结束：done=false 时跳过 end jmp 继续迭代
        let continue_label = ctx.next_label_id();
        ctx.inst(Inst::jmp_if_false(done_reg, continue_label));
        ctx.inst(Inst::jmp(end_label));
        ctx.labels.set_label_pos(continue_label, ctx.insts.len());
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_IN_NEXT, Operand::Reg(key_reg as u32), Operand::None, Operand::None));
        match &fi.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    let name = match &d.id {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                        _ => return Err("destructuring not supported".into()),
                    };
                    let var_reg = ctx.alloc_reg();
                    ctx.declare(name, var_reg, decl.kind, matches!(decl.kind, VariableDeclarationKind::Const))?;
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(key_reg as u32), Operand::None));
                    ctx.init_var(name);
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(key_reg as u32), Operand::None));
            }
            _ => return Err("unsupported for-in left-hand side".into()),
        }
        self.emit_statement(&fi.body, ctx)?;
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.inst(Inst::new(OpCode::FOR_IN_CLEANUP, Operand::None, Operand::None, Operand::None));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
