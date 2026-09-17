//! 数组字面量 emit：`emit_array_expression` 逐元素求值并建数组（含 holes 与 spread）。
//!
//! 纯静态数组按编译期索引直写；含 spread 时用运行期 index 寄存器贯穿整个数组，
//! spread 元素展开为 for-of 循环追加，后续元素从当前 index 继续，保证求值顺序正确。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::ArrayExpressionElement;
impl Emitter {
    pub(crate) fn emit_array_expression(
        &self, arr: &oxide_parser::ArrayExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let arr_reg = ctx.alloc_reg();
        let has_spread = arr.elements.iter().any(|e| e.is_spread());

        if !has_spread {
            // 纯静态数组：NEW_ARRAY 预置元素个数，holes 跳过索引不写，length 由 n 覆盖。
            let n = arr.elements.len() as u16;
            ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(arr_reg), Operand::Imm(n), Operand::None));
            for (i, elem) in arr.elements.iter().enumerate() {
                let Some(e) = elem.as_expression() else {
                    // hole 元素：NEW_ARRAY 已按 n 预填本下标的自有属性（初值 undefined），
                    // 显式删除使 hole 不是自有属性；length 不受 delete 影响，仍为 n。
                    let scratch = ctx.alloc_reg();
                    ctx.inst(Inst::inst_mov(Operand::Reg(scratch), Operand::Reg(arr_reg)));
                    let idx = ctx.add_constant(Constant::Int(i as i32));
                    ctx.inst(Inst::delete_prop_static(Operand::Reg(scratch), idx as u32));
                    continue;
                };
                let val_reg = self.emit_expression(e, ctx)?;
                let idx_reg = ctx.alloc_reg();
                let idx = ctx.add_constant(Constant::Int(i as i32));
                ctx.inst(Inst::load_const(Operand::Reg(idx_reg), idx));
                ctx.inst(Inst::new(
                    OpCode::SET_ELEM,
                    Operand::Reg(arr_reg),
                    Operand::Reg(idx_reg),
                    Operand::Reg(val_reg),
                ));
            }
            return Ok(arr_reg);
        }

        // 含 spread：NEW_ARRAY 预置静态元素个数（length 下界），运行期 index 贯穿全程。
        let static_count = arr.elements.iter().filter(|e| !e.is_spread()).count() as u16;
        ctx.inst(Inst::new(
            OpCode::NEW_ARRAY,
            Operand::Reg(arr_reg),
            Operand::Imm(static_count),
            Operand::None,
        ));
        let index_reg = ctx.alloc_reg();
        let zero_idx = ctx.add_constant(Constant::Int(0));
        ctx.inst(Inst::load_const(Operand::Reg(index_reg), zero_idx));
        let one_reg = ctx.alloc_reg();
        let one_idx = ctx.add_constant(Constant::Int(1));
        ctx.inst(Inst::load_const(Operand::Reg(one_reg), one_idx));

        for elem in &arr.elements {
            if let Some(e) = elem.as_expression() {
                let val_reg = self.emit_expression(e, ctx)?;
                ctx.inst(Inst::new(
                    OpCode::SET_ELEM,
                    Operand::Reg(arr_reg),
                    Operand::Reg(index_reg),
                    Operand::Reg(val_reg),
                ));
                ctx.inst(Inst::new(
                    OpCode::ADD,
                    Operand::Reg(index_reg),
                    Operand::Reg(index_reg),
                    Operand::Reg(one_reg),
                ));
            } else if elem.is_elision() {
                // hole：只推进 index，不写元素，length 由后续 SET_ELEM 越界扩容或初值覆盖。
                ctx.inst(Inst::new(
                    OpCode::ADD,
                    Operand::Reg(index_reg),
                    Operand::Reg(index_reg),
                    Operand::Reg(one_reg),
                ));
            } else {
                let ArrayExpressionElement::SpreadElement(sp) = elem else {
                    unreachable!("数组元素非表达式、hole 即 spread")
                };
                let iter_src_reg = self.emit_expression(&sp.argument, ctx)?;
                ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(iter_src_reg), Operand::None));
                let start_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                ctx.labels.set_label_pos(start_label, ctx.insts.len());
                let has_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg), Operand::None, Operand::None));
                ctx.inst(Inst::jmp_if_false(has_reg, end_label));
                let val_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg), Operand::None, Operand::None));
                ctx.inst(Inst::new(
                    OpCode::SET_ELEM,
                    Operand::Reg(arr_reg),
                    Operand::Reg(index_reg),
                    Operand::Reg(val_reg),
                ));
                ctx.inst(Inst::new(
                    OpCode::ADD,
                    Operand::Reg(index_reg),
                    Operand::Reg(index_reg),
                    Operand::Reg(one_reg),
                ));
                ctx.inst(Inst::jmp(start_label));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
            }
        }
        Ok(arr_reg)
    }
}
