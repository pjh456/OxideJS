//! Inst 寄存器契约：def/use 扫描 + 副作用判定（DCE 与 liveness 共用单源）。
//!
//! `def_reg` / `use_regs` / `is_pure` 是 `Inst` 的类型契约方法（`impl Inst` 与 `inst.rs`
//! 中的构造 API 同属一类型），落在 oxide_ir 契约层供 DCE 消费、liveness 复用。
//! 寄存器规则与 VM dispatch handler 行为一致：
//! CALL 隐式写 reg 0、GET_PROP 结果写 a/b 槽、HALT 隐式读 reg 0、None 槽映射 Reg(0)。

use oxide_bytecode::opcode::{OpCode, Slot, SlotSpec};
use smallvec::SmallVec;

use crate::inst::Inst;
use crate::operand::Operand;
use crate::IRFunction;

impl Inst {
    /// 本指令定义的寄存器（None 槽按 Reg(0) 映射；CALL 系含隐式 reg 0）。
    /// 无写入返回 None。
    ///
    /// # 步骤
    /// 表驱动解析：取语义表 def 槽位 → 解出对应操作数槽寄存器；Reg0 恒为常量 0。
    ///
    /// # 边界与前提
    /// - `SlotSpec::Range/SpreadArgs/TemplateExprs/BrandReg` 不可能作 def，返回 None。
    pub fn def_reg(&self) -> Option<u32> {
        match self.op.semantics().def? {
            SlotSpec::Slot(slot) => reg_of(slot_operand(slot, self)),
            SlotSpec::Reg0 => Some(0),
            _ => None,
        }
    }

    /// 本指令读取的寄存器（None 槽按 Reg(0) 映射；HALT 特判 reg 0；TEMPLATE_STR 解析 ext）。
    ///
    /// # 步骤
    /// 按语义表 uses 槽位序逐项解析，序即输出序。
    ///
    /// # 边界与前提
    /// - `Range`：nargs=ext[0]，从指定槽起连续 nargs 个寄存器。
    /// - `SpreadArgs`：ext[1..] 每个字 `& 0x7FFF_FFFF`。
    /// - `TemplateExprs`：ext[1..] 中 `seg>>31==1` 的低 31 位。
    /// - `BrandReg`：ext[0] 非 0 才产生 use。
    /// - `ExtReg`：ext[0] `& 0x7FFF_FFFF`（高位标记的寄存器号）。
    pub fn use_regs(&self) -> SmallVec<[u32; 4]> {
        let mut uses = SmallVec::new();
        for &spec in self.op.semantics().uses {
            match spec {
                SlotSpec::Slot(slot) => push_operand(&mut uses, slot_operand(slot, self)),
                SlotSpec::Reg0 => uses.push(0),
                SlotSpec::Range(slot) => {
                    let nargs = self.ext.first().copied().unwrap_or(0);
                    push_range(&mut uses, reg_of(slot_operand(slot, self)), nargs);
                }
                SlotSpec::SpreadArgs => {
                    for &w in self.ext.iter().skip(1) {
                        uses.push(w & 0x7FFF_FFFF);
                    }
                }
                SlotSpec::TemplateExprs => {
                    for seg in self.ext.iter().skip(1) {
                        if seg >> 31 == 1 {
                            uses.push(seg & 0x7FFF_FFFF);
                        }
                    }
                }
                SlotSpec::BrandReg => {
                    let brand_reg = self.ext.first().copied().unwrap_or(0);
                    if brand_reg != 0 {
                        uses.push(brand_reg);
                    }
                }
                SlotSpec::ExtReg => {
                    let w = self.ext.first().copied().unwrap_or(0);
                    uses.push(w & 0x7FFF_FFFF);
                }
            }
        }
        uses
    }

    /// 本指令是否无观察副作用（LOAD_VAR 的 This+derived 特判需要函数元信息）。
    ///
    /// 纯 = 结果未用时可删。表内 pure 是静态字段，LOAD_VAR 上下文例外在此叠加。
    pub fn is_pure(&self, f: &IRFunction) -> bool {
        let base = self.op.semantics().pure;
        base && !(self.op == OpCode::LOAD_VAR && self.a == Operand::This && f.is_derived_constructor)
    }
}

/// 槽位 → 对应操作数（表解析用；Const/Imm/Label 由 reg_of 过滤）。
fn slot_operand(slot: Slot, i: &Inst) -> &Operand {
    match slot {
        Slot::Rd => &i.rd,
        Slot::A => &i.a,
        Slot::B => &i.b,
    }
}

/// 操作数 → 物理寄存器号。None 槽映射 reg 0（与 lower.rs operand_to_u8 一致）；
/// Const/Imm/Label 不是寄存器槽，返回 None。
fn reg_of(o: &Operand) -> Option<u32> {
    match o {
        Operand::Reg(r) => Some(*r),
        Operand::This => Some(254),
        Operand::NewTarget => Some(255),
        Operand::None => Some(0),
        Operand::Const(_) | Operand::Imm(_) | Operand::Label(_) => None,
    }
}

/// 把寄存器操作数推入 use 集合（非寄存器槽跳过）。
fn push_operand(uses: &mut SmallVec<[u32; 4]>, o: &Operand) {
    if let Some(r) = reg_of(o) {
        uses.push(r);
    }
}

/// 推入连续参数区间 [first, first+nargs)。
fn push_range(uses: &mut SmallVec<[u32; 4]>, first: Option<u32>, nargs: u32) {
    if let Some(f) = first {
        uses.extend((0..nargs).map(|i| f + i));
    }
}
