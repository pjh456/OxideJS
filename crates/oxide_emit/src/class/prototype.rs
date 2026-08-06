//! 类原型对象构造：`emit_class_prototype` 建立 `Class.prototype` 与属性初始化。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;

impl Emitter {
    pub(crate) fn emit_class_prototype(
        &self, ctor_reg: u32, proto_reg: u32, super_reg: Option<u32>, sub_idx: u16, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.inst(Inst::create_closure(Operand::Reg(ctor_reg), sub_idx));
        ctx.inst(Inst::new(OpCode::NEW_OBJECT, Operand::Reg(proto_reg), Operand::None, Operand::None));
        if let Some(super_reg) = super_reg {
            let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
            let parent_proto_key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(parent_proto_key_reg), proto_key_idx));
            let parent_proto_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::GET_PROP, Operand::Reg(super_reg), Operand::Reg(parent_proto_reg), Operand::Reg(parent_proto_key_reg)));
            let proto_link_idx = ctx.add_constant(Constant::String("__proto__".to_string()));
            let proto_link_key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(proto_link_key_reg), proto_link_idx));
            ctx.inst(Inst::new(OpCode::SET_PROP, Operand::Reg(proto_reg), Operand::Reg(parent_proto_reg), Operand::Reg(proto_link_key_reg)));
            ctx.inst(Inst::new(OpCode::SET_PROP, Operand::Reg(ctor_reg), Operand::Reg(super_reg), Operand::Reg(proto_link_key_reg)));
        }
        let ctor_key_idx = ctx.add_constant(Constant::String("constructor".to_string()));
        let ctor_key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(ctor_key_reg), ctor_key_idx));
        ctx.inst(Inst::new(OpCode::SET_PROP, Operand::Reg(proto_reg), Operand::Reg(ctor_reg), Operand::Reg(ctor_key_reg)));
        let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
        let proto_key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(proto_key_reg), proto_key_idx));
        ctx.inst(Inst::new(OpCode::SET_PROP, Operand::Reg(ctor_reg), Operand::Reg(proto_reg), Operand::Reg(proto_key_reg)));
        Ok(())
    }
}
