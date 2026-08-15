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
    let si = vm.kernel_core().perm_interner().intern(name).0;
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

/// NEW_OBJECT 字面量必须登记进 epoch 追踪表：`{}` 写入超内联容量的属性会分配
/// hash_props 堆 Box，reset 时经追踪表统一释放；不登记则每个字面量泄漏一个 Box。
#[test]
fn new_object_literal_tracked_and_freed_on_reset() {
    let mut vm = Vm::new();
    let result = run_source(
        &mut vm,
        "var o = {}; for (var i = 0; i < 40; i++) { o['k' + i] = i; } \
         delete o.k7; o.k8",
    );
    assert_eq!(format!("{}", result), "8", "属性写入/删除后读取应正常");
    assert!(vm.epoch_object_count() > 0, "NEW_OBJECT 应登记 epoch 追踪表");

    let freed_before = vm.session_gc_stats().total_bytes_freed;
    vm.reset();
    assert!(
        vm.session_gc_stats().total_bytes_freed > freed_before,
        "reset 应释放字面量对象的属性向量堆数据"
    );
    assert_eq!(vm.epoch_object_count(), 0, "reset 后追踪表应清空");
}

/// full_reset 同样释放 NEW_OBJECT 字面量：带属性写的 epoch 源对象经 promote 深拷贝
/// 到 session 后，完全重置同时释放源对象与克隆的堆数据，不得双重释放。
#[test]
fn new_object_literal_freed_by_full_reset() {
    let mut vm = Vm::new();
    let result = run_source(
        &mut vm,
        "var o = {}; for (var i = 0; i < 40; i++) { o['k' + i] = i; } \
         globalThis.keep = o; o.k7",
    );
    assert_eq!(format!("{}", result), "7", "逃逸后属性读取应正常");
    assert!(vm.epoch_object_count() > 0, "字面量源对象在 promote 后仍登记于 epoch 追踪表");
    assert!(vm.session_object_count() > 0, "逃逸克隆应登记于 session 追踪表");

    vm.full_reset();
    assert_eq!(vm.epoch_object_count(), 0, "full_reset 后 epoch 追踪表应清空");
    assert_eq!(vm.session_object_count(), 0, "full_reset 后 session 追踪表应清空");
    assert!(
        run_source(&mut vm, "globalThis.keep").is_undefined(),
        "full_reset 后逃逸对象不可再访问"
    );
}
