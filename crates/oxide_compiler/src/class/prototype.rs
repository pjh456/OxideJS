use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};

impl Compiler {
    pub(crate) fn count_class_prototype(&self, has_super: bool, ctx: &mut CompileCtx) {
        ctx.count_instr();
        ctx.count_instr();
        if has_super {
            ctx.alloc_reg();
            ctx.count_instr();
            ctx.alloc_reg();
            ctx.count_instr();
            ctx.alloc_reg();
            ctx.count_instr();
            ctx.count_instr();
            ctx.count_instr();
        }
        ctx.count_load_const();
        ctx.count_instr();
        ctx.count_load_const();
        ctx.count_instr();
    }

    pub(crate) fn emit_class_prototype(
        &self, ctor_reg: u8, proto_reg: u8, super_reg: Option<u8>, sub_idx: u32, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.emit_create_closure(ctor_reg, sub_idx);
        ctx.emit(opcode::encode(OpCode::NEW_OBJECT, proto_reg, 0, 0));
        if let Some(super_reg) = super_reg {
            let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
            let parent_proto_key_reg = ctx.alloc_reg();
            ctx.emit_load_const(parent_proto_key_reg, proto_key_idx);
            let parent_proto_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::GET_PROP, super_reg, parent_proto_reg, parent_proto_key_reg));
            let proto_link_idx = ctx.add_constant(Constant::String("__proto__".to_string()));
            let proto_link_key_reg = ctx.alloc_reg();
            ctx.emit_load_const(proto_link_key_reg, proto_link_idx);
            ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, parent_proto_reg, proto_link_key_reg));
            ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, super_reg, proto_link_key_reg));
        }
        let ctor_key_idx = ctx.add_constant(Constant::String("constructor".to_string()));
        let ctor_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(ctor_key_reg, ctor_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, ctor_reg, ctor_key_reg));
        let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
        let proto_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(proto_key_reg, proto_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, proto_reg, proto_key_reg));
        Ok(())
    }
}
