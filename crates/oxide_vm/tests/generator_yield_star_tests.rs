//! `yield*` 委托语法运行时测试：委托数组/字符串/生成器，next/return/throw 转发，
//! 委托完成值传递与空委托场景。

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
        Ok(result) => {
            if result.is_string() {
                vm.lookup_str(result).unwrap_or_default()
            } else {
                format!("{result}")
            }
        }
        Err(e) => format!("vm error: {e}"),
    }
}

// 委托数组：逐元素让出。
#[test]
fn yield_star_array() {
    assert_eq!(eval("function* g(){ yield* [1,2,3] } Array.from(g()).join(',')"), "1,2,3");
}

// 委托字符串：按码元让出。
#[test]
fn yield_star_string() {
    assert_eq!(eval("function* g(){ yield* 'ab' } Array.from(g()).join(',')"), "a,b");
}

// 委托完成值作为 yield* 表达式结果，外层继续执行。
#[test]
fn yield_star_completion_value() {
    assert_eq!(
        eval("function* inner(){ yield 1; yield 2; return 3 } function* outer(){ var r = yield* inner(); yield r } var it=outer(); [it.next().value, it.next().value, it.next().value, it.next().done].join(',')"),
        "1,2,3,true"
    );
}

// 委托夹在普通 yield 之间，保持顺序。
#[test]
fn yield_star_mixed_with_plain_yield() {
    assert_eq!(
        eval("function* g(){ yield 0; yield* [1,2]; yield 3 } Array.from(g()).join(',')"),
        "0,1,2,3"
    );
}

// 嵌套委托（yield* 内再 yield*）。
#[test]
fn yield_star_nested() {
    assert_eq!(
        eval("function* a(){ yield 1 } function* b(){ yield* a() } function* c(){ yield* b() } Array.from(c()).join(',')"),
        "1"
    );
}

// 空委托：立即继续外层。
#[test]
fn yield_star_empty() {
    assert_eq!(eval("function* g(){ yield* [] ; yield 5 } Array.from(g()).join(',')"), "5");
}

// 外层 return() 转发给内层：内层完成值交付。
#[test]
fn yield_star_return_forward() {
    assert_eq!(
        eval("function* inner(){ yield 1 } function* outer(){ yield* inner() } var it=outer(); it.next(); it.return(9).value"),
        "9"
    );
}

// 外层 throw() 转发给内层：内层消化后外层继续。
#[test]
fn yield_star_throw_forward() {
    assert_eq!(
        eval("function* inner(){ try { yield 1 } catch(e) { return 'handled:'+e } } function* outer(){ var r = yield* inner(); yield 'got: ' + r } var it=outer(); var a=it.next(); var b=it.throw('x'); [a.value, b.value].join(',')"),
        "1,got: handled:x"
    );
}

// 委托不可迭代值：经外层 catch 捕获。
#[test]
fn yield_star_not_iterable_throws() {
    assert_eq!(
        eval("function* outer(){ try { yield* 42 } catch(e) { yield 'err' } } Array.from(outer()).join(',')"),
        "err"
    );
}

// 内层无 return 方法：外层 return() 直接完成（值为请求值）。
#[test]
fn yield_star_inner_without_return() {
    assert_eq!(
        eval("var inner = { [Symbol.iterator]() { return this; }, next() { return {value:1, done:false}; } }; function* outer(){ yield* inner; yield 5 } var it=outer(); var a=it.next(); var b=it.return(9); [a.value, b.value, b.done].join(',')"),
        "1,9,true"
    );
}
