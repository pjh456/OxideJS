//! 全局三常量（undefined/NaN/Infinity）描述符与 var 声明交互的回归测试。
//!
//! 覆盖规范全局对象属性描述符 { writable:false, enumerable:false, configurable:true }：
//! 描述符读、delete 真删、delete 后镜像裸读、var 无初始化声明的描述符不漂移与值
//! 保持（writable 不翻位、NaN/Infinity 不被抹成 undefined）、var 初始化声明的值
//! 落槽、枚举面不泄漏、任务只读拦截与 strict TypeError、eval 路径、for-in 枚举面。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

fn run_truthy(source: &str) {
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(compile(source))).expect("run");
    assert!(result.is_bool() && result.as_bool(), "expected true, got: {result:?}\nsource: {source}");
}

fn run_string(source: &str) -> String {
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(compile(source))).expect("run");
    if result.is_undefined() {
        return "undefined".to_string();
    }
    vm.lookup_str(result).unwrap_or_default()
}

/// 求值并返回完成值字符串；运行期异常返回错误文本（供 TypeError 断言）。
fn eval(source: &str) -> String {
    let mut vm = Vm::new();
    match vm.run(&Arc::new(compile(source))) {
        Ok(result) => vm.lookup_str(result).unwrap_or_default(),
        Err(e) => e,
    }
}

const CONSTANTS: [&str; 3] = ["undefined", "NaN", "Infinity"];

#[test]
fn three_constants_descriptor_matches_spec() {
    // #1：描述符 { writable:false, enumerable:false, configurable:true } ×3 常量。
    for name in CONSTANTS {
        let src = format!(
            "var d = Object.getOwnPropertyDescriptor(globalThis, '{name}'); \
             d.writable === false && d.enumerable === false && d.configurable === true"
        );
        run_truthy(&src);
    }
}

#[test]
fn delete_three_constants_removes_property() {
    // #2：configurable → delete 返 true 且属性真删（描述符读为 undefined）×3 常量。
    for name in CONSTANTS {
        let src = format!(
            "delete globalThis.{name} === true \
             && Object.getOwnPropertyDescriptor(globalThis, '{name}') === undefined"
        );
        run_truthy(&src);
    }
}

#[test]
fn delete_then_bare_read_via_mirror() {
    // #3：删后裸读走镜像预载（入口值），无 ReferenceError。
    assert_eq!(run_string("delete globalThis.undefined; typeof undefined"), "undefined");
}

#[test]
fn var_no_init_descriptor_unchanged() {
    // #4：var 无初始化声明——CreateGlobalVarBinding 不更新既有数据描述符
    // （三常量 writable 保 false 不翻位），值幂等保持 ×3 常量。
    for name in CONSTANTS {
        let value_check = match name {
            "NaN" => "Number.isNaN(d.value)",
            "Infinity" => "d.value === Infinity",
            _ => "d.value === undefined",
        };
        let src = format!(
            "var {name}; var d = Object.getOwnPropertyDescriptor(globalThis, '{name}'); \
             {value_check} && d.writable === false && d.enumerable === false \
             && d.configurable === true",
        );
        run_truthy(&src);
    }
}

#[test]
fn var_no_init_preserves_builtin_values() {
    // #5：var 声明不得把现存值抹成 undefined（clobber 回归点）×NaN/Infinity。
    run_truthy("var NaN; Number.isNaN(globalThis.NaN) && globalThis.NaN !== undefined");
    run_truthy("var Infinity; globalThis.Infinity === Infinity && globalThis.Infinity !== undefined");
}

#[test]
fn var_no_init_builtin_not_enumerable() {
    // #6：描述符保 enumerable:false——不泄漏进 Object.keys（回归点）。
    run_truthy("var NaN; Object.keys(globalThis).includes('NaN') === false");
}

#[test]
fn var_with_init_readonly_intercept_unchanged() {
    // #7：var undefined = 3——只读拦截不变（sloppy 静默 no-op，读仍 undefined）。
    assert_eq!(run_string("var undefined = 3; undefined"), "undefined");
}

#[test]
fn strict_var_readonly_builtin_type_error() {
    // #8：strict 下 var undefined = 3 抛 TypeError（拦截路径不受描述符翻动影响）。
    let result = eval("'use strict'; var undefined = 3;");
    assert!(result.contains("TypeError"), "expected TypeError, got: {result}");
}

#[test]
fn var_builtin_object_descriptor_kept() {
    // #9：var Math 无初始化——值与描述符均不漂移（Math 对象保留，e/c 不变）。
    run_truthy(
        "var Math; var d = Object.getOwnPropertyDescriptor(globalThis, 'Math'); \
         d.value === Math && d.writable === true && d.enumerable === false \
         && d.configurable === true",
    );
}

#[test]
fn var_builtin_object_with_init() {
    // #10：var Math = 6——值落槽、描述符保 e:false/c:true、不泄漏进枚举。
    run_truthy(
        "var Math = 6; var d = Object.getOwnPropertyDescriptor(globalThis, 'Math'); \
         d.value === 6 && d.writable === true && d.enumerable === false \
         && d.configurable === true && Object.keys(globalThis).includes('Math') === false",
    );
}

#[test]
fn eval_var_readonly_builtin() {
    // #11：eval var 路径（0x98 判 writable 而非 configurable）——sloppy no-op、
    // strict（eval 脚本内）TypeError，描述符不变。
    assert_eq!(run_string("eval('var undefined = 3'); undefined"), "undefined");
    let strict = eval("eval('\"use strict\"; var undefined = 3')");
    assert!(strict.contains("TypeError"), "expected TypeError, got: {strict}");
    run_truthy(
        "eval('var undefined = 3'); \
         var d = Object.getOwnPropertyDescriptor(globalThis, 'undefined'); \
         d.writable === false && d.configurable === true",
    );
}

#[test]
fn eval_new_name_still_configurable() {
    // #12：eval var 新名属性 configurable:true（0x98 新建分支不变）。
    run_truthy(
        "eval('var __g78x = 1'); \
         Object.getOwnPropertyDescriptor(globalThis, '__g78x').configurable === true",
    );
}

#[test]
fn for_in_does_not_enumerate_three_constants() {
    // #13：for-in 仅 enumerable 面——三常量 e:false 不泄漏（不变）。
    run_truthy(
        "var ks = []; for (var k in globalThis) ks.push(k); \
         (ks.includes('undefined') || ks.includes('NaN') || ks.includes('Infinity')) === false",
    );
}

#[test]
fn control_shapes_unchanged() {
    // #14：对照形——delete Math（基线即 c:true）与 typeof NaN 不变。
    run_truthy("delete globalThis.Math === true");
    assert_eq!(run_string("typeof NaN"), "number");
}

#[test]
fn function_declaration_builtin_name() {
    // 边界：function Math(){}——顶层函数声明取函数值（规范 var 绑定语义），
    // 描述符保 e:false/c:true、值可调用。
    run_truthy(
        "function Math(){ return 1; } \
         var d = Object.getOwnPropertyDescriptor(globalThis, 'Math'); \
         Math() === 1 && d.value === Math && d.writable === true \
         && d.enumerable === false && d.configurable === true",
    );
}

#[test]
fn for_in_var_head_builtin_name_compiles_and_keeps_property() {
    // 顶层 for-in var 头撞 builtin 名：序言预登记槽复用后可编译，
    // 迭代键不触及全局属性（Math 对象保）。
    run_truthy("var m = Math; for (var Math in {a:1,b:2}) {} globalThis.Math === m");
}

#[test]
fn for_in_var_head_readonly_builtin_not_clobbered() {
    // 顶层 for-in var 头撞三常量：迭代键写命中只读内置拦截——
    // sloppy 静默跳过（镜像槽与声明值均不污染），strict 迭代抛 TypeError。
    run_truthy("var n = NaN; for (var NaN in {a:1,b:2}) {} Number.isNaN(NaN) && Number.isNaN(n)");
    run_truthy(
        "var i = Infinity; for (var Infinity in {a:1,b:2}) {} Infinity === i && Infinity === globalThis.Infinity",
    );
    run_truthy("\"use strict\"; try { for (var NaN in {a:1}) {} } catch (e) { e instanceof TypeError }");
}
