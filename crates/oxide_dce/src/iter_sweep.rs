//! Pass B：全函数 use 计数工作队列删除到不动点（连锁删上游写者）。
//!
//! 只对可达存活指令聚合 use 计数（逐条 `inst.use_regs()` 累加），单遍预计算逐寄存器
//! 定义者列表 → 初始候选（`def_reg` 为 Some(r) 且 use_count[r]==0 且 `is_pure(f)` 的
//! 指令）入队 → 出队全量重核判据，命中即删并减去其 use_regs，归零寄存器与已删指令
//! def 寄存器的全部存活定义者入队。删除规则单调（use_count 只减不增、keep 只从真翻假），
//! 不动点删除集合唯一，与逐轮全扫迭代逐位等价。
//! CALL 隐式 def reg0 不参与死删（CALL 本身 is_pure=false 永不删），但 CALL
//! 的 use 计数正常聚合——喂活上游写其参数的纯指令。
//! **label 目标指令强制保留**：块头是控制流汇合点，删除会使存活 JMP 的 label 引用
//! 悬空（lowering "Label not found"）——保守保留，少删不错删。

use std::collections::VecDeque;

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 全函数 use 计数工作队列删除：就地改写 `keep`（保活/删除标记）。
///
/// 工作队列单遍收敛：预计算逐寄存器定义者列表，初始候选（def 寄存器 use 计数为零的
/// 纯指令）入队；出队全量重核判据（重核是权威门，入队判据只作初筛），命中即删并减
/// use 计数，归零寄存器与已删指令 def 寄存器的全部存活定义者入队。删除规则单调
/// （use_count 只减不增、keep 只从真翻假），不动点删除集合唯一，删除集合与逐轮
/// 全扫迭代逐位等价。
pub(super) fn pass_b_dead_code(f: &IRFunction, keep: &mut [bool]) {
    // 寄存器号上界：覆盖可达指令全部 def/use（手工 IR 可能超 n_registers，动态取上界）
    let mut max_reg: u32 = 0;
    for (i, k) in keep.iter().enumerate() {
        if *k {
            if let Some(d) = f.insts[i].def_reg() {
                max_reg = max_reg.max(d);
            }
            for u in f.insts[i].use_regs() {
                max_reg = max_reg.max(u);
            }
        }
    }

    // use 计数：存活指令逐条累加 use_regs
    let mut use_count = vec![0usize; max_reg as usize + 1];
    for (i, k) in keep.iter().enumerate() {
        if *k {
            for u in f.insts[i].use_regs() {
                use_count[u as usize] += 1;
            }
        }
    }

    // 逐寄存器定义者列表：按指令下标升序单遍填充，只收当前存活指令
    //（删除后由 keep 位图过滤，列表本身不收缩）
    let mut definers: Vec<Vec<usize>> = vec![Vec::new(); max_reg as usize + 1];
    for (i, k) in keep.iter().enumerate() {
        if *k {
            if let Some(r) = f.insts[i].def_reg() {
                definers[r as usize].push(i);
            }
        }
    }

    // label 目标指令位图：label_pos 指向的 Inst 下标（越界防御性忽略）
    let mut label_target = vec![false; f.insts.len()];
    for pos in f.label_pos.iter().flatten() {
        if let Some(t) = label_target.get_mut(*pos) {
            *t = true;
        }
    }

    // nested 树引用的变量槽位图：闭包可跨函数读外层变量槽（LOAD_VAR.a 读槽 / STORE_VAR.rd 写槽
    // 以寄存器号编码）。不递归删除 nested，但跨函数引用必须保活当前函数内写该槽的
    // STORE_VAR——否则死 `var k='x'` + 构造器内 `[k]` computed key 读槽被误删（回归锚）。
    let mut escaped_slot = vec![false; max_reg as usize + 1];
    collect_escaped_slots(&f.nested, &mut escaped_slot);

    // 槽逃逸判定：STORE_VAR 写变量槽可被 nested 闭包跨函数读取（computed class field key 等
    // 场景：构造函数 submodule 内 LOAD_VAR a 槽引外层变量槽）。DCE 不递归 nested，
    // 顶层 use 计数看不到跨函数读——凡 nested 树引用的变量槽，其顶层写者保活
    //（保守"少删不错删"）。
    let slot_escapes = |i: usize, r: u32| {
        f.insts[i].op == OpCode::STORE_VAR
            && matches!(f.insts[i].rd, Operand::Reg(_))
            && escaped_slot.get(r as usize).copied().unwrap_or(false)
    };

    // 初始工作队列：收全部现判据候选（def 寄存器 use 计数为零、非 label 目标、
    // 槽未逃逸、纯指令）
    let mut in_queue = vec![false; f.insts.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, k) in keep.iter().enumerate() {
        if !*k || label_target[i] {
            continue;
        }
        let inst = &f.insts[i];
        if let Some(r) = inst.def_reg() {
            if use_count[r as usize] == 0 && !slot_escapes(i, r) && inst.is_pure(f) {
                in_queue[i] = true;
                queue.push_back(i);
            }
        }
    }

    // 出队全量重核判据（重核是权威门，入队判据只作初筛）；命中即置 keep 为假并减
    // use 计数，归零寄存器与已删指令 def 寄存器的全部存活且未入队定义者入队
    //（已删指令删除时其 def 寄存器计数必为零）
    while let Some(i) = queue.pop_front() {
        if !keep[i] || label_target[i] {
            continue;
        }
        let inst = &f.insts[i];
        let Some(r) = inst.def_reg() else {
            continue;
        };
        if use_count[r as usize] != 0 || slot_escapes(i, r) || !inst.is_pure(f) {
            continue;
        }
        keep[i] = false;
        for u in inst.use_regs() {
            use_count[u as usize] -= 1;
            if use_count[u as usize] == 0 {
                for &d in &definers[u as usize] {
                    if keep[d] && !in_queue[d] {
                        in_queue[d] = true;
                        queue.push_back(d);
                    }
                }
            }
        }
        // 已删指令 def 寄存器的计数此时必为零，同式入队
        for &d in &definers[r as usize] {
            if keep[d] && !in_queue[d] {
                in_queue[d] = true;
                queue.push_back(d);
            }
        }
    }
}

/// 递归收集 nested 树中全部变量槽引用（LOAD_VAR.a 读槽、STORE_VAR.rd 写槽）。
///
/// 槽号 = 寄存器号编码的变量槽；其余操作数（局部寄存器/Imm/This）不构成跨函数引用。
/// precise_sweep 复用此函数，保证跨函数保活判定同一来源。
pub(crate) fn collect_escaped_slots(nested: &[IRFunction], out: &mut Vec<bool>) {
    for sub in nested {
        for inst in &sub.insts {
            let slot = match inst.op {
                OpCode::LOAD_VAR => inst.a,
                OpCode::STORE_VAR => inst.rd,
                _ => Operand::None,
            };
            if let Operand::Reg(r) = slot {
                let idx = r as usize;
                if idx >= out.len() {
                    out.resize(idx + 1, false);
                }
                out[idx] = true;
            }
        }
        collect_escaped_slots(&sub.nested, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    /// 线性死链：LOAD_CONST→MOV→结果丢弃，全删（剩 RETURN）。
    #[test]
    fn linear_dead_chain_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0: r0 = const
        f.insts.push(Inst::inst_mov(Operand::Reg(1), Operand::Reg(0))); // 1: r1 = r0
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None)); // 2: return r2
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert_eq!(keep, vec![false, false, true], "死链全删，RETURN 保留");
    }

    /// 有副作用负例：CALL / STORE_VAR / GET_PROP / SET_PROP / INC_POST 永不删。
    #[test]
    fn impure_ops_never_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0
        f.insts.push(Inst::call(Operand::Reg(0), Operand::Reg(0), Operand::Reg(0), 0)); // 1: CALL
        f.insts.push(Inst::load_const(Operand::Reg(1), 1)); // 2
        f.insts.push(Inst::load_const(Operand::Reg(2), 2)); // 3
        f.insts
            .push(Inst::new(OpCode::STORE_VAR, Operand::Reg(1), Operand::Reg(2), Operand::Imm(1))); // 4
        f.insts
            .push(Inst::new(OpCode::GET_PROP, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2))); // 5
        f.insts
            .push(Inst::new(OpCode::SET_PROP, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2))); // 6
        f.insts
            .push(Inst::new(OpCode::INC_POST, Operand::Reg(4), Operand::Reg(5), Operand::None)); // 7
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert!(keep.iter().all(|k| *k), "有副作用指令一律保留");
    }

    /// STORE_VAR 一律保留：写变量槽可能被外部观察（脚本顶层/模块作用域变量、
    /// nested 闭包跨函数读槽），即使本函数内无 use 也不删。
    #[test]
    fn store_var_never_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 1)); // 0
        f.insts
            .push(Inst::new(OpCode::STORE_VAR, Operand::Reg(0), Operand::Reg(1), Operand::None)); // 1
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert!(keep[1], "STORE_VAR 不删，其源 LOAD_CONST 也被 use 保活");
        assert!(keep[0] && keep[2], "源与 RETURN 保留");
    }

    /// 死闭包：CREATE_CLOSURE 结果未用 → 删除；nested 不递归不回收。
    #[test]
    fn dead_create_closure_deleted_nested_untouched() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::create_closure(Operand::Reg(1), 0)); // 0: r1 = nested[0]
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 1
        f.nested.push(IRFunction::new());
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert_eq!(keep, vec![false, true], "死 CREATE_CLOSURE 删，RETURN 保留");
    }

    /// 连锁迭代：写链 LOAD_CONST→MOV→LOAD_CONST 全死，逐轮删除至不动点。
    #[test]
    fn chained_dead_writes_deleted_iteratively() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0: r0（MOV 读，轮 2 删）
        f.insts.push(Inst::inst_mov(Operand::Reg(1), Operand::Reg(0))); // 1: r1（轮 1 删）
        f.insts.push(Inst::load_const(Operand::Reg(2), 2)); // 2: r2 无人读（轮 1 删）
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 3
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert_eq!(keep, vec![false, false, false, true], "连锁全删至不动点");
    }

    /// label 目标指令强制保留（回归：do-while 形态）。`LOAD_CONST r0` 是 label 0 的目标，
    /// 虽然 def 无 use 属死值，但删除会使存活 JMP_IF_TRUE 的 label 引用悬空
    /// （lowering 报 "Label not found"）——保守保留，少删不错删。
    #[test]
    fn label_target_inst_never_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 1)); // 0: body 头，label 0 目标（死值）
        f.insts.push(Inst::load_const(Operand::Reg(1), 1)); // 1: 条件 true
        f.insts.push(Inst::jmp_if_true(1, 0)); // 2: 回跳 label 0
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 3
        f.label_pos = vec![Some(0)];
        f.label_count = 1;
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert!(keep.iter().all(|k| *k), "label 目标指令与跳转链全部保留");
    }

    /// STORE_VAR 跨函数逃逸（回归：computed class field key）。顶层写变量槽 Reg(1) 的
    /// STORE_VAR 在本函数无 use，但 nested 构造器内 LOAD_VAR a=Reg(1) 跨函数读该槽——
    /// DCE 只统计当前函数（不递归 nested），nested 引用槽保活顶层写者。
    #[test]
    fn store_var_slot_read_by_nested_kept() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(2), 0)); // 0: 'x'
        f.insts
            .push(Inst::new(OpCode::STORE_VAR, Operand::Reg(1), Operand::Reg(2), Operand::Imm(0))); // 1: k = 'x'（槽 1）
        f.insts.push(Inst::create_closure(Operand::Reg(4), 0)); // 2
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 3
                                                                                             // nested 构造器：LOAD_VAR a=Reg(1) 跨函数读外层变量槽 1（computed key）
        let mut sub = IRFunction::new();
        sub.insts
            .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(5), Operand::Reg(1), Operand::None));
        sub.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(7), Operand::None, Operand::None));
        f.nested.push(sub);
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert!(keep[0] && keep[1], "nested 引用槽的写者 STORE_VAR 与源 LOAD_CONST 保留");
        assert!(!keep[2], "死 CREATE_CLOSURE 仍删");
    }

    /// 长死链：五十条 MOV 链加源常量全死，断言工作队列单遍级联收敛全删至不动点
    ///（链首常量靠"归零寄存器的定义者入队"逐环传导，钉级联不漏删）。
    #[test]
    fn long_dead_chain_fully_deleted() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(0), 0)); // 0: r0 = const（源，链尾传导后删）
        for r in 1..=50u32 {
            f.insts.push(Inst::inst_mov(Operand::Reg(r), Operand::Reg(r - 1))); // 1..=50: r = r-1
        }
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(60), Operand::None, Operand::None)); // 51
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert!(keep[0..51].iter().all(|k| !*k), "五十条 MOV 链与源常量全删至不动点");
        assert!(keep[51], "RETURN 保留");
    }

    /// 同寄存器多定义者：两个定义者同寄存器均死时两删；一个定义者为 label 目标时
    /// 保留、另一删（钉工作队列删除集合与迭代全扫逐位等价）。
    #[test]
    fn multi_definers_same_register_deleted() {
        // 两定义者均死：同寄存器 use 计数为零，全部定义者入队两删
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 7)); // 0: r1 = 7
        f.insts.push(Inst::load_const(Operand::Reg(1), 8)); // 1: r1 = 8（覆盖）
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert_eq!(keep, vec![false, false, true], "同寄存器两定义者均死，两删");

        // 一个定义者为 label 目标：静态判据时间不变，保活；另一定义者仍删
        let mut f = IRFunction::new();
        f.insts.push(Inst::load_const(Operand::Reg(1), 7)); // 0: label 目标（保活）
        f.insts.push(Inst::load_const(Operand::Reg(1), 8)); // 1: r1 = 8（死）
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 2
        f.label_pos = vec![Some(0)];
        f.label_count = 1;
        let mut keep = vec![true; f.insts.len()];

        pass_b_dead_code(&f, &mut keep);
        assert_eq!(keep, vec![true, false, true], "label 目标定义者保留，另一定义者删");
    }
}
