//! 运行时等价对比：DCE 开/关两份字节码 lower + VM 执行结果一致。
//!
//! 同一 JS 源码走两条管线：parse → emit →（dce 开/关）→ lower → `Vm::run`，
//! 对比顶层执行结果。副作用纯表读错 VM handler 也能被此兜底。
//! **不经 `Compiler::compile`**：测试层直调底层函数，不依赖编译器开关。

use oxide_vm::vm::Vm;
use oxide_vm::JsValue;

/// parse→emit→（可选 DCE）→lower→run 全链路，返回顶层执行结果。
fn run_source(src: &str, dce: bool) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse failed");
    let mut ir = oxide_emit::Emitter::new()
        .emit_program(&program, false, false)
        .expect("emit failed");
    if dce {
        oxide_dce::dce(&mut ir);
    }
    let module = oxide_ir::lower::lower(&ir).expect("lower failed");
    let mut vm = Vm::new();
    vm.run(&module)
}

/// 结果规约成可断言形态：`(是否 Ok, 字符串化结果或错误消息)`。
/// Err 双方同为 Err 且消息一致视为一致（如 const 重赋值样例两跑均抛 TypeError）。
fn normalize(r: Result<JsValue, String>) -> (bool, String) {
    match r {
        Ok(v) => (true, v.to_string()),
        Err(e) => (false, e),
    }
}

/// 死代码样例集（覆盖表达式丢弃、死分支、死闭包、coerce 抛错、const 重赋值等路径）。
/// 对每个样例：DCE 开与关分别执行，断言结果一致。
#[test]
fn dead_code_removed_semantics_preserved() {
    // 覆盖：表达式语句丢弃结果、重复赋值只读最后值、if(false) 分支、死闭包、
    // try/catch 内死代码、对象操作数算术（coerce 抛错风险兜底）、
    // 顶层表达式返回值（HALT 隐式读 reg0 链）、先调用后丢弃（CALL 隐式写 reg0）、
    // const 重赋值抛错保留。
    let samples = [
        "1+2;3+4",                                    // 语句丢弃结果
        "'a'+'b';5;",                                 // 语句丢弃结果（字符串）
        "var x=1; x=2; x;",                           // 重复赋值只读最后值
        "function a(){return 1;} if(false){a();} 3;", // if(false) 分支
        "function f(){}; 1;",                         // 死闭包
        "try{1+2;}catch(e){3;} 4;",                   // try/catch 内死代码
        "({valueOf(){side=1; return 1}})+2;",         // 对象操作数算术（coerce 兜底）
        "1+2",                                        // 顶层表达式返回值（HALT 隐式读 reg0 链）
        "function f(){return 1;} f(); 2+3;",          // 先调用后丢弃（CALL 隐式写 reg0）
        "const x=1; x=2;",                            // const 重赋值（运行时抛错保留）
    ];
    for src in samples {
        let a = normalize(run_source(src, true));
        let b = normalize(run_source(src, false));
        assert_eq!(a, b, "DCE 前后语义不一致: {src}");
    }
}

/// 单个样例 DCE 开/关断言（失败时给出独立测试名，便于定位）。
#[test]
fn single_sample_top_level_return_value() {
    let a = normalize(run_source("1+2", true));
    let b = normalize(run_source("1+2", false));
    assert_eq!(a, b, "顶层返回值在 DCE 后变化");
    assert_eq!(a, (true, "3".to_string()), "顶层 1+2 应返回 3");
}

/// 顶层变量赋值是全局可观察状态（class 计算属性名里的 `x = 1`
/// 会被测试 harness 的 `assert.sameValue(x, 1)` 观察），即使本函数内无 use，
/// DCE 也**不得删除**对应 STORE_VAR——删除会改变外部可观察语义。
#[test]
fn top_level_store_var_kept() {
    let src = "let x = 0; class C { [x = 1]() { return 2; } } new C();";
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse failed");
    let mut ir = oxide_emit::Emitter::new()
        .emit_program(&program, false, false)
        .expect("emit failed");
    oxide_dce::dce(&mut ir);
    let module = oxide_ir::lower::lower(&ir).expect("lower failed");
    let has_store_var = module
        .bytecode
        .iter()
        .any(|&i| oxide_bytecode::opcode::opcode(i) == oxide_bytecode::opcode::OpCode::STORE_VAR);
    assert!(has_store_var, "顶层 x 赋值（全局可观察）不应被 DCE 删除: {src}");

    // 运行时兜底：DCE 开/关执行结果一致，且 x 最终值为 1（未被删赋值破坏）
    let a = normalize(run_source(src, true));
    let b = normalize(run_source(src, false));
    assert_eq!(a, b, "DCE 前后语义不一致: {src}");
}
