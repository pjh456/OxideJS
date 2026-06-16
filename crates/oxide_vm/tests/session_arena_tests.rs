use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn run_source(vm: &mut Vm, source: &str) -> JsValue {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    vm.run(&module).expect("vm run failed")
}

fn global_prop(vm: &Vm, name: &str) -> JsValue {
    let si = vm.kernel_core().string_forge().intern(name).0;
    let global = vm.session().global_object();
    let pos = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(global.shape_id(), si)
        .expect("global property slot");
    global.get_prop_at(pos)
}

#[test]
fn test_basic_object_escape() {
    let mut vm = Vm::new();
    run_source(&mut vm, "globalThis.x = {}; globalThis.x.y = 1");
    vm.reset();

    let result = run_source(&mut vm, "globalThis.x.y");

    assert_eq!(result, JsValue::int(1));
}

#[test]
fn test_array_escape() {
    let mut vm = Vm::new();
    run_source(&mut vm, "globalThis.a = []; globalThis.a.push({ v: 2 })");
    vm.reset();

    let result = run_source(
        &mut vm,
        "var i = 0; while (i < 1000) { var tmp = { v: i }; i = i + 1; } globalThis.a[0].v",
    );

    assert_eq!(result, JsValue::int(2));
}

#[test]
fn test_closure_captured_this_escape() {
    let mut vm = Vm::new();
    run_source(&mut vm, "globalThis.marker = { v: 9 }; globalThis.f = () => this.marker.v");
    vm.reset();

    let function = run_source(&mut vm, "globalThis.f");
    let result = run_source(&mut vm, "globalThis.marker.v");
    let function_obj = unsafe { &*function.as_js_object_ptr() };

    assert!(function.is_object());
    assert!(function_obj.is_session_epoch());
    assert_eq!(global_prop(&vm, "f"), function);
    assert_eq!(result, JsValue::int(9));
}

#[test]
fn test_transitive_escape() {
    let mut vm = Vm::new();
    run_source(&mut vm, "globalThis.root = {}");
    vm.reset();
    run_source(&mut vm, "globalThis.root.child = { v: 3 }");
    vm.reset();

    let result = run_source(&mut vm, "globalThis.root.child.v");

    assert_eq!(result, JsValue::int(3));
}

#[test]
fn test_full_reset_clears_session_state() {
    let mut vm = Vm::new();
    run_source(&mut vm, "globalThis.x = {}; globalThis.x.y = 1");
    vm.reset();
    assert_eq!(run_source(&mut vm, "globalThis.x.y"), JsValue::int(1));

    vm.full_reset();

    let result = run_source(&mut vm, "globalThis.x");
    assert!(result.is_undefined());
}
