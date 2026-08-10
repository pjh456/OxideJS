//! async/await 运行时测试：异步函数调用返回 promise、await 恢复、reject 进 catch、
//! 多 await 顺序、嵌套 async、微任务执行序。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

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

fn format_value(vm: &Vm, val: oxide_vm::JsValue) -> String {
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
        } else if obj.is_array() {
            format!("[array]")
        } else {
            "[object]".to_string()
        }
    } else {
        format!("{val}")
    }
}

#[test]
fn async_returns_immediate_value() {
    assert_eq!(eval("async function f(){ return 42 } f()"), "42");
}

#[test]
fn async_await_primitive() {
    assert_eq!(eval("async function f(){ var v = await 10; return v*2 } f()"), "20");
}

#[test]
fn async_await_resolved_promise() {
    assert_eq!(eval("async function f(){ var v = await Promise.resolve(5); return v+1 } f()"), "6");
}

#[test]
fn async_await_reject_into_catch() {
    assert_eq!(
        eval("async function f(){ try { await Promise.reject('e') } catch(e) { return 'caught:'+e } } f()"),
        "\"caught:e\""
    );
}

#[test]
fn async_multiple_awaits_in_order() {
    assert_eq!(eval("async function f(){ var x = await 1; x += await 2; return x } f()"), "3");
}

#[test]
fn async_await_return_promise() {
    assert_eq!(eval("async function f(){ return await Promise.resolve(7) } f()"), "7");
}

#[test]
fn async_nested_calls() {
    assert_eq!(eval("async function g(){ return 2 } async function f(){ return await g() + 3 } f()"), "5");
}

#[test]
fn async_arrow_in_then() {
    assert_eq!(eval("Promise.resolve().then(async ()=>{ return 9 }).then(v=>v)"), "9");
}

#[test]
fn async_execution_order_suspends_immediately() {
    // await 挂起 body；当前同步代码先跑完，body 恢复在微任务 drain 阶段。
    assert_eq!(
        eval("var order=[]; async function f(){ order.push(1); await 0; order.push(2) } f(); order.push(0); Promise.resolve().then(function(){ return order.join(',') })"),
        "\"1,0,2\""
    );
}

#[test]
fn async_throw_rejects_promise() {
    assert_eq!(eval("async function f(){ throw 'boom' } f()"), "<rejected \"boom\">");
}

#[test]
fn async_is_not_constructor() {
    assert!(eval("new (async function(){})").starts_with("vm error") || eval("new (async function(){})").contains("not a constructor"));
}

#[test]
fn async_constructor_name() {
    assert_eq!(eval("async function f(){}; f.constructor.name"), "\"AsyncFunction\"");
}

#[test]
fn async_class_method() {
    assert_eq!(eval("class A { async m(){ return await 7 } } new A().m()"), "7");
}

#[test]
fn async_gen_rejected_yield_queued_next_gets_done() {
    // P6：yield 值被拒 → 生成器 completed，unwrap 前排队的 next 得 {done:true}，
    // 不得错误继续执行后续 yield。
    assert_eq!(
        eval(
            "async function* g() { yield Promise.reject('e1'); yield 'never'; } \
             async function run() { const it = g(); const p1 = it.next(); const p2 = it.next(); \
             let r1, r2; try { await p1; r1 = 'p1-ok'; } catch (e) { r1 = 'p1-rej:' + e; } \
             try { const v = await p2; r2 = 'p2:' + v.value + ':' + v.done; } catch (e) { r2 = 'p2-rej:' + e; } \
             return r1 + '|' + r2; } run()"
        ),
        "\"p1-rej:e1|p2:undefined:true\""
    );
}

#[test]
fn async_gen_normal_yield_awaits_value() {
    // yield 一个 resolved promise：unwrap 完成后让出展开值；next(arg) 作为 yield 表达式值。
    assert_eq!(
        eval(
            "async function* g() { var v = yield Promise.resolve(9); yield v; } \
             async function run() { const it = g(); const a = (await it.next()).value; \
             const b = (await it.next(100)).value; return a + ',' + b; } run()"
        ),
        "\"9,100\""
    );
}

#[test]
fn async_gen_yield_then_next_continues() {
    // 正常 yield 值（非 promise）后 next 继续执行到完成。
    assert_eq!(
        eval(
            "async function* g() { yield 1; yield 2; } \
             async function run() { const it = g(); const a = (await it.next()).value; \
             const b = (await it.next()).value; const c = (await it.next()).done; \
             return a + ',' + b + ',' + c; } run()"
        ),
        "\"1,2,true\""
    );
}
