//! 异常值通道配对回归测试：`last_uncaught_value` 单槽必须与异常生命周期配对——
//! 忽略路径（IteratorClose 的 return() 抛错被丢弃）不得把值残留进槽，unwind 进入
//! finally/catch 接住异常后槽作废，否则后续 take 会误取残留值污染拒绝原因。

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
        } else {
            "[object]".to_string()
        }
    } else {
        format!("{val}")
    }
}

/// 复现 return() 抛真值（IteratorClose suppress 路径写槽不恢复）+ finally 内 await
/// 窗口（unwind 进 finally 不清槽）→ 拒绝原因必须是循环体抛出的 "body"，
/// 不得被 return() 抛出的 `true` 残留污染。
#[test]
fn return_throw_finally_await_reason_is_body_not_true() {
    assert_eq!(
        eval(
            "var iter = { [Symbol.iterator]() { return {\
             next() { return { value: 1, done: false }; },\
             return() { throw true; } }; } };\
             async function f() {\
             try { for (const x of iter) { throw 'body'; } }\
             finally { await Promise.resolve(); } }\
             f()"
        ),
        "<rejected \"body\">"
    );
}

/// 复现 catch 接住异常后槽残留（return() 抛真值写槽、unwind 进 catch 不清槽），
/// 后续新抛的裸值必须作为拒绝原因，不得被残留的 `true` 替换。
#[test]
fn catch_then_new_throw_reason_is_new_value() {
    assert_eq!(
        eval(
            "var iter = { [Symbol.iterator]() { return {\
             next() { return { value: 1, done: false }; },\
             return() { throw true; } }; } };\
             async function f() {\
             try { for (const x of iter) { throw 'body'; } } catch (e) {}\
             throw 'real'; }\
             f()"
        ),
        "<rejected \"real\">"
    );
}

/// 回归：return() 不抛错时 for-of 提前退出仍调用 return() 且循环体异常优先。
#[test]
fn return_ok_iterator_close_still_called_body_error_wins() {
    assert_eq!(
        eval(
            "var log = '';\
             var iter = { [Symbol.iterator]() { return {\
             next() { return { value: 1, done: false }; },\
             return() { log = 'closed'; return {}; } }; } };\
             try { for (const x of iter) { throw 'A'; } } catch (e) { log += ':' + e; }\
             log"
        ),
        "\"closed:A\""
    );
}

/// 回归：迭代器自然结束（done:true）时不调用 return()，通道无残留。
#[test]
fn natural_done_skips_return_no_slot_residue() {
    assert_eq!(
        eval(
            "var log = '';\
             var iter = { [Symbol.iterator]() { return {\
             next() { return { value: 1, done: true }; },\
             return() { log = 'closed'; return {}; } }; } };\
             for (const x of iter) {}\
             Promise.resolve().then(function () { return log; })"
        ),
        "\"\""
    );
}

/// 复现 B067 RG1 裸值传递：promise 链中抛出的裸值 1 必须原样传递，
/// 拒绝原因不得被前置路径残留值替换。
#[test]
fn promise_chain_bare_reason_preserved() {
    assert_eq!(
        eval(
            "Promise.resolve()\
             .then(function () { throw 1; })\
             .then(function () {}, function (e) { return e; })\
             .then(function (e) { return 'got:' + e; })"
        ),
        "\"got:1\""
    );
}

/// 复现 21 超时代表面（同步 rest 收集）：迭代器 next() 返回 value 为抛错 getter
/// 的结果对象时，`[...it]` 必须同步抛错，不得无限循环。
#[test]
fn sync_rest_poisoned_value_getter_throws() {
    assert_eq!(
        eval(
            "var v = Object.defineProperty({}, 'value', { get: function () { throw 'x'; } });\
             var it = { [Symbol.iterator]() { return { next: function () { return v; } }; } };\
             try { [...it]; 'no-throw' } catch (e) { e === 'x' }"
        ),
        "true"
    );
}

/// 复现 21 超时代表面（async-generator 参数解构）：rest 收集遇 poisoned value 时
/// `f(it)` 调用点必须同步抛原值，异常可被用户 catch 捕获。
#[test]
fn async_gen_param_destructure_poisoned_throws() {
    assert_eq!(
        eval(
            "var v = Object.defineProperty({}, 'value', { get: function () { throw 'x'; } });\
             var it = { [Symbol.iterator]() { return { next: function () { return v; } }; } };\
             async function* f([...x]) {}\
             try { f(it); 'no-throw' } catch (e) { e === 'x' }"
        ),
        "true"
    );
}

/// 回归：生成器参数 rest 收集正常迭代时不误伤（pc 保护只拦截异常展开路径）。
#[test]
fn gen_param_rest_normal_collect_ok() {
    assert_eq!(
        eval(
            "function* f([...x]) { return x.length; }\
             var g = f([1, 2, 3]);\
             var r = g.next();\
             r.value + ':' + r.done"
        ),
        "\"3:true\""
    );
}

/// 回归：async-generator 参数 rest 收集正常迭代时返回正确结果。
#[test]
fn async_gen_param_rest_normal_collect_ok() {
    assert_eq!(
        eval(
            "async function* f([...x]) { return x.join('-'); }\
             f([1, 2]).next().then(function (r) { return r.value + ':' + r.done; })"
        ),
        "\"1-2:true\""
    );
}

/// 复现 21 超时代表面（async for-await-of + rest 解构）：poisoned value getter
/// 抛裸值 'x' 时 promise 必须以 'x' 拒绝，不得无限循环。
#[test]
fn async_for_await_of_rest_poisoned_rejects() {
    assert_eq!(
        eval(
            "var v = Object.defineProperty({}, 'value', { get: function () { throw 'x'; } });\
             var it = { [Symbol.iterator]() { return { next: function () { return v; } }; } };\
             async function fn() { for await (var [...x] of [it]) { return; } }\
             fn()"
        ),
        "<rejected \"x\">"
    );
}

/// 复现 21 超时代表面（同步生成器参数解构）：生成器参数 rest 收集遇 poisoned
/// value 时调用点必须同步抛错，异常可被用户 catch 捕获。
#[test]
fn gen_param_destructure_poisoned_throws() {
    assert_eq!(
        eval(
            "var v = Object.defineProperty({}, 'value', { get: function () { throw 'x'; } });\
             var it = { [Symbol.iterator]() { return { next: function () { return v; } }; } };\
             function* f([...x]) {}\
             try { f(it).next(); 'no-throw' } catch (e) { e === 'x' }"
        ),
        "true"
    );
}

/// 复现 21 超时代表面（嵌套 inline + 真实帧深度链）：rest 收集在 map 回调
/// （inline dispatch）调用的普通函数内执行，poisoned value 异常须穿透
/// 深度链原样到达用户 catch。
#[test]
fn nested_inline_map_rest_poisoned_throws() {
    assert_eq!(
        eval(
            "var v = Object.defineProperty({}, 'value', { get: function () { throw 'x'; } });\
             var it = { [Symbol.iterator]() { return { next: function () { return v; } }; } };\
             function collect(p) { return [...p]; }\
             try { [1].map(function (x) { return collect(it); }); 'no-throw' } catch (e) { e === 'x' }"
        ),
        "true"
    );
}

/// 复现 AsyncFromSyncIterator 的 valueWrapper reject 结算路径（close_sync_iterator
/// 调用户 return() 抛真值）：return() 的抛错被忽略后其值不得残留进槽——异常逃逸
/// 出异步帧（resume_async 经槽恢复原值时取到残留 `true`）拒绝原因被污染。
/// return() 只在 reject-close 第一次调用时抛错，IteratorClose 转发不抛，
/// 保证残留之后没有新写槽覆盖。
#[test]
fn sync_iterator_return_throw_no_slot_pollution() {
    assert_eq!(
        eval(
            "var threw = false;\
             var inner = {\
             next() { throw 'x'; },\
             return() { if (!threw) { threw = true; throw true; } return { done: true }; } };\
             var iterable = { [Symbol.iterator]() { return inner; } };\
             async function f() { for await (var y of iterable) {} }\
             f()"
        ),
        "<rejected \"x\">"
    );
}

/// 回归：聚合迭代器关闭（close_agg_iterator）遇迭代抛错时调用户 return() 抛真值，
/// 拒绝原因保持迭代抛出的原异常，不得被 return() 的 `true` 替换。
#[test]
fn agg_iterator_return_throw_no_slot_pollution() {
    assert_eq!(
        eval(
            "var inner = { next() { throw 'x'; }, return() { throw true; } };\
             var iterable = { [Symbol.iterator]() { return inner; } };\
             Promise.all(iterable).then(function () {}, function (e) { return e === true ? 'polluted:true' : e; })"
        ),
        "\"x\""
    );
}

/// 回归：Array.from 迭代输入抛错时 close_iterator 仍调用 return()（记录日志），
/// 且迭代抛出的原异常优先于 return() 的返回值。
#[test]
fn array_from_iterator_close_original_error_wins() {
    assert_eq!(
        eval(
            "var log = '';\
             var inner = { next() { throw 'x'; }, return() { log += 'closed'; return {}; } };\
             var iterable = { [Symbol.iterator]() { return inner; } };\
             try { Array.from(iterable); } catch (e) { log += ':' + e; }\
             log"
        ),
        "\"closed:x\""
    );
}
