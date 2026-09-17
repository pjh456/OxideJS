//! 元数据回写：n_registers / builtin_reg_map / param_layout / upvalue_captures。
//!
//! - n_registers = map.phys_peak（≤254；最高合法物理槽 253 对应窗口大小 254；
//!   不含参数窗口——窗口是调用点暂存槽，
//!   CALL handler 在 push 前收集 args 进 Vec，n_registers 只控 save_stack 窗口）
//! - builtin_reg_map：Phys → 新号；Spill → spilled_builtin_bindings 的自由色 R（与 rewrite 取同一 AllocMap 分配结果）
//! - param_layout 按参数首槽物理色回写；upvalue_captures 不动（escaped 预着色恒等）

use crate::alloc_map::{Alloc, AllocMap};
use crate::rewrite::spilled_builtin_bindings;
use oxide_ir::IRFunction;

/// 回写 IRFunction 元数据（不动 insts——rewrite 已完成改写）。
pub(super) fn run(f: &mut IRFunction, map: &AllocMap) {
    // 先写 n_registers = phys_peak（最高物理槽数，恒 ≤ 254）。
    debug_assert!(map.phys_peak <= 254, "phys_peak 超 254");
    f.n_registers = map.phys_peak;

    // 再回写 builtin_reg_map：Phys 直接换新号；Spill 取入口
    // spilled_builtin_bindings 的自由色 R（与 rewrite 取同一 AllocMap 分配结果）。
    let spilled = spilled_builtin_bindings(f, map);
    let mut rewritten_builtins = Vec::with_capacity(f.builtin_reg_map.len());
    for (name, vreg) in &f.builtin_reg_map {
        let v = *vreg;
        match map.map.get(&v) {
            Some(Alloc::Phys(p)) => {
                rewritten_builtins.push((name.clone(), *p));
            }
            Some(Alloc::Spill(_)) => {
                if let Some((_, r, _)) = spilled.iter().find(|(n, _, _)| n == name) {
                    debug_assert!(*r <= 253);
                    rewritten_builtins.push((name.clone(), *r));
                } else {
                    debug_assert!(false, "spilled builtin {name} 无对应入口 SPILL");
                }
            }
            None => {}
        }
    }
    f.builtin_reg_map = rewritten_builtins;

    // 参数段保持连续，但继承父上下文产生的高虚拟段会整体移动到低位物理段。
    let pl = f.param_layout;
    if pl.count > 0 {
        if let Some(Alloc::Phys(base)) = map.map.get(&pl.base) {
            for i in 0..pl.count {
                debug_assert_eq!(map.map.get(&(pl.base + i)), Some(&Alloc::Phys(base + i)));
            }
            f.param_layout.base = *base;
        }
        debug_assert!(f.param_layout.base + f.param_layout.count <= map.arg_window_base || map.arg_window_base == 254);
    } else {
        f.param_layout.base = 0;
    }

    // upvalue_captures 不动。enclosing_reg 是编译期产物（MAKE_CELL 定位用），VM 运行时
    // 不读（CREATE_CLOSURE 走 cell_idx，cell 捕获后值在 cell 中）——无需跨函数同步。
    // 直接槽引用（class field 计算键等）的 escaped vreg 已由预着色恒等保留。
    // 注：cell 捕获变量的父 vreg 可被 RegAlloc 移动（MAKE_CELL 后值入 cell，与寄存器无关），
    // 此处不做恒等断言。
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    #[test]
    fn n_registers_equals_phys_peak() {
        let mut f = IRFunction::new();
        let mut map = AllocMap::new();
        map.phys_peak = 42;
        run(&mut f, &map);
        assert_eq!(f.n_registers, 42);
    }

    #[test]
    fn builtin_moved_rewritten() {
        let mut f = IRFunction::new();
        f.builtin_reg_map = vec![("Math".to_string(), 5)];
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
        let mut map = AllocMap::new();
        map.map.insert(5, Alloc::Phys(9));
        map.map.insert(1, Alloc::Phys(1));
        run(&mut f, &map);
        assert_eq!(f.builtin_reg_map[0].1, 9, "builtin 移动后回写新 phys");
    }

    #[test]
    fn builtin_spilled_bound_to_free_color() {
        let mut f = IRFunction::new();
        f.builtin_reg_map = vec![("Math".to_string(), 5)];
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
        let mut map = AllocMap::new();
        map.map.insert(5, Alloc::Spill(0));
        map.map.insert(1, Alloc::Phys(1));
        map.map.insert(20, Alloc::Phys(7));
        map.spills.push(crate::alloc_map::SpillPlan {
            vreg: 5,
            slot: 0,
            defs: vec![],
            uses: vec![(0, 20)],
        });
        let binds = spilled_builtin_bindings(&f, &map);
        run(&mut f, &map);
        assert_eq!(f.builtin_reg_map[0].0, "Math");
        assert_eq!(f.builtin_reg_map[0].1, binds[0].1, "回写 R 与入口 SPILL 取同一 AllocMap 分配结果");
    }

    #[test]
    fn unused_builtin_is_removed_from_frame_metadata() {
        let mut f = IRFunction::new();
        f.builtin_reg_map = vec![("late".to_string(), 426)];
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
        let mut map = AllocMap::new();
        map.map.insert(1, Alloc::Phys(1));
        run(&mut f, &map);
        assert!(f.builtin_reg_map.is_empty());
    }

    #[test]
    fn param_layout_untouched() {
        let mut f = IRFunction::new();
        f.param_layout = oxide_ir::ParamLayout { base: 1, count: 2 };
        let map = AllocMap::new();
        run(&mut f, &map);
        assert_eq!(f.param_layout.base, 1);
        assert_eq!(f.param_layout.count, 2);
    }

    #[test]
    fn zero_parameter_layout_discards_compile_time_boundary() {
        let mut f = IRFunction::new();
        f.param_layout = oxide_ir::ParamLayout { base: 426, count: 0 };
        let map = AllocMap::new();
        run(&mut f, &map);
        assert_eq!(f.param_layout, oxide_ir::ParamLayout { base: 0, count: 0 });
    }

    #[test]
    fn enclosing_reg_unchanged() {
        let mut f = IRFunction::new();
        f.upvalue_captures.push(oxide_bytecode::module::UpvalueCapture {
            name: "x".to_string(),
            enclosing_reg: 7,
            cell_idx: 0,
            parent_uv_idx: None,
        });
        let map = AllocMap::new();
        run(&mut f, &map);
        assert_eq!(f.upvalue_captures[0].enclosing_reg, 7);
    }
}
