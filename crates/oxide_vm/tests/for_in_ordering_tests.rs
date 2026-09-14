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

// 顺序经 JS 内部构建的位置映射断言，比较返回布尔（JsValue 的 Display 只暴露
// number/bool，不暴露字符串内容）。该写法也容忍引擎在对象自有键之后追加的
// 任意继承可枚举键。

#[test]
fn for_in_ordering_integer_indices_first() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {b:2,a:1,'2':3,'1':4}){pos[x]=i;i=i+1;}\
             pos['1']<pos['2'] && pos['2']<pos['b'] && pos['b']<pos['a']"
        ),
        "true",
        "integer indices ascending, then string keys in insertion order"
    );
}

#[test]
fn for_in_ordering_string_only_insertion_order() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {c:1,b:2,a:3}){pos[x]=i;i=i+1;}\
             pos['c']<pos['b'] && pos['b']<pos['a']"
        ),
        "true",
        "string-only keys keep insertion order"
    );
}

#[test]
fn for_in_ordering_mixed_boundary() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {a:1,'0':2,b:3}){pos[x]=i;i=i+1;}\
             pos['0']<pos['a'] && pos['a']<pos['b']"
        ),
        "true",
        "integer index '0' enumerates before string keys"
    );
}

#[test]
fn for_in_ordering_integer_indices_are_numeric_not_lexicographic() {
    assert_eq!(
        eval(
            "var pos={};var i=0;for(var x in {'10':1,'2':2,'1':3}){pos[x]=i;i=i+1;}\
             pos['1']<pos['2'] && pos['2']<pos['10']"
        ),
        "true",
        "index order is 1,2,10 (numeric) not 1,10,2 (lexicographic)"
    );
}

#[test]
fn for_in_still_enumerates_own_keys() {
    assert_eq!(
        eval("var c=0;for(var k in {a:1,b:2}){c=c+1;}c>=2"),
        "true",
        "for-in still enumerates the object's own enumerable keys"
    );
}

#[test]
fn for_in_var_head_reuses_existing_var_binding() {
    // var 头命中同 scope 已预声明的 var 绑定：复用既有槽位不重复声明，
    // 迭代键逐迭代落入该绑定（终值 = 末键）。
    assert_eq!(
        eval("var x = 0; for (var x in {a:1,b:2}) {} x === \"b\""),
        "true",
        "for-in var head reuses the predeclared same-scope var binding"
    );
}

#[test]
fn for_in_var_head_fresh_name() {
    // var 头无其他引用：提升的 var 槽接收迭代键。
    assert_eq!(
        eval("for (var x in {a:1}) {} x === \"a\""),
        "true",
        "for-in var head binds iteration keys into the hoisted var slot"
    );
}

// ── 提升函数引用 for-in/for-of 头 var 名：头名须编译期预声明（hoisting 面）──

#[test]
fn for_in_var_head_hoisted_fn_read() {
    // 顶层提升函数读 for-in 头 var：头名预声明入全局 var 名集，函数体按
    // 全局 var 绑定解析，完成值为迭代键。
    assert_eq!(
        eval("function f(){ return x; } for (var x in {a:1}) {} f() === \"a\""),
        "true",
        "hoisted function reads for-in head var through the global var binding"
    );
}

#[test]
fn for_in_var_head_hoisted_fn_strict_write() {
    // strict 嵌套写：头名预声明为全局 var 绑定后，写落全局属性（A 侧）；
    // 头名缺预声明时该写按未声明名抛 ReferenceError（本钉判别面）。
    assert_eq!(
        eval("'use strict'; function f(){ x = 5; } for (var x in {a:1}) { f(); } x === 5"),
        "true",
        "strict hoisted-function write to for-in head var hits the global binding"
    );
}

#[test]
fn for_of_var_head_hoisted_fn_strict_write() {
    // for-of 头同形：预声明臂覆盖 for-in 与 for-of 两臂。
    assert_eq!(
        eval("'use strict'; function f(){ x = 5; } for (var x of [1,2]) { f(); } x === 5"),
        "true",
        "strict hoisted-function write to for-of head var hits the global binding"
    );
}
