//! 寄存器收缩断言：RegAlloc on ≤253 且 < off（寄存器复用）；大函数 on 编译成功。

fn n_registers(src: &str, regalloc: bool) -> Result<u8, String> {
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
    let module = oxide_ir::lower::lower(&ir)?;
    // 返回模块树最大 n_registers（嵌套函数是实际寄存器压力来源）
    Ok(module_max_n_registers(&module))
}

fn module_max_n_registers(m: &oxide_bytecode::module::CompiledModule) -> u8 {
    let mut max = m.n_registers;
    for sub in &m.sub_modules {
        max = max.max(module_max_n_registers(sub));
    }
    max
}

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

/// 长活 L + n 次短活：L 干扰 n 个短活临时值 → L degree ≥ k → spill L → fresh 单点可着色。
fn gen_spill_function(n: u32) -> String {
    let mut src = String::from("function f(a) { var L = a; var s = 0; ");
    for _ in 0..n {
        src.push_str("s = s + L; ");
    }
    src.push_str("return s; } f(1);");
    src
}

/// 中等函数 on ≤253 且 < off（寄存器复用收缩）。
#[test]
fn n_registers_shrinks_with_regalloc() {
    let samples = [
        gen_sum_function(50),
        "function f(a, b, c) { return a * 100 + b * 10 + c; } function g() { return f(1, 2, 3); } g();".to_string(),
        "function f(n) { var s = 0; for (var i = 0; i < n; i++) { s += i * i; } return s; } f(10);".to_string(),
        "function f() { try { throw 1; } catch (e) { return e + 1; } } f();".to_string(),
        "function f() { var x = 1; return function() { return x++; }; } var g = f(); g();".to_string(),
    ];
    for src in &samples {
        let on = n_registers(src, true).expect("on 应成功");
        let off = n_registers(src, false).expect("off 应成功");
        assert!(on <= 253, "on ≤253: {on} for {src}");
        assert!(on < off, "on({on}) < off({off}) for {src}");
    }
}

/// 大函数：on 编译成功 ≤253；off 报 RangeError（vreg 超上限）。
#[test]
fn large_function_fits_in_253() {
    let src = gen_sum_function(220);
    let on = n_registers(&src, true);
    assert!(on.is_ok(), "大函数样例 on 应编译成功");
    assert!(on.unwrap() <= 253, "on ≤253");
    let off = n_registers(&src, false);
    assert!(off.is_err(), "大函数样例 off 应报 RangeError");
    assert!(off.unwrap_err().contains("too many registers"), "off Err 消息");
}

/// spill 压力样例（长活 L + 254 短活）on ≤253 且成功。
#[test]
fn spill_sample_under_limit() {
    let src = gen_spill_function(254);
    let on = n_registers(&src, true);
    assert!(on.is_ok(), "spill 样例 on 应编译成功: {on:?}");
    assert!(on.unwrap() <= 253, "on ≤253");
}
