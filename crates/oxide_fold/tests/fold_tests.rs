//! 常量折叠 pass 集成测试：手工构造 IR，断言前扫后指令流与常量池形态。
//!
//! 覆盖四类行为：
//! - 常量传播（LOAD_CONST 与 MOV 的 known 表流动）
//! - 折叠（A 组一元与算术改写为 LOAD_CONST，B 组逻辑短路改写为 MOV）
//! - 汇合点（label 目标整体清空 known，跨汇合不折）
//! - 负零守卫（`-0.0 + 0.0` 得 `+0.0`，不取恒等捷径）

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
    let (a, b) = (
        Constant::BigInt(num_bigint::BigInt::from(1)),
        Constant::Int(1),
    );
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
