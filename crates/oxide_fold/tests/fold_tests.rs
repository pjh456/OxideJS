//! 常量折叠 pass 集成测试：手工构造 IR，断言前扫后指令流与常量池形态。
//!
//! 覆盖五类行为：
//! - 常量传播（LOAD_CONST 与 MOV 的 known 表流动）
//! - 折叠（A 组一元与算术改写为 LOAD_CONST，C 组位运算与比较改写为
//!   LOAD_CONST，B 组逻辑短路改写为 MOV）
//! - 汇合点（label 目标整体清空 known，跨汇合不折）
//! - 负零守卫（`-0.0 + 0.0` 得 `+0.0`，不取恒等捷径）
//! - 池溢出守卫（池满时 A 组与 C 组不折、B 组照折；`const_overflow`
//!   置位时整个 pass 跳过）

use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_fold::fold;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 构造空函数并填常量池与指令流。
fn func<const N: usize>(constants: &[Constant; N], insts: Vec<Inst>) -> IRFunction {
    let mut f = IRFunction::new();
    f.constants = constants.to_vec();
    f.insts = insts;
    f
}

/// 取指令操作数：`LOAD_CONST r, idx` 返回 `Some(idx)`，其余返回 `None`。
fn load_const_idx(inst: &Inst) -> Option<u16> {
    if inst.op == OpCode::LOAD_CONST {
        if let Operand::Const(idx) = inst.a {
            return Some(idx);
        }
    }
    None
}

/// 断言指令是 `LOAD_CONST rd, idx`。
fn assert_load_const(inst: &Inst, rd: u32, idx: u16) {
    assert_eq!(inst.op, OpCode::LOAD_CONST, "expected LOAD_CONST, got {inst:?}");
    assert_eq!(inst.rd, Operand::Reg(rd), "expected rd={rd}, got {inst:?}");
    assert_eq!(load_const_idx(inst), Some(idx), "expected const[{idx}], got {inst:?}");
}

/// 断言指令是 `MOV rd, src`。
fn assert_mov(inst: &Inst, rd: u32, src: u32) {
    assert_eq!(inst.op, OpCode::MOV, "expected MOV, got {inst:?}");
    assert_eq!(inst.rd, Operand::Reg(rd), "expected rd={rd}, got {inst:?}");
    assert_eq!(inst.a, Operand::Reg(src), "expected src={src}, got {inst:?}");
}

// ── 常量传播 ──

#[test]
fn propagation_through_mov() {
    // LOAD_CONST r1, 1 / MOV r2, r1 / LOAD_CONST r3, 2 / ADD r4, r2, r3
    // 期望：ADD 折为 LOAD_CONST r4, 3（新常量 Int(3)）。
    let c1 = Constant::Int(1);
    let c2 = Constant::Int(2);
    let mut f = func(
        &[c1, c2],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::inst_mov(Operand::Reg(2), Operand::Reg(1)),
            Inst::load_const(Operand::Reg(3), 1),
            Inst::new(OpCode::ADD, Operand::Reg(4), Operand::Reg(2), Operand::Reg(3)),
        ],
    );
    assert!(fold(&mut f), "应发生改写");
    assert_load_const(&f.insts[3], 4, 2);
    assert_eq!(f.constants[2], Constant::Int(3));
}

#[test]
fn propagation_chain() {
    // LOAD_CONST r1, 5 / MOV r2, r1 / MOV r3, r2 / NEG r4, r3
    // 期望：NEG 折为 LOAD_CONST r4, -5（新常量 Number(-5.0)，NEG 恒升 double）。
    let c = Constant::Int(5);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::inst_mov(Operand::Reg(2), Operand::Reg(1)),
            Inst::inst_mov(Operand::Reg(3), Operand::Reg(2)),
            Inst::new(OpCode::NEG, Operand::Reg(4), Operand::Reg(3), Operand::None),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[3], 4, 1);
    assert_eq!(f.constants[1], Constant::Number(-5.0));
}

#[test]
fn unknown_operand_not_folded() {
    // LOAD_CONST r1, 1 / LOAD_VAR r2, x / ADD r3, r1, r2
    // 期望：r2 未知（LOAD_VAR 不定值），ADD 不折。
    let c = Constant::Int(1);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::LOAD_VAR, Operand::Reg(2), Operand::Reg(7), Operand::None),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "不应发生改写");
    assert_eq!(f.insts[2].op, OpCode::ADD, "ADD 不应被折叠");
    assert_eq!(f.constants.len(), 1, "不应新增常量");
}

// ── 折叠：A 组一元 ──

#[test]
fn fold_not() {
    // LOAD_CONST r1, 1 / NOT r2, r1 → Boolean(false)
    let c = Constant::Int(1);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::NOT, Operand::Reg(2), Operand::Reg(1), Operand::None),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[1], 2, 1);
    assert_eq!(f.constants[1], Constant::Boolean(false));
}

#[test]
fn fold_void() {
    // VOID r1（无操作数）→ Undefined
    let mut f = func(&[], vec![Inst::new(OpCode::VOID, Operand::Reg(1), Operand::None, Operand::None)]);
    fold(&mut f);
    assert_load_const(&f.insts[0], 1, 0);
    assert_eq!(f.constants[0], Constant::Undefined);
}

#[test]
fn fold_typeof() {
    // LOAD_CONST r1, 1 / TYPEOF r2, r1 → "number"
    let c = Constant::Int(1);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::TYPEOF, Operand::Reg(2), Operand::Reg(1), Operand::None),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[1], 2, 1);
    // "number" 的池键形态（无 FFFD，恒等）。
    assert_eq!(f.constants[1], Constant::String("number".to_string()));
}

#[test]
fn fold_unary_plus() {
    // LOAD_CONST r1, 2 / UNARY_PLUS r2, r1 → Number(2.0)
    let c = Constant::Int(2);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::UNARY_PLUS, Operand::Reg(2), Operand::Reg(1), Operand::None),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[1], 2, 1);
    assert_eq!(f.constants[1], Constant::Number(2.0));
}

#[test]
fn fold_bit_not() {
    // LOAD_CONST r1, 5 / BIT_NOT r2, r1 → Int(!5) = Int(-6)
    let c = Constant::Int(5);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::BIT_NOT, Operand::Reg(2), Operand::Reg(1), Operand::None),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[1], 2, 1);
    assert_eq!(f.constants[1], Constant::Int(!5i32));
}

// ── 折叠：A 组二元算术 ──

#[test]
fn fold_add_int_int() {
    // LOAD_CONST r1, 1 / LOAD_CONST r2, 2 / ADD r3, r1, r2 → Int(3)
    let (a, b) = (Constant::Int(1), Constant::Int(2));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 2);
    assert_eq!(f.constants[2], Constant::Int(3));
}

#[test]
fn fold_add_int_overflow_promotes() {
    // i32::MAX + 1 溢出 → Number（镜像 int_add）。
    let (a, b) = (Constant::Int(i32::MAX), Constant::Int(1));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 2);
    assert_eq!(f.constants[2], Constant::Number(i32::MAX as f64 + 1.0));
}

#[test]
fn fold_sub_mul_div_mod() {
    // 10 - 4 = 6 / 3 * 5 = 15 / 8 / 2 = 4 / 7 % 3 = 1，均恒 Number。
    let base = [
        Constant::Int(10),
        Constant::Int(4),
        Constant::Int(3),
        Constant::Int(5),
        Constant::Int(8),
        Constant::Int(2),
        Constant::Int(7),
    ];
    let mut f = func(
        &base,
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::SUB, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
            Inst::load_const(Operand::Reg(4), 2),
            Inst::load_const(Operand::Reg(5), 3),
            Inst::new(OpCode::MUL, Operand::Reg(6), Operand::Reg(4), Operand::Reg(5)),
            Inst::load_const(Operand::Reg(7), 4),
            Inst::load_const(Operand::Reg(8), 5),
            Inst::new(OpCode::DIV, Operand::Reg(9), Operand::Reg(7), Operand::Reg(8)),
            Inst::load_const(Operand::Reg(10), 6),
            Inst::new(OpCode::MOD, Operand::Reg(11), Operand::Reg(10), Operand::Reg(8)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 7);
    assert_eq!(f.constants[7], Constant::Number(6.0));
    assert_load_const(&f.insts[5], 6, 8);
    assert_eq!(f.constants[8], Constant::Number(15.0));
    assert_load_const(&f.insts[8], 9, 9);
    assert_eq!(f.constants[9], Constant::Number(4.0));
    assert_load_const(&f.insts[10], 11, 10);
    assert_eq!(f.constants[10], Constant::Number(1.0));
}

#[test]
fn fold_div_by_zero_not_folded() {
    // 1 / 0 除零 → 不折（保持运行时指令为除零编码的唯一来源）。
    let (a, b) = (Constant::Int(1), Constant::Int(0));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::DIV, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "除零不应折叠");
    assert_eq!(f.insts[2].op, OpCode::DIV);
    assert_eq!(f.constants.len(), 2);
}

#[test]
fn fold_exp_negative_not_folded() {
    // 2 ** -1 负指数 → 不折。
    let (a, b) = (Constant::Int(2), Constant::Int(-1));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::EXP, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "负指数不应折叠");
    assert_eq!(f.insts[2].op, OpCode::EXP);
}

#[test]
fn fold_exp_positive() {
    // 2 ** 10 = 1024 → Number。
    let (a, b) = (Constant::Int(2), Constant::Int(10));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::EXP, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 2);
    assert_eq!(f.constants[2], Constant::Number(1024.0));
}

#[test]
fn fold_add_bigint() {
    // 1n + 2n = 3n → BigInt。
    let (a, b) = (
        Constant::BigInt(num_bigint::BigInt::from(1)),
        Constant::BigInt(num_bigint::BigInt::from(2)),
    );
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 2);
    assert_eq!(f.constants[2], Constant::BigInt(num_bigint::BigInt::from(3)));
}

#[test]
fn fold_add_bigint_mixed_not_folded() {
    // 1n + 1 BigInt 混合 → 不折（运行期抛 TypeError）。
    let (a, b) = (Constant::BigInt(num_bigint::BigInt::from(1)), Constant::Int(1));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "BigInt 混合不应折叠");
    assert_eq!(f.insts[2].op, OpCode::ADD);
}

// ── 折叠：B 组逻辑短路 ──

#[test]
fn fold_and_truthy_selects_right() {
    // LOAD_CONST r1, 1（真）/ AND r2, r1, r3 → MOV r2, r3。
    let c = Constant::Int(1);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::AND, Operand::Reg(2), Operand::Reg(1), Operand::Reg(3)),
        ],
    );
    fold(&mut f);
    assert_mov(&f.insts[1], 2, 3);
    assert_eq!(f.constants.len(), 1, "MOV 不新增常量");
}

#[test]
fn fold_and_falsy_selects_left() {
    // LOAD_CONST r1, 0（假）/ AND r2, r1, r3 → MOV r2, r1。
    let c = Constant::Int(0);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::AND, Operand::Reg(2), Operand::Reg(1), Operand::Reg(3)),
        ],
    );
    fold(&mut f);
    assert_mov(&f.insts[1], 2, 1);
}

#[test]
fn fold_or_falsy_selects_right() {
    // LOAD_CONST r1, 0（假）/ OR r2, r1, r3 → MOV r2, r3。
    let c = Constant::Int(0);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::OR, Operand::Reg(2), Operand::Reg(1), Operand::Reg(3)),
        ],
    );
    fold(&mut f);
    assert_mov(&f.insts[1], 2, 3);
}

#[test]
fn fold_nullish_nullish_selects_right() {
    // LOAD_CONST r1, null / NULLISH r2, r1, r3 → MOV r2, r3。
    let c = Constant::Null;
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::NULLISH, Operand::Reg(2), Operand::Reg(1), Operand::Reg(3)),
        ],
    );
    fold(&mut f);
    assert_mov(&f.insts[1], 2, 3);
}

#[test]
fn fold_nullish_non_nullish_selects_left() {
    // LOAD_CONST r1, 5（非 nullish）/ NULLISH r2, r1, r3 → MOV r2, r1。
    let c = Constant::Int(5);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::NULLISH, Operand::Reg(2), Operand::Reg(1), Operand::Reg(3)),
        ],
    );
    fold(&mut f);
    assert_mov(&f.insts[1], 2, 1);
}

// ── 汇合点健全性 ──

#[test]
fn merge_point_clears_known() {
    // LOAD_CONST r1, 1 / JMP label0 / LOAD_CONST r1, 99 / label0: ADD r2, r1, r1
    // label0 是汇合点：整体清空 known，r1 未知 → ADD 不折。
    let (a, b) = (Constant::Int(1), Constant::Int(99));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::jmp(0),
            Inst::load_const(Operand::Reg(1), 1),
            Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(1), Operand::Reg(1)),
        ],
    );
    f.label_pos = vec![Some(3)];
    f.label_count = 1;
    assert!(!fold(&mut f), "汇合点后不应折叠");
    assert_eq!(f.insts[3].op, OpCode::ADD, "跨汇合的 ADD 不应被折叠");
    assert_eq!(f.constants.len(), 2, "不应新增常量");
}

#[test]
fn merge_point_does_not_block_same_path() {
    // label0: LOAD_CONST r1, 1 / ADD r2, r1, r1（label 后同路径）
    // label 清空 known，但其后的 LOAD_CONST 重新定值 r1 → ADD 折。
    let c = Constant::Int(1);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(1), Operand::Reg(1)),
        ],
    );
    f.label_pos = vec![Some(0)];
    f.label_count = 1;
    fold(&mut f);
    assert_load_const(&f.insts[1], 2, 1);
    assert_eq!(f.constants[1], Constant::Int(2));
}

// ── 负零守卫 ──

#[test]
fn negative_zero_add_yields_positive_zero() {
    // LOAD_CONST r1, -0.0 / LOAD_CONST r2, 0.0 / ADD r3, r1, r2
    // 期望：折为 Number(+0.0)（符号位清零），不得取恒等捷径得 -0.0。
    let (a, b) = (Constant::Number(-0.0), Constant::Number(0.0));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    fold(&mut f);
    // 折值 +0.0 经位精确判等去重，命中既有 +0.0 池条目（下标 1），不得误命中
    // 下标 0 的 -0.0 条目。
    assert_load_const(&f.insts[2], 3, 1);
    // 断言符号位：+0.0 的位模式为 0，-0.0 为 0x8000_0000_0000_0000。
    match &f.constants[1] {
        Constant::Number(d) => assert_eq!(d.to_bits(), 0x0000_0000_0000_0000, "应为 +0.0"),
        other => panic!("期望 Number，got {other:?}"),
    }
}

#[test]
fn negative_zero_neg_yields_positive_zero() {
    // LOAD_CONST r1, -0.0 / NEG r2, r1 → Number(+0.0)（NEG 翻转符号位，-(-0.0) = +0.0）。
    let c = Constant::Number(-0.0);
    let mut f = func(
        &[c],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::new(OpCode::NEG, Operand::Reg(2), Operand::Reg(1), Operand::None),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[1], 2, 1);
    match &f.constants[1] {
        Constant::Number(d) => assert_eq!(d.to_bits(), 0x0000_0000_0000_0000, "NEG(-0.0) 应为 +0.0"),
        other => panic!("期望 Number，got {other:?}"),
    }
}

// ── 折叠：C 组位运算 ──

#[test]
fn fold_bit_and_or_xor() {
    // 12 & 10 = 8 / 12 | 10 = 14 / 12 ^ 10 = 6，均恒 Int。
    let (a, b) = (Constant::Int(12), Constant::Int(10));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::BIT_AND, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
            Inst::new(OpCode::BIT_OR, Operand::Reg(4), Operand::Reg(1), Operand::Reg(2)),
            Inst::new(OpCode::BIT_XOR, Operand::Reg(5), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 2);
    assert_eq!(f.constants[2], Constant::Int(8));
    assert_load_const(&f.insts[3], 4, 3);
    assert_eq!(f.constants[3], Constant::Int(14));
    assert_load_const(&f.insts[4], 5, 4);
    assert_eq!(f.constants[4], Constant::Int(6));
}

#[test]
fn fold_shifts() {
    // 1 << 34 = 1 << 2 = 4（移位量与 0x1F 掩码）/
    // -2147483648 >> 1 = -1073741824（算术右移）/
    // -1 >>> 0 = 4294967295（结果超 i32::MAX，升 Number）。
    let (a, b) = (Constant::Int(1), Constant::Int(34));
    let (c, d) = (Constant::Int(i32::MIN), Constant::Int(1));
    let (e, f2) = (Constant::Int(-1), Constant::Int(0));
    let mut f = func(
        &[a, b, c, d, e, f2],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::SHL, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
            Inst::load_const(Operand::Reg(4), 2),
            Inst::load_const(Operand::Reg(5), 3),
            Inst::new(OpCode::SHR, Operand::Reg(6), Operand::Reg(4), Operand::Reg(5)),
            Inst::load_const(Operand::Reg(7), 4),
            Inst::load_const(Operand::Reg(8), 5),
            Inst::new(OpCode::USHR, Operand::Reg(9), Operand::Reg(7), Operand::Reg(8)),
        ],
    );
    fold(&mut f);
    assert_load_const(&f.insts[2], 3, 6);
    assert_eq!(f.constants[6], Constant::Int(4));
    assert_load_const(&f.insts[5], 6, 7);
    assert_eq!(f.constants[7], Constant::Int(-1_073_741_824));
    assert_load_const(&f.insts[8], 9, 8);
    assert_eq!(f.constants[8], Constant::Number(4_294_967_295.0));
}

// ── 折叠：C 组比较 ──

#[test]
fn fold_comparisons() {
    // 1 < 2 = true / 1 > 2 = false / 1 <= 1 = true / 2 >= 3 = false /
    // 1 == 1.0 = true / 1 != 2 = true / 1 === 1.0 = true / 1 !== 2 = true /
    // NaN < 1 = false（关系比较对 NaN 返回 None，归 false）。
    let (a, b, c, d) = (Constant::Int(1), Constant::Int(2), Constant::Number(1.0), Constant::Int(3));
    let (e, f2) = (Constant::Number(f64::NAN), Constant::Int(1));
    let mut f = func(
        &[a, b, c, d, e, f2],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::LT, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
            Inst::new(OpCode::GT, Operand::Reg(4), Operand::Reg(1), Operand::Reg(2)),
            Inst::load_const(Operand::Reg(5), 0),
            Inst::new(OpCode::LTE, Operand::Reg(6), Operand::Reg(5), Operand::Reg(1)),
            Inst::load_const(Operand::Reg(7), 1),
            Inst::load_const(Operand::Reg(8), 3),
            Inst::new(OpCode::GTE, Operand::Reg(9), Operand::Reg(7), Operand::Reg(8)),
            Inst::load_const(Operand::Reg(10), 2),
            Inst::new(OpCode::EQ, Operand::Reg(11), Operand::Reg(1), Operand::Reg(10)),
            Inst::new(OpCode::NEQ, Operand::Reg(12), Operand::Reg(1), Operand::Reg(2)),
            Inst::new(OpCode::STRICT_EQ, Operand::Reg(13), Operand::Reg(1), Operand::Reg(10)),
            Inst::new(OpCode::STRICT_NEQ, Operand::Reg(14), Operand::Reg(1), Operand::Reg(2)),
            Inst::load_const(Operand::Reg(15), 4),
            Inst::new(OpCode::LT, Operand::Reg(16), Operand::Reg(15), Operand::Reg(1)),
        ],
    );
    fold(&mut f);
    // 池原有 6 条：Boolean(true) 首落位下标 6、Boolean(false) 下标 7，
    // 后续同值折叠复用。
    assert_load_const(&f.insts[2], 3, 6);
    assert_eq!(f.constants[6], Constant::Boolean(true));
    assert_load_const(&f.insts[3], 4, 7);
    assert_eq!(f.constants[7], Constant::Boolean(false));
    assert_load_const(&f.insts[5], 6, 6);
    assert_load_const(&f.insts[8], 9, 7);
    assert_load_const(&f.insts[10], 11, 6);
    assert_load_const(&f.insts[11], 12, 6);
    assert_load_const(&f.insts[12], 13, 6);
    assert_load_const(&f.insts[13], 14, 6);
    assert_load_const(&f.insts[15], 16, 7);
    assert_eq!(f.constants.len(), 8, "同值折叠应复用池下标");
}

// ── C 组操作数域守卫（BigInt/String/Undefined 不折） ──

#[test]
fn c_group_bigint_operand_not_folded() {
    // 1n & 1 → 不折（BigInt 操作数不折，运行期抛 TypeError）。
    let (a, b) = (Constant::BigInt(num_bigint::BigInt::from(1)), Constant::Int(1));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::BIT_AND, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "BigInt 操作数不应折叠");
    assert_eq!(f.insts[2].op, OpCode::BIT_AND);
    assert_eq!(f.constants.len(), 2, "不应新增常量");
}

#[test]
fn c_group_string_operand_not_folded() {
    // "1" < "2" → 不折（String 操作数不折，字符串比较路径为后续任务）。
    let (a, b) = (Constant::String("1".to_string()), Constant::String("2".to_string()));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::LT, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "String 操作数不应折叠");
    assert_eq!(f.insts[2].op, OpCode::LT);
}

#[test]
fn c_group_undefined_operand_not_folded() {
    // 1 < undefined → 不折（Undefined 操作数不折，与共享表四臂口径一致）。
    let (a, b) = (Constant::Int(1), Constant::Undefined);
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::LT, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    assert!(!fold(&mut f), "Undefined 操作数不应折叠");
    assert_eq!(f.insts[2].op, OpCode::LT);
}

// ── 池溢出守卫 ──

#[test]
fn pool_full_disables_a_and_c_groups() {
    // 池 65535 条（Int(0)..Int(65534)）即满：NEG（A 组）与 EQ（C 组）折值
    // 不在池内且无空间推尾 → 不折；AND（B 组）MOV 不新增常量 → 照折。
    let mut f = IRFunction::new();
    f.constants = (0..65535).map(Constant::Int).collect();
    f.insts = vec![
        Inst::load_const(Operand::Reg(1), 0),
        Inst::load_const(Operand::Reg(2), 65534),
        Inst::new(OpCode::NEG, Operand::Reg(3), Operand::Reg(1), Operand::None),
        Inst::new(OpCode::EQ, Operand::Reg(4), Operand::Reg(1), Operand::Reg(2)),
        Inst::new(OpCode::AND, Operand::Reg(5), Operand::Reg(1), Operand::Reg(2)),
    ];
    fold(&mut f);
    assert_eq!(f.insts[2].op, OpCode::NEG, "池满时 A 组不应折叠");
    assert_eq!(f.insts[3].op, OpCode::EQ, "池满时 C 组不应折叠");
    assert_mov(&f.insts[4], 5, 1);
    assert_eq!(f.constants.len(), 65535, "池满时不应新增常量");
}

#[test]
fn const_overflow_skips_pass() {
    // const_overflow 已置位（emit 层）：整个 pass 跳过，不改写。
    let (a, b) = (Constant::Int(1), Constant::Int(2));
    let mut f = func(
        &[a, b],
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 1),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
        ],
    );
    f.const_overflow = true;
    assert!(!fold(&mut f), "const_overflow 置位时不应改写");
    assert_eq!(f.insts[2].op, OpCode::ADD);
    assert_eq!(f.constants.len(), 2);
}

// ── 确定性 ──

#[test]
fn fold_is_deterministic() {
    // 同一源连续折叠八次，指令流与常量池指纹全等。
    let build = || {
        func(
            &[Constant::Int(1), Constant::Int(2)],
            vec![
                Inst::load_const(Operand::Reg(1), 0),
                Inst::load_const(Operand::Reg(2), 1),
                Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
                Inst::new(OpCode::NOT, Operand::Reg(4), Operand::Reg(3), Operand::None),
            ],
        )
    };
    let mut f = build();
    fold(&mut f);
    let fingerprint = |f: &IRFunction| (f.insts.clone(), f.constants.clone());
    let first = fingerprint(&f);
    for _ in 0..7 {
        let mut g = build();
        fold(&mut g);
        assert_eq!(fingerprint(&g), first, "折叠结果应确定");
    }
}

#[test]
fn c_group_fold_is_deterministic() {
    // 同一源（C 组位运算加比较）连续折叠八次，指令流与常量池指纹全等。
    let build = || {
        func(
            &[Constant::Int(12), Constant::Int(10), Constant::Int(-1), Constant::Int(0)],
            vec![
                Inst::load_const(Operand::Reg(1), 0),
                Inst::load_const(Operand::Reg(2), 1),
                Inst::new(OpCode::BIT_XOR, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
                Inst::load_const(Operand::Reg(4), 2),
                Inst::load_const(Operand::Reg(5), 3),
                Inst::new(OpCode::USHR, Operand::Reg(6), Operand::Reg(4), Operand::Reg(5)),
                Inst::new(OpCode::LT, Operand::Reg(7), Operand::Reg(3), Operand::Reg(6)),
            ],
        )
    };
    let mut f = build();
    fold(&mut f);
    let fingerprint = |f: &IRFunction| (f.insts.clone(), f.constants.clone());
    let first = fingerprint(&f);
    for _ in 0..7 {
        let mut g = build();
        fold(&mut g);
        assert_eq!(fingerprint(&g), first, "折叠结果应确定");
    }
}
