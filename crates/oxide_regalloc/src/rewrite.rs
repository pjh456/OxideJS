//! 指令改写：消费 AllocMap 就地重写 IR。
//!
//! - 槽位 vreg→phys 重写：仅 `Operand::Reg`；None/This/NewTarget/Const/Imm/Label 零改动
//!   （None→0、This→254、NewTarget→255 由 lower 映射）
//! - spill 落点：def 点后 SPILL、use 点前 UNSPILL；RMW（COMPOUND 系 rd 读旧值）UNSPILL
//!   进 def-fresh 寄存器、use-fresh 弃用
//! - 调用点参数连续性：参数分散则 MOV 搬进 ARG_WINDOW
//! - spilled builtin 入口 SPILL（builtin 无指令 def，VM 入口隐式绑定）
//! - label_pos mark-sweep 重建：label 落 group 起点（跳转先执行 before-插入）
//!
//! 确定性：全 BTreeMap/Vec 排序，禁 HashMap。

use std::collections::BTreeMap;

use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

use crate::alloc_map::{Alloc, AllocMap};

/// 就地改写 f.insts + f.label_pos。
pub(super) fn run(f: &mut IRFunction, map: &AllocMap) {
    // ── 1. spill 查询表 ──
    // defs_at: (inst, vreg) → def-fresh 色；uses_at: (inst, vreg) → use-fresh 色；spill_slot: vreg → slot
    let mut defs_at: BTreeMap<(usize, u32), u32> = BTreeMap::new();
    let mut uses_at: BTreeMap<(usize, u32), u32> = BTreeMap::new();
    let mut spill_slot: BTreeMap<u32, u16> = BTreeMap::new();
    for sp in &map.spills {
        spill_slot.insert(sp.vreg, sp.slot);
        for &(inst, fresh_id) in &sp.defs {
            if let Some(Alloc::Phys(c)) = map.map.get(&fresh_id) {
                defs_at.insert((inst, sp.vreg), *c);
            }
        }
        for &(inst, fresh_id) in &sp.uses {
            if let Some(Alloc::Phys(c)) = map.map.get(&fresh_id) {
                uses_at.insert((inst, sp.vreg), *c);
            }
        }
    }

    // ── 2. spilled builtin 入口 SPILL ──
    let spilled_builtins = spilled_builtin_bindings(f, map);

    // ── 3. 单遍构建新 insts + old_to_new ──
    let mut new_insts: Vec<Inst> = Vec::new();
    let mut old_to_new: Vec<usize> = vec![0; f.insts.len()];
    // 入口 SPILL（先于所有 before-插入与 inst 0）
    for (_, r, slot) in &spilled_builtins {
        new_insts.push(Inst::inst_spill(Operand::Reg(*r), *slot));
    }
    for (i, inst) in f.insts.iter().enumerate() {
        old_to_new[i] = new_insts.len(); // group 起点（before-插入首条位置）

        // RMW 判定：inst 同时 def 且 use 同一 spilled vreg
        let rmw_v = inst
            .def_reg()
            .filter(|d| uses_at.contains_key(&(i, *d)) && spill_slot.contains_key(d));

        // before-插入：非 RMW 的 use 点 UNSPILL
        let mut use_unspills: Vec<(u32, u32)> = Vec::new(); // (vreg, fresh 色)
        for (&(inst_at, vreg), &color) in &uses_at {
            if inst_at == i && rmw_v != Some(vreg) {
                use_unspills.push((vreg, color));
            }
        }
        use_unspills.sort_unstable();

        // 重写后该 inst 各槽的 spill 色映射（vreg → fresh 色）
        let mut slot_color: BTreeMap<u32, u32> = BTreeMap::new();
        if let Some(v) = rmw_v {
            // RMW：UNSPILL 进 def-fresh 寄存器（rd 读旧值写新值），use-fresh 弃用
            if let Some(&c) = defs_at.get(&(i, v)) {
                slot_color.insert(v, c);
            }
        }
        for &(vreg, color) in &use_unspills {
            slot_color.insert(vreg, color);
        }
        // 纯 def 点：rd → def-fresh 色（值写入后立即 SPILL）
        if rmw_v.is_none() {
            if let Some(d) = inst.def_reg() {
                if let Some(&c) = defs_at.get(&(i, d)) {
                    slot_color.insert(d, c);
                }
            }
        }

        // 重写 inst 本体（含调用点参数连续性 MOV）
        let (rewritten, arg_movs) = rewrite_inst(f, inst, map, &slot_color, i);

        // 发射顺序：UNSPILLs（按 vreg 升序）→ RMW UNSPILL → 连续性 MOVs → 指令 → SPILLs
        for &(vreg, color) in &use_unspills {
            let slot = spill_slot[&vreg];
            new_insts.push(Inst::inst_unspill(Operand::Reg(color), slot));
        }
        if let Some(v) = rmw_v {
            if let Some(&c) = defs_at.get(&(i, v)) {
                let slot = spill_slot[&v];
                new_insts.push(Inst::inst_unspill(Operand::Reg(c), slot));
            }
        }
        for (dst, src) in arg_movs {
            new_insts.push(Inst::inst_mov(Operand::Reg(dst), Operand::Reg(src)));
        }
        new_insts.push(rewritten);

        // after-插入：def 点 SPILL（RMW 与纯 def 都写 rd；terminator 跳过——def 结果必死）
        if let Some(d) = inst.def_reg() {
            if spill_slot.contains_key(&d) {
                if let Some(&c) = defs_at.get(&(i, d)) {
                    if !is_terminator(inst.op) {
                        let slot = spill_slot[&d];
                        new_insts.push(Inst::inst_spill(Operand::Reg(c), slot));
                    }
                }
            }
        }
    }

    f.insts = new_insts;

    // ── 4. label_pos 重映射（label 落 group 起点）──
    for pos in f.label_pos.iter_mut() {
        if let Some(old) = *pos {
            *pos = old_to_new.get(old).copied();
        }
    }
    debug_assert!(f.label_pos.iter().all(|p| p.map_or(true, |np| np <= f.insts.len())));
}

/// 重写单条指令：槽位映射 + 调用点参数连续性 MOV 补位。返回 (重写后指令, 参数 MOV 列表)。
fn rewrite_inst(
    f: &IRFunction, inst: &Inst, map: &AllocMap, slot_color: &BTreeMap<u32, u32>, i: usize,
) -> (Inst, Vec<(u32, u32)>) {
    let raw_nargs = inst.ext.first().copied().unwrap_or(0);
    let nargs = raw_nargs;
    let first_arg = match inst.op {
        OpCode::CALL | OpCode::CALL_NATIVE | OpCode::NEW_EXPRESSION => Some(inst.b),
        OpCode::SUPER_CALL => Some(inst.a),
        _ => None,
    };

    // 调用点参数 phys 映射（arg k → 物理号）
    let mut arg_movs: Vec<(u32, u32)> = Vec::new();
    let mut arg_base: Option<u32> = None;
    if let Some(Operand::Reg(fr)) = first_arg {
        if nargs > 0 {
            let mut args_phys: Vec<u32> = Vec::with_capacity(nargs as usize);
            for k in 0..nargs {
                let v = fr + k;
                let p = if let Some(&c) = slot_color.get(&v) {
                    c
                } else {
                    match map.map.get(&v) {
                        Some(Alloc::Phys(p)) => *p,
                        _ => v, // 非真实 vreg / 防御：原样
                    }
                };
                args_phys.push(p);
            }
            // 连续性检查：phys[k+1] == phys[k] + 1
            let contiguous = args_phys.windows(2).all(|w| w[1] == w[0] + 1);
            if !contiguous {
                for (k, &p) in args_phys.iter().enumerate() {
                    arg_movs.push((map.arg_window_base + k as u32, p));
                }
                arg_base = Some(map.arg_window_base);
            }
        }
    }
    let _ = i;

    // 槽位重写
    let rewrite = |o: Operand| -> Operand {
        match o {
            Operand::Reg(r) => {
                if let Some(&c) = slot_color.get(&r) {
                    Operand::Reg(c)
                } else {
                    match map.map.get(&r) {
                        Some(Alloc::Phys(p)) => Operand::Reg(*p),
                        Some(Alloc::Spill(_)) => {
                            // spilled vreg 出现在非 def/use 点（不应发生，防御保留原样）
                            debug_assert!(false, "spilled vreg {r} 出现在无插入点");
                            Operand::Reg(r)
                        }
                        None => Operand::Reg(r),
                    }
                }
            }
            other => other, // None/This/NewTarget/Const/Imm/Label 零改动，lower 映射语义号
        }
    };

    let rd = rewrite(inst.rd);
    let mut a = rewrite(inst.a);
    let mut b = rewrite(inst.b);
    // TEMPLATE_STR 的 ext 编码表达式寄存器（seg>>31==1 时低 8 位为 expr_reg）——必须随
    // RegAlloc 重映射，否则读旧 vreg 号对应的物理槽（错值）。spread 调用系 ext[1..] 每个
    // 字是完整 spread 源 vreg，同样需重映射到物理号。
    let ext = match inst.op {
        OpCode::TEMPLATE_STR => {
            let mut ext = inst.ext.clone();
            for seg in ext.iter_mut().skip(1) {
                if *seg >> 31 == 1 {
                    let r = *seg & 0xFF;
                    let nr = remap_ext_reg(r, slot_color, map);
                    *seg = (*seg & !0xFFu32) | (nr & 0xFF);
                }
            }
            ext
        }
        OpCode::CALL_SPREAD | OpCode::NEW_EXPRESSION_SPREAD | OpCode::SUPER_CALL_SPREAD => {
            // 有序实参字：静态字直接重映射，spread 字保留高位标记、低 31 位重映射。
            let mut ext = inst.ext.clone();
            for seg in ext.iter_mut().skip(1) {
                if *seg >> 31 == 1 {
                    let nr = remap_ext_reg(*seg & 0x7FFF_FFFF, slot_color, map);
                    *seg = 0x8000_0000 | nr;
                } else {
                    *seg = remap_ext_reg(*seg, slot_color, map);
                }
            }
            ext
        }
        // GET_PRIVATE/SET_PRIVATE/PRIVATE_BRAND_IN：ext[0] 是 brand 对象寄存器
        // （0 表示跳过检查），须重映射。
        OpCode::GET_PRIVATE | OpCode::SET_PRIVATE | OpCode::PRIVATE_BRAND_IN => {
            let mut ext = inst.ext.clone();
            if let Some(brand_reg) = ext.first_mut() {
                if *brand_reg != 0 {
                    *brand_reg = remap_ext_reg(*brand_reg, slot_color, map);
                }
            }
            ext
        }
        _ => inst.ext.clone(),
    };
    // 调用点首参槽改指 arg_window_base（桥接后）
    if let Some(ab) = arg_base {
        match inst.op {
            OpCode::CALL | OpCode::CALL_NATIVE | OpCode::NEW_EXPRESSION => {
                if matches!(first_arg, Some(Operand::Reg(_))) {
                    b = Operand::Reg(ab);
                }
            }
            OpCode::SUPER_CALL => {
                if matches!(first_arg, Some(Operand::Reg(_))) {
                    a = Operand::Reg(ab);
                }
            }
            _ => {}
        }
    }
    let _ = f;
    // 防御：imm16 编码位的操作数必须是 Imm/None，不得是 Reg——
    // 否则会被上面的 Reg 重写改号，VM 侧 imm16 读到错值（历史 B020 根因）。
    match inst.op {
        OpCode::LOAD_UPVALUE | OpCode::CREATE_CLOSURE => {
            debug_assert!(
                matches!(a, Operand::Imm(_) | Operand::None),
                "{}: a 槽是 imm16 编码位，不得为 Reg",
                inst.op
            );
        }
        OpCode::MAKE_CELL | OpCode::MAKE_CELL_FRESH | OpCode::CELL_GET | OpCode::CELL_SET | OpCode::STORE_UPVALUE => {
            debug_assert!(
                matches!(b, Operand::Imm(_) | Operand::None),
                "{}: b 槽是 imm16 编码位，不得为 Reg",
                inst.op
            );
        }
        _ => {}
    }
    (Inst { op: inst.op, rd, a, b, ext }, arg_movs)
}

/// ext 内嵌寄存器重映射：优先用本指令点的 fresh 色（spilled vreg 的 use 点 UNSPILL），
/// 其次 AllocMap 物理色；未映射（防御）保留原号。
fn remap_ext_reg(r: u32, slot_color: &BTreeMap<u32, u32>, map: &AllocMap) -> u32 {
    if let Some(&c) = slot_color.get(&r) {
        c
    } else {
        match map.map.get(&r) {
            Some(Alloc::Phys(p)) => *p,
            _ => r,
        }
    }
}

/// terminator 判定：跳转族 + RETURN/HALT/THROW（def 结果必死，SPILL 跳过）。
fn is_terminator(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::JMP
            | OpCode::BREAK
            | OpCode::CONTINUE
            | OpCode::JMP_IF_TRUE
            | OpCode::JMP_IF_FALSE
            | OpCode::JMP_IF_NULLISH
            | OpCode::RETURN
            | OpCode::HALT
            | OpCode::THROW
    )
}

/// spilled builtin：对 builtin_reg_map 中 map[v]=Spill 的项分配确定性自由色 R。
/// 返回 (name, R, slot)。rewrite（入口 SPILL）与 finish（回写）共享唯一来源。
pub(super) fn spilled_builtin_bindings(f: &IRFunction, map: &AllocMap) -> Vec<(String, u32, u16)> {
    // 候选色排除集
    let mut excluded: Vec<u32> = Vec::new();
    for (v, a) in &map.map {
        if let Alloc::Phys(c) = a {
            excluded.push(*c);
        }
        let _ = v;
    }
    let pl = f.param_layout;
    for c in pl.base..pl.base + pl.count {
        excluded.push(c);
    }
    // escaped 色（递归 nested 的 LOAD_VAR.a / STORE_VAR.rd）
    collect_escaped(&f.nested, &mut excluded);
    // own-escaped 色：本函数 LOAD_VAR.a / STORE_VAR.rd 引用父槽（< base）。
    // 与 graph.rs collect_own_escaped 对称——spill 自由色不得占用父槽号。
    collect_own_escaped(f, &mut excluded);
    // 窗口
    for c in map.arg_window_base..=253 {
        excluded.push(c);
    }
    excluded.sort_unstable();
    excluded.dedup();
    let excluded_set: std::collections::BTreeSet<u32> = excluded.iter().copied().collect();

    let mut result: Vec<(String, u32, u16)> = Vec::new();
    let mut used: Vec<u32> = Vec::new();
    // 按 name 升序确定性
    let mut candidates: Vec<&(String, u32)> = f
        .builtin_reg_map
        .iter()
        .filter(|(_, v)| matches!(map.map.get(v), Some(Alloc::Spill(_))))
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, vreg) in candidates {
        let slot = match map.map.get(vreg) {
            Some(Alloc::Spill(s)) => *s,
            _ => continue,
        };
        // 自由色 = 1..=253 中最小未排除未使用
        let r = (1u32..=253)
            .find(|c| !excluded_set.contains(c) && !used.contains(c))
            .unwrap_or_else(|| {
                debug_assert!(false, "spilled builtin 无自由色");
                1
            });
        used.push(r);
        result.push((name.clone(), r, slot));
    }
    result
}

/// 递归收集 nested 树中变量槽引用（LOAD_VAR.a 读槽、STORE_VAR.rd 写槽）→ escaped 色。
fn collect_escaped(nested: &[IRFunction], out: &mut Vec<u32>) {
    for sub in nested {
        for inst in &sub.insts {
            let slot = match inst.op {
                OpCode::LOAD_VAR => inst.a,
                OpCode::STORE_VAR => inst.rd,
                _ => Operand::None,
            };
            if let Operand::Reg(r) = slot {
                out.push(r);
            }
        }
        collect_escaped(&sub.nested, out);
    }
}

/// 收集本函数 LOAD_VAR.a / STORE_VAR.rd 中引用父槽（槽号 < param_layout.base）的 vreg。
/// 与 graph.rs::collect_own_escaped 对称：base = emit inherited_reg_start 分界线。
fn collect_own_escaped(f: &IRFunction, out: &mut Vec<u32>) {
    let base = f.param_layout.base;
    if base == 0 {
        return;
    }
    for inst in &f.insts {
        let slot = match inst.op {
            OpCode::LOAD_VAR => inst.a,
            OpCode::STORE_VAR => inst.rd,
            _ => Operand::None,
        };
        if let Operand::Reg(r) = slot {
            if r < base {
                out.push(r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::operand::Operand;

    fn empty_function() -> IRFunction {
        IRFunction::new()
    }

    fn rewrite_with_map(insts: Vec<Inst>, map: AllocMap) -> IRFunction {
        let mut f = empty_function();
        f.insts = insts;
        run(&mut f, &map);
        f
    }

    #[test]
    fn semantic_operands_untouched() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::This, Operand::NewTarget, Operand::None));
        f.insts.push(Inst::load_const(Operand::Reg(1), 3));
        let map = AllocMap::new();
        run(&mut f, &map);
        assert_eq!(f.insts[0].rd, Operand::This);
        assert_eq!(f.insts[0].a, Operand::NewTarget);
        assert_eq!(f.insts[0].b, Operand::None);
    }

    #[test]
    fn slots_rewritten_per_map() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        let mut map = AllocMap::new();
        map.map.insert(1, Alloc::Phys(10));
        map.map.insert(2, Alloc::Phys(11));
        map.map.insert(3, Alloc::Phys(12));
        run(&mut f, &map);
        assert_eq!(f.insts[0].rd, Operand::Reg(12));
        assert_eq!(f.insts[0].a, Operand::Reg(10));
        assert_eq!(f.insts[0].b, Operand::Reg(11));
        assert_eq!(f.insts[1].rd, Operand::Reg(12));
    }

    #[test]
    fn scattered_args_insert_mov_bridge() {
        let insts = vec![Inst::call(Operand::Reg(2), Operand::Reg(3), Operand::Reg(4), 3)];
        let mut map = AllocMap::new();
        map.arg_window_base = 251;
        map.map.insert(2, Alloc::Phys(2));
        map.map.insert(3, Alloc::Phys(3));
        map.map.insert(4, Alloc::Phys(7));
        map.map.insert(5, Alloc::Phys(9));
        map.map.insert(6, Alloc::Phys(8));
        let f = rewrite_with_map(insts, map);
        // 期望：3 条 MOV（251←7, 252←9, 253←8）+ 调用点 b 槽 → Reg(251)
        let movs: Vec<&Inst> = f.insts.iter().filter(|i| i.op == OpCode::MOV).collect();
        assert_eq!(movs.len(), 3, "分散参数应插 3 条 MOV");
        let call = f.insts.iter().find(|i| i.op == OpCode::CALL).unwrap();
        assert_eq!(call.b, Operand::Reg(251), "调用点 b 槽应改指窗口基址");
        // MOV 源分别为 7/9/8，目标 251/252/253
        let mv: Vec<(u32, u32)> = movs
            .iter()
            .map(|i| match (i.rd, i.a) {
                (Operand::Reg(d), Operand::Reg(s)) => (d, s),
                _ => panic!("MOV 槽位异常"),
            })
            .collect();
        assert!(mv.contains(&(251, 7)) && mv.contains(&(252, 9)) && mv.contains(&(253, 8)));
    }

    #[test]
    fn spill_points_inserted_at_defs_and_uses() {
        let insts = vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(2), 0),
            Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
            Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None),
        ];
        let mut map = AllocMap::new();
        map.map.insert(1, Alloc::Spill(0));
        map.map.insert(2, Alloc::Phys(2));
        map.map.insert(3, Alloc::Phys(3));
        map.map.insert(10, Alloc::Phys(5)); // def-fresh
        map.map.insert(11, Alloc::Phys(6)); // use-fresh
        map.spills.push(crate::alloc_map::SpillPlan {
            vreg: 1,
            slot: 0,
            defs: vec![(0, 10)],
            uses: vec![(2, 11)],
        });
        let f = rewrite_with_map(insts, map);
        // inst 0 后 SPILL(Reg(5), 0)；inst 2 前 UNSPILL(Reg(6), 0)；inst 2 的 a 槽(r1) → Reg(6)
        let ops: Vec<OpCode> = f.insts.iter().map(|i| i.op).collect();
        // 找 SPILL 与 UNSPILL 位置
        let spill_pos = ops.iter().position(|&o| o == OpCode::SPILL).unwrap();
        let unspill_pos = ops.iter().position(|&o| o == OpCode::UNSPILL).unwrap();
        assert_eq!(f.insts[spill_pos].rd, Operand::Reg(5));
        assert_eq!(f.insts[unspill_pos].rd, Operand::Reg(6));
        // 指令序：UNSPILL 在 ADD 前
        let add_pos = ops.iter().position(|&o| o == OpCode::ADD).unwrap();
        assert!(unspill_pos < add_pos, "UNSPILL 应在 ADD 前");
        let add = &f.insts[add_pos];
        assert_eq!(add.a, Operand::Reg(6), "ADD 的 a 槽(r1) → use-fresh 色 6");
    }

    #[test]
    fn rmw_spill_reloads_def_fresh() {
        // COMPOUND_ADD RMW（r1 读旧值写新值）→ def-fresh 5；RETURN 读新值 → use-fresh 8
        let insts = vec![
            Inst::new(OpCode::COMPOUND_ADD, Operand::Reg(1), Operand::Reg(2), Operand::None),
            Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None),
        ];
        let mut map = AllocMap::new();
        map.map.insert(1, Alloc::Spill(0));
        map.map.insert(2, Alloc::Phys(2));
        map.map.insert(5, Alloc::Phys(5)); // def-fresh
        map.map.insert(6, Alloc::Phys(6)); // use-fresh@0（弃用）
        map.map.insert(7, Alloc::Phys(8)); // use-fresh@1（RETURN）
        map.spills.push(crate::alloc_map::SpillPlan {
            vreg: 1,
            slot: 0,
            defs: vec![(0, 5)],
            uses: vec![(0, 6), (1, 7)],
        });
        let f = rewrite_with_map(insts, map);
        let ops: Vec<OpCode> = f.insts.iter().map(|i| i.op).collect();
        let unspill_pos = ops.iter().position(|&o| o == OpCode::UNSPILL).unwrap();
        let compound_pos = ops.iter().position(|&o| o == OpCode::COMPOUND_ADD).unwrap();
        let spill_pos = ops.iter().position(|&o| o == OpCode::SPILL).unwrap();
        let ret_pos = ops.iter().position(|&o| o == OpCode::RETURN).unwrap();
        // UNSPILL(def-fresh 5) → COMPOUND(rd=5) → SPILL(5)；RETURN 前 UNSPILL(use-fresh 8)
        assert_eq!(f.insts[unspill_pos].rd, Operand::Reg(5));
        assert!(unspill_pos < compound_pos && compound_pos < spill_pos);
        assert_eq!(f.insts[compound_pos].rd, Operand::Reg(5), "COMPOUND rd → def-fresh 色");
        assert_eq!(f.insts[spill_pos].rd, Operand::Reg(5));
        assert!(spill_pos < ret_pos, "SPILL 在 RETURN 前");
        // RETURN rd → use-fresh 8，其前有 UNSPILL(8)
        assert_eq!(f.insts[ret_pos].rd, Operand::Reg(8));
        let unspill8 = f
            .insts
            .iter()
            .position(|i| i.op == OpCode::UNSPILL && i.rd == Operand::Reg(8))
            .unwrap();
        assert!(unspill8 < ret_pos, "RETURN 前 UNSPILL use-fresh");
        // use-fresh 6（RMW 点弃用）不得产生 UNSPILL
        assert!(
            f.insts.iter().all(|i| !(i.op == OpCode::UNSPILL && i.rd == Operand::Reg(6))),
            "RMW use-fresh 弃用"
        );
    }

    #[test]
    fn label_pos_lands_on_group_start() {
        // 1: JMP_IF_FALSE(Reg(9), Label(0))  → 跳转目标 inst 3 是 spill use 点（有 before-UNSPILL）
        let insts = vec![
            Inst::load_const(Operand::Reg(2), 0),
            Inst::jmp_if_false(9, 0),
            Inst::load_const(Operand::Reg(3), 0),
            Inst::new(OpCode::ADD, Operand::Reg(4), Operand::Reg(1), Operand::Reg(2)),
            Inst::new(OpCode::RETURN, Operand::Reg(4), Operand::None, Operand::None),
        ];
        let mut f = empty_function();
        f.insts = insts;
        f.label_pos = vec![Some(3)];
        f.label_count = 1;
        let mut map = AllocMap::new();
        map.map.insert(1, Alloc::Spill(0));
        map.map.insert(2, Alloc::Phys(2));
        map.map.insert(3, Alloc::Phys(3));
        map.map.insert(4, Alloc::Phys(4));
        map.map.insert(9, Alloc::Phys(9));
        map.map.insert(11, Alloc::Phys(6)); // use-fresh for r1 at inst 3
        map.spills.push(crate::alloc_map::SpillPlan {
            vreg: 1,
            slot: 0,
            defs: vec![],
            uses: vec![(3, 11)],
        });
        run(&mut f, &map);
        // label 0 应指向 inst 3 的 group 起点（UNSPILL 位置）
        let label_target = f.label_pos[0].expect("label 重映射");
        let target_inst = &f.insts[label_target];
        assert_eq!(target_inst.op, OpCode::UNSPILL, "label 落 group 起点（UNSPILL）");
        // 跳转 offset 落在 UNSPILL 前 → lower 成功
        let result = oxide_ir::lower::lower(&f);
        assert!(result.is_ok(), "lower 应成功（label 解析到 group 起点）");
    }

    #[test]
    fn spilled_builtin_entry_spill_inserted() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        f.builtin_reg_map = vec![("Math".to_string(), 10)];
        let mut map = AllocMap::new();
        map.map.insert(10, Alloc::Spill(0));
        map.map.insert(5, Alloc::Phys(5));
        map.map.insert(20, Alloc::Phys(7)); // use-fresh
        map.spills.push(crate::alloc_map::SpillPlan {
            vreg: 10,
            slot: 0,
            defs: vec![],
            uses: vec![(0, 20)],
        });
        let binds = spilled_builtin_bindings(&f, &map);
        assert_eq!(binds.len(), 1);
        let (name, r, slot) = &binds[0];
        assert_eq!(name, "Math");
        assert_eq!(*slot, 0);
        assert!(*r >= 1 && *r <= 253, "R 应在物理域");
        assert_ne!(*r, 5, "R 不得与 Phys 色冲突");
        run(&mut f, &map);
        // 入口 SPILL 存在
        let first = f.insts.first().expect("入口 SPILL");
        assert_eq!(first.op, OpCode::SPILL);
        assert_eq!(first.rd, Operand::Reg(*r));
        assert_eq!(first.ext.as_slice(), &[0]);
    }
}
