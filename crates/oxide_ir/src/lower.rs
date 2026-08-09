//! IRFunction → CompiledModule 等价转换。
//!
//! 字节码格式（4 字节 u32 + 扩展字）逐指令一致。
//! label 用前缀和：`label_pos` 记 Inst 下标，平铺时维护 inst→instr 映射，
//! 跳转 offset 以 Instr（含 ext 字）为单位。

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode::{self, OpCode};

use crate::inst::Inst;
use crate::operand::Operand;
use crate::IRFunction;

/// IRFunction → CompiledModule 等价转换。溢出/标签错误消息为既有格式，逐字保持一致。
pub fn lower(f: &IRFunction) -> Result<CompiledModule, String> {
    // 溢出检查 1：常量池（先于寄存器——超大常量池伴生的海量 vreg 会使后续检查的
    // 稠密 bitset 表示爆内存；常量池超限是更基础的失效，先报它）
    if f.const_overflow || f.constants.len() > u16::MAX as usize {
        return Err("RangeError: too many constants".into());
    }

    // 溢出检查 2：寄存器（仅 Operand::Reg 变体；This/NewTarget 是语义操作数，映射 254/255 合法）
    for inst in &f.insts {
        for o in [&inst.rd, &inst.a, &inst.b] {
            if let Operand::Reg(r) = o {
                if *r > 253 {
                    return Err("RangeError: function body uses too many registers (max 253)".into());
                }
            }
        }
    }

    // 溢出检查 2b：n_registers（u32 → CompiledModule u8 前的显式截断检查）。
    // n_registers = max_regs = 最高分配号 + 1，合法上限 254（reg 0..253）。
    // 纯参数/空体函数可能无指令引用最高号寄存器，逐指令检查漏掉，计数检查兜底防 u8 静默截断。
    if f.n_registers > 254 {
        return Err("RangeError: function body uses too many registers (max 253)".into());
    }

    let mut instrs: Vec<u32> = Vec::new();
    let mut inst_to_instr: Vec<usize> = Vec::with_capacity(f.insts.len());
    // (当前指令的起始 Instr index, label id)
    let mut jumps: Vec<(usize, u32)> = Vec::new();

    for inst in &f.insts {
        inst_to_instr.push(instrs.len());
        encode_inst(inst, &mut instrs, &mut jumps)?;
    }

    // 跳转回填：offset = 目标 Instr - 当前 Instr（前缀和）
    for (instr_idx, label) in jumps {
        let target_inst = f
            .label_pos
            .get(label as usize)
            .and_then(|p| *p)
            .ok_or_else(|| format!("Label {label} not found in bytecode map"))?;
        if target_inst >= inst_to_instr.len() {
            return Err(format!("Label {label} not found in bytecode map"));
        }
        let target_instr = inst_to_instr[target_inst];
        let offset = target_instr as isize - instr_idx as isize;
        if offset < i16::MIN as isize || offset > i16::MAX as isize {
            return Err("RangeError: jump offset out of range".into());
        }
        let offset = offset as i16;
        let existing = instrs[instr_idx];
        let op = opcode::opcode(existing);
        let rd = opcode::rd(existing);
        instrs[instr_idx] = opcode::encode(op, rd, (offset as u16 & 0xFF) as u8, ((offset as u16 >> 8) & 0xFF) as u8);
    }

    let mut sub_modules = Vec::with_capacity(f.nested.len());
    for nested in &f.nested {
        sub_modules.push(lower(nested)?);
    }

    Ok(CompiledModule {
        bytecode: instrs,
        constants: f.constants.clone(),
        n_registers: f.n_registers as u8,
        n_args: f.param_layout.count as u8,
        param_base: f.param_layout.base as u8,
        builtin_reg_map: f.builtin_reg_map.clone(),
        sub_modules,
        is_arrow: f.is_arrow,
        captured_this_const_idx: f.captured_this_const_idx,
        function_name: f.function_name.clone(),
        function_length: f.function_length,
        is_class_constructor: f.is_class_constructor,
        is_derived_constructor: f.is_derived_constructor,
        needs_home_object: f.needs_home_object,
        is_generator: f.is_generator,
        is_async: f.is_async,
        upvalue_captures: f.upvalue_captures.clone(),
        cells_needed: f.cells_needed,
        flat_id: 0,
    })
}

/// Operand → u8 槽位。Const/Imm 在 a 槽走拆字（operand_pair_to_u8）；
/// 出现在 b/rd 槽时按低字节直写（const_flag、cell_idx、uv_idx 等一字节立即数）。
fn operand_to_u8(o: &Operand) -> u8 {
    match o {
        Operand::Reg(r) => *r as u8,
        Operand::This => 254,
        Operand::NewTarget => 255,
        Operand::Imm(v) => (v & 0xFF) as u8,
        Operand::Const(v) => (v & 0xFF) as u8,
        Operand::Label(_) | Operand::None => 0,
    }
}

/// 普通指令的 a/b 槽：a 槽的 Const/Imm 拆 lo/hi（对应 LOAD_CONST/CREATE_CLOSURE 布局）。
fn operand_pair_to_u8(a: &Operand, b: &Operand) -> (u8, u8) {
    match a {
        Operand::Const(idx) => ((idx & 0xFF) as u8, (idx >> 8) as u8),
        Operand::Imm(v) => ((v & 0xFF) as u8, (v >> 8) as u8),
        _ => (operand_to_u8(a), operand_to_u8(b)),
    }
}

fn encode_inst(inst: &Inst, instrs: &mut Vec<u32>, jumps: &mut Vec<(usize, u32)>) -> Result<(), String> {
    if is_jump_op(inst.op) {
        let label = match inst.b {
            Operand::Label(id) => id,
            _ => return Err(format!("jump op {:?} requires Label operand in b slot", inst.op)),
        };
        let cond = operand_to_u8(&inst.rd);
        let start = instrs.len();
        let instr = match inst.op {
            OpCode::JMP => opcode::encode_jmp(0),
            OpCode::BREAK | OpCode::CONTINUE => opcode::encode(inst.op, cond, 0, 0),
            OpCode::JMP_IF_FALSE => opcode::encode_jmp_if_false(cond, 0),
            OpCode::JMP_IF_TRUE => opcode::encode_jmp_if_true(cond, 0),
            OpCode::JMP_IF_NULLISH => opcode::encode_jmp_if_nullish(cond, 0),
            OpCode::TRY_BEGIN => opcode::encode_try_begin(0),
            OpCode::TRY_FINALLY_BEGIN => opcode::encode_try_finally_begin(0),
            _ => unreachable!("is_jump_op mismatch"),
        };
        instrs.push(instr);
        jumps.push((start, label));
        return Ok(());
    }

    let rd = operand_to_u8(&inst.rd);
    let (a, b) = operand_pair_to_u8(&inst.a, &inst.b);
    instrs.push(opcode::encode(inst.op, rd, a, b));
    instrs.extend_from_slice(&inst.ext);
    Ok(())
}

fn is_jump_op(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::JMP
            | OpCode::BREAK
            | OpCode::CONTINUE
            | OpCode::JMP_IF_FALSE
            | OpCode::JMP_IF_TRUE
            | OpCode::JMP_IF_NULLISH
            | OpCode::TRY_BEGIN
            | OpCode::TRY_FINALLY_BEGIN
    )
}
