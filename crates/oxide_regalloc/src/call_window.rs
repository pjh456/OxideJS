//! 调用点存活上界编码：RegAlloc 改写后按物理存活集为调用指令回填 ext 高 8 位。
//!
//! 调用指令 `ext[0] = nargs(低 8 位) | 存活上界(高 8 位)`。上界 = 调用点存活
//! 物理寄存器最大槽号 + 1（即压帧窗口 `regs[0..上界]`）；上界 0 表示未编码
//! （运行时回退到调用方 `active_reg_limit` 全量窗口）。`regs[254]/[255]`
//! （this/new.target）由帧单独保存不占窗口，计算时排除。生成器 / 异步函数体
//! 内部调用点不编码——挂起恢复按全量寄存器快照搬移，保持保守语义。含 TRY
//! 指令的函数同样不编码——异常 handler 的存活集不经分支/循环内调用点传播，
//! 截断窗口会丢仅 handler 存活的槽（见 `encode_call_window` 内 has_try 说明）。

use std::borrow::Cow;

use oxide_bytecode::opcode::OpCode;
use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 为每个字节码调用指令（CALL/NEW_EXPRESSION/SUPER_CALL）编码存活上界到 ext 高 8 位。
/// 嵌套子函数递归处理：每个子函数独立重跑 CFG + liveness（与 `alloc` 同纪律）。
///
/// # 边界与前提
/// - `live` 与 `f.insts` 维度不符时内部重算（防过期 LiveInfo 下标错位）
/// - 生成器 / 异步函数体内部调用点不编码（保持全量窗口）
/// - 只改写 `ext[0]` 高 8 位，nargs 低 8 位不动；无存活时保持 0（全量）
pub fn encode_call_window(f: &mut IRFunction, live: &LiveInfo) {
    if f.insts.is_empty() && f.nested.is_empty() {
        return;
    }
    // 含 try/catch/finally 的函数跳过编码：异常边只从 TRY 标记所在 BB 发出，
    // 分支/循环 BB 内调用点的 liveness 不含仅 catch/finally 存活的寄存器，
    // 截断窗口会丢槽（unwind 恢复后 handler 读到 callee 残留）。整体回退全量窗口。
    let has_try = f
        .insts
        .iter()
        .any(|i| matches!(i.op, OpCode::TRY_BEGIN | OpCode::TRY_FINALLY_BEGIN));
    // LiveInfo 维度守卫：与 alloc 相同纪律，过期则重算。维度匹配时借用而非复制
    // （LiveInfo 稠密矩阵全量 clone 只读数据无收益）。
    let live = if live.inst_live_before.len() == f.insts.len() && !f.insts.is_empty() {
        Cow::Borrowed(live)
    } else {
        let cfg = oxide_cfg::build_cfg(f);
        Cow::Owned(oxide_liveness::liveness(f, &cfg))
    };
    // 生成器 / 异步函数体 + 含 try 的函数：调用点保持全量窗口（挂起恢复按全量
    // 寄存器快照搬移；异常 handler 存活集经截断窗口会丢值）。
    if !has_try && !f.is_generator && !f.is_async {
        for (i, inst) in f.insts.iter_mut().enumerate() {
            if matches!(inst.op, OpCode::CALL | OpCode::NEW_EXPRESSION | OpCode::SUPER_CALL) {
                let after = &live.inst_live_after[i];
                // 存活上界 = 最大存活槽号 + 1；254/255 由帧单独保存，不进窗口。
                // 按 u64 位集逐字取置位（活集小，仅遍历实际存活的槽）。
                let mut upper = 0u32;
                for (word_idx, word) in after.iter().enumerate() {
                    let mut bits = *word;
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        let reg = word_idx * 64 + bit;
                        if reg < 254 {
                            upper = (reg + 1) as u32;
                        }
                        bits &= bits - 1;
                    }
                }
                if upper > 0 {
                    inst.ext[0] = (inst.ext[0] & 0xFF) | (upper << 8);
                }
            }
        }
    }
    for child in &mut f.nested {
        let cfg = oxide_cfg::build_cfg(child);
        let l = oxide_liveness::liveness(child, &cfg);
        encode_call_window(child, &l);
    }
}
