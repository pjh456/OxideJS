//! 单向前扫常量传播：per-register known 表 + 汇合点清空 + 就地改写。
//!
//! pass 形态：对单个函数逐条前扫，维护 `known: Vec<Option<Constant>>`（按 vreg
//! 号索引）记录各寄存器的已知常量值。命中改写表即就地改写指令（改 op 与操作数
//! 槽，指令位置不动），不删除任何指令、不重映射 `label_pos`。
//!
//! 汇合点健全性：`label_pos` 指向的指令下标（控制流汇合点）整体清空 known 表——
//! 单向前扫状态只在前一条路径上有效，跨汇合的陈旧知识会误折。
//!
//! 改写后失去消费者的 `LOAD_CONST` 与死 `MOV` 由下游 DCE 清理（保守 DCE 的 use
//! 计数迭代删纯指令、精确 DCE 删死 MOV），本 pass 不产生残留。

use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

use crate::fold_table;

/// 常量传播与折叠：单函数单向前扫，就地改写。
///
/// 返回是否发生改写（仅调试日志用）。不删指令、不重映射 label；改写只改变指令
/// 的 op 与操作数，不改变跳转结构，CFG 的块划分与边集不变，由下游 DCE 在折叠后
/// 的指令流上重建 CFG 吸收。
///
/// pass 级前置：`const_overflow` 已被 emit 层置位时程序已知无效，直接返回不改写
/// （无折叠无效程序的意义，且加常量会雪上加霜）。
pub fn fold_function(f: &mut IRFunction) -> bool {
    // pass 级前置：const_overflow 已置位即程序已知无效，跳过整个 pass。
    if f.const_overflow {
        return false;
    }

    let insts = &mut f.insts;
    let constants = &mut f.constants;
    let label_pos = &f.label_pos;

    // 预计算 label 目标位图（控制流汇合点）。
    let mut label_targets = vec![false; insts.len()];
    for &pos in label_pos.iter().flatten() {
        if pos < insts.len() {
            label_targets[pos] = true;
        }
    }

    // known 表上界 = 函数内 def/use 寄存器号最大值加一。
    let max_reg = insts.iter().fold(0u32, |m, inst| {
        let mut mx = m;
        if let Some(r) = inst.def_reg() {
            mx = mx.max(r);
        }
        for r in inst.use_regs() {
            mx = mx.max(r);
        }
        mx
    });

    let mut known: Vec<Option<Constant>> = vec![None; (max_reg + 1) as usize];
    let mut changed = false;

    for (i, inst) in insts.iter_mut().enumerate() {
        // 汇合点：label 目标整体清空 known（单向前扫状态只在前一条路径上有效）。
        if label_targets[i] {
            known.fill(None);
        }

        match inst.op {
            // LOAD_CONST 是常量值来源：known[rd] = constants[idx]。
            OpCode::LOAD_CONST => {
                if let Some(rd) = inst.def_reg() {
                    known[rd as usize] = match inst.a {
                        Operand::Const(idx) => constants.get(idx as usize).cloned(),
                        _ => None,
                    };
                }
                continue;
            }
            // MOV 是纯拷贝：known[rd] = known[src]。
            OpCode::MOV => {
                if let Some(rd) = inst.def_reg() {
                    known[rd as usize] = match inst.a {
                        Operand::Reg(src) => known.get(src as usize).and_then(|c| c.clone()),
                        _ => None,
                    };
                }
                continue;
            }
            _ => {}
        }

        // A 组：常量折叠（操作数均已知）→ 改写为 LOAD_CONST。
        let folded = match inst.op {
            // VOID 无操作数，恒折 Undefined。
            OpCode::VOID => Some(Constant::Undefined),
            // 其余一元：操作数须为已知寄存器。
            OpCode::NOT | OpCode::TYPEOF | OpCode::NEG | OpCode::UNARY_PLUS | OpCode::BIT_NOT => match inst.a {
                Operand::Reg(x) => known
                    .get(x as usize)
                    .and_then(|c| c.as_ref())
                    .and_then(|c| fold_table::fold_unary(inst.op, c)),
                _ => None,
            },
            // 二元算术：两操作数须为已知寄存器。
            OpCode::ADD | OpCode::SUB | OpCode::MUL | OpCode::DIV | OpCode::MOD | OpCode::EXP => {
                match (inst.a, inst.b) {
                    (Operand::Reg(x), Operand::Reg(y)) => match (
                        known.get(x as usize).and_then(|c| c.as_ref()),
                        known.get(y as usize).and_then(|c| c.as_ref()),
                    ) {
                        (Some(l), Some(r)) => fold_table::fold_binary(inst.op, l, r),
                        _ => None,
                    },
                    _ => None,
                }
            }
            // C 组位运算与比较：两操作数须为已知寄存器且均在四变体域
            // （BigInt/String/Undefined 操作数由表内守卫不折）。
            OpCode::BIT_AND
            | OpCode::BIT_OR
            | OpCode::BIT_XOR
            | OpCode::SHL
            | OpCode::SHR
            | OpCode::USHR
            | OpCode::LT
            | OpCode::GT
            | OpCode::LTE
            | OpCode::GTE
            | OpCode::EQ
            | OpCode::NEQ
            | OpCode::STRICT_EQ
            | OpCode::STRICT_NEQ => match (inst.a, inst.b) {
                (Operand::Reg(x), Operand::Reg(y)) => match (
                    known.get(x as usize).and_then(|c| c.as_ref()),
                    known.get(y as usize).and_then(|c| c.as_ref()),
                ) {
                    (Some(l), Some(r)) => fold_table::fold_c_group(inst.op, l, r),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };

        if let Some(value) = folded {
            // 新常量入池（去重加溢出守卫）；池满时 A 组失效，跳过改写并清 known。
            if let Some(idx) = intern(constants, &value) {
                *inst = Inst::load_const(inst.rd, idx);
                changed = true;
                if let Some(rd) = inst.def_reg() {
                    known[rd as usize] = Some(value);
                }
            } else if let Some(rd) = inst.def_reg() {
                known[rd as usize] = None;
            }
        } else {
            // B 组：逻辑短路（左操作数已知）→ 改写为 MOV。
            let select_left = match (inst.op, inst.a) {
                (OpCode::AND | OpCode::OR | OpCode::NULLISH, Operand::Reg(x)) => known
                    .get(x as usize)
                    .and_then(|c| c.as_ref())
                    .and_then(|c| fold_table::short_circuit(inst.op, c)),
                _ => None,
            };
            if let Some(select_left) = select_left {
                let src = if select_left { inst.a } else { inst.b };
                *inst = Inst::inst_mov(inst.rd, src);
                changed = true;
                if let (Some(rd), Operand::Reg(s)) = (inst.def_reg(), src) {
                    known[rd as usize] = known.get(s as usize).and_then(|c| c.clone());
                }
            } else {
                // 不可折：def 寄存器为不定值。
                if let Some(rd) = inst.def_reg() {
                    known[rd as usize] = None;
                }
            }
        }
    }

    changed
}

/// 常量池判等：`Number` 臂按位模式比较（区分 `-0.0` 与 `+0.0`，与 emit 层
/// `ConstantKey` 的 `to_bits` 口径一致），其余臂用派生 `PartialEq`。
fn const_eq(a: &Constant, b: &Constant) -> bool {
    match (a, b) {
        (Constant::Number(x), Constant::Number(y)) => x.to_bits() == y.to_bits(),
        _ => a == b,
    }
}

/// 新常量入池：线性扫描判等（命中复用既有下标）；未命中推尾取新下标；池满
/// （`u16::MAX`）时返回 `None` 禁用 A 组与 C 组改写（不置 `const_overflow`，
/// 程序在 pass 前是有效的；B 组 MOV 不新增常量，不受影响）。
fn intern(constants: &mut Vec<Constant>, c: &Constant) -> Option<u16> {
    if let Some(idx) = constants.iter().position(|x| const_eq(x, c)) {
        return Some(idx as u16);
    }
    if constants.len() >= u16::MAX as usize {
        return None;
    }
    constants.push(c.clone());
    Some((constants.len() - 1) as u16)
}
