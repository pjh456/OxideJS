//! 内嵌 dispatch 调度标志作用域测试：`do_return` 的"属主内嵌 dispatch 交付"条件
//! （frames 清空且 generator/async/construct 调度标志为真）只应终止标志的属主
//! dispatch。state-swap 边界（C 侧 builtin 经 `ordinary_get` 无帧调用 accessor
//! 等嵌套内联 dispatch）不得继承外层调度上下文——否则被调 getter 内
//! `new 字节码构造器()` 的构造帧是帧栈唯一帧，其弹出后 frames 清空，嵌套
//! dispatch 被误判为属主而提前交付，getter 构造之后的字节码（throw、后续语句）
//! 被整体跳过，C 侧读到构造残留值而非本应传播的异常。
//! 覆盖生成器体内 C 侧路径触发的构造后 throw 与取值两种形态；帧边界
//! （字节码 GET_PROP 压 getter 帧）形态与循环失控形态（test262 族）另行回测。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval_str(source: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(result).unwrap_or_else(|| format!("{result}"))
}

// ── 生成器体内 Object.assign（C 侧 ordinary_get 无帧调 getter）：
// getter 构造后再 throw，异常须穿透 next()（钉的是传播本身；builtin 侧
// 错误消息保真是独立缺口，不在本钉断言内） ──
#[test]
fn c_side_accessor_construct_then_throw_in_generator_propagates() {
    let s = eval_str(
        "function E() { this.tag = 1; }\n\
         var o = { get v() { var e = new E(); throw new Error('boom'); } };\n\
         function* f() { Object.assign({}, o); }\n\
         var ok = false;\n\
         try { f().next(); } catch (e) { ok = e instanceof Error; }\n\
         ok",
    );
    assert_eq!(s, "true", "C 侧 accessor 构造后 throw 应穿透生成器 next()，实际 {s:?}");
}

// ── 同场景取值形态：getter 构造后 return 值须原样交付（而非构造残留） ──
#[test]
fn c_side_accessor_construct_then_return_in_generator_delivers_value() {
    let s = eval_str(
        "function E() { this.tag = 1; }\n\
         var o = { get v() { var e = new E(); return 42; } };\n\
         function* f() { return Object.assign({}, o).v; }\n\
         f().next().value",
    );
    assert_eq!(s, "42", "C 侧 accessor 构造后的 return 值应原样交付，实际 {s:?}");
}

// ── 帧边界对照：字节码 GET_PROP 压 getter 帧执行（非 state-swap），
// 构造后 throw 本就穿透（防过度修正改变帧路径行为） ──
#[test]
fn frame_bound_accessor_construct_then_throw_in_generator_propagates() {
    let s = eval_str(
        "function E() { this.tag = 1; }\n\
         var o = { get v() { var e = new E(); throw new Error('boom'); } };\n\
         function* f() { return o.v; }\n\
         var caught = '';\n\
         try { f().next(); } catch (e) { caught = e.message; }\n\
         caught",
    );
    assert_eq!(s, "boom", "帧边界 getter 构造后 throw 应穿透生成器 next()，实际 {s:?}");
}

// ── 顶层对照：无调度标志时 C 侧 accessor 构造后 return 本就正常 ──
#[test]
fn c_side_accessor_construct_then_return_top_level_delivers_value() {
    let s = eval_str(
        "function E() { this.tag = 1; }\n\
         var o = { get v() { var e = new E(); return 42; } };\n\
         Object.assign({}, o).v",
    );
    assert_eq!(s, "42", "顶层 C 侧 accessor 构造后 return 值应原样交付，实际 {s:?}");
}
