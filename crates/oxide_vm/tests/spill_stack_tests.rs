//! VM spill 栈执行语义测试：MOV 复制、SPILL/UNSPILL 往返、
//! 帧边界（父 spill 跨子调用完好）、嵌套双 spill 交互、UNSPILL 越界防御。
//!
//! 手工 IR 是唯一可达路径——RegAlloc 未接入，真实 JS 编译产物不会含三指令。

use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_vm::vm::Vm;

fn run_ir(ir: IRFunction) -> String {
    let module = oxide_ir::lower::lower(&ir).expect("lower failed");
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(v) => format!("{v}"),
        Err(e) => format!("vm error: {e}"),
    }
}

fn int_module(insts: Vec<Inst>, value: i32, n_registers: u8, nested: Vec<IRFunction>) -> IRFunction {
    let mut f = IRFunction::new();
    f.insts = insts;
    f.constants = vec![Constant::Int(value)];
    f.n_registers = n_registers as u32;
    f.nested = nested;
    f
}

/// 测试 1：MOV 寄存器复制语义。r3=42 → MOV r5=r3 → HALT 输出 42。
#[test]
fn mov_copies_register() {
    let insts = vec![
        Inst::load_const(Operand::Reg(3), 0),
        Inst::inst_mov(Operand::Reg(5), Operand::Reg(3)),
        Inst::inst_mov(Operand::Reg(0), Operand::Reg(5)),
        Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None),
    ];
    let out = run_ir(int_module(insts, 42, 6, Vec::new()));
    assert_eq!(out, "42");
}

/// 测试 2：SPILL→UNSPILL 往返。SPILL r3 slot0 → MOV 弄脏 r5 → UNSPILL r5 slot0 → 读回 42。
#[test]
fn spill_unspill_roundtrip() {
    let insts = vec![
        Inst::load_const(Operand::Reg(3), 0),
        Inst::inst_spill(Operand::Reg(3), 0),
        Inst::inst_mov(Operand::Reg(5), Operand::Reg(3)),
        Inst::inst_unspill(Operand::Reg(5), 0),
        Inst::inst_mov(Operand::Reg(0), Operand::Reg(5)),
        Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None),
    ];
    let out = run_ir(int_module(insts, 42, 6, Vec::new()));
    assert_eq!(out, "42");
}

/// 测试 3：帧边界——父函数 SPILL 后 CALL 子函数，子函数返回后父 UNSPILL 仍得 42。
#[test]
fn frame_boundary_parent_spill_survives_call() {
    let child = int_module(
        vec![
            Inst::load_const(Operand::Reg(3), 0),
            Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None),
        ],
        99,
        4,
        Vec::new(),
    );
    let insts = vec![
        Inst::load_const(Operand::Reg(3), 0),
        Inst::inst_spill(Operand::Reg(3), 0),
        Inst::create_closure(Operand::Reg(4), 1),
        Inst::call(Operand::Reg(4), Operand::This, Operand::Reg(4), 0),
        Inst::inst_unspill(Operand::Reg(5), 0),
        Inst::inst_mov(Operand::Reg(0), Operand::Reg(5)),
        Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None),
    ];
    let out = run_ir(int_module(insts, 42, 6, vec![child]));
    assert_eq!(out, "42", "父 spill 数据在子调用后必须完好");
}

/// 测试 4：嵌套交互——父 SPILL 42、子函数自身也 SPILL（子帧基址 = 父已用长度 1），父 UNSPILL 仍得 42。
#[test]
fn nested_call_both_spill() {
    let child = int_module(
        vec![
            Inst::load_const(Operand::Reg(3), 0),
            Inst::inst_spill(Operand::Reg(3), 0),
            Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None),
        ],
        99,
        4,
        Vec::new(),
    );
    let insts = vec![
        Inst::load_const(Operand::Reg(3), 0),
        Inst::inst_spill(Operand::Reg(3), 0),
        Inst::create_closure(Operand::Reg(4), 1),
        Inst::call(Operand::Reg(4), Operand::This, Operand::Reg(4), 0),
        Inst::inst_unspill(Operand::Reg(5), 0),
        Inst::inst_mov(Operand::Reg(0), Operand::Reg(5)),
        Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None),
    ];
    let out = run_ir(int_module(insts, 42, 6, vec![child]));
    assert_eq!(out, "42", "子帧 spill 截断后父数据必须完好");
}

/// 测试 5：UNSPILL 空栈越界防御——读 undefined 不 panic。
#[test]
fn unspill_empty_stack_returns_undefined() {
    let insts = vec![
        Inst::inst_unspill(Operand::Reg(5), 0),
        Inst::inst_mov(Operand::Reg(0), Operand::Reg(5)),
        Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None),
    ];
    let out = run_ir(int_module(insts, 42, 6, Vec::new()));
    assert_eq!(out, "undefined");
}
