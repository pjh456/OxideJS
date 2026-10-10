//! 迭代器包装器（make_iterator_for_value 建出的统一迭代器对象）的
//! return/next 转发语义测试：零参转发、关闭期惰性 GetMethod（getter 抛错
//! 传播、不可调用 TypeError、null 无操作）、构造期不触碰内层 return getter。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
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
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn wrapper_return_forwards_zero_args() {
    // 包装器 return 默认零参转发内层 return（IteratorClose 步 4c）。
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},\
             return:function(){globalThis.n=arguments.length;return {};}};\
             for(var v of it){break;}globalThis.n===0"
        ),
        "true",
        "wrapper return must call inner return with zero arguments"
    );
}

#[test]
fn wrapper_next_forwards_zero_args() {
    // 包装器 next 默认零参转发内层 next（IteratorNext 步 4）。
    assert_eq!(
        eval(
            "var it={next:function(){globalThis.n=arguments.length;\
             return {value:1,done:true};}};\
             for(var v of it){}globalThis.n===0"
        ),
        "true",
        "wrapper next must call inner next with zero arguments"
    );
}

#[test]
fn wrapper_return_getter_throw_propagates_on_normal_close() {
    // 正常完成关闭时，内层 return getter 抛错必须传播（关闭期惰性 GetMethod）。
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},\
             get return(){throw new TypeError('getter');}};\
             globalThis.r=false;\
             try{for(var v of it){break;}}\
             catch(e){globalThis.r=(e.name==='TypeError'&&e.message==='getter');}\
             globalThis.r"
        ),
        "true",
        "getter throw on inner return must propagate on normal-completion close"
    );
}

#[test]
fn wrapper_return_non_callable_throws_type_error() {
    // 正常完成关闭时，内层 return 为已定义不可调用值必须抛 TypeError。
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},return:1};\
             globalThis.r=false;\
             try{for(var v of it){break;}}\
             catch(e){globalThis.r=(e instanceof TypeError);}\
             globalThis.r"
        ),
        "true",
        "defined non-callable inner return must throw TypeError on close"
    );
}

#[test]
fn wrapper_return_null_is_noop() {
    // 内层 return 为 null 时关闭无操作（GetMethod 步 2 返回 unused）。
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},return:null};\
             var threw=false;\
             try{for(var v of it){break;}}catch(e){threw=true;}\
             threw===false"
        ),
        "true",
        "null inner return must be a no-op on close"
    );
}

#[test]
fn wrapper_return_getter_throw_suppressed_on_throw_close() {
    // throw 完成关闭时，内层 return getter 抛错被在途异常替代（原异常胜出）。
    assert_eq!(
        eval(
            "var it={next:function(){return {value:1,done:false};},\
             get return(){throw new TypeError('getter');}};\
             globalThis.r=false;\
             try{for(var v of it){throw new Error('body');}}\
             catch(e){globalThis.r=(e.name==='Error'&&e.message==='body');}\
             globalThis.r"
        ),
        "true",
        "getter throw on inner return must be suppressed when close follows a throw"
    );
}

#[test]
fn wrapper_return_not_eagerly_bound_at_construction() {
    // 构造包装器时不得触碰内层 return getter（计数时点在关闭期）。
    assert_eq!(
        eval(
            "var count=0;\
             var it={next:function(){return {value:1,done:false};},\
             get return(){count++;throw new TypeError('getter');}};\
             var w=Iterator.from(it);count===0"
        ),
        "true",
        "wrapper construction must not touch the inner return getter"
    );
}

#[test]
fn wrapper_return_getter_called_once_at_close() {
    // 内层 return getter 在关闭期恰好被调用一次。
    assert_eq!(
        eval(
            "var count=0;\
             var it={next:function(){return {value:1,done:false};},\
             get return(){count++;return function(){return {};};}};\
             for(var v of it){break;}count===1"
        ),
        "true",
        "inner return getter must be invoked exactly once, at close"
    );
}
