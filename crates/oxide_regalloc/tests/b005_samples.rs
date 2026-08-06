//! 大函数寄存器收缩复现 + n_registers 汇总断言。
//!
//! 低层管线直调（不经 Compiler——oxide_regalloc dev-dep oxide_compiler 会构成依赖环）：
//! parse → emit → dce（保守）→（regalloc on/off）→ lower。on 分支按编译管线：
//! 精确 DCE 后重跑 build_cfg + liveness 再 alloc（删指令后下标位移，重建避免错位）；
//! off 分支为降级路径（vreg 原样当物理号 → lower Reg>253 抛 RangeError）。

use oxide_bytecode::module::CompiledModule;

fn compile_source(src: &str, regalloc: bool) -> Result<CompiledModule, String> {
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
    oxide_ir::lower::lower(&ir)
}

/// 大函数样例生成器：n 个逐语句独立 vreg（各值立即死）→ vreg 总数 ≈ 2n ≫ 253，
/// 但峰值活度极小 → RegAlloc 有充分复用空间。
fn gen_big(n: usize) -> String {
    let mut src = String::from("function f() { ");
    for i in 0..n {
        src.push_str(&format!("let v{i} = {i}; v{i}; "));
    }
    src.push_str(&format!("return v{}; }} f();", n - 1));
    src
}

/// 遍历模块树找最大 n_registers。
fn module_max_n_registers(m: &CompiledModule) -> u8 {
    let mut max = m.n_registers;
    for sub in &m.sub_modules {
        max = max.max(module_max_n_registers(sub));
    }
    max
}

/// 前置复现：off 模式（降级路径）250 变量函数报 RangeError。
#[test]
fn large_function_errors_without_regalloc() {
    let err = match compile_source(&gen_big(250), false) {
        Err(e) => e,
        Ok(_) => panic!("off 模式应报 RangeError"),
    };
    assert!(err.contains("RangeError"), "off 模式应报 RangeError: {err}");
}

/// 修复达成：on 模式 250 变量函数编译成功，模块树 n_registers ≤ 253。
#[test]
fn large_function_compiles_with_regalloc() {
    let module = compile_source(&gen_big(250), true).expect("on 模式应编译成功");
    assert!(module_max_n_registers(&module) <= 253, "模块树 n_registers 应 ≤253");
}

/// 样例集 n_registers on ≤ off 且至少一个严格下降（证明寄存器复用）。
#[test]
fn n_registers_on_off_aggregate() {
    let samples = [
        // 12 变量顺序死链：on 个位 vs off 数十（复用空间大）
        "function f() { let v0=0; v0; let v1=1; v1; let v2=2; v2; let v3=3; v3; let v4=4; v4; let v5=5; v5; let v6=6; v6; let v7=7; v7; let v8=8; v8; let v9=9; v9; let v10=10; v10; let v11=11; v11; return v11; } f();",
        // for 循环累加
        "function f(n) { var s = 0; for (var i = 0; i < n; i++) { s += i * i; } return s; } f(10);",
        // 嵌套调用
        "function f(a, b, c) { return a * 100 + b * 10 + c; } function g() { return f(1, 2, 3) + f(4, 5, 6); } g();",
        // try/catch
        "function f() { try { throw 1; } catch (e) { return e + 1; } } f();",
        // 闭包捕获
        "function f() { var x = 1; return function() { return x++; }; } var g = f(); g();",
        // 解构
        "function f() { var [a, b] = [1, 2]; var { c, d } = { c: 3, d: 4 }; return a + b + c + d; } f();",
        // 箭头函数
        "var f = (a, b) => a * b; f(6, 7);",
        // switch+const（const guard 回归）
        "function f(x) { switch (x) { case 1: break; } const c = 5; return c; } f(1);",
        // 数组字面量
        "function f() { var a = [1, 2, 3, 4, 5]; return a[0] + a[4]; } f();",
    ];
    let mut strict_decrease = false;
    for (i, src) in samples.iter().enumerate() {
        let on = compile_source(src, true).expect("on 应编译成功");
        let on_max = module_max_n_registers(&on);
        assert!(on_max <= 253, "样例 {i} on n_registers {on_max} 超 253");
        if let Ok(off) = compile_source(src, false) {
            let off_max = module_max_n_registers(&off);
            assert!(on_max <= off_max, "样例 {i} on({on_max}) > off({off_max})——RegAlloc 不应增寄存器窗口");
            if on_max < off_max {
                strict_decrease = true;
            }
        }
    }
    assert!(strict_decrease, "至少一个样例应严格 on < off（复用证明）");
}

/// 确定性：同一源码 on 模式编译两次字节码逐字节相同（确定性排序，缓存 key 依赖）。
#[test]
fn regalloc_deterministic_output() {
    let a = compile_source(&gen_big(50), true).expect("on 编译成功");
    let b = compile_source(&gen_big(50), true).expect("on 编译成功");
    assert_eq!(a.bytecode, b.bytecode, "两次编译字节码应逐字节相同");
    assert_eq!(a.n_registers, b.n_registers);
}
