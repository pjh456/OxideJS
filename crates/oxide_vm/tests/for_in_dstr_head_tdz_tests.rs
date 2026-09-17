//! for-in/for-of 解构声明头的引擎侧形态锁：解构头绑定发射（for-in 支持缺口）
//! 与解构默认值表达式的 TDZ 窗口（默认值求值早于体区 fresh cell 创建）。
//!
//! 完成值一律收敛为布尔（JsValue 的 Display 只暴露 number/bool，不暴露
//! 字符串内容），字符串比较在引擎内完成。

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

// ── for-in 解构声明头绑定 ──

#[test]
fn for_in_array_pattern_head_binds_each_key() {
    // 数组 pattern 对迭代键字符串解构：单字符键首元素即键本身。
    assert_eq!(
        eval("var r; for (let [x] in {i:1}) { r = x; } r === \"i\""),
        "true",
        "for-in array pattern head binds the iteration key"
    );
}

#[test]
fn for_in_object_pattern_head_compiles() {
    // 对象 pattern 头：编译通过且迭代体执行一次（属性值受原始字符串装箱面限制，
    // 只钉绑定发射不报编译错与单次迭代）。
    assert_eq!(
        eval("var n = 0; for (let {length: L} in {abc:1}) { n += 1; } n === 1"),
        "true",
        "for-in object pattern head compiles and iterates once"
    );
}

#[test]
fn for_in_dstr_head_body_closure_shadows_outer() {
    // 体闭包捕获头绑定（值为末迭代键）；外层同名 let 不受影响。
    assert_eq!(
        eval("let x = \"outside\"; var p; for (let [x] in {i:1}) { p = function(){ return x; }; } p() === \"i\" && x === \"outside\""),
        "true",
        "body closure captures the destructured head binding, outer name unchanged"
    );
}

#[test]
fn for_in_dstr_head_default_closure_reads_bound_key() {
    // 先绑定的叶在默认值表达式中被闭包捕获：后续调用读到本迭代键。
    assert_eq!(
        eval("var pd; for (let [x, _ = pd = function(){ return x; }] in {i:1}) {} pd() === \"i\""),
        "true",
        "default value closure reads the already-bound leading element"
    );
}

#[test]
fn for_in_dstr_head_scope_head_lex_close_shape() {
    // scope-head-lex-close 三段断言：右值区闭包读头名抛 ReferenceError；
    // 声明默认值闭包与体闭包读本迭代键。
    assert_eq!(
        eval(
            "let x = \"outside\"; var pd, pe, pb; \
             for (let [x, _ = pd = function(){ return x; }] in { i: pe = function(){ typeof x; } }) \
               pb = function(){ return x; }; \
             var ok = false; \
             try { pe(); } catch (e) { ok = (e instanceof ReferenceError); } \
             ok && pd() === \"i\" && pb() === \"i\" && x === \"outside\""
        ),
        "true",
        "for-in destructuring head TDZ: RHS closure throws, declaration/body closures bind the key"
    );
}

#[test]
fn for_in_nested_dstr_head_binds_inner_elements() {
    // 嵌套 pattern 头：外层键解构后内层叶继续解构。
    assert_eq!(
        eval("var r; for (let [a, [b]] in {xy:1}) { r = a + b; } r === \"xy\""),
        "true",
        "nested destructuring head binds inner elements from the key"
    );
}

// ── 解构默认值窗口的 TDZ（体区 cell 由绑定点前的未初始化占位保护）──

#[test]
fn for_of_dstr_default_self_read_throws_reference_error() {
    // `[x = x]`：默认值读本叶头名，体区 cell 尚未初始化，抛 ReferenceError。
    assert_eq!(
        eval("var r = 0; try { for (let [x = x] of [[]]) {} r = 2; } catch (e) { r = (e instanceof ReferenceError) ? 1 : 3; } r === 1"),
        "true",
        "for-of default value reading its own leaf throws ReferenceError"
    );
}

#[test]
fn for_of_object_dstr_default_self_read_throws_reference_error() {
    // 对象 pattern 默认值直读本叶：同上 TDZ 面。
    assert_eq!(
        eval("var r = 0; try { for (let {x = x} of [{}]) {} r = 2; } catch (e) { r = (e instanceof ReferenceError) ? 1 : 3; } r === 1"),
        "true",
        "for-of object pattern default self-read throws ReferenceError"
    );
}

#[test]
fn for_of_dstr_default_later_leaf_self_read_throws_reference_error() {
    // 后叶默认值读后叶名：前叶已绑定不影响后叶的未初始化状态。
    assert_eq!(
        eval("var r = 0; try { for (let [a, b = b] of [[1]]) {} r = 2; } catch (e) { r = (e instanceof ReferenceError) ? 1 : 3; } r === 1"),
        "true",
        "for-of later-leaf default self-read throws ReferenceError"
    );
}

#[test]
fn for_in_dstr_default_self_read_throws_reference_error() {
    // for-in 空字符串键：数组 pattern 取不到元素，默认值读本叶头名抛 ReferenceError。
    assert_eq!(
        eval("var r = 0; try { for (let [x = x] in {\"\":1}) {} r = 2; } catch (e) { r = (e instanceof ReferenceError) ? 1 : 3; } r === 1"),
        "true",
        "for-in default value reading its own leaf throws ReferenceError"
    );
}

#[test]
fn for_in_dstr_default_bound_leaf_no_throw() {
    // 默认值仅在元素为 undefined 时求值：键为单字符时元素已绑定，不触发 TDZ。
    assert_eq!(
        eval("var r; for (let [x = x] in {i:1}) { r = x; } r === \"i\""),
        "true",
        "for-in default not evaluated when the element is defined"
    );
}

// ── 绿守卫：既有语义不被破坏 ──

#[test]
fn for_of_simple_lexical_head_per_iteration_closure() {
    // 简单 for-of 词法头每迭代新绑定：体内闭包读末值。
    assert_eq!(
        eval("var p; for (let x of [1,2]) { p = function(){ return x; }; } p() === 2"),
        "true",
        "for-of simple lexical head keeps per-iteration bindings"
    );
}

#[test]
fn for_in_simple_lexical_head_per_iteration_closure() {
    // 简单 for-in 词法头每迭代新绑定：体内闭包读末迭代键。
    assert_eq!(
        eval("var p; for (let x in {a:1,b:2}) { p = function(){ return x; }; } p() === \"b\""),
        "true",
        "for-in simple lexical head keeps per-iteration bindings"
    );
}

#[test]
fn for_in_dstr_var_head_no_lexical_tdz() {
    // var 解构头无 TDZ 环境：默认值求值不被未初始化 cell 拦截。
    assert_eq!(
        eval("var r; for (var [x = \"d\"] in {\"\":1}) { r = x; } r === \"d\""),
        "true",
        "var destructuring head has no lexical TDZ"
    );
}
