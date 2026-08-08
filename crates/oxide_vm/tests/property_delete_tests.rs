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

// ── 删除可配置属性成功 ──
#[test]
fn delete_configurable_property_succeeds_and_returns_true() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, configurable: true})",
        "delete obj.x",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete configurable should return true, got {:?}", r);
}

// ── 删除不可配置属性：sloppy 模式返回 false（不抛错）──
#[test]
fn delete_non_configurable_returns_false_in_sloppy_mode() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, configurable: false})",
        "delete obj.x",
    ])
    .unwrap();
    assert!(r.is_bool() && !r.as_bool(), "delete non-configurable in sloppy should return false, got {:?}", r);
}

// ── 删除不存在的属性返回 true ──
#[test]
fn delete_non_existent_returns_true() {
    let r = eval_many(&["var obj = {}", "delete obj.x"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete non-existent should return true, got {:?}", r);
}

// ── 删除默认（无显式 defineProperty）可配置属性成功 ──
#[test]
fn delete_default_configurable_succeeds() {
    let r = eval_many(&["var obj = {}", "obj.x = 1", "delete obj.x"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete default configurable should return true, got {:?}", r);
}

// ── 子对象删除原型上不可配置的属性：子对象不拥有它，因此返回 true ──
#[test]
fn delete_non_configurable_proto_property_on_child_returns_true() {
    let r = eval_many(&[
        "var proto = {}",
        "proto.x = 1",
        "Object.defineProperty(proto, 'x', {configurable: false})",
        "var child = Object.create(proto)",
        "delete child.x",
    ])
    .unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "delete on child for non-configurable proto prop should return true (not own), got {:?}",
        r
    );
}

// -- Delete configurable proto-level property through child --
#[test]
fn delete_configurable_proto_property_on_child_returns_true() {
    let r = eval_many(&[
        "var proto = {}",
        "Object.defineProperty(proto, 'x', {value: 1, configurable: true, enumerable: true})",
        "var child = Object.create(proto)",
        "delete child.x",
    ])
    .unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "delete configurable proto prop from child should return true (not own), got {:?}",
        r
    );
}
