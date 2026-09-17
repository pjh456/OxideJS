//! session_gc 内联测试：GC 根集与标记清扫行为、字符串与 rope 及闭包单元收集、字节账目、运行期收集、Promise 晋升迁移行为。
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use std::sync::Arc;

use super::*;
use crate::vm::{CallFrame, FrameContinuation};
use oxide_builtins::{array_buffer, data_view, disposable_stack, map, set, typed_array};
use oxide_runtime_api::NativeResult;

fn plain_object(vm: &mut Vm) -> *mut JsObject {
    let proto_ptr = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
    let ptr = vm.epoch.alloc(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto_ptr),
    ));
    // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
    unsafe { (*ptr).set_is_epoch(true) };
    ptr
}

fn has_ptr(roots: &[JsValue], ptr: *mut JsObject) -> bool {
    roots
        .iter()
        .any(|value| value.is_object() && std::ptr::eq(value.as_js_object_ptr(), ptr))
}

#[test]
fn uncaught_value_is_gc_root() {
    let mut vm = Vm::new();
    let obj = plain_object(&mut vm);
    let session = vm.promote_object(obj);
    vm.last_uncaught_value = Some(JsValue::from_js_object(session));
    let mut roots = Vec::new();
    vm.for_each_root(|v| roots.push(v));
    assert!(has_ptr(&roots, session));
}

#[test]
fn suspended_signal_fields_are_roots() {
    let mut vm = Vm::new();
    let a = plain_object(&mut vm);
    let b = plain_object(&mut vm);
    let c = plain_object(&mut vm);
    let a_s = vm.promote_object(a);
    let b_s = vm.promote_object(b);
    let c_s = vm.promote_object(c);
    vm.generator_suspended = Some(JsValue::from_js_object(a_s));
    vm.async_context = Some(JsValue::from_js_object(b_s));
    vm.async_gen_context = Some(JsValue::from_js_object(c_s));
    let mut roots = Vec::new();
    vm.for_each_root(|v| roots.push(v));
    assert!(has_ptr(&roots, a_s));
    assert!(has_ptr(&roots, b_s));
    assert!(has_ptr(&roots, c_s));
}

#[test]
fn gc_roots_contains_registers_frames_and_root_roots() {
    let mut vm = Vm::new();
    let root = plain_object(&mut vm);
    let frame_obj = plain_object(&mut vm);
    let saved_this = plain_object(&mut vm);
    let child = plain_object(&mut vm);

    unsafe {
        (*root).set_prop_at(0, JsValue::from_js_object(child));
    }

    let root_session = vm.promote_object(root);
    let frame_session = vm.promote_object(frame_obj);
    let this_session = vm.promote_object(saved_this);
    let child_session = vm.promote_object(child);
    vm.regs[0] = JsValue::from_js_object(root_session);

    vm.frames.push(CallFrame {
        return_addr: 0,
        function_name: 0,
        caller_reg_limit: 1,
        caller_active_reg_limit: 1,
        saved_reg_offset: 0,
        spill_offset: 0,
        arguments_base: 0,
        arguments_count: 0,
        saved_this: JsValue::from_js_object(this_session),
        saved_new_target: JsValue::from_js_object(child_session),
        callee: JsValue::from_js_object(child_session),
        construct_result_reg: None,
        strict: false,
        constructed_this: Some(JsValue::from_js_object(child_session)),
        is_derived_constructor: false,
        super_called: false,
        continuation: FrameContinuation::None,
    });
    vm.save_stack.push(JsValue::from_js_object(frame_session));

    vm.regs[1] = JsValue::from_js_object(child_session);
    vm.exception_value = Some(JsValue::from_js_object(root_session));
    vm.pending_exception = Some(JsValue::from_js_object(child_session));
    vm.iters.for_of_iters.push(crate::vm_state::ForOfEntry {
        iterator: JsValue::from_js_object(child_session),
        last_result: JsValue::from_js_object(root_session),
        is_async: false,
    });

    let mut roots = Vec::new();
    vm.for_each_root(|v| roots.push(v));
    assert!(has_ptr(&roots, root_session));
    assert!(has_ptr(&roots, frame_session));
    assert!(has_ptr(&roots, this_session));
    assert!(has_ptr(&roots, child_session));
    assert!(has_ptr(&roots, vm.session.global_object().as_ptr() as *mut JsObject));
    assert!(!roots.is_empty());
    assert!(roots.contains(&JsValue::from_js_object(root_session)));
    assert_eq!(vm.exception_value, Some(JsValue::from_js_object(root_session)));
}

#[test]
fn rewrite_matches_roots_coverage() {
    // 指针重写必须覆盖根收集访问的同一组执行核心字段：session→marker 映射后，
    // 每个持有 session 的字段都应被改写为 marker（与上文基于 for_each 的覆盖
    // 测试同口径，共同构成孪生清单验证）。
    let mut vm = Vm::new();
    let obj = plain_object(&mut vm);
    let session = vm.promote_object(obj);
    let marker = plain_object(&mut vm);
    let marker_session = vm.promote_object(marker);
    vm.regs[0] = JsValue::from_js_object(session);
    vm.last_uncaught_value = Some(JsValue::from_js_object(session));
    vm.generator_suspended = Some(JsValue::from_js_object(session));
    vm.delegated_iterator = Some(JsValue::from_js_object(session));
    vm.async_context = Some(JsValue::from_js_object(session));
    vm.async_gen_context = Some(JsValue::from_js_object(session));
    vm.inline_callee = Some(JsValue::from_js_object(session));
    vm.pending_completion = Some(crate::vm::Completion::Return {
        value: JsValue::from_js_object(session),
        remaining_finally: 0,
        for_of_count: 0,
        for_in_count: 0,
    });

    let mut forwarding = std::collections::HashMap::new();
    forwarding.insert(session, marker_session);
    vm.rewrite_values(|v| {
        if v.is_object() {
            if let Some(&m) = forwarding.get(&v.as_js_object_ptr()) {
                return JsValue::from_js_object(m);
            }
        }
        v
    });
    assert_eq!(vm.regs[0].as_js_object_ptr(), marker_session);
    assert_eq!(vm.last_uncaught_value.unwrap().as_js_object_ptr(), marker_session);
    assert_eq!(vm.generator_suspended.unwrap().as_js_object_ptr(), marker_session);
    assert_eq!(vm.delegated_iterator.unwrap().as_js_object_ptr(), marker_session);
    assert_eq!(vm.async_context.unwrap().as_js_object_ptr(), marker_session);
    assert_eq!(vm.async_gen_context.unwrap().as_js_object_ptr(), marker_session);
    assert_eq!(vm.inline_callee.unwrap().as_js_object_ptr(), marker_session);
    match vm.pending_completion.unwrap() {
        crate::vm::Completion::Return { value, .. } => assert_eq!(value.as_js_object_ptr(), marker_session),
        _ => panic!("expected Return completion"),
    }
}

#[test]
fn mark_phase_reaches_cycles_and_unreachable_are_unmarked() {
    let mut vm = Vm::new();
    let root = plain_object(&mut vm);
    let reachable = plain_object(&mut vm);
    let unreachable = plain_object(&mut vm);
    unsafe {
        (*root).set_prop_at(0, JsValue::from_js_object(reachable));
        (*reachable).set_prop_at(0, JsValue::from_js_object(root));
    }

    let root_session = vm.promote_object(root);
    let reachable_session = unsafe { (*root_session).get_prop_at(0).as_js_object_ptr() };
    let unreachable_session = vm.promote_object(unreachable);

    vm.regs[0] = JsValue::from_js_object(root_session);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.mark(&vm);
    vm.gc_state.session_gc = gc;

    assert!(unsafe { (*root_session).is_gc_marked() });
    assert!(unsafe { (*reachable_session).is_gc_marked() });
    assert!(!unsafe { (*unreachable_session).is_gc_marked() });
    assert_ne!(unreachable_session, root_session);
}

#[test]
fn sweep_preserves_cycle_and_collects_unreachable() {
    let mut vm = Vm::new();
    let root = plain_object(&mut vm);
    let child = plain_object(&mut vm);
    let dead = plain_object(&mut vm);
    unsafe {
        (*root).set_prop_at(0, JsValue::from_js_object(child));
        (*child).set_prop_at(0, JsValue::from_js_object(root));
    }

    let root_session = vm.promote_object(root);
    vm.regs[0] = JsValue::from_js_object(root_session);
    let dead_session = vm.promote_object(dead);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.mark(&vm);
    let _ = gc.sweep(&mut vm);
    vm.gc_state.session_gc = gc;

    assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);
    assert!(!vm.gc_state.session_object_ptrs.contains(&root_session));
    assert!(!vm.gc_state.session_object_ptrs.contains(&dead_session));
    assert!(!vm
        .gc_state
        .session_object_ptrs
        .iter()
        .any(|ptr| unsafe { (*(*ptr)).is_gc_marked() }));
}

#[test]
fn sweep_preserves_array_elements_and_collects_dead_element_object() {
    let mut vm = Vm::new();
    let array_proto = vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.epoch.alloc(JsObject::new_array(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        2,
        vm.epoch.bump(),
    ));
    // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
    unsafe { (*arr).set_is_epoch(true) };
    let live_elem = plain_object(&mut vm);
    let dead_elem = plain_object(&mut vm);
    unsafe {
        (*arr).set_prop_at(0, JsValue::from_js_object(live_elem));
        (*arr).set_prop_at(1, JsValue::from_js_object(dead_elem));
    }
    // 晋升数组会把元素对象一并带入 session；随后断开元素 1 的引用，使其成为
    // mark 不可达的死对象，供 sweep 回收。
    let arr_session = vm.promote_object(arr);
    unsafe {
        (*arr_session).set_prop_at(1, JsValue::undefined());
    }
    vm.regs[0] = JsValue::from_js_object(arr_session);

    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.mark(&vm);
    let _ = gc.sweep(&mut vm);
    vm.gc_state.session_gc = gc;

    // sweep 复制存活对象并把 VM 根改写为新 arena 指针：regs[0] 是数组的新地址，
    // 旧指针已随旧 arena 释放，不可再用。
    let arr_new = vm.regs[0].as_js_object_ptr();
    let live_session = unsafe { (*arr_new).get_prop_at(0).as_js_object_ptr() };
    assert_eq!(unsafe { (*live_session).prop_count() }, 0);
    assert_eq!(unsafe { (*arr_new).prop_count() }, 2);
    assert_eq!(unsafe { (*arr_new).get_prop_at(0).as_js_object_ptr() }, live_session);
    // 元素 1 引用已断开：死元素对象被回收，存活集合只剩数组与元素 0 的对象。
    assert_eq!(unsafe { (*arr_new).get_prop_at(1) }, JsValue::undefined());
    assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);
}

#[test]
fn forwarding_is_cleared_after_sweep() {
    let mut vm = Vm::new();
    let root = plain_object(&mut vm);
    let child = plain_object(&mut vm);
    unsafe {
        (*root).set_prop_at(0, JsValue::from_js_object(child));
    }
    let root_session = vm.promote_object(root);
    vm.regs[0] = JsValue::from_js_object(root_session);
    let _ = vm.promote_object(child);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.mark(&vm);
    let _ = gc.sweep(&mut vm);
    vm.gc_state.session_gc = gc;

    // 复用的 forwarding 表必须在 sweep 后清空，否则 promote 会观察到指向已释放
    // arena 的过期 old->new 条目。
    assert!(vm.gc_state.forwarding.is_empty());
}

fn vm_with_low_threshold() -> Vm {
    let mut cfg = KernelConfig::minimal();
    cfg.set_session_gc_threshold(1);
    let core = KernelCore::new(cfg);
    Vm::with_kernel_core(core)
}

fn native_ok(result: NativeResult) -> JsValue {
    match result {
        NativeResult::Ok(value) => value,
        NativeResult::Err(err) => panic!("native error: {err}"),
        NativeResult::TailCall { .. } => panic!("unexpected native bytecode call"),
    }
}

/// 分配一个原型指向 Map.prototype 的占位对象并写入寄存器，作为构造器调用的 `this`。
fn map_this(vm: &mut Vm, reg: u8) -> JsValue {
    let proto = vm.session.builtin_world().map_proto.as_ptr() as *mut JsObject;
    let obj = vm.epoch.alloc(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
    unsafe { (*obj).set_is_epoch(true) };
    let val = JsValue::from_js_object(obj);
    vm.regs[reg as usize] = val;
    val
}

/// 分配一个原型指向 Set.prototype 的占位对象并写入寄存器，作为构造器调用的 `this`。
fn set_this(vm: &mut Vm, reg: u8) -> JsValue {
    let proto = vm.session.builtin_world().set_proto.as_ptr() as *mut JsObject;
    let obj = vm.epoch.alloc(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
    unsafe { (*obj).set_is_epoch(true) };
    let val = JsValue::from_js_object(obj);
    vm.regs[reg as usize] = val;
    val
}

/// 分配一个原型指向 DisposableStack.prototype 的占位对象并写入寄存器，
/// 作为构造器调用的 `this`。
fn dispose_stack_this(vm: &mut Vm, reg: u8) -> JsValue {
    let proto = vm.session.builtin_world().disposable_stack_proto.as_ptr() as *mut JsObject;
    let obj = vm.epoch.alloc(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
    unsafe { (*obj).set_is_epoch(true) };
    let val = JsValue::from_js_object(obj);
    vm.regs[reg as usize] = val;
    val
}

/// 置函数位标志的占位对象（通过 `is_callable` 判定），供 adopt 的
/// onDispose 槽使用；测试不调用它。
fn function_placeholder(vm: &mut Vm) -> JsValue {
    let proto = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
    let obj = vm.epoch.alloc(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    // 测试辅助函数绕过 alloc_object，需手动置位 EPOCH_BIT。
    unsafe {
        (*obj).set_is_epoch(true);
        (*obj).set_function(true);
    }
    JsValue::from_js_object(obj)
}

#[test]
fn reset_maybe_collect_collects_after_threshold() {
    let mut vm = vm_with_low_threshold();
    let obj = plain_object(&mut vm);
    vm.promote_object(obj);
    assert!(!vm.gc_state.session_object_ptrs.is_empty());
    let tracked_before = vm.gc_state.session_object_ptrs.len();

    vm.regs[0] = JsValue::undefined();
    vm.regs[1] = JsValue::undefined();
    vm.reset();

    assert!(vm.gc_state.session_object_ptrs.len() <= tracked_before);
    assert_eq!(vm.gc_state.session_object_ptrs.len(), 0);
    assert_eq!(vm.gc_state.session_bytes_allocated, 0);
}

#[test]
fn gc_stats_summary_includes_collection() {
    let mut vm = vm_with_low_threshold();
    let obj = plain_object(&mut vm);
    vm.promote_object(obj);
    vm.regs[0] = JsValue::undefined();
    vm.maybe_collect_session_gc();
    let summary = vm.gc_state.session_gc.stats_summary();
    assert!(summary.contains("[GC] collection"));
}

#[test]
fn moving_sweep_rewrites_global_root_edges() {
    let mut vm = vm_with_low_threshold();
    let obj = plain_object(&mut vm);
    unsafe {
        (*obj).set_prop_at(0, JsValue::int(42));
    }
    let old_ptr = vm.promote_object(obj);
    let key = vm.kernel_core.perm_interner().intern("gcRoot").0;
    let global_ptr = vm.session.global_object().as_ptr() as *mut JsObject;
    unsafe {
        let global = &mut *global_ptr;
        vm.set_or_create_prop_value(global, key, JsValue::from_js_object(old_ptr));
    }

    vm.maybe_collect_session_gc();

    let global = vm.session.global_object();
    let pos = vm
        .kernel_core
        .shape_forge()
        .lookup_position(global.shape_id(), key)
        .expect("global slot");
    let new_value = global.get_prop_at(pos);
    assert!(new_value.is_object());
    assert!(!std::ptr::eq(new_value.as_js_object_ptr(), old_ptr));
    assert_eq!(unsafe { (*new_value.as_js_object_ptr()).get_prop_at(0) }, JsValue::int(42));
}

#[test]
fn map_native_storage_is_not_a_normal_object_edge() {
    let mut vm = Vm::new();
    map_this(&mut vm, 1);
    let map_value = native_ok(map::map_constructor(&mut vm, &[1]));
    let map_obj = unsafe { &*map_value.as_js_object_ptr() };
    let native_ptr = map_obj.native_data() as *mut JsObject;

    assert!(map_obj.hash_props_vec().is_none());
    assert!(!map_obj.native_data().is_null());
    // Map 的 native 存储指针不得被当作普通对象边扫描：扫到会把 native 堆数据
    // 误拉进 mark 集合。
    let mut stack = Vec::new();
    let mut live = HashSet::with_hasher(FxBuildHasher);
    let mut live_bigints = HashSet::with_hasher(FxBuildHasher);
    SessionGc::scan_edges_for_mark(map_obj, &vm, &mut stack, &mut live, &mut live_bigints);
    assert!(!stack.iter().any(|&ptr| std::ptr::eq(ptr, native_ptr)));
}

#[test]
fn session_gc_traces_map_object_key_and_value() {
    let mut vm = vm_with_low_threshold();
    map_this(&mut vm, 3);
    let map_value = native_ok(map::map_constructor(&mut vm, &[3]));
    let key = JsValue::from_js_object(plain_object(&mut vm));
    let value = JsValue::from_js_object(plain_object(&mut vm));
    vm.regs[0] = map_value;
    vm.regs[1] = key;
    vm.regs[2] = value;
    native_ok(map::map_set(&mut vm, &[0, 1, 2]));

    let map_session = vm.promote_object(map_value.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(map_session);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.collect(&mut vm);
    vm.gc_state.session_gc = gc;

    let live_map = unsafe { &*vm.regs[0].as_js_object_ptr() };
    let edges = map::map_native_edges(live_map);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|value| vm.is_session_ptr(value.as_js_object_ptr())));
    assert_eq!(vm.gc_state.session_object_ptrs.len(), 3);
}

#[test]
fn session_gc_traces_set_object_key() {
    let mut vm = vm_with_low_threshold();
    set_this(&mut vm, 2);
    let set_value = native_ok(set::set_constructor(&mut vm, &[2]));
    let key = JsValue::from_js_object(plain_object(&mut vm));
    vm.regs[0] = set_value;
    vm.regs[1] = key;
    native_ok(set::set_add(&mut vm, &[0, 1]));

    let set_session = vm.promote_object(set_value.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(set_session);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.collect(&mut vm);
    vm.gc_state.session_gc = gc;

    let live_set = unsafe { &*vm.regs[0].as_js_object_ptr() };
    let edges = set::set_native_edges(live_set);
    assert_eq!(edges.len(), 1);
    assert!(vm.is_session_ptr(edges[0].as_js_object_ptr()));
    assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);
}

/// 盒持唯一引用的 session 串与 BigInt 经 Map native 边进入存活集：
/// 清寄存器仅留 Map 根后完整收集，两值仍登记在 session 表。
#[test]
fn session_gc_traces_map_string_and_bigint_value() {
    let mut vm = vm_with_low_threshold();
    map_this(&mut vm, 3);
    let map_value = native_ok(map::map_constructor(&mut vm, &[3]));
    let str = vm.new_string("map-box-string");
    let str_ptr = str.as_string_ptr_mut();
    let bi = vm.new_bigint(num_bigint::BigInt::from(121932631112635269u128));
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;
    vm.regs[0] = map_value;
    vm.regs[1] = JsValue::int(1);
    vm.regs[2] = str;
    native_ok(map::map_set(&mut vm, &[0, 1, 2]));
    vm.regs[1] = JsValue::int(2);
    vm.regs[2] = bi;
    native_ok(map::map_set(&mut vm, &[0, 1, 2]));

    let map_session = vm.promote_object(map_value.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(map_session);
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_string_ptrs.contains(&str_ptr));
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

/// 盒持唯一引用的 session 串与 BigInt 经 Set native 边进入存活集：
/// 清寄存器仅留 Set 根后完整收集，两值仍登记在 session 表。
#[test]
fn session_gc_traces_set_string_and_bigint_value() {
    let mut vm = vm_with_low_threshold();
    set_this(&mut vm, 2);
    let set_value = native_ok(set::set_constructor(&mut vm, &[2]));
    let str = vm.new_string("set-box-string");
    let str_ptr = str.as_string_ptr_mut();
    let bi = vm.new_bigint(num_bigint::BigInt::from(987654321987654321u128));
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;
    vm.regs[0] = set_value;
    vm.regs[1] = str;
    native_ok(set::set_add(&mut vm, &[0, 1]));
    vm.regs[1] = bi;
    native_ok(set::set_add(&mut vm, &[0, 1]));

    let set_session = vm.promote_object(set_value.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(set_session);
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_string_ptrs.contains(&str_ptr));
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

/// 盒持唯一引用的 session 串与 BigInt 经资源栈条目边进入存活集：
/// adopt 入栈后清寄存器仅留栈根，完整收集后两值仍登记在 session 表。
#[test]
fn session_gc_traces_dispose_stack_string_and_bigint_value() {
    let mut vm = vm_with_low_threshold();
    dispose_stack_this(&mut vm, 3);
    let stack_value = native_ok(disposable_stack::disposable_stack_constructor(&mut vm, &[3]));
    let on_dispose = function_placeholder(&mut vm);

    let str = vm.new_string("stack-box-string");
    let str_ptr = str.as_string_ptr_mut();
    let bi = vm.new_bigint(num_bigint::BigInt::from(4611686018427387904u128));
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;
    vm.regs[0] = stack_value;
    vm.regs[1] = str;
    vm.regs[2] = on_dispose;
    native_ok(disposable_stack::disposable_stack_adopt(&mut vm, &[0, 1, 2]));
    vm.regs[1] = bi;
    native_ok(disposable_stack::disposable_stack_adopt(&mut vm, &[0, 1, 2]));

    let stack_session = vm.promote_object(stack_value.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(stack_session);
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_string_ptrs.contains(&str_ptr));
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

/// 挂起帧内期望值 BigInt 查找：挂起点唯一持有期望值（帧内死槽残留的
/// 字面量值与之不同值，不命中匹配）。
fn find_suspended_bigint(frame: &crate::suspended::SuspendedFrame, vm: &Vm, expected: &num_bigint::BigInt) -> JsValue {
    let mut found = None;
    frame.for_each_value(|v| {
        if found.is_none() && v.is_bigint() && vm.bigint_value(v) == expected {
            found = Some(v);
        }
    });
    found.expect("挂起帧应持有期望 BigInt")
}

/// Promise 结算值持唯一引用的 session 串与 BigInt：清寄存器仅留两个 Promise
/// 根后完整收集，两值仍登记在 session 表。
#[test]
fn session_gc_traces_promise_string_and_bigint_result() {
    let mut vm = vm_with_low_threshold();
    let (promise, _resolve, _reject) = vm.new_promise_capability();
    let str = vm.new_string("promise-box-string");
    let str_ptr = str.as_string_ptr_mut();
    vm.fulfill_promise(promise, str).expect("fulfill string");
    let (promise2, _resolve2, _reject2) = vm.new_promise_capability();
    let bi = vm.new_bigint(num_bigint::BigInt::from(4611686018427387904u128));
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;
    vm.fulfill_promise(promise2, bi).expect("fulfill bigint");

    let promise_session = vm.promote_object(promise.as_js_object_ptr());
    let promise2_session = vm.promote_object(promise2.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(promise_session);
    vm.regs[1] = JsValue::from_js_object(promise2_session);
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_string_ptrs.contains(&str_ptr));
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

/// 挂起生成器帧寄存器持唯一引用 session BigInt：yield 挂起时 b 存活
///（恢复后 return 用），快照入盒；清寄存器仅留生成器根后完整收集，
/// 仍登记在 session 表。
#[test]
fn session_gc_traces_suspended_generator_bigint_value() {
    let mut vm = vm_with_low_threshold();
    vm.run(&Arc::new(compile(
        "(function(){ \
         function* gen() { var b = 987654321n * 123456789n; yield 1; return b; } \
         var g = gen(); g.next(); globalThis.g = g; })(); 0",
    )))
    .expect("run");

    let g_val = global_prop_opt(&vm, "g").expect("g 应挂在 global 上");
    let g_session = vm.promote_object(g_val.as_js_object_ptr());
    let g_obj = unsafe { &*g_session };
    // SAFETY: 生成器状态盒经 Box::into_raw 挂对象构造，生命周期与对象一致。
    let state = g_obj.native_data() as *mut crate::generator::GeneratorState;
    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    let bi = find_suspended_bigint(unsafe { &(*state).suspended }, &vm, &expected);
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;

    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(g_session);
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

/// 挂起异步函数帧持唯一引用 session BigInt：body await 永不结算的 promise，
/// 挂起点 b 存活（恢复后 return 用），快照入盒；挂起链经 await 目标
/// promise 的反应闭包持有上下文对象。清寄存器仅留 await 目标 promise 根
/// 后完整收集，BigInt 仍登记在 session 表。
#[test]
fn session_gc_traces_suspended_async_function_bigint_value() {
    let mut vm = vm_with_low_threshold();
    vm.run(&Arc::new(compile(
        "(function(){ \
         var p = new Promise(function(){}); \
         async function f() { var b = 987654321n * 123456789n; await p; return b; } \
         f(); globalThis.p = p; })(); 0",
    )))
    .expect("run");

    // 挂起链：await 目标 promise → 恢复反应 → 闭包 → 异步上下文对象（状态盒）
    // → 挂起帧。读盒发生在收集前（BigInt 全部存活，解引用安全）。
    let p_val = global_prop_opt(&vm, "p").expect("p 应挂在 global 上");
    let p_obj = unsafe { &*p_val.as_js_object_ptr() };
    // SAFETY: Promise 状态盒经 Box::into_raw 挂构造，生命周期与对象一致。
    let p_state = p_obj.native_data() as *mut crate::promise::PromiseState;
    let handler = unsafe { &(*p_state).reactions }
        .iter()
        .find(|r| r.handler.is_object())
        .expect("await 恢复反应应已登记")
        .handler;
    let handler_obj = unsafe { &*handler.as_js_object_ptr() };
    let ctx_si = vm.kernel_core().perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(handler_obj, ctx_si).expect("恢复闭包应携带异步上下文");
    let ctx_obj = unsafe { &*ctx.as_js_object_ptr() };
    // SAFETY: 异步状态盒经 Box::into_raw 挂上下文构造，生命周期与对象一致。
    let state = ctx_obj.native_data() as *mut crate::async_func::AsyncState;
    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    let bi = find_suspended_bigint(unsafe { &(*state).suspended }, &vm, &expected);
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;

    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = p_val;
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

/// 挂起异步生成器帧持唯一引用 session BigInt：首次 next() 挂起于 yield，
/// 挂起点 b 存活（恢复后 return 用），快照入盒；请求 promise 结果只持 1、
/// 与 b 无关。清寄存器仅留迭代器根后完整收集，BigInt 仍登记在 session 表。
#[test]
fn session_gc_traces_suspended_async_generator_bigint_value() {
    let mut vm = vm_with_low_threshold();
    vm.run(&Arc::new(compile(
        "async function* gen() { var b = 987654321n * 123456789n; yield 1; return b; } \
         var it = gen(); it.next(); globalThis.it = it; 0",
    )))
    .expect("run");

    let it_val = global_prop_opt(&vm, "it").expect("it 应挂在 global 上");
    let it_session = vm.promote_object(it_val.as_js_object_ptr());
    let it_obj = unsafe { &*it_session };
    // SAFETY: 异步生成器状态盒经 Box::into_raw 挂迭代器构造，生命周期与对象一致。
    let state = it_obj.native_data() as *mut crate::async_generator::AsyncGeneratorState;
    let expected = num_bigint::BigInt::from(987654321u64) * num_bigint::BigInt::from(123456789u64);
    let bi = find_suspended_bigint(unsafe { &(*state).suspended }, &vm, &expected);
    let bi_ptr = bi.as_bigint_ptr() as *mut num_bigint::BigInt;

    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(it_session);
    collect(&mut vm);

    // 只断言表成员——sweep 已释放的指针解引用即 UB。
    assert!(vm.gc_state.session_bigint_ptrs.borrow().contains(&bi_ptr));
}

#[test]
fn session_gc_keeps_shared_array_buffer_alive_through_view_native_edges() {
    let mut vm = vm_with_low_threshold();
    let root = plain_object(&mut vm);

    vm.regs[1] = JsValue::int(8);
    let buffer = native_ok(array_buffer::array_buffer_constructor(&mut vm, &[0, 1]));

    vm.regs[1] = buffer;
    let typed = native_ok(typed_array::int32array_constructor(&mut vm, &[0, 1]));

    vm.regs[1] = buffer;
    let view = native_ok(data_view::data_view_constructor(&mut vm, &[0, 1]));

    vm.regs[0] = view;
    vm.regs[1] = JsValue::int(0);
    vm.regs[2] = JsValue::int(42);
    vm.regs[3] = JsValue::bool(true);
    native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2, 3]));

    unsafe {
        (*root).set_prop_at(0, typed);
        (*root).set_prop_at(1, view);
    }

    let root_session = vm.promote_object(root);
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(root_session);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.collect(&mut vm);
    vm.gc_state.session_gc = gc;

    let live_root = unsafe { &*vm.regs[0].as_js_object_ptr() };
    let live_typed = live_root.get_prop_at(0);
    let live_view = live_root.get_prop_at(1);
    let typed_obj = unsafe { &*live_typed.as_js_object_ptr() };
    let view_obj = unsafe { &*live_view.as_js_object_ptr() };
    let typed_edges = typed_array::typed_array_native_edges(typed_obj);
    let view_edges = data_view::data_view_native_edges(view_obj);

    assert_eq!(typed_edges.len(), 1);
    assert_eq!(view_edges.len(), 1);
    assert!(vm.is_session_ptr(typed_edges[0].as_js_object_ptr()));
    assert!(vm.is_session_ptr(view_edges[0].as_js_object_ptr()));
    assert!(std::ptr::eq(typed_edges[0].as_js_object_ptr(), view_edges[0].as_js_object_ptr()));

    vm.regs[0] = live_typed;
    vm.regs[1] = JsValue::int(0);
    assert_eq!(native_ok(typed_array::typed_array_at(&mut vm, &[0, 1])), JsValue::int(42));

    vm.regs[0] = live_view;
    vm.regs[1] = JsValue::int(4);
    vm.regs[2] = JsValue::int(7);
    vm.regs[3] = JsValue::bool(true);
    native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2, 3]));

    vm.regs[0] = live_typed;
    vm.regs[1] = JsValue::int(1);
    assert_eq!(native_ok(typed_array::typed_array_at(&mut vm, &[0, 1])), JsValue::int(7));
}

#[test]
fn session_gc_rewrites_buffer_retained_only_by_data_view_native_edge() {
    let mut vm = vm_with_low_threshold();
    vm.regs[1] = JsValue::int(8);
    let buffer = native_ok(array_buffer::array_buffer_constructor(&mut vm, &[0, 1]));
    vm.regs[1] = buffer;
    let view = native_ok(data_view::data_view_constructor(&mut vm, &[0, 1]));

    vm.regs[0] = view;
    vm.regs[1] = JsValue::int(0);
    vm.regs[2] = JsValue::int(9);
    native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2]));

    let view_session = vm.promote_object(view.as_js_object_ptr());
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(view_session);
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.collect(&mut vm);
    vm.gc_state.session_gc = gc;

    let live_view = vm.regs[0];
    let live_view_obj = unsafe { &*live_view.as_js_object_ptr() };
    let edges = data_view::data_view_native_edges(live_view_obj);
    assert_eq!(edges.len(), 1);
    assert!(vm.is_session_ptr(edges[0].as_js_object_ptr()));
    assert_eq!(vm.gc_state.session_object_ptrs.len(), 2);

    vm.regs[0] = live_view;
    vm.regs[1] = JsValue::int(0);
    assert_eq!(native_ok(data_view::data_view_get_int32(&mut vm, &[0, 1])), JsValue::int(9));
}

fn collect(vm: &mut Vm) {
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.collect(vm);
    vm.gc_state.session_gc = gc;
}

#[test]
fn session_string_collected_when_dead() {
    let mut vm = Vm::new();
    let dead = vm.new_string("dead-string");
    let dead_ptr = dead.as_string_ptr_mut();
    assert!(vm.gc_state.session_string_ptrs.contains(&dead_ptr));

    // 没有根引用 `dead`（它只作为值包装存在于 Rust 栈上）。
    collect(&mut vm);

    assert!(!vm.gc_state.session_string_ptrs.contains(&dead_ptr));
}

#[test]
fn session_string_survives_when_in_register() {
    let mut vm = Vm::new();
    let live = vm.new_string("live-in-reg");
    let live_ptr = live.as_string_ptr_mut();
    vm.regs[0] = live;

    collect(&mut vm);

    assert!(vm.gc_state.session_string_ptrs.contains(&live_ptr));
    // 存活字符串永不搬移——寄存器仍指向同一 box。
    assert_eq!(vm.regs[0].as_string_ptr_mut(), live_ptr);
    assert_eq!(unsafe { (*live_ptr).as_str() }, "live-in-reg");
}

#[test]
fn session_string_survives_via_object_property() {
    let mut vm = Vm::new();
    let obj = plain_object(&mut vm);
    let s = vm.new_string("prop-string");
    let s_ptr = s.as_string_ptr_mut();
    unsafe {
        (*obj).set_prop_at(0, s);
    }
    let obj_session = vm.promote_object(obj);

    // 该字符串仅通过存活对象的属性可达，不经过任何寄存器。
    vm.regs.fill(JsValue::undefined());
    vm.regs[0] = JsValue::from_js_object(obj_session);

    collect(&mut vm);

    assert!(vm.gc_state.session_string_ptrs.contains(&s_ptr));
    assert_eq!(unsafe { (*s_ptr).as_str() }, "prop-string");
}

#[test]
fn permanent_string_untouched_by_sweep() {
    let mut vm = Vm::new();
    let perm = vm.perm_string("perm");
    let perm_ptr = perm.as_string_ptr_mut();
    // 永久字符串位于 PermInterner，绝不在 session 集合中。
    assert!(!vm.gc_state.session_string_ptrs.contains(&perm_ptr));
    vm.regs[0] = perm;

    collect(&mut vm);

    // 绝不被 session 清扫释放（仍可读，仍不在 session 集合中）。
    assert!(!vm.gc_state.session_string_ptrs.contains(&perm_ptr));
    assert_eq!(unsafe { (*perm_ptr).as_str() }, "perm");
}

#[test]
fn string_sweep_byte_accounting() {
    let mut vm = Vm::new();
    let dead = vm.new_string("0123456789");
    let _ = dead.as_string_ptr_mut();
    let expected = (size_of::<JsString>() + "0123456789".len()) as u64;

    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    let before = gc.total_bytes_freed;
    gc.collect(&mut vm);
    let after = gc.total_bytes_freed;
    vm.gc_state.session_gc = gc;

    assert!(after >= before + expected);
}

fn vm_with_threshold(bytes: usize) -> Vm {
    let mut cfg = KernelConfig::minimal();
    cfg.set_session_gc_threshold(bytes);
    let core = KernelCore::new(cfg);
    Vm::with_kernel_core(core)
}

// ── upvalue cell 独立堆分配：跨对象 sweep 存活 ─────────────────────────

fn compile(source: &str) -> oxide_bytecode::module::CompiledModule {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Compiler::new().compile(&program).expect("compile")
}

fn global_prop_opt(vm: &Vm, name: &str) -> Option<JsValue> {
    let global = vm.session.global_object();
    let si = vm.kernel_core().perm_interner().intern(name).0;
    vm.resolve_property(global, si)
}

/// 闭包捕获变量跨对象 sweep 存活：reset 触发完整收集（session_epoch 替换）。
/// cell 独立堆分配、不入 session arena，arena 整体回收时不连带释放——否则跨
/// arena 存活的 cell 指针悬垂；sweep 只重写 cell.value 中的对象引用，值跨搬移保留。
#[test]
fn closure_cell_survives_object_sweep() {
    let mut vm = vm_with_threshold(1);
    // counter 函数作用域局部：被 inc 经 cell 捕获（顶层 var 直连全局属性不走 cell）。
    vm.run(&Arc::new(compile(
        "function outer() { var counter = 0; function inc() { return ++counter; } globalThis.inc = inc; return 0; } outer(); 0",
    )))
    .expect("run1");
    assert!(!vm.gc_state.session_cell_ptrs.borrow().is_empty(), "run1 应分配 upvalue cell");

    // reset 触发对象 sweep：存活对象搬到新 arena，旧 arena 释放。
    vm.reset();
    assert!(vm.session_gc_stats().total_collections > 0, "reset 应触发对象收集");

    // 跨搬移后从 global 重新取 inc 函数对象：upvalue cell 指针稳定、值保留。
    let inc_val = global_prop_opt(&vm, "inc").expect("inc 应挂在 global 上");
    let obj = unsafe { &*inc_val.as_js_object_ptr() };
    let cells = obj.upvalues_slice();
    assert_eq!(cells.len(), 1, "inc 应捕获 counter 一个 cell");
    let cell = unsafe { &*cells[0] };
    assert_eq!(cell.value, JsValue::int(0), "sweep 后 cell 值应保留为 counter 初值");
    assert!(cell.is_initialized(), "sweep 后 cell 初始化位应保留");
}

/// 私有字段类的 brand cell 跨对象 sweep 存活：`@@class_brand` upvalue cell
/// 存类 brand 对象，sweep 后其值经 forwarding 重写为搬移后的新对象地址。
/// cell 独立堆分配，指针在周边 arena 回收后仍稳定，值不被内存复用覆盖。
#[test]
fn private_brand_cell_survives_object_sweep() {
    let mut vm = vm_with_threshold(1);
    vm.run(&Arc::new(compile(
        "class C { #x = 0; set(v){ this.#x = v; } get(){ return this.#x; } } \
         globalThis.c = new C(); globalThis.c.set(4); 0",
    )))
    .expect("run1");

    vm.reset();
    assert!(vm.session_gc_stats().total_collections > 0, "reset 应触发对象收集");

    // 实例 c 的方法 get（类原型上）捕获 @@class_brand cell：值须仍为对象。
    let c_val = global_prop_opt(&vm, "c").expect("c 应挂在 global 上");
    let c_obj = unsafe { &*c_val.as_js_object_ptr() };
    let proto_ptr = c_obj.proto().as_js_object_ptr();
    let get_val = vm
        .resolve_property(unsafe { &*proto_ptr }, vm.kernel_core().perm_interner().intern("get").0)
        .expect("类原型应有 get 方法");
    let get_obj = unsafe { &*get_val.as_js_object_ptr() };
    let cells = get_obj.upvalues_slice();
    assert!(!cells.is_empty(), "get 应捕获 @@class_brand cell");
    let brand = unsafe { &*cells[0] }.value;
    assert!(brand.is_object(), "sweep 后 brand cell 值应仍为 brand 对象");
}

/// full_reset 统一释放全部 cell box 且可重新分配：追踪表清空（无 double-free），
/// 重置后新闭包正常建立新 cell。
#[test]
fn cells_freed_by_full_reset_and_reallocatable() {
    let mut vm = Vm::new();
    // x/y 函数作用域局部：被 f/g 经 cell 捕获（顶层 var 直连全局属性不走 cell）。
    vm.run(&Arc::new(compile(
        "function outer() { var x = 1; function f() { return x; } globalThis.f = f; return f(); } outer()",
    )))
    .expect("run1");
    assert!(!vm.gc_state.session_cell_ptrs.borrow().is_empty());

    vm.full_reset();
    assert!(vm.gc_state.session_cell_ptrs.borrow().is_empty(), "full_reset 应释放全部 cell");

    let result = vm
        .run(&Arc::new(compile(
            "function outer() { var y = 2; function g() { return y; } return g(); } outer()",
        )))
        .expect("run2");
    assert_eq!(format!("{result}"), "2", "重置后新 cell 应正常分配与读取");
}

#[test]
fn allocation_does_not_trigger_gc_before_safe_point() {
    let mut vm = vm_with_threshold(1);
    // 分配点不触发回收：触发已移到 dispatch 指令边界，局部持有跨分配点安全。
    let seed = vm.new_string_owned("seed".repeat(16));
    let seed_ptr = seed.as_string_ptr_mut();
    let returned = vm.new_string_owned("returned".repeat(16));

    assert_eq!(vm.gc_state.session_gc.total_collections, 0);
    assert!(vm.gc_state.session_string_ptrs.contains(&seed_ptr));
    assert!(vm.gc_state.session_string_ptrs.contains(&returned.as_string_ptr_mut()));

    // 显式触发后：无根引用的 seed 被回收，入根的 returned 存活（安全点语义）。
    vm.regs[0] = returned;
    vm.maybe_collect_session_strings();
    assert!(vm.gc_state.session_gc.total_collections >= 1);
    assert!(!vm.gc_state.session_string_ptrs.contains(&seed_ptr));
    assert!(vm.gc_state.session_string_ptrs.contains(&returned.as_string_ptr_mut()));
}

#[test]
fn strings_only_collection_preserves_all_root_kinds() {
    // 阈值 1：每次 new_string_owned 分配前自动触发回收，逐步验证各类根的保护。
    let mut vm = vm_with_threshold(1);
    // 寄存器根：直接持有 session 串。
    let reg_str = vm.new_string_owned("reg-root".repeat(16));
    vm.regs[0] = reg_str;
    // 存活对象属性根：session 对象经 promote 后持有 session 串（分配即触发回收，
    // reg_str 仍在寄存器，prop_str 挂到对象后才被下次回收看到）。
    let obj = plain_object(&mut vm);
    let prop_str = vm.new_string_owned("prop-root".repeat(16));
    unsafe {
        (*obj).set_prop_at(0, prop_str);
    }
    let obj_session = vm.promote_object(obj);
    vm.regs[1] = JsValue::from_js_object(obj_session);
    // 非 session 根对象属性：epoch 根对象（在寄存器）持有 session 串，mark 走
    // 非 session 根扫描路径保护。
    let epoch_obj = plain_object(&mut vm);
    let epoch_str = vm.new_string_owned("epoch-root".repeat(16));
    unsafe {
        (*epoch_obj).set_prop_at(0, epoch_str);
    }
    vm.regs[2] = JsValue::from_js_object(epoch_obj);
    // 死串：无任何根引用，最后手动触发一轮回收它。
    let dead = vm.new_string_owned("dead".repeat(16));
    let dead_ptr = dead.as_string_ptr_mut();

    vm.maybe_collect_session_strings();

    assert!(vm.gc_state.session_string_ptrs.contains(&reg_str.as_string_ptr_mut()));
    assert!(vm.gc_state.session_string_ptrs.contains(&prop_str.as_string_ptr_mut()));
    assert!(vm.gc_state.session_string_ptrs.contains(&epoch_str.as_string_ptr_mut()));
    assert!(!vm.gc_state.session_string_ptrs.contains(&dead_ptr));
    // 存活串内容可读，地址稳定。
    assert_eq!(unsafe { (*reg_str.as_string_ptr_mut()).as_str() }, "reg-root".repeat(16));
    assert_eq!(unsafe { (*prop_str.as_string_ptr_mut()).as_str() }, "prop-root".repeat(16));
    assert_eq!(unsafe { (*epoch_str.as_string_ptr_mut()).as_str() }, "epoch-root".repeat(16));
}

#[test]
fn strings_only_collection_does_not_move_objects() {
    let mut vm = vm_with_threshold(1);
    let obj = plain_object(&mut vm);
    let obj_session = vm.promote_object(obj);
    vm.regs[0] = JsValue::from_js_object(obj_session);
    let before: Vec<_> = vm.gc_state.session_object_ptrs.clone();

    let s = vm.new_string_owned("x".repeat(64));
    vm.regs[1] = s;
    // 分配不自动触发（安全点语义），显式触发一轮验证对象不搬移。
    vm.maybe_collect_session_strings();

    // 对象指针逐一相同：不搬移、不重写根。
    assert_eq!(vm.gc_state.session_object_ptrs, before);
    assert_eq!(vm.regs[0].as_js_object_ptr(), obj_session);
    assert_eq!(vm.session_object_count(), 1);
}

#[test]
fn multiple_strings_only_cycles_keep_objects_alive() {
    let mut vm = vm_with_threshold(1);
    let obj = plain_object(&mut vm);
    let obj_session = vm.promote_object(obj);
    let live = vm.new_string_owned("keep".repeat(32));
    unsafe {
        (*obj_session).set_prop_at(0, live);
    }
    vm.regs[0] = JsValue::from_js_object(obj_session);

    // 连续多轮触发字符串回收：对象 mark 残留位被显式清理，对象与挂载串持续存活。
    for _ in 0..5 {
        for _ in 0..8 {
            let _ = vm.new_string_owned("dead".repeat(64));
        }
        // 分配不自动触发，每轮显式触发一次回收。
        vm.maybe_collect_session_strings();
        assert!(vm.gc_state.session_object_ptrs.contains(&obj_session));
        assert_eq!(unsafe { (*obj_session).get_prop_at(0) }, live);
    }
    assert!(vm.gc_state.session_string_ptrs.contains(&live.as_string_ptr_mut()));
}

#[test]
fn strings_only_then_full_collect_keeps_object_strings_live() {
    // strings-only 收集后接完整收集（reset 路径）：strings-only 残留的 mark 位
    // 不得使完整收集的 mark DFS 短路漏标，存活对象属性中的串须跨收集存活。
    let mut vm = vm_with_threshold(1);
    let obj = plain_object(&mut vm);
    let s = vm.new_string_owned("kept".repeat(8));
    let s_ptr = s.as_string_ptr_mut();
    unsafe {
        (*obj).set_prop_at(0, s);
    }
    let obj_session = vm.promote_object(obj);
    vm.regs[0] = JsValue::from_js_object(obj_session);

    // 第一轮 strings-only：对象被 mark 置位。收尾清残留，否则该位残留至完整
    // 收集会使 mark DFS 在对象处短路、漏标其属性中的串；随后校验串跨完整收集存活。
    vm.maybe_collect_session_strings();
    assert!(vm.gc_state.session_string_ptrs.contains(&s_ptr));

    // 完整收集（reset 的 maybe_collect 路径）：对象搬移、字符串地址稳定。
    let mut gc = std::mem::take(&mut vm.gc_state.session_gc);
    gc.collect(&mut vm);
    vm.gc_state.session_gc = gc;

    // 存活对象经重写仍可达其串：串未被误释放，内容可读。
    assert!(vm.gc_state.session_string_ptrs.contains(&s_ptr));
    assert_eq!(unsafe { (*s_ptr).as_str() }, "kept".repeat(8));
    assert_eq!(unsafe { (*vm.regs[0].as_js_object_ptr()).get_prop_at(0) }, s);
}

#[test]
fn strings_only_collection_byte_accounting_matches_survivors() {
    let mut vm = vm_with_threshold(1);
    let obj = plain_object(&mut vm);
    let obj_session = vm.promote_object(obj);
    vm.regs[0] = JsValue::from_js_object(obj_session);
    let live = vm.new_string_owned("live".repeat(8));
    let live_ptr = live.as_string_ptr_mut();
    vm.regs[1] = live;
    let dead = vm.new_string_owned("dead".repeat(8));
    let _ = dead.as_string_ptr_mut();

    vm.maybe_collect_session_strings();

    // 账目 = 对象头 + 存活串（size_of::<JsString>() + len），死串不再计入。
    let expected = size_of::<JsObject>() + (size_of::<JsString>() + unsafe { (*live_ptr).payload_bytes() });
    assert_eq!(vm.gc_state.session_bytes_allocated, expected);
    assert!(!vm.gc_state.session_string_ptrs.contains(&dead.as_string_ptr_mut()));
}

#[test]
fn runtime_collection_below_threshold_is_noop() {
    let mut vm = Vm::new();
    let s = vm.new_string_owned("small".repeat(4));
    vm.regs[0] = s;

    assert_eq!(vm.gc_state.session_gc.total_collections, 0);
    assert!(vm.gc_state.session_string_ptrs.contains(&s.as_string_ptr_mut()));
}

// ── rope（Cons）GC 传播闭包 ──

/// 构造 `left + right` 的 Cons 节点（返回 (父, 左, 右) 三指针）。
fn make_cons_pair(vm: &mut Vm, l: &str, r: &str) -> (JsValue, *mut JsString, *mut JsString) {
    let left = vm.new_string(l);
    let right = vm.new_string(r);
    let parent = vm.new_cons_string(left, right);
    (parent, left.as_string_ptr_mut(), right.as_string_ptr_mut())
}

#[test]
fn rope_survives_collection_with_children_propagated() {
    let mut vm = Vm::new();
    let (parent, l_ptr, r_ptr) = make_cons_pair(&mut vm, "left-part", "right-part");
    vm.regs[0] = parent;

    collect(&mut vm);

    // 父（寄存器根）+ 子节点（经传播闭包）全部存活，内容可读。
    let parent_ptr = parent.as_string_ptr_mut();
    assert!(vm.gc_state.session_string_ptrs.contains(&parent_ptr));
    assert!(vm.gc_state.session_string_ptrs.contains(&l_ptr));
    assert!(vm.gc_state.session_string_ptrs.contains(&r_ptr));
    assert_eq!(unsafe { (*parent_ptr).as_lossy_str() }, "left-partright-part");
}

#[test]
fn rope_children_swept_when_parent_dead() {
    let mut vm = Vm::new();
    let (parent, l_ptr, r_ptr) = make_cons_pair(&mut vm, "left", "right");
    let parent_ptr = parent.as_string_ptr_mut();
    assert!(vm.gc_state.session_string_ptrs.contains(&parent_ptr));

    // 父与子均无根引用 → 整树回收。
    collect(&mut vm);

    assert!(!vm.gc_state.session_string_ptrs.contains(&parent_ptr));
    assert!(!vm.gc_state.session_string_ptrs.contains(&l_ptr));
    assert!(!vm.gc_state.session_string_ptrs.contains(&r_ptr));
}

#[test]
fn rope_product_freed_with_parent() {
    let mut vm = Vm::new();
    let (parent, _, _) = make_cons_pair(&mut vm, "left-part", "right-part");
    let parent_ptr = parent.as_string_ptr_mut();
    // 触发扁平化：产物发布到 flat_cache。
    assert_eq!(unsafe { (*parent_ptr).as_lossy_str() }, "left-partright-part");
    let flat_ptr = unsafe { (*parent_ptr).flat_cache_ptr() };
    assert!(!flat_ptr.is_null());

    collect(&mut vm);

    // 父死 → 连带释放产物（不 double-free、不泄漏；产物从不进 session 表）。
    let flat_mut = flat_ptr as *mut JsString;
    assert!(!vm.gc_state.session_string_ptrs.contains(&parent_ptr));
    assert!(!vm.gc_state.session_string_ptrs.contains(&flat_mut));
}

#[test]
fn rope_deep_chain_mark_iterative() {
    let mut vm = Vm::new();
    // 1024 层左倾链：mark 传播用显式栈，不爆栈、不遗漏子节点。
    let mut chain = vm.new_string("root");
    for _ in 0..1024 {
        let leaf = vm.new_string("x");
        chain = vm.new_cons_string(chain, leaf);
    }
    vm.regs[0] = chain;
    let chain_ptr = chain.as_string_ptr_mut();

    collect(&mut vm);

    assert!(vm.gc_state.session_string_ptrs.contains(&chain_ptr));
    assert_eq!(unsafe { (*chain_ptr).as_lossy_str() }, format!("root{}", "x".repeat(1024)));
}

#[test]
fn rope_perm_child_untouched_by_sweep() {
    let mut vm = Vm::new();
    let perm = vm.perm_string("perm-leaf");
    let perm_ptr = perm.as_string_ptr_mut();
    let session = vm.new_string("session-leaf");
    let session_ptr = session.as_string_ptr_mut();
    let parent = vm.new_cons_string(perm, session);
    vm.regs[0] = parent;

    collect(&mut vm);

    // perm 子节点永不释放（不在 session 表）；session 子节点随父存活。
    assert!(!vm.gc_state.session_string_ptrs.contains(&perm_ptr));
    assert!(vm.gc_state.session_string_ptrs.contains(&session_ptr));
    assert_eq!(unsafe { (*parent.as_string_ptr_mut()).as_lossy_str() }, "perm-leafsession-leaf");
}

#[test]
fn full_reset_frees_rope_and_product() {
    let mut vm = Vm::new();
    let (parent, _, _) = make_cons_pair(&mut vm, "left-part", "right-part");
    // 触发扁平化（产物挂在父上）。
    assert_eq!(unsafe { (*parent.as_string_ptr_mut()).as_lossy_str() }, "left-partright-part");
    vm.regs[0] = parent;

    // full_reset 清空全部 session 字符串（连带产物）：无泄漏、无 double-free。
    vm.full_reset();
    assert!(vm.gc_state.session_string_ptrs.is_empty());
}

// ── 执行期两档收集 ──────────────────────────────────────────────────────

/// 执行期收集有效性：死 epoch 对象随换新 Bump 回收、死 session 对象原地
/// 回收出表，活对象晋升 session 后值保持可读。
#[test]
fn in_run_collection_reclaims_dead_and_keeps_live() {
    let mut vm = vm_with_threshold(65536);
    vm.run(&Arc::new(compile("globalThis.keep = { a: 1 }; 0"))).expect("run1");
    // churn：IIFE 局部数组 + 50 个未入根的函数（函数 session 直分配、
    // 数组 epoch 分配），IIFE 返回后全部不可达。
    vm.run(&Arc::new(compile(
        "(function(){ var t = []; for (var i = 0; i < 50; i++) { t[i] = function() { return i; }; } })(); 0",
    )))
    .expect("run2");
    // run 边界清空执行状态：寄存器文件中的陈旧值不再是执行根，churn 对象
    // 自此真正不可达。
    vm.run(&Arc::new(compile("0"))).expect("run3");

    assert!(!vm.gc_state.epoch_object_ptrs.is_empty(), "churn 应留有 epoch 对象");
    let session_before = vm.session_object_count();
    assert!(session_before > 0, "churn 应产生 session 直分配函数");

    vm.maybe_collect_in_run();

    assert!(vm.session_gc_stats().total_collections >= 1, "执行期收集应跑一轮");
    assert!(vm.gc_state.epoch_object_ptrs.is_empty(), "epoch 对象应全部晋升或回收");
    assert!(vm.session_object_count() < session_before, "死 session 函数应被原地回收");

    // 活对象晋升 session 后值可读。
    let keep = global_prop_opt(&vm, "keep").expect("keep 应挂在 global");
    assert!(keep.is_object());
    let keep_ptr = keep.as_js_object_ptr();
    assert!(unsafe { (*keep_ptr).is_session_epoch() }, "keep 晋升后应为 session 对象");
    let a = vm.resolve_property(unsafe { &*keep_ptr }, vm.kernel_core().perm_interner().intern("a").0);
    assert_eq!(a, Some(JsValue::int(1)));
}

/// for-in 门控双形钉住：活跃形（迭代器在 `vm.iters` 表内）与挂起形
/// （生成器在 for-in 内 yield，迭代器搬入状态盒）都须拦下执行期收集——
/// 换新 Bump 使 ForInIter body 即时失效；门控开后方可收集。
#[test]
fn active_and_suspended_for_in_block_in_run_collection() {
    // 活跃形：迭代器直接压在 vm.iters，O(1) 子句即拦。
    let mut vm = vm_with_threshold(65536);
    let dead = plain_object(&mut vm);
    vm.gc_state.epoch_object_ptrs.push(dead);
    let iter = vm.epoch.alloc(crate::vm::ForInIter {
        keys: bumpalo::collections::Vec::new_in(vm.epoch.bump()),
        index: 0,
    });
    vm.iters.push_for_in(iter.cast::<crate::vm::ForInIter<'static>>());
    let bump_before = vm.epoch.current_id();

    vm.maybe_collect_in_run();
    assert_eq!(vm.session_gc_stats().total_collections, 0, "活跃 for-in：门控关闭，不收集");
    assert_eq!(vm.epoch.current_id(), bump_before, "门控关闭：epoch Bump 未换新");
    assert!(vm.gc_state.epoch_object_ptrs.contains(&dead), "死对象仍在 epoch 表");

    // 迭代器出表后同一调用点即应收集。
    vm.iters.for_in_iters.pop();
    vm.maybe_collect_in_run();
    assert_eq!(vm.session_gc_stats().total_collections, 1, "门控开：应收集");
    assert!(vm.epoch.current_id() > bump_before, "收集应换新 epoch Bump");
    assert!(vm.gc_state.epoch_object_ptrs.is_empty(), "死 epoch 对象应随 Bump 回收");

    // 挂起形（拦截）：生成器在 for-in 内 yield 后 run 结束，迭代器经
    // 状态盒持有（vm.iters 已空）——收集点必须拦下，Bump 不得换新。
    let mut vm = vm_with_threshold(65536);
    let bump_before = vm.epoch.current_id();
    vm.run(&Arc::new(compile(
        "var o = { a: 1, b: 2 }; \
         function* gen() { for (var k in o) { yield k; } return 'done'; } \
         var g = gen(); globalThis.g = g; g.next(); 0",
    )))
    .expect("run1");
    assert!(vm.iters.for_in_iters.is_empty(), "挂起时迭代器已搬入状态盒");
    assert!(vm.suspended_holds_for_in(), "生成器状态盒应持挂起 for-in 迭代器");

    vm.maybe_collect_in_run();
    assert_eq!(vm.session_gc_stats().total_collections, 0, "挂起 for-in：门控关闭，不收集");
    assert!(vm.epoch.current_id() == bump_before, "门控关闭：epoch Bump 未换新");
    // 挂起生成器对象保持原址（未晋升、未释放）：状态盒与迭代器一体存活。
    let gen_in_epoch = vm
        .gc_state
        .epoch_object_ptrs
        .iter()
        .any(|&p| !p.is_null() && unsafe { (*p).type_tag == JsObject::OBJ_TYPE_GENERATOR });
    assert!(gen_in_epoch, "挂起生成器应仍在 epoch 表");

    // 挂起形（放行）：同一 run 内续跑至 for-in 结束（迭代器释放），
    // 阈值 1 使指令边界自动触发：挂起期门控关（Bump 不换新），完成后
    // 门控开、执行期收集放行；yield 值跨收集保持正确。
    let mut vm = vm_with_threshold(1);
    vm.run(&Arc::new(compile(
        "var o = { a: 1, b: 2 }; \
         function* gen() { for (var k in o) { yield k; } return 'done'; } \
         var g = gen(); globalThis.g = g; \
         globalThis.v1 = g.next().value; globalThis.v2 = g.next().value; g.next(); 0",
    )))
    .expect("run2");
    assert_eq!(vm.lookup_str(global_prop_opt(&vm, "v1").expect("v1")), Some("a".to_string()));
    assert_eq!(vm.lookup_str(global_prop_opt(&vm, "v2").expect("v2")), Some("b".to_string()));
    assert!(!vm.suspended_holds_for_in(), "for-in 结束后状态盒不应再持迭代器");
    assert!(vm.session_gc_stats().total_collections >= 1, "完成后门控开：执行期收集应放行");

    // 生成器对象跨收集仍为合法生成器（晋升 session 或保持 epoch 完好）。
    let g = global_prop_opt(&vm, "g").expect("g 应挂在 global");
    assert!(unsafe { (*g.as_js_object_ptr()).is_generator_obj() }, "收集后生成器对象应完好");
}

/// 非移动不变量：执行期收集的存活 session 对象地址不变（元素堆区亦不
/// 换盒）——用户可观察 identity 不分裂，免 forwarding/rewrite。
#[test]
fn in_run_collection_keeps_live_addresses_stable() {
    let mut vm = vm_with_threshold(65536);
    vm.run(&Arc::new(compile("globalThis.o = { a: [1, 2, 3], f: function() { return 1; } }; 0")))
        .expect("run1");

    // 第一轮执行期收集：o 及其 epoch 子引用晋升 session。
    vm.maybe_collect_in_run();
    let o = global_prop_opt(&vm, "o").expect("o 应挂在 global");
    let o_ptr = o.as_js_object_ptr();
    assert!(unsafe { (*o_ptr).is_session_epoch() }, "o 晋升后应为 session 对象");
    let a_ptr = vm
        .resolve_property(unsafe { &*o_ptr }, vm.kernel_core().perm_interner().intern("a").0)
        .expect("o.a")
        .as_js_object_ptr();
    let f_ptr = vm
        .resolve_property(unsafe { &*o_ptr }, vm.kernel_core().perm_interner().intern("f").0)
        .expect("o.f")
        .as_js_object_ptr();
    let a_elems = unsafe { (*a_ptr).array_elements_raw() };

    // 第二轮 churn + 收集：死对象回收，存活对象与堆区地址保持不变。
    vm.run(&Arc::new(compile(
        "(function(){ var t = []; for (var i = 0; i < 50; i++) { t[i] = { x: i }; } })(); 0",
    )))
    .expect("run2");
    // run 边界清空执行状态：churn 局部（t 数组与 50 个对象）自此不可达。
    vm.run(&Arc::new(compile("0"))).expect("run3");
    vm.maybe_collect_in_run();
    assert!(vm.session_gc_stats().total_collections >= 2, "两轮收集均应执行");

    assert_eq!(global_prop_opt(&vm, "o").expect("o").as_js_object_ptr(), o_ptr, "存活对象地址不变");
    assert_eq!(
        vm.resolve_property(unsafe { &*o_ptr }, vm.kernel_core().perm_interner().intern("a").0)
            .expect("o.a")
            .as_js_object_ptr(),
        a_ptr,
        "数组对象地址不变"
    );
    assert_eq!(
        vm.resolve_property(unsafe { &*o_ptr }, vm.kernel_core().perm_interner().intern("f").0)
            .expect("o.f")
            .as_js_object_ptr(),
        f_ptr,
        "函数对象地址不变"
    );
    assert_eq!(unsafe { (*a_ptr).array_elements_raw() }, a_elems, "元素堆区不换盒");
    assert_eq!(
        vm.resolve_property(unsafe { &*a_ptr }, vm.kernel_core().perm_interner().intern("1").0)
            .unwrap_or(JsValue::int(-1)),
        JsValue::int(2),
        "元素值跨收集可读"
    );
}

// ── Promise 晋升结算传导（promise × promote × settle 组合） ──────────────

/// 结算矩阵钉：结算走原件（barrier 窗口，闭包仍指 epoch 原件）、消费者
/// 注册在晋升后的克隆上。
///
/// 原件结算沿结算链传导到最新克隆，克隆侧（晋升后登记）的反应随之触发，
/// 回归保传导通路。
#[test]
fn promise_settle_via_original_propagates_to_clone_reaction() {
    let mut vm = vm_with_threshold(65536);
    vm.run(&Arc::new(compile(
        "(function(){ \
         var r; \
         var p = new Promise(function(res){ r = res; }); \
         globalThis.res = r; \
         globalThis.p = p; \
         globalThis.p.then(function(v){ globalThis.got = v; }); \
         r(42); })(); 0",
    )))
    .expect("run1");

    assert_eq!(
        global_prop_opt(&vm, "got"),
        Some(JsValue::int(42)),
        "原件结算应传导到克隆并触发克隆侧消费者"
    );
}

/// 结算矩阵钉：登记侧原件晋升前、结算走改写后的克隆引用（本洞主钉）。
///
/// 单 run 内完成全拓扑：执行器把结算闭包写向 global 时逃逸写屏障连带晋升
/// 目标 promise（克隆一）；`.then` 登记在 epoch 原件上；原件再写向 global
/// 触发再晋升——反应迁入最新克隆、旧克隆接进结算链。IIFE 返回后的顶层
/// 指令边界触发执行期收集（阈值 1），原件随 epoch 释放；随后结算入口是
/// 克隆一（闭包目标槽在晋升时已改写到它），反应在最新克隆上——结算须沿
/// 链触达全部迁移反应，消费者（同 run 末微任务触发，字节码子模块表仍有效）
/// 方能落值。反应迁入克隆并接结算链后，原件与旧克隆的结算沿链传到最新克隆，
/// 反应不因原件释放而静默丢失。
#[test]
fn promise_promote_migrates_pre_promote_reactions_to_clone() {
    let mut vm = vm_with_threshold(1);
    vm.run(&Arc::new(compile(
        "(function(){ \
         var p = new Promise(function(res){ globalThis.res = res; }); \
         p.then(function(v){ globalThis.got = v; }); \
         globalThis.p = p; })(); \
         globalThis.res(42); 0",
    )))
    .expect("run1");

    assert!(vm.session_gc_stats().total_collections >= 1, "执行期收集应已触发（原件随 epoch 释放）");
    assert_eq!(
        global_prop_opt(&vm, "got"),
        Some(JsValue::int(42)),
        "晋升前登记的消费者应在结算走克隆后被触发"
    );
}

/// 结算矩阵钉：reject 变体——登记侧原件晋升前、拒绝走改写后的克隆引用。
///
/// 与完成路径同拓扑（单 run、同 run 末微任务触发）：反应随晋升迁入，拒绝
/// 走克隆沿结算链传导，仅 reject 角色的反应以拒绝原因触发。
#[test]
fn promise_promote_migrates_reject_reactions_to_clone() {
    let mut vm = vm_with_threshold(1);
    vm.run(&Arc::new(compile(
        "(function(){ \
         var p = new Promise(function(_res, rej){ globalThis.rej = rej; }); \
         p.then(undefined, function(e){ globalThis.got = e; }); \
         globalThis.p = p; })(); \
         globalThis.rej('boom'); 0",
    )))
    .expect("run1");

    assert!(vm.session_gc_stats().total_collections >= 1, "执行期收集应已触发（原件随 epoch 释放）");
    assert_eq!(
        vm.lookup_str(global_prop_opt(&vm, "got").expect("got 应被设置")),
        Some("boom".to_string()),
        "晋升前登记的拒绝消费者应在拒绝走克隆后被触发"
    );
}
