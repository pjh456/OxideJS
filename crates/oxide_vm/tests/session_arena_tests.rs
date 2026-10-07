use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn run_source(vm: &mut Vm, source: &str) -> JsValue {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    vm.run(&Arc::new(module)).expect("vm run failed")
}

fn global_prop(vm: &Vm, name: &str) -> JsValue {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    let session = vm.session();
    let global = session.global_object();
    let pos = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(global.shape_id(), si)
        .expect("global property slot");
    global.get_prop_at(pos)
}

/// 低阈值 VM：`reset()` 的 session 收集受阈值门控（默认 32MiB），小负载须把
/// 阈值压到 1 字节才能让 reset 真正回收，断言「reset 后追踪表清空/字节释放」
/// 的测试用此构造。
fn low_threshold_vm() -> Vm {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(1);
    Vm::with_kernel_core(KernelCore::new(config))
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

/// NEW_OBJECT 字面量必须登记进统一追踪表：`{}` 写入超内联容量的属性会分配
/// hash_props 堆 Box，收集路径经追踪表统一释放；不登记则每个字面量泄漏一个 Box。
/// 对象变量置于函数作用域：顶层 var 在声明写即逃逸 promote，后续属性写落
/// session 克隆（其堆数据不属源对象，reset 不释放）。
#[test]
fn new_object_literal_tracked_and_freed_on_reset() {
    let mut vm = low_threshold_vm();
    let result = run_source(
        &mut vm,
        "(function(){var o = {}; for (var i = 0; i < 40; i++) { o['k' + i] = i; } \
         delete o.k7; return o.k8;})()",
    );
    assert_eq!(format!("{}", result), "8", "属性写入/删除后读取应正常");
    assert!(vm.session_object_count() > 0, "NEW_OBJECT 应登记统一追踪表");

    vm.reset();
    assert_eq!(vm.session_object_count(), 0, "reset 后追踪表应清空");
}

/// full_reset 同样释放 NEW_OBJECT 字面量：带属性写的源对象经逃逸写直通
/// 留存 global 槽（不克隆），完全重置随 global 重建丢弃槽位并释放源对象堆
/// 数据，不得悬垂。
#[test]
fn new_object_literal_freed_by_full_reset() {
    let mut vm = Vm::new();
    let result = run_source(
        &mut vm,
        "var o = {}; for (var i = 0; i < 40; i++) { o['k' + i] = i; } \
         globalThis.keep = o; o.k7",
    );
    assert_eq!(format!("{}", result), "7", "逃逸后属性读取应正常");
    assert!(vm.session_object_count() > 0, "字面量源对象直通留存 global 槽，登记于统一追踪表");
    let keep = global_prop(&vm, "keep");
    // SAFETY: 逃逸值是本 VM 自有的 session 对象，本 session 内指针有效。
    assert!(
        unsafe { (&*keep.as_js_object_ptr()).is_session_epoch() },
        "逃逸值应直通留存 global 槽，不克隆进 session"
    );

    vm.full_reset();
    assert_eq!(vm.session_object_count(), 0, "full_reset 后统一追踪表应清空");
    assert!(run_source(&mut vm, "globalThis.keep").is_undefined(), "full_reset 后逃逸对象不可再访问");
}

/// regexp exec 结果数组必须登记统一追踪表：数组元素区 Box 与 index/input 命名
/// 属性 hash_props Box 随收集路径统一释放；未登记则每次 exec/Symbol.match 泄漏。
#[test]
fn regexp_exec_result_array_tracked_and_freed_on_reset() {
    let mut vm = low_threshold_vm();
    let result = run_source(
        &mut vm,
        "for (var i = 0; i < 20; i++) { /a(b)?/.exec('ab'); } \
         var r = /a(b)?/.exec('ab'); r.length",
    );
    assert_eq!(format!("{}", result), "2", "exec 结果数组长度应正常");
    assert!(vm.session_object_count() > 0, "exec 结果数组应登记统一追踪表");

    vm.reset();
    // r 是顶层 var（global 根），reset 后存活；20 个临时 exec 结果数组已释放。
    assert_eq!(vm.session_object_count(), 1, "reset 后仅 global 根 r 存活");
}

/// split/match 结果数组必须登记统一追踪表：元素区 Box 随收集路径统一释放；
/// 未登记则每次 split/match 泄漏一个 Box。
#[test]
fn string_split_match_result_array_tracked_and_freed_on_reset() {
    let mut vm = low_threshold_vm();
    let result = run_source(
        &mut vm,
        "for (var i = 0; i < 20; i++) { 'a,b,c'.split(','); 'ab'.match(/a/); } \
         'a,b,c'.split(',').length",
    );
    assert_eq!(format!("{}", result), "3", "split 结果数组长度应正常");
    assert!(vm.session_object_count() > 0, "split/match 结果数组应登记统一追踪表");

    vm.reset();
    assert_eq!(vm.session_object_count(), 0, "reset 后追踪表应清空");
}

/// 错误对象必须登记统一追踪表：message 非空时 push_prop 分配 hash_props Box，
/// 随收集路径统一释放；未登记则每次抛错（全引擎最频繁路径）泄漏一个 Box。
/// 同时覆盖构造器路径（`new TypeError`）与内部抛错路径（builtin 内 create_type_error）。
#[test]
fn error_object_tracked_and_freed_on_reset() {
    let mut vm = low_threshold_vm();
    let result = run_source(
        &mut vm,
        "for (var i = 0; i < 20; i++) { \
           try { throw new TypeError('boom'); } catch (e) {} \
           try { Error.prototype.toString.call(1); } catch (e) {} \
         } \
         try { throw new TypeError('boom'); } catch (e) { e.message }",
    );
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "boom", "错误对象 message 应正常");
    assert!(vm.session_object_count() > 0, "错误对象应登记统一追踪表");

    vm.reset();
    assert_eq!(vm.session_object_count(), 0, "reset 后追踪表应清空");
}

/// gOPD/Reflect 描述符对象必须登记统一追踪表：4 个描述符属性 push 分配
/// hash_props Box，随收集路径统一释放；未登记则每次 getOwnPropertyDescriptor 泄漏。
#[test]
fn gopd_descriptor_tracked_and_freed_on_reset() {
    let mut vm = low_threshold_vm();
    let result = run_source(
        &mut vm,
        "for (var i = 0; i < 20; i++) { \
           Object.getOwnPropertyDescriptor({x:1,y:2,z:3,w:4}, 'x'); \
         } \
         Object.getOwnPropertyDescriptor({x:5}, 'x').value",
    );
    assert_eq!(format!("{}", result), "5", "gOPD 描述符值应正常");
    assert!(vm.session_object_count() > 0, "描述符对象应登记统一追踪表");

    vm.reset();
    assert_eq!(vm.session_object_count(), 0, "reset 后追踪表应清空");
}
