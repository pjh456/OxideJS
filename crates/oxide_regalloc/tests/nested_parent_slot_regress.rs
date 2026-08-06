//! 嵌套父槽引用回归：nested 子模块直接槽引用父槽，RegAlloc on/off 运行时等价。
//!
//! 父侧 collect_escaped 保证父不移动槽；但子模块自身 alloc 时引用父槽的
//! LOAD_VAR.a / STORE_VAR.rd 曾被当作子模块自己的 vreg 染色移走（→ 读错物理槽，
//! on={object} vs off=42）。修复：graph.rs/rewrite.rs 对称 collect_own_escaped。
//! 槽号 ≥ inherited_reg_start（param_layout.base）时触发，故用高压力变量推高槽号。

use oxide_vm::vm::Vm;

fn run_source(src: &str, regalloc: bool) -> (bool, String) {
    let allocator = oxide_parser::Allocator::default();
    let program = match oxide_parser::parse(&allocator, src) {
        Ok(p) => p,
        Err(e) => return (false, format!("parse error: {}", e[0].message)),
    };
    let mut ir = match oxide_emit::Emitter::new().emit_program(&program) {
        Ok(i) => i,
        Err(e) => return (false, format!("emit error: {e}")),
    };
    oxide_dce::dce(&mut ir);
    if regalloc {
        let cfg = oxide_cfg::build_cfg(&ir);
        let live = oxide_liveness::liveness(&ir, &cfg);
        oxide_dce::dce_precise(&mut ir, &live);
        let cfg2 = oxide_cfg::build_cfg(&ir);
        let live2 = oxide_liveness::liveness(&ir, &cfg2);
        if let Err(e) = oxide_regalloc::alloc(&mut ir, &live2) {
            return (false, format!("alloc error: {e}"));
        }
    }
    let module = match oxide_ir::lower::lower(&ir) {
        Ok(m) => m,
        Err(e) => return (false, format!("lower error: {e}")),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(v) => (true, v.to_string()),
        Err(e) => (false, e),
    }
}

fn pressure(n: usize) -> String {
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!("var v{i} = {i}; "));
    }
    src
}

#[test]
fn field_reads_outer_40_pressure() {
    let src = format!("{}var a = 42; class C {{ p = a; }} new C().p;", pressure(40));
    let off = run_source(&src, false);
    let on = run_source(&src, true);
    assert_eq!(on, off, "field 读外层 40 压力: off={off:?} on={on:?}");
}

#[test]
fn method_reads_outer_40_pressure() {
    let src = format!("{}var a = 42; class C {{ m() {{ return a; }} }} new C().m();", pressure(40));
    let off = run_source(&src, false);
    let on = run_source(&src, true);
    assert_eq!(on, off, "method 读外层 40 压力: off={off:?} on={on:?}");
}

#[test]
fn field_reads_outer_30_pressure() {
    let src = format!("{}var a = 42; class C {{ p = a; }} new C().p;", pressure(30));
    let off = run_source(&src, false);
    let on = run_source(&src, true);
    assert_eq!(on, off, "field 读外层 30 压力: off={off:?} on={on:?}");
}

#[test]
fn field_reads_outer_25_pressure() {
    let src = format!("{}var a = 42; class C {{ p = a; }} new C().p;", pressure(25));
    let off = run_source(&src, false);
    let on = run_source(&src, true);
    assert_eq!(on, off, "field 读外层 25 压力: off={off:?} on={on:?}");
}

#[test]
fn field_reads_outer_hi_last_40_pressure() {
    let mut src = pressure(40);
    src.push_str("var a_hi = 777; class C { p = a_hi; } new C().p;");
    let off = run_source(&src, false);
    let on = run_source(&src, true);
    assert_eq!(on, off, "field 读外层末位 40 压力: off={off:?} on={on:?}");
}
