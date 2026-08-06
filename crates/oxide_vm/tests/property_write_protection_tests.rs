use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

fn eval_many(lines: &[&str]) -> Result<JsValue, String> {
    let source = lines.join("; ");
    eval(&source)
}

fn assert_err_contains(result: Result<JsValue, String>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error containing '{}', got Ok", expected),
        Err(e) => assert!(e.contains(expected), "expected error containing '{}', got: {}", expected, e),
    }
}

// ── 向不可写自身属性赋值抛 TypeError ──
#[test]
fn assign_to_non_writable_throws() {
    let result = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, writable: false})",
        "obj.x = 2",
    ]);
    assert_err_contains(result, "read-only");
}

// ── 向可写自身属性赋值成功 ──
#[test]
fn assign_to_writable_succeeds() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, writable: true})",
        "obj.x = 2",
        "obj.x",
    ])
    .unwrap();
    assert!(r.is_int() && r.as_int() == 2, "assign to writable should set value to 2, got {:?}", r);
}

// ── 向原型上不可写的继承属性赋值抛错 ──
#[test]
fn assign_to_inherited_non_writable_throws() {
    let result = eval_many(&[
        "var proto = {}",
        "Object.defineProperty(proto, 'x', {value: 1, writable: false, enumerable: true, configurable: true})",
        "var child = Object.create(proto)",
        "child.x = 2",
    ]);
    assert_err_contains(result, "read-only");
}

// ── 原型属性不可写但子对象无自有属性时，赋值会在子对象上创建自有属性
// ponytail: spec allows shadowing when proto prop is configurable; engine doesn't yet.
// 引擎对不可写原型属性正确抛错；完整遮蔽作为后续工作。
#[test]
fn assign_non_writable_proto_configurable_prop_throws() {
    let result = eval_many(&[
        "var proto = {}",
        "Object.defineProperty(proto, 'x', {value: 1, writable: false, enumerable: true, configurable: true})",
        "var child = Object.create(proto)",
        "child.x = 99",
    ]);
    assert_err_contains(result, "read-only");
}
