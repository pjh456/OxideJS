//! Inst 寄存器契约集成测试：def/use 扫描 + 副作用判定，golden 表为全 opcode 契约回归锚。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

// ── def_reg / use_regs 契约测试（反直觉槽位断言）──

#[test]
fn binary_op_def_rd_use_ab() {
    let inst = Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
    assert_eq!(inst.def_reg(), Some(0));
    assert_eq!(inst.use_regs().as_slice(), &[1, 2]);
}

#[test]
fn unary_op_def_rd_use_a() {
    let inst = Inst::new(OpCode::NEG, Operand::Reg(0), Operand::Reg(1), Operand::None);
    assert_eq!(inst.def_reg(), Some(0));
    assert_eq!(inst.use_regs().as_slice(), &[1]);
}

#[test]
fn get_prop_def_is_a_slot_not_rd() {
    // GET_PROP rd=obj 是 use，结果写 a 槽
    let inst = Inst::new(OpCode::GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
    assert_eq!(inst.def_reg(), Some(1));
    assert_eq!(inst.use_regs().as_slice(), &[0, 2]);
}

#[test]
fn get_prop_dynamic_def_is_b_slot() {
    // GET_PROP_DYNAMIC rd=obj 是 use，结果写 b 槽
    let inst = Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2));
    assert_eq!(inst.def_reg(), Some(2));
    assert_eq!(inst.use_regs().as_slice(), &[0, 1]);
}

#[test]
fn ic_get_prop_a_slot_is_use_and_def() {
    // IC_GET_PROP a 槽既是对象 use 又是结果 def
    let inst = Inst::ic_get(Operand::Reg(1), Operand::Reg(2));
    assert_eq!(inst.def_reg(), Some(1));
    let uses = inst.use_regs();
    assert_eq!(uses.as_slice(), &[1, 2]);
}

#[test]
fn call_def_is_implicit_reg0_with_arg_range() {
    // CALL rd=callee 是 use，结果隐式写 reg 0；参数 b..b+nargs 连续
    let inst = Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 3);
    assert_eq!(inst.def_reg(), Some(0));
    assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2, 3, 4]);
}

#[test]
fn call_native_def_is_implicit_reg0() {
    let inst = Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 1);
    assert_eq!(inst.def_reg(), Some(0));
    assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2]);
}

#[test]
fn new_expression_uses_ctor_and_arg_range() {
    let inst = Inst::new_expression(Operand::Reg(3), Operand::Reg(0), Operand::Reg(1), 2);
    assert_eq!(inst.def_reg(), Some(3));
    assert_eq!(inst.use_regs().as_slice(), &[0, 1, 2]);
}

#[test]
fn super_call_uses_arg_range() {
    let inst = Inst::super_call(Operand::Reg(3), Operand::Reg(1), 1);
    assert_eq!(inst.def_reg(), Some(3));
    assert_eq!(inst.use_regs().as_slice(), &[1]);
}

#[test]
fn halt_implicitly_uses_reg0() {
    // HALT 返回 regs[0]，顶层 LOAD_VAR(None, r) 链靠它保活
    let inst = Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None);
    assert_eq!(inst.def_reg(), None);
    assert_eq!(inst.use_regs().as_slice(), &[0]);
}

#[test]
fn spread_call_def_and_uses() {
    // CALL_SPREAD：结果隐式写 reg 0；use = callee + this + ext 有序实参字（静态/spread 同编）
    let call = Inst::call_spread(Operand::Reg(0), Operand::Reg(1), &[5, 0x8000_0000 | 9, 300]);
    assert_eq!(call.def_reg(), Some(0));
    assert_eq!(call.use_regs().as_slice(), &[0, 1, 5, 9, 300]);

    // NEW_EXPRESSION_SPREAD：rd 即 def；use = ctor + 实参字
    let ne = Inst::new_expression_spread(Operand::Reg(3), Operand::Reg(0), &[7, 0x8000_0000 | 12]);
    assert_eq!(ne.def_reg(), Some(3));
    assert_eq!(ne.use_regs().as_slice(), &[0, 7, 12]);

    // SUPER_CALL_SPREAD：无 callee/this 槽
    let sc = Inst::super_call_spread(Operand::Reg(3), &[4, 0x8000_0000 | 8]);
    assert_eq!(sc.def_reg(), Some(3));
    assert_eq!(sc.use_regs().as_slice(), &[4, 8]);
}

#[test]
fn void_defs_rd_no_use() {
    // VM 的 VOID handler 不读 a 槽
    let inst = Inst::new(OpCode::VOID, Operand::Reg(1), Operand::Reg(2), Operand::None);
    assert_eq!(inst.def_reg(), Some(1));
    assert!(inst.use_regs().is_empty());
}

#[test]
fn load_const_create_closure_load_upvalue_no_use() {
    // a 槽是 Const/Imm 立即数，非寄存器
    let lc = Inst::load_const(Operand::Reg(1), 5);
    assert_eq!(lc.def_reg(), Some(1));
    assert!(lc.use_regs().is_empty());

    let cc = Inst::create_closure(Operand::Reg(1), 0);
    assert_eq!(cc.def_reg(), Some(1));
    assert!(cc.use_regs().is_empty());

    let lu = Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(1), Operand::Imm(0), Operand::None);
    assert_eq!(lu.def_reg(), Some(1));
    assert!(lu.use_regs().is_empty());
}

#[test]
fn none_slot_maps_to_reg0() {
    // None 槽与 lower.rs operand_to_u8 一致，统一映射物理 reg 0
    let lv = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::None, Operand::None);
    assert_eq!(lv.use_regs().as_slice(), &[0]);

    let cg = Inst::new(OpCode::CELL_GET, Operand::Reg(0), Operand::None, Operand::Imm(0));
    assert_eq!(cg.use_regs().as_slice(), &[0]);
}

#[test]
fn template_str_parses_expr_regs_from_ext() {
    // ext[0] 跳过；表达式段在 RegAlloc 前保留完整 u32 vreg。
    let with_expr = Inst::template_str(Operand::Reg(1), 2, 10, &[0x1234, 0x8000_0000 | 300]);
    assert_eq!(with_expr.use_regs().as_slice(), &[300]);

    // 纯 quasi（无表达式）：不误报寄存器
    let no_expr = Inst::template_str(Operand::Reg(1), 1, 10, &[0x1234]);
    assert!(no_expr.use_regs().is_empty());
}

#[test]
fn jmp_if_false_uses_cond_reg() {
    let inst = Inst::jmp_if_false(3, 9);
    assert_eq!(inst.def_reg(), None);
    assert_eq!(inst.use_regs().as_slice(), &[3]);
}

#[test]
fn jmp_and_try_no_use() {
    assert!(Inst::jmp(9).use_regs().is_empty());
    assert!(Inst::try_begin(9).use_regs().is_empty());
    assert!(Inst::try_finally_begin(9).use_regs().is_empty());
}

#[test]
fn make_cell_no_def_uses_rd() {
    let inst = Inst::new(OpCode::MAKE_CELL, Operand::Reg(5), Operand::None, Operand::None);
    assert_eq!(inst.def_reg(), None);
    assert_eq!(inst.use_regs().as_slice(), &[5]);
}

#[test]
fn cell_set_and_store_upvalue_use_a_no_def() {
    let cs = Inst::new(OpCode::CELL_SET, Operand::None, Operand::Reg(1), Operand::Imm(0));
    assert_eq!(cs.def_reg(), None);
    assert_eq!(cs.use_regs().as_slice(), &[1]);

    let su = Inst::new(OpCode::STORE_UPVALUE, Operand::None, Operand::Reg(1), Operand::Imm(0));
    assert_eq!(su.def_reg(), None);
    assert_eq!(su.use_regs().as_slice(), &[1]);
}

#[test]
fn return_throw_use_rd_with_none_mapping_reg0() {
    let ret = Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None);
    assert_eq!(ret.def_reg(), None);
    assert_eq!(ret.use_regs().as_slice(), &[0]);

    let thr = Inst::new(OpCode::THROW, Operand::Reg(2), Operand::None, Operand::None);
    assert_eq!(thr.def_reg(), None);
    assert_eq!(thr.use_regs().as_slice(), &[2]);
}

#[test]
fn rest_object_uses_a() {
    let inst = Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7, None);
    assert_eq!(inst.def_reg(), Some(0));
    // b 槽 None 映射 reg 0（excl_arr 缺省），REST_OBJECT 运行时同时读源与排除键。
    assert_eq!(inst.use_regs().as_slice(), &[1, 0]);
}

#[test]
fn spread_object_def_rd_uses_rd_and_a() {
    // SPREAD_OBJECT 原地改目标对象：def=rd，use=[rd, a]，不可删除（有观察副作用）
    let inst = Inst::spread_object(Operand::Reg(3), Operand::Reg(7));
    assert_eq!(inst.def_reg(), Some(3));
    assert_eq!(inst.use_regs().as_slice(), &[3, 7]);
    let f = IRFunction::new();
    assert!(!inst.is_pure(&f));
}

#[test]
fn create_arguments_defs_rd_no_register_use_impure() {
    // CREATE_ARGUMENTS 写 rd；实参从 spill 栈读取，无寄存器 use；
    // 创建对象有观察副作用（分配的 arguments 可被外部观察到），不可删除。
    let inst = Inst::create_arguments(Operand::Reg(6));
    assert_eq!(inst.def_reg(), Some(6));
    assert!(inst.use_regs().is_empty());
    let f = IRFunction::new();
    assert!(!inst.is_pure(&f));
}

#[test]
fn for_in_next_defs_rd_no_use() {
    let inst = Inst::new(OpCode::FOR_IN_NEXT, Operand::Reg(0), Operand::None, Operand::None);
    assert_eq!(inst.def_reg(), Some(0));
    assert!(inst.use_regs().is_empty());
}

#[test]
fn store_var_def_rd_use_a() {
    let inst = Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None);
    assert_eq!(inst.def_reg(), Some(0));
    assert_eq!(inst.use_regs().as_slice(), &[1]);
}

#[test]
fn regalloc_opcodes_def_use_contract() {
    let mov = Inst::inst_mov(Operand::Reg(3), Operand::Reg(7));
    assert_eq!(mov.def_reg(), Some(3));
    assert_eq!(mov.use_regs().as_slice(), &[7]);

    let spill = Inst::inst_spill(Operand::Reg(5), 42);
    assert_eq!(spill.def_reg(), None);
    assert_eq!(spill.use_regs().as_slice(), &[5]);

    let unspill = Inst::inst_unspill(Operand::Reg(9), 42);
    assert_eq!(unspill.def_reg(), Some(9));
    assert!(unspill.use_regs().is_empty());
}

// ── is_pure 契约测试（纯/非纯族代表 opcode）──

#[test]
fn pure_ops_are_deletable() {
    let f = IRFunction::new();
    // 算术 / 比较 / 位 / 逻辑
    assert!(Inst::new(OpCode::ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::SUB, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::MUL, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::NEG, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::EQ, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::STRICT_EQ, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::LT, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::BIT_AND, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::BIT_NOT, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::AND, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::OR, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(Inst::new(OpCode::NOT, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::NULLISH, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    // 分配/常量/空操作
    assert!(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None).is_pure(&f));
    assert!(Inst::load_const(Operand::Reg(1), 0).is_pure(&f));
    assert!(Inst::new(OpCode::VOID, Operand::Reg(1), Operand::None, Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::TYPEOF, Operand::Reg(1), Operand::Reg(2), Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::NEW_OBJECT, Operand::Reg(1), Operand::None, Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(1), Operand::None, Operand::None).is_pure(&f));
    // 闭包/单元读取
    assert!(Inst::create_closure(Operand::Reg(1), 0).is_pure(&f));
    assert!(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(1), Operand::Imm(0), Operand::None).is_pure(&f));
    assert!(Inst::new(OpCode::CELL_GET, Operand::Reg(0), Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
    // 模板字符串
    assert!(Inst::template_str(Operand::Reg(1), 1, 10, &[0x1234]).is_pure(&f));
    // RegAlloc 辅助：MOV 纯
    assert!(Inst::inst_mov(Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    // LOAD_VAR 普通（a=Reg）纯
    assert!(Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::Reg(2), Operand::None).is_pure(&f));
    // STORE_VAR 一律有副作用（顶层变量赋值全局可观察，不删）
    assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
    assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(1)).is_pure(&f));
}

#[test]
fn impure_ops_are_not_deletable() {
    let f = IRFunction::new();
    // getter / 写对象属性
    assert!(!Inst::ic_get(Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::ic_set(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::SET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::SUPER_GET_PROP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::DEFINE_ACCESSOR, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::DEFINE_ACCESSOR_DYNAMIC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::DELETE_PROP_STATIC, Operand::Reg(0), Operand::Reg(0), Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::DELETE_PROP_DYNAMIC, Operand::Reg(0), Operand::None, Operand::Reg(2)).is_pure(&f));
    // 调用 / 异常 / 控制流
    assert!(!Inst::call(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 0).is_pure(&f));
    assert!(!Inst::call_native(Operand::Reg(0), Operand::Reg(1), Operand::Reg(2), 0).is_pure(&f));
    assert!(!Inst::new_expression(Operand::Reg(3), Operand::Reg(0), Operand::Reg(1), 0).is_pure(&f));
    assert!(!Inst::super_call(Operand::Reg(3), Operand::Reg(1), 0).is_pure(&f));
    assert!(!Inst::call_spread(Operand::Reg(0), Operand::Reg(1), &[3]).is_pure(&f));
    assert!(!Inst::new_expression_spread(Operand::Reg(3), Operand::Reg(0), &[2]).is_pure(&f));
    assert!(!Inst::super_call_spread(Operand::Reg(3), &[2]).is_pure(&f));
    assert!(!Inst::new(OpCode::CREATE_REGEXP, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::THROW, Operand::Reg(2), Operand::None, Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None).is_pure(&f));
    assert!(!Inst::jmp(9).is_pure(&f));
    assert!(!Inst::jmp_if_false(3, 9).is_pure(&f));
    assert!(!Inst::try_begin(9).is_pure(&f));
    assert!(!Inst::try_finally_begin(9).is_pure(&f));
    // 迭代器 / 复合更新 / 写共享状态
    assert!(!Inst::new(OpCode::FOR_IN_NEXT, Operand::Reg(0), Operand::None, Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(0), Operand::None, Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::INC_PRE, Operand::Reg(0), Operand::Reg(0), Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::COMPOUND_ADD, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::MEMBER_INC, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::COMPOUND_MEMBER_ADD, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::rest_object(Operand::Reg(0), Operand::Reg(1), 7, None).is_pure(&f));
    assert!(!Inst::new(OpCode::INSTANCEOF, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::IN, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::GET_PRIVATE, Operand::Reg(0), Operand::Reg(1), Operand::Reg(2)).is_pure(&f));
    assert!(!Inst::new(OpCode::SET_HOME_OBJECT, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::STORE_UPVALUE, Operand::None, Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
    assert!(!Inst::new(OpCode::CELL_SET, Operand::None, Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
    assert!(!Inst::new(OpCode::MAKE_CELL, Operand::Reg(5), Operand::None, Operand::None).is_pure(&f));
    // RegAlloc 辅助：SPILL/UNSPILL 有副作用（删 SPILL 后 UNSPILL 读陈旧值）
    assert!(!Inst::inst_spill(Operand::Reg(5), 42).is_pure(&f));
    assert!(!Inst::inst_unspill(Operand::Reg(9), 42).is_pure(&f));
    // 占位 opcode 防御性有副作用
    assert!(!Inst::new(OpCode::SWITCH_TABLE, Operand::None, Operand::None, Operand::None).is_pure(&f));
}

#[test]
fn store_var_all_branches_are_impure() {
    // STORE_VAR 写目标变量寄存器，顶层/模块作用域变量全局可观察，一律不删
    // （const guard 抛错、普通 var/let 赋值都可能改变外部观察状态）
    let f = IRFunction::new();
    assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None).is_pure(&f));
    assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(0)).is_pure(&f));
    assert!(!Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::Imm(1)).is_pure(&f));
}

#[test]
fn load_var_this_derived_constructor_special_case() {
    // a==This + is_derived_constructor 时读 this 可能抛 ReferenceError，不可删
    let mut derived = IRFunction::new();
    derived.is_derived_constructor = true;
    let non_derived = IRFunction::new();

    let this_load = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::This, Operand::None);
    assert!(!this_load.is_pure(&derived));
    assert!(this_load.is_pure(&non_derived));

    // a=Reg 普通 LOAD_VAR 在 derived 函数中也纯
    let reg_load = Inst::new(OpCode::LOAD_VAR, Operand::Reg(1), Operand::Reg(2), Operand::None);
    assert!(reg_load.is_pure(&derived));
}

// ── 表驱动解析 vs 老 match 的一致性（Phase B1 迁移校验）──

/// 每个 opcode 构造一条规范指令：rd/a/b 置 Reg(1/2/3)，ext 按族填真实布局。
fn canonical_inst(op: OpCode) -> Inst {
    match op {
        OpCode::CALL | OpCode::CALL_NATIVE => Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 2),
        OpCode::NEW_EXPRESSION => Inst::new_expression(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 2),
        OpCode::SUPER_CALL => Inst::super_call(Operand::Reg(1), Operand::Reg(2), 2),
        OpCode::CALL_SPREAD => Inst::call_spread(Operand::Reg(1), Operand::Reg(2), &[5, 0x8000_0000 | 9]),
        OpCode::NEW_EXPRESSION_SPREAD => {
            Inst::new_expression_spread(Operand::Reg(1), Operand::Reg(2), &[5, 0x8000_0000 | 9])
        }
        OpCode::SUPER_CALL_SPREAD => Inst::super_call_spread(Operand::Reg(1), &[5, 0x8000_0000 | 9]),
        OpCode::IC_GET_PROP => Inst::ic_get(Operand::Reg(1), Operand::Reg(2)),
        OpCode::IC_SET_PROP => Inst::ic_set(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::MEMBER_INC => Inst::member_inc(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::MEMBER_DEC => Inst::member_dec(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_ADD => Inst::compound_member_add(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_SUB => Inst::compound_member_sub(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_MUL => Inst::compound_member_mul(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_DIV => Inst::compound_member_div(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_MOD => Inst::compound_member_mod(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_EXP => Inst::compound_member_exp(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_BIT_AND => {
            Inst::compound_member_bit_and(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3))
        }
        OpCode::COMPOUND_MEMBER_BIT_OR => {
            Inst::compound_member_bit_or(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3))
        }
        OpCode::COMPOUND_MEMBER_BIT_XOR => {
            Inst::compound_member_bit_xor(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3))
        }
        OpCode::COMPOUND_MEMBER_SHL => Inst::compound_member_shl(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_SHR => Inst::compound_member_shr(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::COMPOUND_MEMBER_USHR => Inst::compound_member_ushr(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
        OpCode::TEMPLATE_STR => Inst::template_str(Operand::Reg(1), 2, 10, &[0x1234, 0x8000_0000 | 5]),
        OpCode::CONCAT_N => Inst::concat_n(Operand::Reg(1), &[2, 5, 9]),
        OpCode::GET_PRIVATE => Inst::get_private(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 7, 99),
        OpCode::SET_PRIVATE => Inst::set_private(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 7, 99),
        OpCode::INIT_PRIVATE => Inst::init_private(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), true),
        OpCode::PRIVATE_BRAND_IN => Inst::private_brand_in(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 7, 99),
        OpCode::SPILL => Inst::inst_spill(Operand::Reg(1), 42),
        OpCode::UNSPILL => Inst::inst_unspill(Operand::Reg(1), 42),
        OpCode::REST_OBJECT => Inst::rest_object(Operand::Reg(1), Operand::Reg(2), 7, None),
        OpCode::DEFINE_ACCESSOR => Inst::define_accessor(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 42),
        OpCode::DEFINE_ACCESSOR_DYNAMIC => {
            Inst::define_accessor_dynamic(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 7)
        }
        OpCode::DELETE_PROP_STATIC => Inst::delete_prop_static(Operand::Reg(1), 9),
        _ => Inst::new(op, Operand::Reg(1), Operand::Reg(2), Operand::Reg(3)),
    }
}

/// 全 opcode 表遍历断言（golden）：def/use/pure 与表逐项一致。
fn assert_contract(op: OpCode, def: Option<u32>, uses: &[u32], pure: bool) {
    let inst = canonical_inst(op);
    let f = IRFunction::new();
    assert_eq!(inst.def_reg(), def, "{op}: def");
    assert_eq!(inst.use_regs().as_slice(), uses, "{op}: uses");
    assert_eq!(inst.is_pure(&f), pure, "{op}: pure");
}

/// 同一规格的多个 opcode 批量断言。
fn assert_group(ops: &[OpCode], def: Option<u32>, uses: &[u32], pure: bool) {
    for &op in ops {
        assert_contract(op, def, uses, pure);
    }
}

/// 每个 opcode 的 def/use/pure 与现有语义表一致（canonical rd=1, a=2, b=3）。
#[test]
fn opcode_semantics_golden_table() {
    // 二元运算：def=rd, uses=[a,b]，纯
    assert_group(
        &[
            OpCode::ADD,
            OpCode::SUB,
            OpCode::MUL,
            OpCode::DIV,
            OpCode::MOD,
            OpCode::EQ,
            OpCode::NEQ,
            OpCode::LT,
            OpCode::GT,
            OpCode::LTE,
            OpCode::GTE,
            OpCode::AND,
            OpCode::OR,
            OpCode::STRICT_EQ,
            OpCode::STRICT_NEQ,
            OpCode::BIT_AND,
            OpCode::BIT_OR,
            OpCode::BIT_XOR,
            OpCode::SHL,
            OpCode::SHR,
            OpCode::USHR,
            OpCode::NULLISH,
        ],
        Some(1),
        &[2, 3],
        true,
    );
    // 二元但不可删：coerce 有观察路径
    assert_group(&[OpCode::IN, OpCode::INSTANCEOF, OpCode::CREATE_REGEXP], Some(1), &[2, 3], false);
    // 一元：def=rd, uses=[a]
    assert_group(
        &[OpCode::NEG, OpCode::NOT, OpCode::UNARY_PLUS, OpCode::BIT_NOT, OpCode::TYPEOF],
        Some(1),
        &[2],
        true,
    );
    // 复合赋值：def=rd, uses=[rd,a]，不可删
    assert_group(
        &[
            OpCode::COMPOUND_ADD,
            OpCode::COMPOUND_SUB,
            OpCode::COMPOUND_MUL,
            OpCode::COMPOUND_DIV,
            OpCode::COMPOUND_MOD,
            OpCode::COMPOUND_EXP,
            OpCode::COMPOUND_AND,
            OpCode::COMPOUND_OR,
            OpCode::COMPOUND_XOR,
            OpCode::COMPOUND_SHL,
            OpCode::COMPOUND_SHR,
            OpCode::COMPOUND_USHR,
        ],
        Some(1),
        &[1, 2],
        false,
    );
    // RegAlloc 辅助
    assert_contract(OpCode::MOV, Some(1), &[2], true);
    assert_contract(OpCode::SPILL, None, &[1], false);
    assert_contract(OpCode::UNSPILL, Some(1), &[], false);
    assert_contract(OpCode::SPREAD_OBJECT, Some(1), &[1, 2], false);
    // 无条件跳转 / try 标记 / 生成器 body 标记：无 def 无 use
    assert_group(
        &[
            OpCode::BREAK,
            OpCode::CONTINUE,
            OpCode::JMP,
            OpCode::TRY_BEGIN,
            OpCode::TRY_END,
            OpCode::TRY_FINALLY_BEGIN,
            OpCode::TRY_FINALLY_ENTER,
            OpCode::TRY_FINALLY_END,
            OpCode::FOR_IN_CLEANUP,
            OpCode::FOR_OF_CLOSE,
            OpCode::SUSPEND_BODY,
        ],
        None,
        &[],
        false,
    );
    // 条件跳转：uses=[rd]
    assert_group(&[OpCode::JMP_IF_FALSE, OpCode::JMP_IF_TRUE, OpCode::JMP_IF_NULLISH], None, &[1], false);
    // 迭代器 INIT 读 a；NEXT/DONE 无 use 写 rd
    assert_group(&[OpCode::FOR_OF_INIT, OpCode::FOR_IN_INIT, OpCode::FOR_AWAIT_OF_INIT], None, &[2], false);
    assert_group(
        &[
            OpCode::FOR_OF_NEXT,
            OpCode::FOR_IN_NEXT,
            OpCode::FOR_IN_DONE,
            OpCode::FOR_OF_DONE,
            OpCode::FOR_AWAIT_OF_NEXT,
        ],
        Some(1),
        &[],
        false,
    );
    assert_contract(OpCode::FOR_AWAIT_OF_DONE, Some(1), &[2], false);
    // 自增/自减：def=rd, uses=[rd]
    assert_group(
        &[OpCode::INC_PRE, OpCode::INC_POST, OpCode::DEC_PRE, OpCode::DEC_POST],
        Some(1),
        &[1],
        false,
    );
    // 占位 opcode：def=rd 保守，不可删
    assert_group(
        &[
            OpCode::SWITCH_TABLE,
            OpCode::PROFILE_SHAPE,
            OpCode::PROFILE_BRANCH,
            OpCode::PROFILE_CALL,
            OpCode::FORK,
            OpCode::JOIN,
        ],
        Some(1),
        &[],
        false,
    );
    // 异常 / 返回 / 停机
    assert_contract(OpCode::THROW, None, &[1], false);
    assert_contract(OpCode::RETURN, None, &[1], false);
    assert_contract(OpCode::HALT, None, &[0], false);
    // 模板字符串：表达式寄存器来自 ext
    assert_contract(OpCode::TEMPLATE_STR, Some(1), &[5], true);
    // 多操作数拼接：a 槽首操作数 + ext[1..] 后续操作数（SpreadArgs 解析）
    assert_contract(OpCode::CONCAT_N, Some(1), &[2, 5, 9], true);
    // 小语言特性
    assert_contract(OpCode::DELETE_PROP_STATIC, None, &[1], false);
    assert_contract(OpCode::DELETE_PROP_DYNAMIC, None, &[1, 3], false);
    assert_contract(OpCode::MAKE_CELL, None, &[1], false);
    assert_contract(OpCode::MAKE_CELL_FRESH, None, &[1], false);
    assert_contract(OpCode::CELL_SET, None, &[2], false);
    assert_contract(OpCode::CELL_GET, Some(1), &[2], true);
    assert_contract(OpCode::REST_OBJECT, Some(1), &[2, 0], false);
    // 变量加载族
    assert_contract(OpCode::LOAD_VAR, Some(1), &[2], true);
    assert_contract(OpCode::STORE_VAR, Some(1), &[2], false);
    assert_contract(OpCode::LOAD_CONST, Some(1), &[], true);
    assert_contract(OpCode::LOAD_GLOBAL, Some(1), &[], false);
    assert_contract(OpCode::LOAD_GLOBAL_TYPEOF, Some(1), &[], false);
    assert_contract(OpCode::LOAD_UPVALUE, Some(1), &[], true);
    assert_contract(OpCode::CREATE_CLOSURE, Some(1), &[], true);
    assert_contract(OpCode::STORE_UPVALUE, None, &[2], false);
    // 调用族：CALL 系隐式写 reg 0
    assert_contract(OpCode::CALL, Some(0), &[1, 2, 3, 4], false);
    assert_contract(OpCode::CALL_NATIVE, Some(0), &[1, 2, 3, 4], false);
    assert_contract(OpCode::NEW_EXPRESSION, Some(1), &[2, 3, 4], false);
    assert_contract(OpCode::SUPER_CALL, Some(1), &[2, 3], false);
    assert_contract(OpCode::CALL_SPREAD, Some(0), &[1, 2, 5, 9], false);
    assert_contract(OpCode::NEW_EXPRESSION_SPREAD, Some(1), &[2, 5, 9], false);
    assert_contract(OpCode::SUPER_CALL_SPREAD, Some(1), &[5, 9], false);
    // super 属性读 / home object / accessor
    assert_group(&[OpCode::SUPER_GET_PROP, OpCode::SUPER_STATIC_GET_PROP], Some(1), &[2, 3], false);
    assert_contract(OpCode::SET_HOME_OBJECT, None, &[1, 2], false);
    assert_contract(OpCode::DEFINE_ACCESSOR, None, &[1, 2, 3], false);
    assert_contract(OpCode::DEFINE_ACCESSOR_DYNAMIC, None, &[1, 2, 3, 7], false);
    // Object Property：GET_PROP 写 a 槽、DYNAMIC 写 b 槽
    assert_contract(OpCode::GET_PROP, Some(2), &[1, 3], false);
    assert_contract(OpCode::GET_PROP_DYNAMIC, Some(3), &[1, 2], false);
    assert_contract(OpCode::IC_GET_PROP, Some(1), &[1, 2], false);
    assert_contract(OpCode::SET_PROP, None, &[1, 2, 3], false);
    assert_contract(OpCode::SET_PROP_DYNAMIC, None, &[1, 2, 3], false);
    // SET_PROP_BATCH：b 槽是 Imm(slot) 非寄存器，canonical 构造下仍按槽位读。
    assert_contract(OpCode::SET_PROP_BATCH, None, &[1, 2, 3], false);
    assert_contract(OpCode::IC_SET_PROP, None, &[1, 2, 3], false);
    assert_contract(OpCode::SET_ELEM, None, &[1, 2, 3], false);
    assert_group(&[OpCode::NEW_OBJECT, OpCode::NEW_SESSION_OBJECT, OpCode::NEW_ARRAY], Some(1), &[], true);
    // define 语义属性写入 / arguments / rest
    assert_group(
        &[
            OpCode::DEFINE_PROP,
            OpCode::DEFINE_GLOBAL_PROP,
            OpCode::DEFINE_GLOBAL_PROP_IF_ABSENT,
            OpCode::DEFINE_GLOBAL_FUNC_BIND,
        ],
        None,
        &[1, 2, 3],
        false,
    );
    // 全局函数绑定声明检查：a 槽无操作数，use 集只含 rd/b。
    assert_contract(OpCode::CAN_DECLARE_GLOBAL_FUNC, None, &[1, 3], false);
    assert_group(&[OpCode::DEFINE_GLOBAL_PROP_C, OpCode::DEFINE_GLOBAL_PROP_C_IF_ABSENT], None, &[2], false);
    // delete 全局内置：结果写 rd、镜像槽寄存器是 a 槽 use
    assert_contract(OpCode::DELETE_GLOBAL_PROP_C, Some(1), &[2], false);
    assert_contract(OpCode::DEFINE_PROP_ATTRS, None, &[1, 2, 3], false);
    assert_contract(OpCode::DEFINE_ACCESSOR_ATTRS, None, &[1, 2, 3], false);
    assert_group(&[OpCode::CREATE_ARGUMENTS, OpCode::CREATE_REST_ARRAY], Some(1), &[], false);
    // 成员更新：val 槽原地写（a/b 槽）
    assert_group(&[OpCode::MEMBER_INC, OpCode::MEMBER_DEC], Some(2), &[1, 3], false);
    assert_group(&[OpCode::DYN_MEMBER_INC, OpCode::DYN_MEMBER_DEC], Some(3), &[1, 2], false);
    assert_group(
        &[
            OpCode::COMPOUND_MEMBER_ADD,
            OpCode::COMPOUND_MEMBER_SUB,
            OpCode::COMPOUND_MEMBER_MUL,
            OpCode::COMPOUND_MEMBER_DIV,
            OpCode::COMPOUND_MEMBER_MOD,
            OpCode::COMPOUND_MEMBER_EXP,
            OpCode::COMPOUND_MEMBER_BIT_AND,
            OpCode::COMPOUND_MEMBER_BIT_OR,
            OpCode::COMPOUND_MEMBER_BIT_XOR,
            OpCode::COMPOUND_MEMBER_SHL,
            OpCode::COMPOUND_MEMBER_SHR,
            OpCode::COMPOUND_MEMBER_USHR,
        ],
        Some(2),
        &[1, 2, 3],
        false,
    );
    // 私有成员：brand_reg 非 0 时产生额外 use
    assert_contract(OpCode::GET_PRIVATE, Some(1), &[2, 3, 7], false);
    assert_contract(OpCode::SET_PRIVATE, None, &[1, 2, 3, 7], false);
    assert_contract(OpCode::INIT_PRIVATE, None, &[1, 2, 3], false);
    assert_contract(OpCode::PRIVATE_BRAND_IN, Some(1), &[2, 3], false);
    // 生成器 / 异步：恢复值经 reg 0
    assert_group(&[OpCode::YIELD, OpCode::YIELD_STAR, OpCode::AWAIT], Some(0), &[1], false);
    assert_contract(OpCode::FOR_AWAIT_OF_CLOSE, None, &[], false);
    // 位 / 对象 / 杂项
    assert_contract(OpCode::TO_OBJECT, Some(1), &[1], false);
    // NOP：rd=None 时 def_reg 仍为 Some(0)（catch-all 语义，表内保持 def=Rd）
    assert_eq!(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None).def_reg(), Some(0));
    assert_contract(OpCode::NOP, Some(1), &[], true);
    assert_contract(OpCode::VOID, Some(1), &[], true);
}
