//! Inst 构造器 API 集成测试：各构造器族的 ext 字布局与操作数槽位断言。

use oxide_bytecode::opcode::{OpCode, IC_EXT_WORDS};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;

#[test]
fn inst_new_has_empty_ext() {
    let inst = Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
    assert!(inst.ext.is_empty());
    assert_eq!(inst.op, OpCode::ADD);
}

#[test]
fn ic_instructions_carry_ic_slots_zero_ext_words() {
    let insts = [
        Inst::ic_get(Operand::Reg(1), Operand::Reg(2)),
        Inst::ic_set(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::member_inc(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::member_dec(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_add(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_sub(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_mul(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_div(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_mod(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_exp(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_bit_and(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_bit_or(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_bit_xor(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_shl(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_shr(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
        Inst::compound_member_ushr(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)),
    ];
    for inst in &insts {
        assert_eq!(
            inst.ext.as_slice(),
            &[0; IC_EXT_WORDS],
            "IC op {} must carry {IC_EXT_WORDS} zero ext words",
            inst.op
        );
        assert_eq!(inst.ext.len(), IC_EXT_WORDS);
    }
}

#[test]
fn call_instructions_carry_nargs() {
    let call = Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 3);
    assert_eq!(call.ext.as_slice(), &[3]);
    assert_eq!(call.rd, Operand::Reg(0));
    assert_eq!(call.a, Operand::Reg(1));
    assert_eq!(call.b, Operand::Reg(2));

    let native = Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 0);
    assert_eq!(native.ext.as_slice(), &[0]);

    let new_expr = Inst::new_expression(Operand::Reg(3), Operand::Reg(0), Operand::Reg(1), 2);
    assert_eq!(new_expr.ext.as_slice(), &[2]);
    assert_eq!(new_expr.rd, Operand::Reg(3));
    assert_eq!(new_expr.a, Operand::Reg(0));

    let super_call = Inst::super_call(Operand::Reg(3), Operand::Reg(1), 1);
    assert_eq!(super_call.ext.as_slice(), &[1]);
    assert_eq!(super_call.rd, Operand::Reg(3));
    assert_eq!(super_call.a, Operand::Reg(1));
}

#[test]
fn single_ext_word_instructions() {
    let accessor = Inst::define_accessor(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 42);
    assert_eq!(accessor.ext.as_slice(), &[42]);
    assert_eq!(accessor.rd, Operand::Reg(0));
    assert_eq!(accessor.a, Operand::Reg(1));
    assert_eq!(accessor.b, Operand::Reg(2));

    let dyn_accessor = Inst::define_accessor_dynamic(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 7);
    assert_eq!(dyn_accessor.op, OpCode::DEFINE_ACCESSOR_DYNAMIC);
    assert_eq!(dyn_accessor.ext.as_slice(), &[0x8000_0000 | 7]);
    assert_eq!(dyn_accessor.rd, Operand::Reg(0));
    assert_eq!(dyn_accessor.a, Operand::Reg(1));
    assert_eq!(dyn_accessor.b, Operand::Reg(2));

    let rest = Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7, None);
    assert_eq!(rest.ext.as_slice(), &[7]);

    let spread = Inst::spread_object(Operand::Reg(0), Operand::Reg(1));
    assert_eq!(spread.op, OpCode::SPREAD_OBJECT);
    assert_eq!(spread.rd, Operand::Reg(0));
    assert_eq!(spread.a, Operand::Reg(1));
    assert_eq!(spread.b, Operand::None);
    assert!(spread.ext.is_empty());
}

#[test]
fn spread_call_constructors_carry_header_and_source_regs() {
    let call = Inst::call_spread(Operand::Reg(0), Operand::Reg(1), &[10, 0x8000_0000 | 300]);
    assert_eq!(call.op, OpCode::CALL_SPREAD);
    assert_eq!(call.rd, Operand::Reg(0));
    assert_eq!(call.a, Operand::Reg(1));
    assert_eq!(call.b, Operand::None);
    assert_eq!(call.ext.as_slice(), &[1 | (1 << 8), 10, 0x8000_0000 | 300]);

    let ne = Inst::new_expression_spread(Operand::Reg(3), Operand::Reg(0), &[0x8000_0000 | 7]);
    assert_eq!(ne.op, OpCode::NEW_EXPRESSION_SPREAD);
    assert_eq!(ne.rd, Operand::Reg(3));
    assert_eq!(ne.a, Operand::Reg(0));
    assert_eq!(ne.ext.as_slice(), &[1 << 8, 0x8000_0000 | 7]);

    let sc = Inst::super_call_spread(Operand::Reg(3), &[1, 0x8000_0000 | 5]);
    assert_eq!(sc.op, OpCode::SUPER_CALL_SPREAD);
    assert_eq!(sc.rd, Operand::Reg(3));
    assert_eq!(sc.a, Operand::None);
    assert_eq!(sc.b, Operand::None);
    assert_eq!(sc.ext.as_slice(), &[1 | (1 << 8), 1, 0x8000_0000 | 5]);
}

#[test]
fn regalloc_constructors_carry_expected_slots() {
    let mov = Inst::inst_mov(Operand::Reg(3), Operand::Reg(7));
    assert_eq!(mov.op, OpCode::MOV);
    assert_eq!(mov.rd, Operand::Reg(3));
    assert_eq!(mov.a, Operand::Reg(7));
    assert_eq!(mov.b, Operand::None);
    assert!(mov.ext.is_empty());

    let spill = Inst::inst_spill(Operand::Reg(5), 42);
    assert_eq!(spill.op, OpCode::SPILL);
    assert_eq!(spill.rd, Operand::Reg(5));
    assert_eq!(spill.a, Operand::None);
    assert_eq!(spill.b, Operand::None);
    assert_eq!(spill.ext.as_slice(), &[42]);

    let unspill = Inst::inst_unspill(Operand::Reg(9), 0xFFFF);
    assert_eq!(unspill.op, OpCode::UNSPILL);
    assert_eq!(unspill.rd, Operand::Reg(9));
    assert_eq!(unspill.a, Operand::None);
    assert_eq!(unspill.b, Operand::None);
    assert_eq!(unspill.ext.as_slice(), &[0xFFFF]);
}

#[test]
fn load_const_and_create_closure_keep_semantic_operands() {
    let lc = Inst::load_const(Operand::Reg(4), 300);
    assert_eq!(lc.a, Operand::Const(300));
    assert_eq!(lc.b, Operand::None);
    assert!(lc.ext.is_empty());

    let cc = Inst::create_closure(Operand::Reg(4), 5);
    assert_eq!(cc.a, Operand::Imm(5));
    assert_eq!(cc.b, Operand::None);
    assert!(cc.ext.is_empty());

    let ca = Inst::create_arguments(Operand::Reg(6));
    assert_eq!(ca.op, OpCode::CREATE_ARGUMENTS);
    assert_eq!(ca.rd, Operand::Reg(6));
    assert_eq!(ca.a, Operand::None);
    assert_eq!(ca.b, Operand::None);
    assert!(ca.ext.is_empty());
}

#[test]
fn jump_family_puts_label_in_b_slot() {
    let jmp = Inst::jmp(9);
    assert_eq!(jmp.b, Operand::Label(9));
    assert_eq!(jmp.rd, Operand::None);

    let cond = Inst::jmp_if_false(3, 9);
    assert_eq!(cond.rd, Operand::Reg(3));
    assert_eq!(cond.b, Operand::Label(9));

    let true_jmp = Inst::jmp_if_true(3, 9);
    assert_eq!(true_jmp.b, Operand::Label(9));

    let nullish = Inst::jmp_if_nullish(3, 9);
    assert_eq!(nullish.b, Operand::Label(9));

    let try_begin = Inst::try_begin(9);
    assert_eq!(try_begin.b, Operand::Label(9));
    assert_eq!(try_begin.rd, Operand::None);

    let try_fin = Inst::try_finally_begin(9);
    assert_eq!(try_fin.b, Operand::Label(9));
}

#[test]
fn template_str_packs_segment_count_and_hint() {
    let inst = Inst::template_str(Operand::Reg(1), 3, 10, &[0x1234, 0x8000_0000 | 5]);
    assert_eq!(inst.op, OpCode::TEMPLATE_STR);
    assert_eq!(inst.rd, Operand::Reg(1));
    assert_eq!(inst.ext.len(), 3);
    assert_eq!(inst.ext[0], (3 << 16) | 10);
    assert_eq!(inst.ext[1], 0x1234);
    assert_eq!(inst.ext[2], 0x8000_0000 | 5);
}

#[test]
fn concat_n_packs_n_header_and_operand_regs() {
    // 6 操作数：a 槽=op1，ext=[6, op2..op6]
    let inst = Inst::concat_n(Operand::Reg(1), &[2, 5, 9, 12, 300, 7]);
    assert_eq!(inst.op, OpCode::CONCAT_N);
    assert_eq!(inst.rd, Operand::Reg(1));
    assert_eq!(inst.a, Operand::Reg(2));
    assert_eq!(inst.b, Operand::None);
    assert_eq!(inst.ext.as_slice(), &[6, 5, 9, 12, 300, 7]);
}
