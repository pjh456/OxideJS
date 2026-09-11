//! 循环每迭代绑定（CreatePerIterationEnvironment）：for/for-of/for-in 的 let/const
//! 闭包捕获各自迭代快照。对应 PLAN 06-01（正确性报告 P1/P2）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

#[test]
fn c_style_for_let_captures_per_iteration() {
    let (vm, result) =
        eval("let fns = []; for (let i = 0; i < 3; i++) fns.push(() => i); [fns[0](), fns[1](), fns[2]()].join(',')")
            .unwrap();
    assert_eq!(to_str(&vm, result), "0,1,2");
}

#[test]
fn c_style_for_const_update_writes_per_iteration() {
    // for-const 的 update 段写 per-iteration 可变绑定（CreateMutableBinding），
    // 不得抛 "Assignment to constant variable"（V8 输出 0 1 2）。
    let (vm, result) = eval("let out=[]; for (const i = 0; i < 3; i++) out.push(i); out.join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "0,1,2");
}

#[test]
fn c_style_for_const_captures_per_iteration() {
    // for-const 循环变量被闭包捕获：每迭代 fresh，update 段写寄存器不污染本迭代 cell。
    let (vm, result) =
        eval("let fns = []; for (const i = 0; i < 3; i++) fns.push(() => i); [fns[0](), fns[1](), fns[2]()].join(',')")
            .unwrap();
    assert_eq!(to_str(&vm, result), "0,1,2");
}

#[test]
fn c_style_for_const_compound_update_writes_per_iteration() {
    // 复合赋值形态的 update 同样合法（i += 2 是 update 段的 per-iteration 可变写）。
    let (vm, result) = eval("let out=[]; for (const i = 0; i < 6; i += 2) out.push(i); out.join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "0,2,4");
}

#[test]
fn c_style_for_let_captures_after_multiple_updates() {
    let (vm, result) = eval(
        "let fns = []; for (let i = 0; i < 5; i += 2) fns.push(() => i); [fns[0](), fns[1](), fns[2]()].join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "0,2,4");
}

#[test]
fn c_style_for_var_remains_single_binding() {
    // var 循环变量是单绑定（规范：闭包捕获同一绑定，最终值共享）。
    let (vm, result) =
        eval("let fns = []; for (var i = 0; i < 3; i++) fns.push(() => i); [fns[0](), fns[1](), fns[2]()].join(',')")
            .unwrap();
    assert_eq!(to_str(&vm, result), "3,3,3");
}

#[test]
fn for_of_const_captures_per_iteration() {
    let (vm, result) =
        eval("let fns = []; for (const a of [1, 2]) fns.push(() => a); [fns[0](), fns[1]()].join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "1,2");
}

#[test]
fn for_of_let_captures_per_iteration() {
    let (vm, result) =
        eval("let fns = []; for (let a of [10, 20, 30]) fns.push(() => a); [fns[0](), fns[1](), fns[2]()].join(',')")
            .unwrap();
    assert_eq!(to_str(&vm, result), "10,20,30");
}

#[test]
fn for_of_destructuring_captures_per_iteration() {
    // P2：解构 + 闭包捕获不再产出垃圾值。
    let (vm, result) = eval(
        "let fns = []; for (const [a, b] of [[1, 2], [3, 4]]) fns.push(() => a + b); [fns[0](), fns[1]()].join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "3,7");
}

#[test]
fn for_of_object_destructuring_captures_per_iteration() {
    let (vm, result) =
        eval("let fns = []; for (const {x, y} of [{x:1,y:2},{x:3,y:4}]) fns.push(() => x * y); [fns[0](), fns[1]()].join(',')")
            .unwrap();
    assert_eq!(to_str(&vm, result), "2,12");
}

#[test]
fn for_in_const_captures_per_iteration() {
    let (vm, result) =
        eval("let fns = []; for (const k in {a:1,b:2}) fns.push(() => k); [fns[0](), fns[1]()].join(',')").unwrap();
    assert_eq!(to_str(&vm, result), "a,b");
}

#[test]
fn nested_loop_closure_captures_distinct_bindings() {
    let (vm, result) = eval(
        "let fns = []; for (let i = 0; i < 2; i++) for (let j = 0; j < 2; j++) fns.push(() => i + j); [fns[0](), fns[1](), fns[2](), fns[3]()].join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "0,1,1,2");
}

#[test]
fn loop_variable_mutation_visible_to_same_iteration() {
    // 同迭代内变量修改（非闭包）仍直接可见：fresh 每迭代一次，迭代内读写走同一 cell。
    let (vm, result) = eval(
        "let fns = []; for (let i = 0; i < 3; i++) { fns.push(() => i); i += 0; } [fns[0](), fns[1](), fns[2]()].join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "0,1,2");
}
