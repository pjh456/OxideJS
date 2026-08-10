//! RegAlloc 改写 pass：干涉图染色决策（前半）+ 指令改写/元数据回写（后半）。
//!
//! 消费 `oxide_liveness::LiveInfo`（inst_live_before 逐指令活集）与
//! `oxide_ir::IRFunction`（param_layout 参数段 + nested escaped 收集）。染色：干涉图
//! （graph.rs）+ Kemp 简化/贪心/spill 区间拆分（color.rs）→ AllocMap；改写：rewrite.rs
//! （指令槽 vreg→phys 重写 + spill 插入 + 参数连续性 MOV + label 重建）+ finish.rs
//! （元数据回写）。`alloc()` 是完整改写 pass 入口，`color()` 只产决策。

mod alloc_map;
mod color;
mod finish;
mod graph;
mod regalloc_log;
mod rewrite;

pub use alloc_map::{Alloc, AllocMap, FreshKind, FreshVreg, SpillPlan};

use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 染色：`&IRFunction + &LiveInfo → Result<AllocMap, String>`。纯函数，只读 IR。
/// 空 IRFunction 退化：Ok(AllocMap::new())。Err = 无可行染色（RangeError 路径，
/// 消息与 lower 逐字一致，编译接线时原样传播）。
pub fn color(f: &IRFunction, live: &LiveInfo) -> Result<AllocMap, String> {
    if f.insts.is_empty() {
        return Ok(AllocMap::new());
    }
    color::run(f, live)
}

/// 寄存器分配改写：`&mut IRFunction + &LiveInfo → Result<(), String>`。
///
/// # 步骤
/// - LiveInfo 维度守卫：inst 数不符说明 LiveInfo 已过期（精确 DCE 删指令后下标位移），
///   内部重跑 build_cfg + liveness 重建，不重复建分析引擎
/// - `color()` 产染色决策（Err 原样传播）
/// - `rewrite::run` 指令改写 → `finish::run` 元数据回写
/// - nested 递归自顶向下：每子函数独立 vreg 空间独立 alloc
///
/// # 注意事项
/// - 嵌套子函数不继承父 LiveInfo，各自重跑 build_cfg + liveness。
pub fn alloc(f: &mut IRFunction, live: &LiveInfo) -> Result<(), String> {
    if f.insts.is_empty() && f.nested.is_empty() {
        return Ok(());
    }
    // LiveInfo 维度守卫：删改后的过期 LiveInfo 索引错位会误删活指令
    let live = if live.inst_live_before.len() == f.insts.len() && !f.insts.is_empty() {
        live.clone()
    } else {
        let cfg = oxide_cfg::build_cfg(f);
        oxide_liveness::liveness(f, &cfg)
    };
    let map = color(f, &live)?;
    regalloc_debug!("alloc: {} vregs -> {} phys", map.map.len(), map.phys_peak);
    rewrite::run(f, &map);
    finish::run(f, &map);
    for child in &mut f.nested {
        let cfg = oxide_cfg::build_cfg(child);
        let l = oxide_liveness::liveness(child, &cfg);
        alloc(child, &l)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    #[test]
    fn alloc_empty_ok() {
        let mut f = IRFunction::new();
        assert!(alloc(&mut f, &LiveInfo::new()).is_ok());
    }

    #[test]
    fn alloc_stale_live_reruns() {
        let mut f = IRFunction::new();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        // 空 LiveInfo（维度不符）→ 内部重跑
        let stale = LiveInfo::new();
        assert!(alloc(&mut f, &stale).is_ok());
        // 重写后全部槽 ≤253
        for inst in &f.insts {
            for o in [&inst.rd, &inst.a, &inst.b] {
                if let Operand::Reg(r) = o {
                    assert!(*r <= 253, "重写后槽 {r} 超 253");
                }
            }
        }
    }

    #[test]
    fn alloc_err_propagates() {
        // 255 参数 CALL：参数窗口吞并全部可分配色 → k=0 → 无可行染色 → Err
        let mut f = IRFunction::new();
        f.insts
            .push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3000), 255));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        let cfg = oxide_cfg::build_cfg(&f);
        let live = oxide_liveness::liveness(&f, &cfg);
        let err = alloc(&mut f, &live).unwrap_err();
        assert!(err.contains("too many registers"), "Err: {err}");
    }

    #[test]
    fn alloc_recurses_nested() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
        let mut sub = IRFunction::new();
        sub.insts.push(Inst::load_const(Operand::Reg(3), 0));
        sub.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        f.nested.push(sub);
        let cfg = oxide_cfg::build_cfg(&f);
        let live = oxide_liveness::liveness(&f, &cfg);
        alloc(&mut f, &live).unwrap();
        // 父与子函数全部槽 ≤253
        for inst in &f.insts {
            for o in [&inst.rd, &inst.a, &inst.b] {
                if let Operand::Reg(r) = o {
                    assert!(*r <= 253);
                }
            }
        }
        assert!(!f.nested[0].insts.is_empty(), "子函数也被重写");
        assert!(f.nested[0].n_registers <= 253, "子函数 n_registers 已回写");
    }
}
