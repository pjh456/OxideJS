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
fn structural_hash_ignores_binding_names() {
    // 结构哈希忽略绑定名：仅参数/变量改名后哈希相等。
    assert_eq!(structural("function f(x){ return x }"), structural("function f(y){ return y }"));
    assert_eq!(structural("var a = 1; a + a"), structural("var b = 1; b + b"));
    // 编译模块哈希纳入绑定名：改名的程序哈希不等。
    assert_ne!(compiled("function f(x){ return x }"), compiled("function f(y){ return y }"));
}
