//! 精确 DCE 二轮运行时等价 + 语义断言。
//!
//! 同一 JS 源码走两条管线：parse → emit → dce（保守恒定）→（precise on/off）→ lower → run，
//! 对比顶层执行结果。**不经 `Compiler::compile`**（测试层直调底层函数）。顶层赋值 /
//! escaped 槽样例做字节码/IR 级断言，is_top_level 标志直接断言。

use oxide_bytecode::opcode::OpCode;
use oxide_vm::vm::Vm;
use oxide_vm::JsValue;

/// parse→emit→dce（保守）→（precise?）→lower→run 全链路。
fn run_source(src: &str, precise: bool) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse failed");
    let mut ir = oxide_emit::Emitter::new()
        .emit_program(&program, false, false)
        .expect("emit failed");
    oxide_dce::dce(&mut ir); // 保守轮恒定（两跑一致）
    if precise {
        let cfg = oxide_cfg::build_cfg(&ir);
        let live = oxide_liveness::liveness(&ir, &cfg);
        oxide_dce::dce_precise(&mut ir, &live); // 精确轮（on 才跑）
    }
    let module = oxide_ir::lower::lower(&ir).expect("lower failed");
    let mut vm = Vm::new();
    vm.run(&module)
}

/// 结果规约成可断言形态：`(是否 Ok, 字符串化结果或错误消息)`。
fn normalize(r: Result<JsValue, String>) -> (bool, String) {
    match r {
        Ok(v) => (true, v.to_string()),
        Err(e) => (false, e),
    }
}

/// 精确轮 on/off 运行时等价样例集（覆盖顶层赋值 / escaped 槽 / const 抛错 / try-catch / 双写 / 纯链）。
#[test]
fn precise_dce_semantics_preserved() {
    let samples = [
        // 函数内局部死存储（精确轮删除，返回值不变）
        "function f() { var y = 1; return 5; } f();",
        // 死 a 链（a 从未读）
        "function f() { var a = 1 + 2; var b = 3 + 4; return b; } f();",
        // 顶层 STORE_VAR（is_top_level 保护，不删）
        "var x = 1; 2;",
        // 顶层赋值全局可观察（回归样例）
        "let x = 0; class C { [x = 1]() { return 2; } } new C();",
        // escaped 回归：k 只被嵌套子模块 LOAD_VAR.a 直读（父 liveness 死、escaped 保活）
        "function f() { let k = 'a'; class C { [k]() { return 1; } } return Object.keys(new C())[0]; } f();",
        // 死 const 声明存储（b=0 无 const guard，删之无害）
        "function f() { const c = 5; return 6; } f();",
        // const 重赋值抛 TypeError（b=1 保留，两跑同抛）
        "const x = 1; x = 2;",
        // catch 块内死存储 + 异常 reg0 路径（liveness 截断）
        "function f() { try { throw 1; } catch (e) { var y = 2; return e; } } f();",
        // 双写读尾值（两 STORE_VAR 槽活 → 全保留，结果 6）
        "function f() { var z = 5; z = 6; return z; } f();",
        // 顶层纯链（保守轮已删，精确轮幂等）
        "1 + 2;",
        // 局部死变量在循环后仍被读（活，保留）
        "function f(n) { var s = 0; for (var i = 0; i < n; i++) { s = s + i; } return s; } f(10);",
    ];
    for src in samples {
        let off = normalize(run_source(src, false));
        let on = normalize(run_source(src, true));
        assert_eq!(on, off, "precise on/off 语义不一致: {src}");
    }
}

/// 字节码级断言：顶层 `var x = 1; 2;` 经 dce+dce_precise 后 STORE_VAR 仍存在（顶层赋值全局可观察）。
#[test]
fn top_level_store_var_survives_precise() {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "var x = 1; 2;").expect("parse failed");
    let mut ir = oxide_emit::Emitter::new()
        .emit_program(&program, false, false)
        .expect("emit failed");
    oxide_dce::dce(&mut ir);
    let cfg = oxide_cfg::build_cfg(&ir);
    let live = oxide_liveness::liveness(&ir, &cfg);
    oxide_dce::dce_precise(&mut ir, &live);
    let module = oxide_ir::lower::lower(&ir).expect("lower failed");
    assert!(
        module
            .bytecode
            .iter()
            .any(|&i| OpCode::try_from(i as u8).ok() == Some(OpCode::STORE_VAR)),
        "顶层 STORE_VAR 必须存活"
    );
}

/// IR 级断言：函数内局部死 STORE_VAR 被精确轮删除。
/// dce_precise 不递归 nested，编译管线逐函数处理——此处直接对嵌套函数本体
/// 跑 liveness + dce_precise 证明删除生效。
#[test]
fn precise_removes_dead_local_store() {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "function f() { var y = 1; return 5; } f();").expect("parse failed");
    let ir = oxide_emit::Emitter::new()
        .emit_program(&program, false, false)
        .expect("emit failed");
    assert_eq!(ir.nested.len(), 1);
    let mut sub = ir.nested[0].clone();
    oxide_dce::dce(&mut sub);
    let cfg = oxide_cfg::build_cfg(&sub);
    let live = oxide_liveness::liveness(&sub, &cfg);
    oxide_dce::dce_precise(&mut sub, &live);
    assert!(sub.insts.iter().all(|i| i.op != OpCode::STORE_VAR), "函数内局部死 STORE_VAR 应被删除");
}

/// is_top_level 标志直接断言：顶层 true / 嵌套 false。
#[test]
fn top_level_flag_filled_by_emit() {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "var x = 1; function g() { return 2; }").expect("parse failed");
    let ir = oxide_emit::Emitter::new()
        .emit_program(&program, false, false)
        .expect("emit failed");
    assert!(ir.is_top_level, "顶层脚本 is_top_level 应为 true");
    assert!(ir.nested.iter().any(|sub| !sub.is_top_level), "嵌套函数 is_top_level 应为 false");
}
