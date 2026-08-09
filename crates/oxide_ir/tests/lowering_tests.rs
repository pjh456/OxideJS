//! lowering 全编码形态断言：IRFunction → CompiledModule 逐 Instr 等价。

use oxide_bytecode::module::{CompiledModule, Constant};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_ir::inst::Inst;
use oxide_ir::lower::lower;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

fn base_module() -> IRFunction {
    IRFunction::new()
}

fn lower_ok(f: &IRFunction) -> CompiledModule {
    lower(f).expect("lower should succeed")
}

fn lower_err(f: &IRFunction) -> String {
    match lower(f) {
        Ok(_) => panic!("lower should fail"),
        Err(e) => e,
    }
}

#[test]
fn three_operand_opcode_encodes_rd_a_b() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)));
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 1);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::ADD);
    assert_eq!(opcode::rd(m.bytecode[0]), 0);
    assert_eq!(opcode::a(m.bytecode[0]), 1);
    assert_eq!(opcode::b(m.bytecode[0]), 2);
}

#[test]
fn load_const_splits_imm16_into_a_b() {
    let mut f = base_module();
    f.insts.push(Inst::load_const(Operand::Reg(4), 0x012C)); // 300
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 1);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::LOAD_CONST);
    assert_eq!(opcode::rd(m.bytecode[0]), 4);
    assert_eq!(opcode::a(m.bytecode[0]), 0x2C); // low byte
    assert_eq!(opcode::b(m.bytecode[0]), 0x01); // high byte
}

#[test]
fn create_closure_splits_sub_idx_into_a_b() {
    let mut f = base_module();
    f.insts.push(Inst::create_closure(Operand::Reg(4), 5));
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 1);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::CREATE_CLOSURE);
    assert_eq!(opcode::rd(m.bytecode[0]), 4);
    assert_eq!(opcode::imm16(m.bytecode[0]), 5);
}

#[test]
fn forward_jump_encodes_positive_offset() {
    let mut f = base_module();
    f.insts.push(Inst::jmp(0));
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.label_pos = vec![Some(1)];
    f.label_count = 1;
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 2);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::JMP);
    assert_eq!(opcode::offset16(m.bytecode[0]), 1);
}

#[test]
fn backward_jump_encodes_negative_offset() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.insts.push(Inst::jmp(0));
    f.label_pos = vec![Some(0)];
    f.label_count = 1;
    let m = lower_ok(&f);
    assert_eq!(opcode::opcode(m.bytecode[1]), OpCode::JMP);
    assert_eq!(opcode::offset16(m.bytecode[1]), -1);
}

#[test]
fn jump_offset_counts_ext_words_in_target() {
    // jmp(0) → ic_get（1 + 3 ext = 4 instr）→ ADD（instr 5）
    let mut f = base_module();
    f.insts.push(Inst::jmp(0));
    f.insts.push(Inst::ic_get(Operand::Reg(1), Operand::Reg(2)));
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)));
    f.label_pos = vec![Some(2)];
    f.label_count = 1;
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 1 + 4 + 1);
    assert_eq!(opcode::offset16(m.bytecode[0]), 5);
}

#[test]
fn ic_get_carries_three_zero_ext_words() {
    let mut f = base_module();
    f.insts.push(Inst::ic_get(Operand::Reg(1), Operand::Reg(2)));
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 4);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::IC_GET_PROP);
    assert_eq!(opcode::rd(m.bytecode[0]), 0);
    assert_eq!(opcode::a(m.bytecode[0]), 1);
    assert_eq!(opcode::b(m.bytecode[0]), 2);
    assert_eq!(m.bytecode[1..], [0, 0, 0]);
}

#[test]
fn call_carries_nargs_ext_word() {
    let mut f = base_module();
    f.insts.push(Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 3));
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 2);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::CALL);
    assert_eq!(opcode::rd(m.bytecode[0]), 0);
    assert_eq!(opcode::a(m.bytecode[0]), 1);
    assert_eq!(opcode::b(m.bytecode[0]), 2);
    assert_eq!(m.bytecode[1], 3);
}

#[test]
fn define_accessor_and_rest_object_carry_single_ext_word() {
    let mut f = base_module();
    f.insts
        .push(Inst::define_accessor(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 42));
    f.insts.push(Inst::rest_object(Operand::Reg(3), Operand::Reg(4), 7, None));
    let m = lower_ok(&f);
    assert_eq!(m.bytecode.len(), 4);
    assert_eq!(opcode::opcode(m.bytecode[0]), OpCode::DEFINE_ACCESSOR);
    assert_eq!(m.bytecode[1], 42);
    assert_eq!(opcode::opcode(m.bytecode[2]), OpCode::REST_OBJECT);
    assert_eq!(opcode::rd(m.bytecode[2]), 3);
    assert_eq!(opcode::a(m.bytecode[2]), 4);
    assert_eq!(m.bytecode[3], 7);
}

#[test]
fn this_and_new_target_map_to_254_255() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::This, Operand::None));
    f.insts
        .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(2), Operand::NewTarget, Operand::None));
    let m = lower_ok(&f);
    assert_eq!(opcode::a(m.bytecode[0]), 254);
    assert_eq!(opcode::a(m.bytecode[1]), 255);
}

#[test]
fn nested_ir_functions_pack_into_sub_modules() {
    let mut outer = base_module();
    let mut inner = base_module();
    inner
        .insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)));
    inner.n_registers = 3;
    outer.nested.push(inner);
    let m = lower_ok(&outer);
    assert_eq!(m.sub_modules.len(), 1);
    assert_eq!(m.sub_modules[0].bytecode.len(), 1);
    assert_eq!(opcode::opcode(m.sub_modules[0].bytecode[0]), OpCode::ADD);
    assert_eq!(m.sub_modules[0].n_registers, 3);
}

#[test]
fn module_fields_are_copied_through() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.constants.push(Constant::Int(42));
    f.param_layout = oxide_ir::ParamLayout { base: 2, count: 3 };
    f.n_registers = 8;
    f.is_arrow = true;
    f.builtin_reg_map.push(("Math".to_string(), 1));
    f.upvalue_captures.push(oxide_bytecode::module::UpvalueCapture {
        name: "x".to_string(),
        enclosing_reg: 5,
        cell_idx: 0,
        parent_uv_idx: None,
    });
    f.cells_needed = 1;
    let m = lower_ok(&f);
    assert_eq!(m.constants, vec![Constant::Int(42)]);
    assert_eq!(m.param_base, 2);
    assert_eq!(m.n_args, 3);
    assert_eq!(m.n_registers, 8);
    assert!(m.is_arrow);
    assert_eq!(m.builtin_reg_map, vec![("Math".to_string(), 1)]);
    assert_eq!(m.upvalue_captures.len(), 1);
    assert_eq!(m.cells_needed, 1);
}

#[test]
fn n_registers_overflow_errors() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.n_registers = 300;
    let err = lower_err(&f);
    assert!(err.contains("too many registers"), "unexpected error: {err}");
}

#[test]
fn explicit_reg_254_errors() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(254), Operand::Reg(1), Operand::Reg(2)));
    let err = lower_err(&f);
    assert!(err.contains("too many registers"), "unexpected error: {err}");
}

#[test]
fn jump_offset_overflow_errors() {
    let mut f = base_module();
    f.insts.push(Inst::jmp(0));
    for _ in 0..40_000 {
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    }
    f.label_pos = vec![Some(40_000)];
    f.label_count = 1;
    let err = lower_err(&f);
    assert!(err.contains("jump offset"), "unexpected error: {err}");
}

#[test]
fn missing_label_errors() {
    let mut f = base_module();
    f.insts.push(Inst::jmp(0));
    f.label_pos = vec![None];
    f.label_count = 1;
    let err = lower_err(&f);
    assert!(err.contains("not found"), "unexpected error: {err}");
}

#[test]
fn const_overflow_errors() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.constants = vec![Constant::Int(0); 70_000];
    let err = lower_err(&f);
    assert!(err.contains("too many constants"), "unexpected error: {err}");
}

#[test]
fn const_overflow_flag_errors() {
    let mut f = base_module();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.const_overflow = true;
    let err = lower_err(&f);
    assert!(err.contains("too many constants"), "unexpected error: {err}");
}

#[test]
fn ir_function_domain_assemble_default_clone() {
    let mut f = IRFunction::new();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.constants.push(Constant::Int(1));
    f.param_layout = oxide_ir::ParamLayout { base: 0, count: 1 };
    f.n_registers = 2;
    f.is_arrow = true;
    f.builtin_reg_map.push(("Math".to_string(), 1));
    f.upvalue_captures.push(oxide_bytecode::module::UpvalueCapture {
        name: "x".to_string(),
        enclosing_reg: 0,
        cell_idx: 0,
        parent_uv_idx: None,
    });
    f.cells_needed = 1;
    f.function_name = Some("f".to_string());
    let mut inner = IRFunction::new();
    inner
        .insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.nested.push(inner);

    let d = IRFunction::new();
    assert!(d.insts.is_empty());
    assert!(d.nested.is_empty());
    assert_eq!(d.n_registers, 0);
    assert_eq!(d.label_pos.len(), 0);

    let mut g = f.clone();
    g.insts.clear();
    g.nested.clear();
    assert_eq!(f.insts.len(), 1, "clone must be deep copy for insts");
    assert_eq!(f.nested.len(), 1, "clone must be deep copy for nested");
    assert_eq!(f.function_name.as_deref(), Some("f"));
}

#[test]
fn operand_this_is_semantic_not_physical_index() {
    assert_ne!(Operand::This, Operand::Reg(254));
    assert_ne!(Operand::NewTarget, Operand::Reg(255));
    assert_ne!(Operand::None, Operand::Reg(0));
    assert_eq!(Operand::This, Operand::This);
    assert_eq!(Operand::NewTarget, Operand::NewTarget);
}
