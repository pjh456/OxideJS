//! RegAlloc on/off 运行时等价（D-21 主验证）+ B005/spill 样例运行正确（REG-04）。
//!
//! 同一 JS 源码走两条管线：parse → emit → dce（保守恒定）→（regalloc on/off）→ lower → run，
//! 对比顶层执行结果。**不经 Compiler::compile**（测试层直调底层函数）。

use oxide_bytecode::opcode::OpCode;
use oxide_vm::vm::Vm;
use oxide_vm::JsValue;

/// parse→emit→dce→（regalloc?）→lower→run 全链路。
fn run_source(src: &str, regalloc: bool) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, src).expect("parse failed");
    let mut ir = oxide_emit::Emitter::new().emit_program(&program).expect("emit failed");
    oxide_dce::dce(&mut ir);
    if regalloc {
        let cfg = oxide_cfg::build_cfg(&ir);
        let live = oxide_liveness::liveness(&ir, &cfg);
        oxide_dce::dce_precise(&mut ir, &live);
        let cfg2 = oxide_cfg::build_cfg(&ir);
        let live2 = oxide_liveness::liveness(&ir, &cfg2);
        oxide_regalloc::alloc(&mut ir, &live2)?;
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

/// 主等价断言：RegAlloc on/off 运行时结果一致。
#[test]
fn regalloc_preserves_semantics() {
    let samples = [
        // switch+const（B011 回归）
        "function f(x) { switch (x) { case 1: break; } const c = 5; return c; } f(1);",
        // 类字段（B012 行为锚）
        "var x = 0; class C { p = (x = 1); } new C(); x;",
        // try-catch
        "function f() { try { throw 1; } catch (e) { return e + 1; } } f();",
        // 嵌套调用（参数连续性压力）
        "function f(a, b, c) { return a * 100 + b * 10 + c; } function g() { return f(1, 2, 3) + f(4, 5, 6); } g();",
        // 循环（label 重指向压力）
        "function f(n) { var s = 0; for (var i = 0; i < n; i++) { s += i * i; } return s; } f(10);",
        // 解构
        "function f() { var [a, b] = [1, 2]; var { c, d } = { c: 3, d: 4 }; return a + b + c + d; } f();",
        // 闭包（upvalue cell 路径）
        "function f() { var x = 1; return function() { return x++; }; } var g = f(); var r = [g(), g(), g()]; r[0] + r[1] + r[2];",
        // 箭头函数
        "var f = (a, b) => a * b; f(6, 7);",
        // 内置函数（builtin vreg 移动）
        "function f(a) { return Math.max(a, Math.min(2, 3)); } f(5);",
        // 递归
        "function f(n) { return n <= 1 ? 1 : n * f(n - 1); } f(8);",
        // const 重赋值（两跑同抛）
        "const x = 1; x = 2;",
    ];
    for src in samples {
        let off = normalize(run_source(src, false));
        let on = normalize(run_source(src, true));
        assert_eq!(on, off, "RegAlloc on/off 语义不一致: {src}");
    }
}

/// 生成 N 个变量的求和函数源码（程序化，结果确定 = N*(N-1)/2）。
fn gen_sum_function(n: u32) -> String {
    let mut src = String::from("function f() { ");
    for i in 0..n {
        src.push_str(&format!("var v{i} = {i}; "));
    }
    src.push_str("return ");
    for i in 0..n {
        if i > 0 {
            src.push('+');
        }
        src.push_str(&format!("v{i}"));
    }
    src.push_str("; } f();");
    src
}

/// 长活 L + n 次短活：L 干扰 n 个短活临时值 → 触发 spill（L degree ≥ k）。
fn gen_spill_function(n: u32) -> String {
    let mut src = String::from("function f(a) { var L = a; var s = 0; ");
    for _ in 0..n {
        src.push_str("s = s + L; ");
    }
    src.push_str("return s; } f(1);");
    src
}

/// B005 大函数（200+ 变量）与 spill 压力样例 on 运行正确。
/// off 路径 lower 失败（B005 上限）——不能做 on/off 相等，on 运行正确即 REG-04 证据。
#[test]
fn b005_and_spill_samples_run_correctly() {
    // B005：220 变量 → vreg 数远超 253，但同时存活 < 253 → RegAlloc 复用解决
    let src_b005 = gen_sum_function(220);
    let expected_b005 = 220 * 219 / 2;
    let out = run_source(&src_b005, true).expect("B005 样例 on 应编译成功");
    assert_eq!(out.to_string(), expected_b005.to_string(), "B005 样例结果错误");

    // spill 压力：长活 L + 254 短活 → L 溢出，验证 SPILL/UNSPILL 真实发生且结果正确
    let src_spill = gen_spill_function(254);
    let out = run_source(&src_spill, true).expect("spill 样例 on 应编译成功");
    assert_eq!(out.to_string(), "254", "spill 样例结果错误");

    // 字节码扫描：spill 样例经 alloc + lower 后应含 SPILL 与 UNSPILL opcode
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, &src_spill).expect("parse");
    let mut ir = oxide_emit::Emitter::new().emit_program(&program).expect("emit");
    oxide_dce::dce(&mut ir);
    let cfg = oxide_cfg::build_cfg(&ir);
    let live = oxide_liveness::liveness(&ir, &cfg);
    oxide_dce::dce_precise(&mut ir, &live);
    let cfg2 = oxide_cfg::build_cfg(&ir);
    let live2 = oxide_liveness::liveness(&ir, &cfg2);
    oxide_regalloc::alloc(&mut ir, &live2).expect("alloc");
    let module = oxide_ir::lower::lower(&ir).expect("lower");
    // 递归扫描模块树（spill 发生在嵌套函数 f 中，非顶层 f(1) 调用）
    fn has_op(m: &oxide_bytecode::module::CompiledModule, target: OpCode) -> bool {
        if m.bytecode.iter().any(|&instr| OpCode::try_from(instr as u8).ok() == Some(target)) {
            return true;
        }
        m.sub_modules.iter().any(|s| has_op(s, target))
    }
    let has_spill = has_op(&module, OpCode::SPILL);
    let has_unspill = has_op(&module, OpCode::UNSPILL);
    assert!(has_spill && has_unspill, "spill 样例应真实插入 SPILL/UNSPILL");
}
