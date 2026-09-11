use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

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
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// 规范（ECMA-262 §13.7.5）：next()-throw 传播原始值且不调用 return()；
// IteratorClose（return()）只在循环体突然完成（throw/break/return）时执行。
// return() 经 globalThis 观测（函数可读写 globalThis 属性；
// 本引擎中它不能写入外层普通 `var`）。

#[test]
fn for_of_next_throw_is_catchable_with_original_type() {
    assert_eq!(
        eval(
            "try{for(var v of {next:function(){throw new TypeError('boom');}}){}}\
             catch(e){e.name==='TypeError' && e.message==='boom'}"
        ),
        "true",
        "next() throw catchable with original error type+message"
    );
}

#[test]
fn for_of_next_throw_preserves_bare_thrown_value() {
    assert_eq!(
        eval("try{for(var v of {next:function(){throw 7;}}){}}catch(e){e===7}"),
        "true",
        "a bare thrown value is preserved, not re-wrapped"
    );
}

#[test]
fn for_of_next_throw_does_not_call_return() {
    assert_eq!(
        eval(
            "globalThis.r=false;\
             var it={next:function(){throw new Error('x');},return:function(){globalThis.r=true;return {};}};\
             try{for(var v of it){}}catch(e){}globalThis.r===false"
        ),
        "true",
        "return() must NOT be called when next() throws"
    );
}

#[test]
fn for_of_next_throw_without_return_still_propagates() {
    assert_eq!(
        eval("try{for(var v of {next:function(){throw new TypeError('e');}}){}}catch(e){e instanceof TypeError}"),
        "true"
    );
}

#[test]
fn for_of_body_throw_calls_return() {
    assert_eq!(
        eval(
            "globalThis.r=false;\
             var it={next:function(){return {value:1,done:false};},return:function(){globalThis.r=true;return {};}};\
             try{for(var v of it){throw new Error('boom');}}catch(e){}globalThis.r===true"
        ),
        "true",
        "body throw calls return() (IteratorClose)"
    );
}

#[test]
fn for_of_body_throw_propagates_body_error() {
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},return:function(){return {};}};\
             try{for(var v of it){throw new Error('boom');}}catch(e){e.message==='boom'}"
        ),
        "true"
    );
}

#[test]
fn for_of_body_throw_return_also_throws_body_error_wins() {
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},return:function(){throw new Error('B');}};\
             try{for(var v of it){throw new Error('A');}}catch(e){e.message==='A'}"
        ),
        "true",
        "when return() also throws, the body's original error wins"
    );
}

#[test]
fn for_of_break_calls_return() {
    assert_eq!(
        eval(
            "globalThis.r=false;\
             var it={next:function(){return {value:1,done:false};},return:function(){globalThis.r=true;return {};}};\
             for(var v of it){break;}globalThis.r===true"
        ),
        "true",
        "break calls return() (via FOR_OF_CLOSE)"
    );
}

#[test]
fn for_of_array_regression_still_iterates() {
    assert_eq!(eval("var r=0;for(var v of [1,2,3]){r=r+v;}r===6"), "true");
}

#[test]
fn for_of_string_regression_still_iterates() {
    assert_eq!(eval("var r=0;for(var c of 'abc'){r=r+1;}r===3"), "true");
}

#[test]
fn for_of_destructuring_break_calls_return_on_outer_iterator() {
    // 数组解构在循环体内复用 FOR_OF_* 指令：解构自己的 DONE 结果不得覆盖外层
    // 迭代器的结果（按迭代器配对），否则 CLOSE 误判外层已自然结束而漏调 return()。
    assert_eq!(
        eval(
            "var closed=0;\
             var it={[Symbol.iterator](){return{next(){return{value:[1],done:false}},return(){closed++;return{}}}}};\
             for (const [a,b] of it) { break; }closed===1"
        ),
        "true"
    );
}

#[test]
fn for_of_nested_iterators_break_close_inner_only() {
    // 嵌套 for-of 提前退出：内层 break 只关内层迭代器（各自条目结果配对），
    // 外层迭代器保持打开继续下一轮。
    assert_eq!(
        eval(
            "var log=[];\
             var inner={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('i');return{}}}}};\
             var outer={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('o');return{}}}}};\
             for (var a of outer) { for (var b of inner) { break; } break; }log.join(',')==='i,o'"
        ),
        "true"
    );
}

#[test]
fn for_of_return_escape_closes_iterator() {
    // return 逃出 for-of：跳转前执行 IteratorClose（当前实现此前完全漏调）。
    assert_eq!(
        eval(
            "var closed=0;\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){closed++;return{}}}}};\
             function f(){ for (const x of it) { return 1; } } f();closed===1"
        ),
        "true"
    );
}

#[test]
fn for_of_labeled_break_escapes_all_in_lifo_order() {
    // labeled break 逃出两层：按嵌套逆序（内层先）逐层关闭。
    assert_eq!(
        eval(
            "var log=[];\
             var mk=(n)=>({[Symbol.iterator](){return{next(){return{value:n,done:false}},return(){log.push(n);return{}}}}});\
             var it1=mk(1), it2=mk(2);\
             outer: for (const a of it1) { for (const b of it2) { break outer; } }\
             log.join(',')==='2,1'"
        ),
        "true"
    );
}

#[test]
fn for_of_labeled_continue_closes_inner_only() {
    // labeled continue 逃出：内层每轮关闭，外层循环继续迭代不关闭。
    assert_eq!(
        eval(
            "var log=[];\
             var mk=(n)=>({[Symbol.iterator](){return{next(){return{value:n,done:false}},return(){log.push(n);return{}}}}});\
             var it2=mk(2);\
             outer: for (const a of [1,2]) { for (const b of it2) { continue outer; } }\
             log.join(',')==='2,2'"
        ),
        "true"
    );
}

#[test]
fn for_of_escape_finally_runs_before_close() {
    // 规范顺序：循环体完成值（含内部 try-finally 处理）之后才执行 IteratorClose。
    assert_eq!(
        eval(
            "var log=[];\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('close');return{}}}}};\
             function f(){ try { for (const x of it) { log.push('ret'); return 1; } } finally { log.push('finally'); } }\
             f();log.join(',')==='ret,finally,close'"
        ),
        "true"
    );
}

#[test]
fn for_of_escape_return_error_supersedes_and_closes_rest() {
    // 逃出时 return() 抛错：新错误替代完成值被外围 catch 捕获，剩余迭代器
    // （外层）仍被关闭（unwind 兜底，错误优先）。
    assert_eq!(
        eval(
            "var log=[];\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('c1');return{}}}}};\
             var it2={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('c2');throw new Error('B');return{}}}}};\
             function f(){ try { for (var a of it) { for (var b of it2) { return 1; } } } catch(e) { log.push(e.message); } }\
             f();log.join(',')==='c2,c1,B'"
        ),
        "true"
    );
}

#[test]
fn for_of_generator_return_escape_closes_iterators() {
    // 生成器 .return() 注入时关闭挂起点打开的全部迭代器。
    assert_eq!(
        eval(
            "var closed=0;\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){closed++;return{}}}}};\
             function* g(){ for (const x of it) { yield 1; return 2; } }\
             var gen=g();gen.next();gen.return(9);closed===1"
        ),
        "true"
    );
}

#[test]
fn for_in_labeled_break_pops_iterator_stack() {
    // labeled break 逃出 for-in：弹出内层迭代器，后续 for-in 从干净栈开始迭代
    // （此前逃出会使 for_in_iters 残留，后续 FOR_IN_DONE 读错 keys）。
    assert_eq!(
        eval(
            "var seen=[];\
             outer: for (var k in {a:1,b:2}) { for (var j in {c:3}) { break outer; } }\
             for (var m in {d:4}) { seen.push(m); }seen.join(',')==='d'"
        ),
        "true"
    );
}

#[test]
fn for_of_while_break_keeps_outer_iterator() {
    // while 内 break 不逃出外层 for-of：外层迭代器不被关闭（计数按词法深度对齐）。
    assert_eq!(
        eval(
            "var closed=0;\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){closed++;return{}}}}};\
             for (var a of it) { while (true) { break; } break; }closed===1"
        ),
        "true"
    );
}

#[test]
fn for_of_switch_break_keeps_outer_iterator() {
    // switch 内 break 不逃出外层 for-of：只关当前循环自身。
    assert_eq!(
        eval(
            "var closed=0;\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){closed++;return{}}}}};\
             for (var a of it) { switch (a) { case 1: break; } break; }closed===1"
        ),
        "true"
    );
}

#[test]
fn for_of_switch_break_continues_body_then_closes_once() {
    // switch 内 break 只跳出 switch：循环体后续语句继续执行，迭代器保持打开，
    // 直到循环收尾 CLOSE 才关闭一次（此前误在 switch 出口处提前关闭）。
    assert_eq!(
        eval(
            "var log=[];\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){log.push('close');return{}}}}};\
             for (var a of it) { switch (a) { case 1: break; } log.push('body'); break; }\
             log.join(',')==='body,close'"
        ),
        "true"
    );
}

#[test]
fn for_of_labeled_block_break_closes_all() {
    // label 包裹非循环语句：break 逃出整个 label 域，全部 for-of 关闭。
    assert_eq!(
        eval(
            "var closed=0;\
             var it={[Symbol.iterator](){return{next(){return{value:1,done:false}},return(){closed++;return{}}}}};\
             outer: { for (var a of it) { break outer; } }closed===1"
        ),
        "true"
    );
}
