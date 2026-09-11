//! async/await 运行时测试：异步函数调用返回 promise、await 恢复、reject 进 catch、
//! 多 await 顺序、嵌套 async、微任务执行序。

use std::sync::Arc;

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
    match vm.run(&Arc::new(module)) {
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
            "[array]".to_string()
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
    assert!(
        eval("new (async function(){})").starts_with("vm error")
            || eval("new (async function(){})").contains("not a constructor")
    );
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

#[test]
fn for_await_of_break_closes_iterator() {
    // 数组解构的 DONE 结果不得覆盖外层 for-await-of 迭代器的结果：break 提前
    // 退出时 CLOSE 按自身条目判 done，须调用 return() 关闭迭代器。
    assert_eq!(
        eval(
            "let closed=false;\
             const iter={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){closed=true;return Promise.resolve({done:true})}}}};\
             (async()=>{ for await (const [a,b] of iter) { break; } })();\
             Promise.resolve().then(()=>closed)"
        ),
        "true"
    );
}

#[test]
fn for_await_of_natural_done_skips_return() {
    // for-await-of 迭代自然结束（next 返回 done:true）时不调用 return()。
    assert_eq!(
        eval(
            "let closed=false;\
             const iter={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:true})},return(){closed=true;return Promise.resolve({done:true})}}}};\
             (async()=>{ for await (const x of iter) { } })();\
             Promise.resolve().then(()=>closed)"
        ),
        "false"
    );
}

#[test]
fn for_await_of_nested_sync_for_of_break_close_respective() {
    // for-await-of 内嵌同步 for-of：内层 break 只关内层同步迭代器，外层异步
    // 迭代器保持打开（条目配对，互不覆盖结果）。
    assert_eq!(
        eval(
            "let log=[];\
             const sync={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('s');return{}}}}};\
             const asyncIter={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){log.push('a');return Promise.resolve({done:true})}}}};\
             (async()=>{ for await (const x of asyncIter) { for (const y of sync) { break; } break; } })();\
             Promise.resolve().then(()=>log.join(','))"
        ),
         "\"s,a\""
    );
}

#[test]
fn for_await_of_body_throw_closes_and_original_error_wins() {
    // for-await-of 循环体抛错：先调 return() 关闭迭代器，原错误优先传播给 catch。
    assert_eq!(
        eval(
            "let log=[];\
             const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){log.push('r');return Promise.resolve({done:true})}}}};\
             (async()=>{ try { for await (const x of it) { throw 'orig'; } } catch(e) { log.push('c:'+e); } return log.join(','); })()"
        ),
        "\"r,c:orig\""
    );
}

#[test]
fn for_await_of_return_escape_defers_async_close() {
    // return 逃出 for-await-of：异步迭代器 return() 的 promise 须 await 后结算，
    // 走异步关闭机制（后续阶段）；阶段 1 同步逃出路径不得同步调用异步 return()。
    // 断言循环体已执行且没有同步副作用泄露。
    assert_eq!(
        eval(
            "let log=[];\
             const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){log.push('close');return Promise.resolve({done:true})}}}};\
             (async()=>{ for await (const x of it) { log.push('body'); return 1; } })();\
             Promise.resolve().then(()=>log.join(','))"
        ),
        "\"body\""
    );
}

// ── class/object 生成器方法（异步） ──

// class async 生成器方法：yield 顺序与 done 收敛。顶层 then 链驱动
// （普通回调帧内读取类绑定不触发 upvalue cell 误报）。
#[test]
fn class_async_generator_method_yields_in_order() {
    assert_eq!(
        eval(
            "class E { async *ag() { yield 1; yield 2; } }\
             var it = new E().ag(); var a, b, c;\
             var p = it.next().then(function (r) { a = r.value; return it.next(); })\
                      .then(function (r) { b = r.value; return it.next(); })\
                      .then(function (r) { c = r.done; return a + ',' + b + ',' + c; });\
             p"
        ),
        "\"1,2,true\""
    );
}

// class async 生成器方法 throw：异常进挂起点，被 body 内 catch 拦截后继续 yield。
#[test]
fn class_async_generator_method_throw_reaches_inner_catch() {
    assert_eq!(
        eval(
            "class F { async *t() { try { yield 1; } catch (e) { yield 'caught:' + e; } } }\
             var it = new F().t();\
             var p = it.next().then(function (r) { return it.throw('boom'); })\
                      .then(function (r) { return r.value; });\
             p"
        ),
        "\"caught:boom\""
    );
}

// object async 生成器方法：方法形态的 async generator 语义与函数式一致。
#[test]
fn object_async_generator_method_yields() {
    assert_eq!(
        eval(
            "var o = { async *gen() { yield 5; } };\
             var it = o.gen(); var a, d;\
             var p = it.next().then(function (r) { a = r.value; return it.next(); })\
                      .then(function (r) { d = r.done; return a + ':' + d; });\
             p"
        ),
        "\"5:true\""
    );
}

// ── upvalue cell 运行时 TDZ 误报（判别测试） ──

// T1（根因 A）：class 声明在前、async 函数在后。hoisting 使 async 体先编译，
// 若 class 名未进 captured_bindings → 编译期 THROW（命名错），期望经 cell 读到构造器。
#[test]
fn async_body_reads_top_level_class_after_decl() {
    assert_eq!(
        eval(
            "class E {} var r; var p = (async function(){ return E; })().then(function(v){ r = v.name; });\
             p; Promise.resolve().then(function(){ return r; })"
        ),
        "\"E\""
    );
}

// T1-sync（根因 A 对照）：普通函数读其后声明的 class，应与 async 同因失败。
#[test]
fn hoisted_sync_fn_reads_later_class() {
    assert_eq!(eval("function f(){ return C; } class C {} f().name"), "\"C\"");
}

// T2（根因 B）：提升的闭包在 var 声明语句执行前被调用。规范上 var 入口实例化
// 为 undefined，声明语句只是赋值；若语句点才初始化 cell → 运行时通用错误报。
#[test]
fn closure_reads_var_before_declaration_statement_returns_undefined() {
    assert_eq!(eval("var log; function f(){ return x; } log = f(); var x = 1; log"), "undefined");
}

// T3（根因 C）：async-gen 参数默认值内 IIFE 读文件级 var（var 全在 f 之前，
// 理论上捕获 cell 已初始化）。期望 body 断言通过（initCount==1, iterCount==0）。
#[test]
fn async_gen_param_default_closure_reads_file_var() {
    assert_eq!(
        eval(
            "var initCount=0; var iterCount=0; var iter=function*(){iterCount+=1;}(); var callCount=0; var f;\
             f=async function*([[]=function(){initCount+=1; return iter;}()]){ callCount+=1; };\
             var r; var p=f([]).next().then(function(){r=[initCount,iterCount,callCount].join(',');});\
             p; Promise.resolve().then(function(){return r;})"
        ),
        "\"1,0,1\""
    );
}

// T4（根因 C 链式）：async-gen 参数默认值内 IIFE 闭包读两个不同位置的文件级
// var，验证链式 parent_uv_idx 取父 upvalue cell 的序与值。
#[test]
fn async_gen_param_default_iife_chained_upvalue() {
    assert_eq!(
        eval(
            "var a=1; var b=2; var f;\
             f=async function*(x=(function(){return a+b;})()){ yield x; };\
             (async function(){ var g=f(); var r=await g.next(); return r.value; })()"
        ),
        "3"
    );
}
