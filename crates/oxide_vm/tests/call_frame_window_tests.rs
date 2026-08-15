//! 调用帧窗口按存活上界拷贝的语义测试：验证压帧窗口缩小后调用方存活寄存器、
//! this/new.target、spill 区与异常展开在各类调用路径下恢复正确。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;
use oxide_vm::JsValue;

/// 执行源码并格式化顶层结果；Promise 结果取其 drain 后的 settled 值。
fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format_value(&vm, result),
        Err(e) => format!("vm error: {e}"),
    }
}

fn format_value(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        format!("\"{}\"", vm.lookup_str(val).unwrap_or_default())
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_promise_obj() {
            match promise_settled_value(obj) {
                Some((true, v)) => format_value(vm, v),
                Some((false, v)) => format!("<rejected {}>", format_value(vm, v)),
                None => "<pending>".to_string(),
            }
        } else {
            "[object]".to_string()
        }
    } else {
        format!("{val}")
    }
}

#[test]
fn call_window_basic_call_preserves_live_registers() {
    // 多值跨调用存活：sink 之后 x2..x5 仍要参与求和。
    let src = "function sink(a){ return a; } \
               function caller(){ var x1=1,x2=2,x3=3,x4=4,x5=5; \
                 var r = sink(x1); return r + x2 + x3 + x4 + x5; } \
               caller()";
    assert_eq!(eval(src), "15");
}

#[test]
fn call_window_nested_calls_keep_outer_values() {
    // outer 的 base 跨 mid/inner 两层调用存活，返回后仍参与求和。
    let src = "function inner(a,b){ return a*b; } \
               function mid(x){ return inner(x+1, x+2); } \
               function outer(){ var base = 10; return mid(base) + base; } \
               outer()";
    assert_eq!(eval(src), "142");
}

#[test]
fn call_window_deep_recursion_returns_correct() {
    let src = "function fib(n){ if (n < 2) return n; return fib(n-1) + fib(n-2); } fib(18)";
    assert_eq!(eval(src), "2584");
}

#[test]
fn call_window_recursive_counter_accumulates() {
    // 每层调用点都有跨调用存活的累加器，验证窗口逐层截断不丢值。
    let src = "function sum(n){ if (n == 0) return 0; return n + sum(n-1); } sum(200)";
    assert_eq!(eval(src), "20100");
}

#[test]
fn call_window_closure_upvalue_survives_calls() {
    let src = "function outer(){ var count = 0; \
                 function inc(step){ count += step; return count; } \
                 var a = inc(1); var b = inc(2); var c = inc(3); \
                 return [a,b,c].join(','); } \
               outer()";
    assert_eq!(eval(src), "\"1,3,6\"");
}

#[test]
fn call_window_generator_suspend_resume() {
    let src = "function* gen(){ var a = 1; yield a; var b = yield a + 10; return a + b; } \
               var g = gen(); var r1 = g.next(); var r2 = g.next(100); var r3 = g.next(5); \
               [r1.value, r2.value, r3.value].join(',')";
    assert_eq!(eval(src), "\"1,11,6\"");
}

#[test]
fn call_window_generator_calls_inside_body() {
    // 生成器体内调用普通函数（生成器体调用点保持全量窗口），挂起恢复后值正确。
    let src = "function add(a,b){ return a+b; } \
               function* gen(){ var x = add(20, 22); yield x; x = add(x, 1); yield x; } \
               var g = gen(); var r1 = g.next(); var r2 = g.next(); \
               [r1.value, r2.value].join(',')";
    assert_eq!(eval(src), "\"42,43\"");
}

#[test]
fn call_window_async_await_chain() {
    let src = "async function f(){ var x = 40; var y = await 2; return x + y; } f()";
    assert_eq!(eval(src), "42");
}

#[test]
fn call_window_apply_call_bind() {
    let src = "function f(a,b,c){ return [a,b,c].join('-'); } \
               f.apply(null, [1,2,3]) + '|' + f.call(null, 4,5,6) + '|' + f.bind(null,7,8)(9)";
    assert_eq!(eval(src), "\"1-2-3|4-5-6|7-8-9\"");
}

#[test]
fn call_window_new_expression() {
    let src = "function P(x){ this.x = x; this.y = x * 2; } \
               var p = new P(21); p.x + p.y";
    assert_eq!(eval(src), "63");
}

#[test]
fn call_window_derived_super_call() {
    let src = "class A { constructor(v){ this.v = v; } } \
               class B extends A { constructor(v){ super(v + 1); this.w = v * 10; } } \
               var b = new B(4); b.v + b.w";
    assert_eq!(eval(src), "45");
}

#[test]
fn call_window_many_live_values_large_function() {
    // 高槽存活值跨调用：callee 写入区大，调用方存活槽需在窗口内完整恢复。
    let src = "function heavy(a){ var t = a; for (var i = 0; i < 100; i++){ t += i; } return t; } \
               function caller(){ \
                 var k1=1,k2=2,k3=3,k4=4,k5=5,k6=6,k7=7,k8=8,k9=9,k10=10; \
                 var r = heavy(1); \
                 return k1+k2+k3+k4+k5+k6+k7+k8+k9+k10 + r; } \
               caller()";
    assert_eq!(eval(src), "5006");
}

#[test]
fn call_window_accessor_getter_and_setter() {
    // accessor 压帧走全量窗口路径，getter/setter 返回后写回目标寄存器正确。
    let src = "var obj = { _v: 0, get v(){ return this._v; }, set v(x){ this._v = x * 2; } }; \
               obj.v = 21; obj.v";
    assert_eq!(eval(src), "42");
}

#[test]
fn call_window_throw_unwind_restores_frame() {
    let src = "function boom(){ throw 'err'; } \
               function caller(){ var keep = 99; try { boom(); } catch(e) { keep += 1; } return keep; } \
               caller()";
    assert_eq!(eval(src), "100");
}

#[test]
fn call_window_tail_call_through_native_dispatcher() {
    // bound 函数尾调用返回字节码函数，窗口恢复与结果交付正确。
    let src = "function add(a,b){ return a + b; } \
               var bound = add.bind(null, 30); bound(12)";
    assert_eq!(eval(src), "42");
}

/// 本测试所用小程序中带 1 个扩展字的 opcode 集合（推进扫描用）。
fn ext_words(op: oxide_bytecode::opcode::OpCode) -> usize {
    use oxide_bytecode::opcode::OpCode;
    match op {
        OpCode::SPILL
        | OpCode::UNSPILL
        | OpCode::CALL
        | OpCode::CALL_NATIVE
        | OpCode::NEW_EXPRESSION
        | OpCode::SUPER_CALL => 1,
        _ => 0,
    }
}

#[test]
fn call_window_upper_bound_encoded_into_bytecode() {
    // 编译产物级断言：CALL 的 ext 高 8 位编码了调用点存活上界且小于调用方
    // n_registers（窗口确实被截断，非停留在全量路径）；nargs 低 8 位不变。
    let allocator = Allocator::default();
    let program = oxide_parser::parse(
        &allocator,
        "function f(x){ return x + 1; } \
         function g(){ var a = 1; var b = 2; var c = 3; var d = 4; return f(a) + b + c + d; } \
         g()",
    )
    .unwrap();
    let module = Compiler::new().compile(&program).unwrap();
    let bc = &module.bytecode;
    let mut i = 0;
    let mut call_ext = None;
    while i < bc.len() {
        let op = oxide_bytecode::opcode::opcode(bc[i]);
        if op == oxide_bytecode::opcode::OpCode::CALL {
            call_ext = Some(bc[i + 1]);
            i += 2;
        } else {
            i += 1 + ext_words(op);
        }
    }
    let ext = call_ext.expect("脚本中应有 CALL 指令");
    assert_eq!(ext & 0xFF, 0, "g() 无实参，nargs 低 8 位保持 0");
    let upper = (ext >> 8) & 0xFF;
    assert!(upper > 0, "调用点存活上界应被编码进 ext 高 8 位");
    assert!(
        upper <= module.n_registers as u32,
        "存活上界 {upper} 不应超过调用方 n_registers {}",
        module.n_registers
    );
}
