//! 未实装 profile 族 opcodes 的显式失败钉：run 遇 PROFILE_SHAPE 须返回
//! 显式错误而非静默按数据指令消费（手工 CompiledModule 直达，不经编译器）。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode;
use oxide_vm::vm::Vm;

#[test]
fn unimplemented_profile_opcode_fails_explicitly() {
    let module = CompiledModule {
        bytecode: Arc::from(vec![
            opcode::encode(opcode::OpCode::PROFILE_SHAPE, 0, 0, 0),
            opcode::encode(opcode::OpCode::HALT, 0, 0, 0),
        ]),
        n_registers: 1,
        ..CompiledModule::new()
    };
    let mut vm = Vm::new();
    let err = vm
        .run(&Arc::new(module))
        .expect_err("unimplemented opcode should fail explicitly");
    assert_eq!(err, "opcode PROFILE_SHAPE not yet implemented");
}
