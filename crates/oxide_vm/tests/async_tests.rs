//! async/await 运行时测试：异步函数调用返回 promise、await 恢复、reject 进 catch、
//! 多 await 顺序、嵌套 async、微任务执行序。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
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
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
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
    // 走异步关闭机制。return() 同步调用（log.push('close') 同步执行），promise 经
    // 微任务结算后完成返回。断言循环体与关闭副作用都已执行。
    assert_eq!(
        eval(
            "let log=[];\
             const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){log.push('close');return Promise.resolve({done:true})}}}};\
             (async()=>{ for await (const x of it) { log.push('body'); return 1; } })();\
             Promise.resolve().then(()=>log.join(','))"
        ),
        "\"body,close\""
    );
}

#[test]
fn for_await_of_close_return_getter_throw_propagates_original() {
    // 收尾时 return 的 getter 抛出：外围 catch 须收到原始抛出值，
    // 不得是重建的 Error 对象。
    assert_eq!(
        eval(
            "let caught;\
             const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},get return(){throw 'ret-getter';}}}};\
             (async()=>{ try { for await (const x of it) { break; } } catch (e) { caught = e; } return 'caught:' + caught; })()"
        ),
        "\"caught:ret-getter\""
    );
}

#[test]
fn for_await_of_close_return_call_throw_propagates_original() {
    // 收尾时 return() 调用抛出：外围 catch 须收到原始抛出值。
    assert_eq!(
        eval(
            "let caught;\
             const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){throw 'ret-call';}}}};\
             (async()=>{ try { for await (const x of it) { break; } } catch (e) { caught = e; } return 'caught:' + caught; })()"
        ),
        "\"caught:ret-call\""
    );
}

#[test]
fn for_await_of_close_throw_object_identity_preserved() {
    // 收尾时 return() 抛出对象：外围 catch 收到的须是同一对象（身份保留），
    // 证明原始异常值经异常通道传播而非按文本重建。
    assert_eq!(
        eval(
            "let caught;\
             const marker={tag:'x'};\
             const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){throw marker;}}}};\
             (async()=>{ try { for await (const x of it) { break; } } catch (e) { caught = e; } return caught === marker ? 'same' : 'diff:' + caught; })()"
        ),
        "\"same\""
    );
}

// ── 逃出关闭测试矩阵（E1-E10）：多层 LIFO 关闭与 Completion 边角 ──
//
// for-await-of 的 next() 经 await 结算，循环体在 N 层嵌套时深 N 个微任务 tick；
// 读取 log 的 then 回调须比最深的结算闭包更晚入队，故用足够长的 then 链延迟读取。

/// 延迟 N 个微任务 tick 后读取 log（N 层逃出需 N+2 个 tick 让全部结算闭包跑完）。
fn delay_read(n: usize) -> String {
    let mut chain = "Promise.resolve()".to_string();
    for _ in 0..n {
        chain.push_str(".then(()=>{})");
    }
    chain.push_str(".then(()=>log.join(','))");
    chain
}

#[test]
fn for_await_of_break_escape_middle_layer() {
    // E2b：break 逃出中间 for-await-of 层——内层迭代器关闭，外层继续迭代。
    let source = "let log=[];\
                  const mk=(n)=>({[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:n,done:false})},return(){log.push('c'+n);return Promise.resolve({done:true})}}}});\
                  (async()=>{ for await (const a of mk(1)) { for await (const b of mk(2)) { log.push('body'); break; } log.push('after'); break; } })();\
                  ".to_string()
        + &delay_read(4);
    assert_eq!(eval(&source), "\"body,c2,after,c1\"");
}

#[test]
fn for_await_of_labeled_continue_closes_inner() {
    // E3：labeled continue 逃出内层 for-await-of——内层迭代器每轮关闭，外层继续。
    // 外层迭代器有限（两轮后 done），验证 continue 不关闭外层、外层继续迭代。
    let source = "let log=[];\
                  let outerN=0;\
                  const outer={[Symbol.asyncIterator](){return{next(){outerN++;if(outerN>2)return Promise.resolve({value:undefined,done:true});return Promise.resolve({value:outerN,done:false});},return(){log.push('cO');return Promise.resolve({done:true})}}}};\
                  const inner={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){log.push('cI');return Promise.resolve({done:true})}}}};\
                  (async()=>{ outer: for await (const a of outer) { for await (const b of inner) { log.push('body'); continue outer; } log.push('after-inner'); } })();\
                  ".to_string()
        + &delay_read(6);
    assert_eq!(eval(&source), "\"body,cI,body,cI\"");
}

#[test]
fn for_await_of_multi_layer_return_escape_lifo() {
    // E4：多层 return 逃出（两层 for-await-of）——LIFO 顺序关闭（内层先）。
    // 内层 return() 先被调（c2），外层后（c1），关闭顺序验证 LIFO。
    let source = "let log=[];\
                  const mk=(n)=>({[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:n,done:false})},return(){log.push('c'+n);return Promise.resolve({done:true})}}}});\
                  (async()=>{ for await (const a of mk(1)) { for await (const b of mk(2)) { log.push('body'); return 42; } } })();\
                  ".to_string()
        + &delay_read(4);
    assert_eq!(eval(&source), "\"body,c2,c1\"");
}

#[test]
fn for_await_of_multi_layer_labeled_break_escape_lifo() {
    // E5：多层 labeled break 逃出——LIFO 顺序关闭（内层先），跳转目标正确。
    let source = "let log=[];\
                  const mk=(n)=>({[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:n,done:false})},return(){log.push('c'+n);return Promise.resolve({done:true})}}}});\
                  (async()=>{ outer: for await (const a of mk(1)) { for await (const b of mk(2)) { log.push('body'); break outer; } } log.push('after'); })();\
                  ".to_string()
        + &delay_read(4);
    assert_eq!(eval(&source), "\"body,c2,c1,after\"");
}

#[test]
fn for_await_of_escape_close_reject_outer_catch() {
    // E6：逃出时 return() 拒绝——拒绝原因替代完成值，外围 catch 捕获。
    let source = "let log=[];\
                  const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){return Promise.reject('rej');}}}};\
                  (async()=>{ try { for await (const x of it) { log.push('body'); return 1; } } catch (e) { log.push('caught:'+e); } log.push('done'); })();\
                  ".to_string()
        + &delay_read(3);
    assert_eq!(eval(&source), "\"body,caught:rej,done\"");
}

#[test]
fn for_await_of_escape_no_return_method() {
    // E8：迭代器无 return 方法——不登记挂起，完成直接继续。
    let source = "let log=[];\
                  const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})}}}};\
                  (async()=>{ for await (const x of it) { log.push('body'); return 1; } })();\
                  "
    .to_string()
        + &delay_read(2);
    assert_eq!(eval(&source), "\"body\"");
}

#[test]
fn for_await_of_escape_settlement_precedes_then() {
    // E10：微任务时序——结算闭包（执行完成）先于后续 then 回调。
    // break 逃出后，完成跳转（log.push('after')）在结算闭包内执行，须先于 then 回调。
    let source = "let log=[];\
                  const it={[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:1,done:false})},return(){return Promise.resolve({done:true})}}}};\
                  (async()=>{ for await (const x of it) { log.push('body'); break; } log.push('after'); })();\
                  ".to_string()
        + &delay_read(3);
    assert_eq!(eval(&source), "\"body,after\"");
}

#[test]
fn for_await_of_multi_layer_escape_no_signal_leak_into_async_gen() {
    // 反向反例：普通异步函数多层逃出——多层结算循环置位的挂起信号不得泄漏进
    // 异步生成器的 dispatch，异步生成器不得被以 undefined 误完成。时序：f 的两轮
    // 结算（首轮逃出结算 + 多层结算循环）先于 g 的 await 恢复入队，g 恢复时体内
    // return 穿越 finally 命中派发循环检查点。首轮结算入口已清信号，故陈旧信号
    // 必来自多层结算循环。
    let source = "let log=[];\
                  const mk=(n)=>({[Symbol.asyncIterator](){return{next(){return Promise.resolve({value:n,done:false})},return(){log.push('c'+n);return Promise.resolve({done:true})}}}});\
                  async function f() { for await (const a of mk(1)) { for await (const b of mk(2)) { log.push('f-body'); return 42; } } }\
                  let resolveG;\
                  const PG = new Promise((res) => { resolveG = res; });\
                  async function* g() { await PG;\
                    log.push('g-body');\
                    try { return 'g-done'; } finally { log.push('g-finally'); } }\
                  const pF = f();\
                  const itG = g();\
                  const pG1 = itG.next();\
                  Promise.resolve().then(() => {}).then(() => resolveG('x'));\
                  Promise.all([pF, pG1]).then(([fv, gv]) => log.join(',') + '|' + fv + '|' + gv.value + ':' + gv.done)"
        .to_string();
    assert_eq!(eval(&source), "\"f-body,c2,c1,g-body,g-finally|42|g-done:true\"");
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
