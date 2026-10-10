//! 结构哈希确定性碰撞的回归测试：
//! 补全 hash 的 `_ => {}` 兜底漏掉的变体后，不同 AST 不得产生相同哈希。

use oxide_parser::Allocator;

fn compiled(source: &str) -> u64 {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    oxide_compiler::compiler::compiled_module_hash(&program)
}

fn structural(source: &str) -> u64 {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    oxide_compiler::compiler::structural_hash(&program)
}

#[test]
fn array_literals_distinguished() {
    assert_ne!(compiled("[1, 2, 3]"), compiled("[4, 5, 6]"));
    assert_ne!(compiled("[1, 2, 3]"), compiled("[1, , 3]"), "elision vs value must differ");
    assert_ne!(compiled("[1, 2, ...a]"), compiled("[1, 2, 3]"), "spread vs value must differ");
    assert_ne!(compiled("[...a]"), compiled("[...b]"), "different spread sources must differ");
}

#[test]
fn meta_property_distinguished() {
    // `new.target` 是 MetaProperty，`a.b` 是普通成员访问，结构不同。
    assert_ne!(compiled("function f(){ return new.target }"), compiled("function f(){ return a.b }"));
    // 同为 MetaProperty 时 meta/property 名字必须参与哈希（修复前两者哈希相同）。
    assert_ne!(
        compiled("import './x.js'; function f(){ return import.meta }"),
        compiled("import './x.js'; function f(){ return new.target }"),
        "meta/property names must be part of the hash"
    );
}

#[test]
fn parenthesized_distinguished() {
    assert_ne!(compiled("(a)"), compiled("a"), "parenthesized vs bare expression must differ");
    assert_ne!(compiled("(a + b)"), compiled("a + b"));
    assert_ne!(compiled("(a)"), compiled("(b)"), "inner expression must be hashed");
}

#[test]
fn await_yield_plain_distinguished() {
    let await_h = compiled("async function f(){ await x }");
    let yield_h = compiled("function* f(){ yield x }");
    let plain_h = compiled("function f(){ x }");
    assert_ne!(await_h, yield_h);
    assert_ne!(await_h, plain_h);
    assert_ne!(yield_h, plain_h);

    // yield 的 delegate 标志与参数必须参与哈希。
    assert_ne!(compiled("function* f(){ yield x }"), compiled("function* f(){ yield* x }"));
    assert_ne!(compiled("function* f(){ yield x }"), compiled("function* f(){ yield y }"));
}

#[test]
fn object_literals_distinguished() {
    assert_ne!(compiled("var o = { a: 1 }"), compiled("var o = { a: 2 }"));
    assert_ne!(compiled("var o = { a: 1 }"), compiled("var o = { b: 1 }"));
    assert_ne!(
        compiled("var o = { a: 1 }"),
        compiled("var o = { a: 1, b: 2 }"),
        "property count must be hashed"
    );
}

#[test]
fn function_statement_hash_distinguished() {
    assert_ne!(compiled("function f(){ return 1 }"), compiled("function f(){ return 2 }"));
    assert_ne!(compiled("function f(){ return 1 }"), compiled("function g(){ return 1 }"));
}

#[test]
fn for_of_with_distinguished() {
    assert_ne!(compiled("for (x of xs) { body(x) }"), compiled("for (x of ys) { body(x) }"));
    assert_ne!(compiled("for (x of xs) { body(x) }"), compiled("for (x in xs) { body(x) }"));
    assert_ne!(compiled("with (a) { f() }"), compiled("with (b) { f() }"));
}

#[test]
fn import_expression_distinguished() {
    assert_ne!(compiled("import('./a.js')"), compiled("import('./b.js')"));
    assert_ne!(compiled("import('./a.js')"), compiled("import('./a.js', { with: { type: 'json' } })"));
}

#[test]
fn import_defer_distinguished() {
    // import defer 与普通 namespace import 语义不同（defer 不触发依赖求值）：
    // 修复前 phase 不入 hash，两者哈希相同 → 缓存碰撞复用错误字节码。
    assert_ne!(
        compiled("import defer * as ns from './x.js'"),
        compiled("import * as ns from './x.js'"),
        "import defer vs plain namespace import must differ"
    );
    assert_ne!(
        structural("import defer * as ns from './x.js'"),
        structural("import * as ns from './x.js'"),
        "defer flag must be part of the structural key too"
    );
    // 同源等价回归
    assert_eq!(
        compiled("import defer * as ns from './x.js'"),
        compiled("import defer * as ns from './x.js'")
    );
}

#[test]
fn destructuring_binding_names_distinguished() {
    // 声明侧：解构绑定名、键、rest 与嵌套结构全量计入精确键。
    assert_ne!(compiled("var [a, b] = [1, 2]"), compiled("var [p, q] = [1, 2]"));
    assert_ne!(compiled("var {x: a} = o"), compiled("var {y: b} = o"));
    assert_ne!(compiled("var [a, ...r] = o"), compiled("var [a, ...s] = o"));
    assert_ne!(
        compiled("var [{p: a}, b] = o"),
        compiled("var [a, b] = o"),
        "nested pattern structure must be part of the hash"
    );
    // 默认值侧：默认值表达式计入哈希。
    assert_ne!(compiled("var [a = f()] = o"), compiled("var [a = g()] = o"));

    // 赋值目标侧：解构赋值左值整体不得漏哈希。
    assert_ne!(compiled("[a, b] = [1, 2]"), compiled("[p, q] = [1, 2]"));
    assert_ne!(compiled("({a} = o)"), compiled("({b} = o)"));

    // for 头侧：声明形态与赋值形态的解构头都计入哈希。
    assert_ne!(compiled("for (var [a, b] of xs) {}"), compiled("for (var [p, q] of xs) {}"));
    assert_ne!(compiled("for ({a} of xs) {}"), compiled("for ({b} of xs) {}"));

    // 函数参数与 catch 侧：解构参数名计入哈希。
    assert_ne!(compiled("function f([a, b]) {}"), compiled("function f([p, q]) {}"));
    assert_ne!(compiled("try {} catch ([a]) {}"), compiled("try {} catch ([p]) {}"));

    // 同源等价回归。
    assert_eq!(compiled("var [a, b] = [1, 2]"), compiled("var [a, b] = [1, 2]"));
}

#[test]
fn structural_hash_ignores_binding_names() {
    // 结构哈希忽略绑定名：仅参数/变量改名后哈希相等。
    assert_eq!(structural("function f(x){ return x }"), structural("function f(y){ return y }"));
    assert_eq!(structural("var a = 1; a + a"), structural("var b = 1; b + b"));
    // 编译模块哈希纳入绑定名：改名的程序哈希不等。
    assert_ne!(compiled("function f(x){ return x }"), compiled("function f(y){ return y }"));
}

#[test]
fn function_body_directives_distinguished() {
    // 嵌套函数体 directives 决定其严格模式：不入 hash 时 strict 标志随缓存键错配。
    assert_ne!(
        compiled("function f(){ 'use strict'; return 1 }"),
        compiled("function f(){ return 1 }"),
        "function declaration body directive must be part of the hash"
    );
    assert_ne!(
        compiled("var f = function(){ 'use strict'; return 1 }"),
        compiled("var f = function(){ return 1 }"),
        "function expression body directive must be part of the hash"
    );
    assert_ne!(
        compiled("var f = () => { 'use strict'; return 1 }"),
        compiled("var f = () => { return 1 }"),
        "arrow function body directive must be part of the hash"
    );
    assert_ne!(
        compiled("class A { m(){ 'use strict'; return 1 } }"),
        compiled("class A { m(){ return 1 } }"),
        "class method body directive must be part of the hash"
    );
    assert_ne!(
        compiled("var o = { m(){ 'use strict'; return 1 } }"),
        compiled("var o = { m(){ return 1 } }"),
        "object literal method body directive must be part of the hash"
    );
    // 非 prologue 位置的字符串字面量是普通语句，不构成 directive，哈希不混同。
    assert_eq!(
        compiled("function f(){ 'not-a-directive'; return 1 }"),
        compiled("function f(){ 'not-a-directive'; return 1 }"),
        "identical sources must hash equally"
    );
}

#[test]
fn function_async_generator_flags_distinguished() {
    // 函数声明四形态：body 结构相同（return 1），仅标志不同，键必须互异。
    // 修复前 async/generator 不入 hash → 四键全同 → 下方 assert_ne 全红。
    let sync = compiled("function f(){ return 1 }");
    let async_fn = compiled("async function f(){ return 1 }");
    let gen = compiled("function* f(){ return 1 }");
    let async_gen = compiled("async function* f(){ return 1 }");
    assert_ne!(sync, async_fn, "sync vs async function declaration");
    assert_ne!(sync, gen, "sync vs generator declaration");
    assert_ne!(sync, async_gen, "sync vs async generator declaration");
    assert_ne!(async_fn, gen, "async vs generator declaration");
    assert_ne!(async_fn, async_gen, "async vs async generator declaration");
    assert_ne!(gen, async_gen, "generator vs async generator declaration");

    // 函数表达式（含对象字面量方法经 FunctionExpression 分支的覆盖路径）
    assert_ne!(
        compiled("var f = function(){ return 1 }"),
        compiled("var f = async function(){ return 1 }"),
        "function expression async flag"
    );
    assert_ne!(
        compiled("var f = function(){ return 1 }"),
        compiled("var f = function*(){ return 1 }"),
        "function expression generator flag"
    );

    // 箭头函数（仅 async 标志；箭头无 generator）
    assert_ne!(compiled("var f = () => 1"), compiled("var f = async () => 1"), "arrow async flag");

    // 类方法（kind 已入 hash，但 async/generator 与 kind 正交）
    assert_ne!(compiled("class A { m(){ return 1 } }"), compiled("class A { async m(){ return 1 } }"));
    assert_ne!(compiled("class A { m(){ return 1 } }"), compiled("class A { *m(){ return 1 } }"));
    assert_ne!(compiled("class A { m(){ return 1 } }"), compiled("class A { async *m(){ return 1 } }"));

    // 对象字面量方法
    assert_ne!(
        compiled("var o = { m(){ return 1 } }"),
        compiled("var o = { async m(){ return 1 } }"),
        "object literal method async flag"
    );

    // 同源等价回归
    assert_eq!(compiled("async function f(){ return 1 }"), compiled("async function f(){ return 1 }"));
}

#[test]
fn template_segment_values_distinguished() {
    // 模板段值是字节码依赖（发射侧把 cooked 写入常量池）：段值不入哈希时
    // 同键异码，缓存命中复用错误字节码。
    assert_ne!(compiled("`${a}x`"), compiled("`${a}y`"));
    assert_ne!(compiled("t`${a}x`"), compiled("t`${a}y`"));
    // 段值在两种哈希粒度下都计入（非绑定名输入）。
    assert_ne!(structural("`${a}x`"), structural("`${a}y`"));
    // 同源等价回归。
    assert_eq!(compiled("`${a}x`"), compiled("`${a}x`"));
}

#[test]
fn call_spread_arguments_distinguished() {
    assert_ne!(compiled("f(...a)"), compiled("f(...b)"));
    assert_ne!(compiled("new F(...a)"), compiled("new F(...b)"));
    assert_ne!(compiled("o.m(...a)"), compiled("o.m(...b)"));
    // spread 与非 spread 同长同表达式须区分（逐位形态标记）。
    assert_ne!(compiled("f(a, ...b)"), compiled("f(a, b)"));
    // spread 形态标记在两种哈希粒度下都计入（标记本身非绑定名输入）。
    assert_ne!(structural("f(...a, 1)"), structural("f(a, 1)"));
}

#[test]
fn array_spread_elements_distinguished() {
    // spread 元素与值元素同位置须区分（形态标记逐位计入）。
    assert_ne!(compiled("[...a]"), compiled("[a]"), "spread vs value element must differ");
    assert_ne!(
        structural("[...a, 1]"),
        structural("[a, 1]"),
        "spread element shape must be in both granularities"
    );
}

#[test]
fn assignment_target_member_names_distinguished() {
    // 赋值目标静态成员属性名是字节码依赖（发射侧写入常量池）：属性名不入哈希时
    // 同键异码。覆盖赋值、自增减与 for 头三个调用点。
    assert_ne!(compiled("a.x = 1"), compiled("a.y = 1"));
    assert_ne!(compiled("a.x++"), compiled("a.y++"));
    assert_ne!(compiled("for (a.x of iter) {}"), compiled("for (a.y of iter) {}"));
    // 属性名在两种哈希粒度下都计入。
    assert_ne!(structural("a.x = 1"), structural("a.y = 1"));
}
