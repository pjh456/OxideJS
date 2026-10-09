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

// 规范（ECMA-262 `with` 写解析）：with 对象无该属性且名字未在静态作用域
// 声明时，写落隐式全局——sloppy 物化全局属性；对象命中臂与已声明绑定的
// 回退分支行为不变。严格模式禁止 `with` 语句（解析期 SyntaxError），
// 故回退臂的 strict 抛错分支仅经静态写路径可达。

#[test]
fn with_fallback_undeclared_write_goes_to_implicit_global() {
    assert_eq!(
        eval("var o={}; with(o){x=42;} globalThis.x===42"),
        "true",
        "sloppy with 回退未声明名写物化隐式全局属性"
    );
}

#[test]
fn with_hit_write_stays_on_object() {
    assert_eq!(
        eval("var o={x:1}; with(o){x=42;} o.x===42 && globalThis.x===undefined"),
        "true",
        "with 对象命中臂写对象属性，不落全局"
    );
}

#[test]
fn with_fallback_declared_var_writes_outer_binding() {
    assert_eq!(
        eval(
            "(function(){var o={}, y=0; with(o){y=1;} \
             return y===1 && globalThis.y===undefined;})()"
        ),
        "true",
        "with 回退已声明 var 写外层绑定，不落全局"
    );
}

#[test]
fn with_fallback_compound_undeclared_write_keeps_existing_behavior() {
    // 复合赋值回退臂走静态复合路径：未声明名旧值按 undefined 读
    // （GetBaseValue 语义不抛），复合运算结果落隐式全局。
    assert_eq!(
        eval("var o={}; with(o){x+=1;} isNaN(globalThis.x)"),
        "true",
        "with 回退未声明名复合赋值保持既有读 undefined 行为"
    );
}
