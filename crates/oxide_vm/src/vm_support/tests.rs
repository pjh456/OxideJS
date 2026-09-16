//! vm_support 内联测试：全量重置状态清除与全局重建、分配上限、生成器与 BigInt 存活、动态编译及源文本断言行为。
use super::*;

fn global_prop(vm: &Vm, name: &str) -> JsValue {
    global_prop_opt(vm, name).expect("global slot should exist")
}

fn global_prop_opt(vm: &Vm, name: &str) -> Option<JsValue> {
    let global = vm.session.global_object();
    let si = vm.kernel_core.perm_interner().intern(name).0;
    vm.kernel_core
        .shape_forge()
        .lookup_position(global.shape_id(), si)
        .map(|pos| global.get_prop_at(pos))
}

fn run_source(vm: &mut Vm, source: &str) -> JsValue {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = oxide_compiler::compiler::Compiler::new()
        .compile(&program)
        .expect("compile failed");
    vm.run(&Arc::new(module)).expect("vm run failed")
}

/// 计数往返：两条构造路径（独立核/共享核）的登记与 Drop 注销恰好配对，
/// 全部 drop 后计数归零，kernel 可干净 drop（Drop 断言无残留）。
#[test]
fn active_vms_count_roundtrip() {
    let vm = Vm::new();
    assert_eq!(vm.kernel_core.active_vms(), 1);
    drop(vm);

    let core = KernelCore::new(KernelConfig::minimal());
    assert_eq!(core.active_vms(), 0);
    let v1 = Vm::with_kernel_core(Arc::clone(&core));
    let v2 = Vm::with_kernel_core(Arc::clone(&core));
    let v3 = Vm::with_kernel_core(Arc::clone(&core));
    assert_eq!(core.active_vms(), 3);
    drop(v3);
    assert_eq!(core.active_vms(), 2);
    drop(v1);
    drop(v2);
    assert_eq!(core.active_vms(), 0);
    drop(core);
}

/// 守卫判别：持活 VM 调 sweep 触发 debug_assert。VM 声明在 kernel 之后，
/// panic unwind 时先注销计数再 drop kernel，drop 断言不受干扰。
#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "no live VMs"))]
fn sweep_runner_forges_rejects_live_vm() {
    let core = KernelCore::new(KernelConfig::minimal());
    let _vm = Vm::with_kernel_core(Arc::clone(&core));
    core.sweep_runner_forges();
}

#[test]
fn full_reset_with_session_objects_forces_global_rebuild() {
    let mut vm = Vm::new();
    // 池路径场景：`globalThis.Array = {}` 覆盖既有 global 槽（不递增 generation），
    // 新值 `{}` 经 promote 进入 session——global 保留时该指针将悬垂。
    let _ = run_source(&mut vm, "globalThis.Array = {}; 0");
    assert!(!vm.gc_state.session_object_ptrs.is_empty(), "覆盖写应触发 promote 进入 session");
    let old_global = vm.session.global_object.as_ptr();

    vm.full_reset();

    // global 必须重建：旧 global 与其 session 对象随 epoch 释放，Array 恢复内置构造器。
    assert!(!std::ptr::eq(old_global, vm.session.global_object.as_ptr()));
    assert!(std::ptr::eq(
        global_prop(&vm, "Array").as_js_object_ptr(),
        vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
    ));
    assert!(!vm.session.is_dirty_since_snapshot());
}

/// P 原型上方法槽的 wrapper 对象原始指针（rebuild 跨轮复用验证用）。
fn method_wrapper_ptr(vm: &Vm, proto_ptr: *const JsObject, name: &str) -> *const JsObject {
    let si = vm.kernel_core.perm_interner().intern(name).0;
    // SAFETY: proto_ptr 是本 session 的 P 原型，安全点内无并发读者。
    let proto = unsafe { &*proto_ptr };
    let pos = vm
        .kernel_core
        .shape_forge()
        .lookup_position(proto.shape_id(), si)
        .unwrap_or_else(|| panic!("{name} 槽位应存在"));
    let val = proto.get_prop_at(pos);
    assert!(val.is_object(), "{name} 槽位应为 wrapper 对象: {val:?}");
    val.as_js_object_ptr()
}

/// global 属性槽数（槽位原位更新跨轮不追加验证用）。
fn global_slot_count(vm: &Vm) -> usize {
    let g = vm.session.global_object.as_ptr() as *mut JsObject;
    // SAFETY: global 是本 session 对象，安全点内无并发读者。
    unsafe { (*g).prop_vec_len() }
}

/// 选择性重建 wrapper 复用与 global 槽原位更新：每轮「原型脏写 + full_reset」
/// 后存活方法 wrapper 应为同一对象（同 raw 指针、同 shape），释放表计数与
/// global 属性槽数跨轮不增长，重建后方法行为正确。
#[test]
fn full_reset_rebuild_reuses_method_wrappers_across_rounds() {
    let mut vm = Vm::new();
    let _ = run_source(&mut vm, "0");
    let push_before = method_wrapper_ptr(&vm, vm.session.builtin_world().array_proto.as_ptr(), "push");
    let push_shape_before = unsafe { &*push_before }.shape_id();
    let registry_before = vm.session.builtin_world().leaked_object_count();
    let slots_before = global_slot_count(&vm);

    for i in 0..3u32 {
        // 新键写才 bump 原型世代；四家族逐轮全脏，function 家族重建同时
        // 覆盖 wrapper proto 槽重指路径。
        let source = format!(
            "Object.prototype['w{i}'] = 1; Array.prototype['w{i}'] = 2; String.prototype['w{i}'] = 3; \
             Function.prototype['w{i}'] = 4; 0"
        );
        let _ = run_source(&mut vm, &source);
        vm.full_reset();
    }

    let push_after = method_wrapper_ptr(&vm, vm.session.builtin_world().array_proto.as_ptr(), "push");
    assert!(std::ptr::eq(push_before, push_after), "存活方法 wrapper 应复用（同 raw 指针）");
    assert_eq!(push_shape_before, unsafe { &*push_after }.shape_id(), "wrapper shape 应稳定");
    let registry_after = vm.session.builtin_world().leaked_object_count();
    assert!(
        registry_after <= registry_before,
        "释放表计数跨轮不应增长: {registry_before} -> {registry_after}"
    );
    let slots_after = global_slot_count(&vm);
    assert!(slots_after <= slots_before, "global 槽数跨轮不应增长: {slots_before} -> {slots_after}");
    assert_eq!(run_source(&mut vm, "[1, 2].push(3)"), JsValue::int(3));
    assert_eq!(run_source(&mut vm, "String.prototype.charCodeAt.call('A', 0)"), JsValue::int(65));
}

/// global 槽位原位更新锚点：错误/资源栈家族脏重建（子类型构造器经 Box 自建
/// 路径）多轮后 global 属性槽数不增长、Error 槽指向新构造器。
#[test]
fn full_reset_rebuild_keeps_global_slot_count_flat() {
    let mut vm = Vm::new();
    let _ = run_source(&mut vm, "0");
    let slots_before = global_slot_count(&vm);

    for i in 0..3u32 {
        // 新键写脏错误家族（子类型原型重建触发构造器 Box 路径）与对象家族。
        let source =
            format!("Error.prototype['e{i}'] = 1; TypeError.prototype['e{i}'] = 2; Object.prototype['e{i}'] = 3; 0");
        let _ = run_source(&mut vm, &source);
        vm.full_reset();
    }

    let slots_after = global_slot_count(&vm);
    assert!(slots_after <= slots_before, "global 槽数跨轮不应增长: {slots_before} -> {slots_after}");
    // Error 槽应指向本轮重建的构造器（非滞留旧指针）。
    assert!(std::ptr::eq(
        global_prop(&vm, "Error").as_js_object_ptr(),
        vm.session.builtin_world().error_constructor.as_ptr() as *mut JsObject
    ));
    assert!(global_prop(&vm, "TypeError").is_object());
    assert_eq!(run_source(&mut vm, "new TypeError('x') instanceof TypeError"), JsValue::bool(true));
}

fn vm_with_low_threshold() -> Vm {
    let mut cfg = KernelConfig::minimal();
    cfg.set_session_gc_threshold(1);
    Vm::with_kernel_core(KernelCore::new(cfg))
}

/// 单 run 分配上限拦截失控分配：死循环持续 push 的 run 须在触达步数上限前
/// 以 memory limit 错误终止。
#[test]
fn run_alloc_cap_stops_runaway_allocation() {
    let mut cfg = KernelConfig::minimal();
    cfg.max_alloc_bytes = Some(256 * 1024);
    let mut vm = Vm::with_kernel_core(KernelCore::new(cfg));
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, "var a = []; while (true) { a.push(1); }").expect("parse failed");
    let module = oxide_compiler::compiler::Compiler::new()
        .compile(&program)
        .expect("compile failed");
    let err = vm
        .run(&Arc::new(module))
        .expect_err("runaway allocation must hit the alloc cap");
    assert!(err.contains("memory limit"), "unexpected error: {err}");
    assert!(!err.contains("step limit"), "cap 应先于步数上限生效: {err}");
}

/// 上限不误伤正常规模分配：同配置下 1 万元素数组构造正常完成。
#[test]
fn run_alloc_cap_allows_normal_allocation() {
    let mut cfg = KernelConfig::minimal();
    cfg.max_alloc_bytes = Some(4 * 1024 * 1024);
    let mut vm = Vm::with_kernel_core(KernelCore::new(cfg));
    let result = run_source(&mut vm, "var a = []; for (var i = 0; i < 10000; i++) { a.push(i); } a.length");
    assert!(result.is_int(), "expected int length, got {result:?}");
    assert_eq!(result.as_int(), 10000);
}

/// native 终端循环泵送小 JS 重入：每次重入 dispatch 远短于循环内 64 指令采样点，
/// 顶层 steps 不推进——重入边界每 64 hop 强制采样兜底，失控分配以 memory limit
/// 终止而非无限循环。
#[test]
fn reentry_pump_hits_alloc_cap() {
    let mut cfg = KernelConfig::minimal();
    cfg.max_alloc_bytes = Some(256 * 1024);
    let mut vm = Vm::with_kernel_core(KernelCore::new(cfg));
    // 永不 done 的生成器经鸭子对象交给 toArray：native 循环每步泵送两个短重入
    // （闭包调用 + 生成器恢复）并新产一个结果对象，分配无界增长。
    let source = "var g = (function* () { for (var i = 0; ; ++i) { yield i; } })(); \
                  var obj = { next: function () { return g.next(); } }; \
                  Iterator.from(obj).toArray();";
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = oxide_compiler::compiler::Compiler::new()
        .compile(&program)
        .expect("compile failed");
    let err = vm.run(&Arc::new(module)).expect_err("reentry pump must hit the alloc cap");
    assert!(err.contains("memory limit"), "unexpected error: {err}");
}

/// 直接恢复生成器一步：等价 `it.next()`——回归聚焦 GC 搬移后的状态盒
/// 有效性（同 run 内恢复，不经 run 边界换表）。
fn resume_one_step(vm: &mut Vm, gen: JsValue) -> JsValue {
    match vm.resume_generator(gen, crate::generator::GeneratorResumeMode::Next(JsValue::undefined())) {
        Ok(crate::generator::GeneratorStep::Suspended { value }) => value,
        Ok(crate::generator::GeneratorStep::Completed { value }) => value,
        Ok(other) => panic!(
            "unexpected step: {:?}",
            match other {
                crate::generator::GeneratorStep::Thrown { value } => format!("Thrown({value})"),
                crate::generator::GeneratorStep::SuspendedRaw { value } => format!("SuspendedRaw({value})"),
                _ => String::new(),
            }
        ),
        Err(e) => panic!("resume failed: {e}"),
    }
}

#[test]
fn generator_survives_object_sweep_and_resumes() {
    let mut vm = vm_with_low_threshold();
    let _ = run_source(&mut vm, "function* g(){ yield 1; yield 2; } globalThis.it = g(); globalThis.it.next(); 0");

    // 直接触发完整收集（保留执行上下文）：存活生成器克隆进新 arena，
    // 状态盒深拷贝为新 Box（走收集入口而非 run 边界：聚焦 GC 搬移本身）。
    vm.maybe_collect_session_gc();
    assert!(vm.session_gc_stats().total_collections > 0, "应触发对象收集");

    // 从 global 取 sweep 重写后的生成器（同 run，模块表未换发，可恢复）。
    let it = global_prop(&vm, "it");
    assert_eq!(resume_one_step(&mut vm, it), JsValue::int(2), "sweep 后应恢复第二次 yield");
}

#[test]
fn generator_captured_upvalue_survives_sweep() {
    let mut vm = vm_with_low_threshold();
    // x 函数作用域局部：被生成器 g 经 cell 捕获（顶层 var 直连全局属性不走 cell）。
    let _ = run_source(
        &mut vm,
        "function outer() { var x = 0; function* g(){ x++; yield x; x++; yield x; } globalThis.it = g(); globalThis.it.next(); return 0; } outer(); 0",
    );

    vm.maybe_collect_session_gc();
    assert!(vm.session_gc_stats().total_collections > 0, "应触发对象收集");

    // 挂起帧 cell_stack 与闭包 upvalues 中的 cell 独立堆分配（地址稳定），
    // 恢复后继续读写捕获变量。
    let it = global_prop(&vm, "it");
    assert_eq!(resume_one_step(&mut vm, it), JsValue::int(2), "sweep 后应恢复捕获变量读写");
}

#[test]
fn generator_promoted_clone_owns_independent_state_box() {
    let mut vm = Vm::new();
    // `(function(){ var it = g(); it.next(); return it; })()`：it 为函数局部（非顶层
    // var，不经全局属性逃逸），保持 epoch 生成器对象（未 promote）。
    let it = run_source(
        &mut vm,
        "function* g(){ yield 1; yield 2; } (function(){ var it = g(); it.next(); return it; })()",
    );
    assert!(it.is_object());
    let epoch_ptr = it.as_js_object_ptr();
    let epoch_box = unsafe { (*epoch_ptr).native_data() };

    // 手动 promote：克隆应深拷贝状态盒（新 Box），与源盒互不共享。
    let promoted = vm.promote_object(epoch_ptr);
    assert!(!std::ptr::eq(promoted, epoch_ptr));
    let promoted_box = unsafe { (*promoted).native_data() };
    assert!(!std::ptr::eq(epoch_box, promoted_box), "promote 应深拷贝生成器状态盒");

    // 模拟 full_reset 的 epoch 侧释放：源对象与其状态盒随 epoch 回收，
    // 并从追踪表移除登记（克隆的后续回收仍由 VM 统一处理）。
    let _ = crate::session_gc::SessionGc::drop_object_heap_data(epoch_ptr, false);
    vm.gc_state.epoch_object_ptrs.retain(|&p| !std::ptr::eq(p, epoch_ptr));

    // 克隆直接恢复执行：读新盒中的挂起状态，不得悬垂。
    assert_eq!(resume_one_step(&mut vm, JsValue::from_js_object(promoted)), JsValue::int(2));
}

#[test]
fn full_reset_clean_keeps_session_objects() {
    let mut vm = Vm::new();
    let world_ptr = Arc::as_ptr(&vm.session.builtin_world);
    let global_ptr = vm.session.global_object.as_ptr();
    let object_proto_ptr = vm.session.builtin_world().object_proto.as_ptr();

    vm.full_reset();

    assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&vm.session.builtin_world)));
    assert!(std::ptr::eq(global_ptr, vm.session.global_object.as_ptr()));
    assert!(std::ptr::eq(object_proto_ptr, vm.session.builtin_world().object_proto.as_ptr()));
    assert!(!vm.session.is_dirty_since_snapshot());
}

#[test]
fn full_reset_global_dirty_rebuilds_global_and_restores_slots() {
    let mut vm = Vm::new();
    let world_ptr = Arc::as_ptr(&vm.session.builtin_world);
    let global_ptr = vm.session.global_object.as_ptr();
    let global = unsafe { &mut *(vm.session.global_object.as_ptr() as *mut JsObject) };
    bindings::bind_global_value(&vm.kernel_core, global, "userGlobal", JsValue::int(99));
    unsafe { &mut *(vm.session.global_object.as_ptr() as *mut JsObject) }.bump_generation();

    vm.full_reset();

    assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&vm.session.builtin_world)));
    assert!(!std::ptr::eq(global_ptr, vm.session.global_object.as_ptr()));
    assert!(global_prop_opt(&vm, "userGlobal").is_none());
    assert!(std::ptr::eq(
        global_prop(&vm, "Array").as_js_object_ptr(),
        vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
    ));
    assert!(std::ptr::eq(
        global_prop(&vm, "globalThis").as_js_object_ptr(),
        vm.session.global_object.as_ptr() as *mut JsObject
    ));
    assert!(!vm.session.is_dirty_since_snapshot());
}

#[test]
fn full_reset_dirty_builtin_rebinds_global_slot() {
    let mut vm = Vm::new();
    let old_object_proto = vm.session.builtin_world().object_proto.as_ptr();
    let old_array_proto = vm.session.builtin_world().array_proto.as_ptr();
    unsafe { &mut *(old_array_proto as *mut JsObject) }.bump_generation();

    vm.full_reset();

    assert!(std::ptr::eq(old_object_proto, vm.session.builtin_world().object_proto.as_ptr()));
    assert!(!std::ptr::eq(old_array_proto, vm.session.builtin_world().array_proto.as_ptr()));
    assert!(std::ptr::eq(
        global_prop(&vm, "Array").as_js_object_ptr(),
        vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
    ));
    let constructor_si = vm.kernel_core.perm_interner().intern("constructor").0;
    let array_proto = &*vm.session.builtin_world().array_proto;
    let constructor = vm
        .resolve_property(array_proto, constructor_si)
        .expect("Array.prototype.constructor");
    assert!(std::ptr::eq(
        constructor.as_js_object_ptr(),
        vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
    ));
    assert!(!vm.session.is_dirty_since_snapshot());
}

#[test]
fn full_reset_dirty_function_keeps_call_working() {
    let mut vm = Vm::new();
    let function_proto = vm.session.builtin_world().function_proto.as_ptr();
    unsafe { &mut *(function_proto as *mut JsObject) }.bump_generation();

    vm.full_reset();

    // Function 原型家族重建后，未重建家族（array/string/object/map 等）的方法
    // wrapper 是跨重置存活对象，其原型链必须仍能解析出 call/apply/bind：
    // 任何一条路径失效都说明 wrapper 原型指向了已释放的旧 Function 原型。
    assert_eq!(run_source(&mut vm, "Array.prototype.push.call([1], 2)"), JsValue::int(2));
    assert_eq!(run_source(&mut vm, "Array.prototype.push.apply([], [1, 2, 3])"), JsValue::int(3));
    let replaced = run_source(&mut vm, "String.prototype.replace.call('a', 'a', 'b')");
    assert_eq!(vm.lookup_str(replaced).as_deref(), Some("b"));
    let has = run_source(&mut vm, "Object.prototype.hasOwnProperty.call({x: 1}, 'x')");
    assert_eq!(has, JsValue::bool(true));
    let fixed = run_source(&mut vm, "Number.prototype.toFixed.call(1.5, 1)");
    assert_eq!(vm.lookup_str(fixed).as_deref(), Some("1.5"));
    let in_map = run_source(&mut vm, "Map.prototype.has.call(new Map([[1, 2]]), 1)");
    assert_eq!(in_map, JsValue::bool(true));
    let mapped = run_source(&mut vm, "Array.prototype.map.call([1, 2], function(x){ return x + 1; }).join(',')");
    assert_eq!(vm.lookup_str(mapped).as_deref(), Some("2,3"));
    assert!(!vm.session.is_dirty_since_snapshot());
}

/// Function 家族脏重建：未重建家族的方法 wrapper 是跨重置存活对象，其 proto
/// 槽（绑定时固化为旧 fn_proto）必须被重指到新 fn_proto，旧原型链不可再读。
#[test]
fn full_reset_dirty_function_repoints_retained_wrapper_proto() {
    let mut vm = Vm::new();
    let old_fn_proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    // 保留方法 wrapper：array 家族不重建，wrapper 对象与其 proto 槽跨重置存活。
    let array_proto = vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
    let push_si = vm.kernel_core.perm_interner().intern("push").0;
    let push = vm
        .resolve_property(unsafe { &*array_proto }, push_si)
        .expect("Array.prototype.push");
    let push_ptr = push.as_js_object_ptr();
    assert!(std::ptr::eq(unsafe { (*push_ptr).proto().as_js_object_ptr() }, old_fn_proto));

    unsafe { &mut *old_fn_proto }.bump_generation();

    vm.full_reset();

    let new_fn_proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    assert!(!std::ptr::eq(new_fn_proto, old_fn_proto));
    // 保留 wrapper proto 槽已重指新 fn_proto，call 链走新原型。
    assert!(std::ptr::eq(unsafe { (*push_ptr).proto().as_js_object_ptr() }, new_fn_proto));
    assert_eq!(run_source(&mut vm, "Array.prototype.push.call([1], 2)"), JsValue::int(2));
    assert!(!vm.session.is_dirty_since_snapshot());
}

/// 新键写脏 object/array/string/function 四家族（S2 脏源形态）：full_reset 后
/// 跨家族读语义保持——保留 wrapper 原型链经重指后取 call、同家族
/// constructor/prototype 对自洽、保留原型链指向新 Object.prototype。
#[test]
fn full_reset_dirty_four_families_cross_family_reads() {
    let mut vm = Vm::new();
    run_source(
        &mut vm,
        "Object.prototype['d'] = 1; Array.prototype['d'] = 2; String.prototype['d'] = 3; Function.prototype['d'] = 4;",
    );
    assert!(vm.session.is_dirty_since_snapshot());

    vm.full_reset();

    assert_eq!(run_source(&mut vm, "Array.prototype.push.call([1], 2)"), JsValue::int(2));
    let replaced = run_source(&mut vm, "String.prototype.replace.call('a', 'a', 'b')");
    assert_eq!(vm.lookup_str(replaced).as_deref(), Some("b"));
    let has = run_source(&mut vm, "Object.prototype.hasOwnProperty.call({x: 1}, 'x')");
    assert_eq!(has, JsValue::bool(true));
    assert_eq!(
        run_source(&mut vm, "Object.getPrototypeOf(Array.prototype) === Object.prototype"),
        JsValue::bool(true)
    );
    assert_eq!(run_source(&mut vm, "Array.prototype.constructor === Array"), JsValue::bool(true));
    assert!(!vm.session.is_dirty_since_snapshot());
}

#[test]
fn session_epoch_survives_reset() {
    let mut vm = Vm::new();
    let session_ptr = vm.gc_state.session_epoch.alloc(123i32) as *mut i32;

    vm.reset();

    assert!(unsafe { *session_ptr } == 123);
}

/// 未捕获异常侧通道是执行期状态：原生错误后可能残留原值，reset/full_reset
/// 边界须随执行状态一并清空——否则其持有的 epoch 对象指针在池回收后悬垂
/// （后续原生错误经 raise_call_error 消费残留值即 UAF）。
#[test]
fn reset_drops_stale_uncaught_value() {
    let mut vm = Vm::new();
    let _ = run_source(&mut vm, "0");
    vm.last_uncaught_value = Some(JsValue::float(42.0));
    vm.reset();
    assert!(vm.last_uncaught_value.is_none(), "reset 应清空未捕获异常侧通道");

    vm.last_uncaught_value = Some(JsValue::float(42.0));
    vm.full_reset();
    assert!(vm.last_uncaught_value.is_none(), "full_reset 应清空未捕获异常侧通道");
}

/// run 边界换新 Bump 后双 arena 保留锚恰 0：重源 run 冲高水位，
/// full_reset 后 epoch/session 两 arena 均空（容量不跨 reset 保留）。
#[test]
fn full_reset_zeroes_arena_retained() {
    let mut vm = Vm::new();
    run_source(
        &mut vm,
        "var t = 0; for (var i = 0; i < 20000; i++) { var o = { s: 'ab' + i, a: [i] }; t += o.s.length + o.a.length; } t",
    );
    assert!(vm.epoch.bump().allocated_bytes() > 0, "重源 run 应冲高 epoch arena 水位");

    vm.full_reset();

    assert_eq!(vm.epoch.bump().allocated_bytes(), 0);
    assert_eq!(vm.gc_state.session_epoch.allocated_bytes(), 0);
}

/// 直 session 分配站（函数对象 + prototype 子对象）计入 session 堆账目：
/// 分配点计数 = 对象头（属性区容量扩张属 promote 同口径的既有盲区，不钉）。
/// 另含 CREATE_CLOSURE 站将推断函数名物化为 session 串（name 数据属性，
/// 既有行为），钉值一并计入。
#[test]
fn direct_session_alloc_counted_in_session_bytes() {
    let mut vm = Vm::new();
    let base = vm.session_bytes_allocated();
    run_source(&mut vm, "var f = function(){}; 0");
    let delta = vm.session_bytes_allocated() - base;
    let name_len = "f".len();
    assert_eq!(delta, 2 * std::mem::size_of::<JsObject>() + std::mem::size_of::<JsString>() + name_len);
}

/// 标签模板 cooked/raw 数组直 session 分配计入 session 堆账目：
/// 含模板的 run 账目增量须超出同等函数对象口径（两数组各含对象头 + 元素区）。
/// 单 run 内完成（标签函数定义 + 模板调用）：账目口径与函数对象钉同 run 对齐。
#[test]
fn tagged_template_object_counted_in_session_bytes() {
    let mut vm = Vm::new();
    let base = vm.session_bytes_allocated();
    // 对照 run：仅函数对象（本体 + prototype 子对象）。
    run_source(&mut vm, "var tag = function(){ return 0; }; 0");
    let fn_cost = vm.session_bytes_allocated() - base;
    // 实验 run：同函数 + 标签模板（cookeD/raw 两数组）。
    let before = vm.session_bytes_allocated();
    run_source(&mut vm, "var tag2 = function(){ return 0; }; tag2`a${1}b`; 0");
    let both_cost = vm.session_bytes_allocated() - before;
    assert!(
        both_cost > fn_cost + 2 * std::mem::size_of::<JsObject>(),
        "模板数组未计入账目: both={both_cost} fn={fn_cost}"
    );
}

#[test]
fn immutables_filled_once_per_module() {
    let mut vm = Vm::new();
    // `f` 递归（同一子模块进入 4 次），其不可变常量经 OnceLock 只转换一次。
    let result = run_source(&mut vm, "function f(n){ if(n<=0){ return 'done'; } return f(n-1); } f(3)");
    assert!(result.is_string());
    assert_eq!(vm.lookup_str(result).as_deref(), Some("done"));
    // 缓存 = 顶层模块 + 1 个子模块（f）；子模块槽由这些调用初始化。
    assert_eq!(vm.current_table().immutables.len(), 2);
    // 子模块常量改走 temp_immutables（避免缓存下标冲突）
}

#[test]
fn dynamic_function_basic_arity() {
    let mut vm = Vm::new();
    // 与等价静态函数返回值逐位一致（引擎函数调用统一产 Double 数值）。
    let expected = run_source(&mut vm, "function f(a,b){return a+b} f(3,4)");
    let result = run_source(&mut vm, "new Function('a','b','return a+b')(3,4)");
    assert_eq!(result, expected);
}

#[test]
fn dynamic_function_called_without_new() {
    let mut vm = Vm::new();
    let expected = run_source(&mut vm, "function f(a,b){return a*b} f(6,7)");
    let result = run_source(&mut vm, "Function('a','b','return a*b')(6,7)");
    assert_eq!(result, expected);
}

#[test]
fn dynamic_function_empty_body_returns_undefined() {
    let mut vm = Vm::new();
    assert!(run_source(&mut vm, "Function()()").is_undefined());
}

#[test]
fn dynamic_function_syntax_error_throws_syntax_error() {
    let mut vm = Vm::new();
    let result = run_source(&mut vm, "try{new Function('return {{')}catch(e){e.name}");
    assert!(result.is_string());
    assert_eq!(vm.lookup_str(result).as_deref(), Some("SyntaxError"));
}

#[test]
fn dynamic_function_nested_closure_renumbering() {
    let mut vm = Vm::new();
    // 匿名函数体声明嵌套函数 g，返回值是引用 g 的闭包：验证子树 flat_id 重编号
    // 与 CREATE_CLOSURE imm16 重写后嵌套调用仍指向正确的子模块。
    let expected = run_source(
        &mut vm,
        "function outer(){var g=function(n){return n*2}; return function(){return g(21)}} outer()()",
    );
    let result = run_source(
        &mut vm,
        "new Function('var g=function(n){return n*2}; return function(){return g(21)}')()()",
    );
    assert_eq!(result, expected);
}

#[test]
fn dynamic_function_multiple_in_one_run() {
    let mut vm = Vm::new();
    // 同一 run 内连续创建多个动态函数：验证 base 偏移累计正确。
    let expected = run_source(
        &mut vm,
        "function f1(){return 1} function f2(){return 2} function f3(a){return a*3} f1()+f2()+f3(4)",
    );
    let result = run_source(
        &mut vm,
        "new Function('return 1')() + new Function('return 2')() + new Function('a','return a*3')(4)",
    );
    assert_eq!(result, expected);
}

#[test]
fn dynamic_function_name_and_length() {
    let mut vm = Vm::new();
    let name = run_source(&mut vm, "var f=new Function('a','b','return a'); f.name");
    assert!(name.is_string());
    assert_eq!(vm.lookup_str(name).as_deref(), Some("anonymous"));
    let len = run_source(&mut vm, "var f=new Function('a','b','return a'); f.length");
    assert_eq!(len, JsValue::int(2));
}

#[test]
fn dynamic_function_comma_split_params_count() {
    let mut vm = Vm::new();
    // 单个实参 "a,b,c" 拼接解析为 3 个形参，length 应为解析后的形参数。
    let result = run_source(&mut vm, "new Function('a,b,c','null').length");
    assert_eq!(result, JsValue::int(3));
}

#[test]
fn dynamic_function_name_and_length_attributes() {
    let mut vm = Vm::new();
    // length/name 为不可写、不可枚举、可配置的数据属性。
    let attrs = run_source(
        &mut vm,
        "var d=Object.getOwnPropertyDescriptor(new Function('a','return a'),'length'); String(d.value)+d.writable+d.enumerable+d.configurable",
    );
    assert_eq!(vm.lookup_str(attrs).as_deref(), Some("1falsefalsetrue"));
    let name_attrs = run_source(
        &mut vm,
        "var d=Object.getOwnPropertyDescriptor(Function(),'name'); String(d.writable)+d.enumerable+d.configurable",
    );
    assert_eq!(vm.lookup_str(name_attrs).as_deref(), Some("falsefalsetrue"));
}

#[test]
fn dynamic_function_rethrows_to_string_exception() {
    let mut vm = Vm::new();
    // 形参 ToString 回调抛出的原始值须原样传播，而非包成 TypeError。
    let result = run_source(&mut vm, "try{new Function({toString:function(){throw 7}})}catch(e){e}");
    assert_eq!(result, JsValue::int(7));
}

#[test]
fn session_epoch_replacement_is_only_in_full_reset_state_clear() {
    let src = include_str!("../vm_support.rs");
    let production = src.split("#[cfg(test)]").next().expect("production source");
    assert_eq!(production.matches("self.gc_state.session_epoch = bumpalo::Bump::new()").count(), 1);
    assert!(production.contains("fn clear_full_reset_state(&mut self)"));
    assert!(production.contains("self.gc_state.session_epoch = bumpalo::Bump::new();"));
}

#[test]
fn full_reset_refreshes_object_prototype_after_object_dirty() {
    let mut vm = Vm::new();
    let old_object_proto = vm.session.builtin_world().object_proto.as_ptr();
    unsafe { &mut *(old_object_proto as *mut JsObject) }.bump_generation();

    vm.full_reset();

    assert!(!std::ptr::eq(old_object_proto, vm.session.builtin_world().object_proto.as_ptr()));
    assert!(std::ptr::eq(
        vm.object_prototype.as_ptr(),
        vm.session.builtin_world().object_proto.as_ptr()
    ));
    assert!(!vm.session.is_dirty_since_snapshot());
}

#[test]
fn full_reset_object_dirty_rebinds_iterator_family() {
    let mut vm = Vm::new();
    let old_object_proto = vm.session.builtin_world().object_proto.as_ptr();
    let old_iterator_proto = vm.session.builtin_world().iterator_proto.as_ptr();
    // 用户修改 Object.prototype：object 家族世代递增，global 未动。
    unsafe { &mut *(old_object_proto as *mut JsObject) }.bump_generation();

    vm.full_reset();

    // object 家族与迭代器原型全部重建（新原型链到新 Object.prototype）。
    assert!(!std::ptr::eq(old_object_proto, vm.session.builtin_world().object_proto.as_ptr()));
    assert!(!std::ptr::eq(old_iterator_proto, vm.session.builtin_world().iterator_proto.as_ptr()));
    // global 保留（dirty.global=false）：其 Iterator 函数对象的 prototype
    // 属性须对齐到重建后的 %IteratorPrototype%。
    let iter_val = global_prop(&vm, "Iterator");
    let si_prototype = vm.kernel_core.perm_interner().intern("prototype").0;
    let iter_obj = unsafe { &*iter_val.as_js_object_ptr() };
    let proto_pos = vm
        .kernel_core
        .shape_forge()
        .lookup_position(iter_obj.shape_id(), si_prototype)
        .expect("Iterator should have prototype slot");
    assert!(std::ptr::eq(
        iter_obj.get_prop_at(proto_pos).as_js_object_ptr(),
        vm.session.builtin_world().iterator_proto.as_ptr() as *mut JsObject
    ));
    // full_reset 后 session 干净；此后 run_source 执行才重新累积世代变化。
    assert!(!vm.session.is_dirty_since_snapshot());
    // 迭代器家族功能完整：原型 next 就位，for-of/spread/Array.from/Iterator.from/
    // Map/Set/String/yield* 全部可用，原型链与 Iterator.prototype 一致。
    let r = run_source(&mut vm, "[...[1,2,3]].join(',')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2,3"));
    let r = run_source(&mut vm, "Array.from([1,2]).join(',')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2"));
    let r = run_source(&mut vm, "Iterator.from({next:function(){return {value:42,done:false}}}).next().value");
    assert_eq!(r, JsValue::int(42));
    let r = run_source(&mut vm, "[...new Map([[1,2],[3,4]])].map(function(x){return x.join(':')}).join(';')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("1:2;3:4"));
    let r = run_source(&mut vm, "[...new Set([1,2,3])].join(',')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2,3"));
    let r = run_source(&mut vm, "[...'ab'].join(',')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("a,b"));
    let r = run_source(&mut vm, "function* g(){yield* [1,2]} [...g()].join(',')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2"));
    let r = run_source(
        &mut vm,
        "Object.getPrototypeOf(Object.getPrototypeOf([].values())) === Iterator.prototype",
    );
    assert_eq!(r, JsValue::bool(true));
    let r = run_source(&mut vm, "var it=[].values(); it[Symbol.iterator]()===it");
    assert_eq!(r, JsValue::bool(true));
}

#[test]
fn full_reset_global_dirty_keeps_iterator_proto_slots_stable() {
    let mut vm = Vm::new();
    let arr_iter_proto = vm.session.builtin_world().array_iterator_proto.as_ptr() as *mut JsObject;
    let slots_before = unsafe { &*arr_iter_proto }.hash_props_vec().map_or(0, |v| v.len());
    // global 世代递增（用户写 global），builtin 家族未动。
    unsafe { &mut *(vm.session.global_object.as_ptr() as *mut JsObject) }.bump_generation();

    vm.full_reset();

    // builtin 未脏 → 迭代器原型保留原对象且属性槽不膨胀（重复 full_reset 不再追加）。
    let arr_iter_proto_after = vm.session.builtin_world().array_iterator_proto.as_ptr() as *mut JsObject;
    assert!(std::ptr::eq(arr_iter_proto, arr_iter_proto_after));
    let slots_after = unsafe { &*arr_iter_proto_after }.hash_props_vec().map_or(0, |v| v.len());
    assert_eq!(slots_before, slots_after);
    assert!(!vm.session.is_dirty_since_snapshot());
    // 迭代器功能经保留原型仍完整（run_source 起再次累积世代变化）。
    let r = run_source(&mut vm, "[...[1,2,3]].join(',')");
    assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2,3"));
    let r = run_source(&mut vm, "var it=new Set([1]).values(); it[Symbol.iterator]()===it");
    assert_eq!(r, JsValue::bool(true));
}

#[test]
fn bigint_literal_arithmetic_and_comparison() {
    let mut vm = Vm::new();
    assert_eq!(run_source(&mut vm, "100n + 23n"), run_source(&mut vm, "123n"));
    assert_eq!(run_source(&mut vm, "100n - 30n"), run_source(&mut vm, "70n"));
    assert_eq!(run_source(&mut vm, "7n * 6n"), run_source(&mut vm, "42n"));
    assert_eq!(run_source(&mut vm, "10n / 4n"), run_source(&mut vm, "2n"));
    assert_eq!(run_source(&mut vm, "10n % 3n"), run_source(&mut vm, "1n"));
    assert_eq!(run_source(&mut vm, "-7n"), run_source(&mut vm, "0n - 7n"));
    assert_eq!(run_source(&mut vm, "123n == 123n"), JsValue::bool(true));
    assert_eq!(run_source(&mut vm, "123n === 123n"), JsValue::bool(true));
    assert_eq!(run_source(&mut vm, "5n < 3n"), JsValue::bool(false));
    assert_eq!(run_source(&mut vm, "5n > 3n"), JsValue::bool(true));
    assert_eq!(run_source(&mut vm, "1n === 1"), JsValue::bool(false));
    let typeof_result = run_source(&mut vm, "typeof 123n");
    assert!(typeof_result.is_string());
    assert_eq!(vm.lookup_str(typeof_result).as_deref(), Some("bigint"));
}

#[test]
fn bigint_constructor_and_string() {
    let mut vm = Vm::new();
    assert_eq!(run_source(&mut vm, "BigInt(42)"), run_source(&mut vm, "42n"));
    assert_eq!(run_source(&mut vm, "BigInt('123')"), run_source(&mut vm, "123n"));
    assert_eq!(run_source(&mut vm, "BigInt('0x10')"), run_source(&mut vm, "16n"));
    let s = run_source(&mut vm, "String(123n)");
    assert!(s.is_string());
    assert_eq!(vm.lookup_str(s).as_deref(), Some("123"));
    let ts = run_source(&mut vm, "(123n).toString()");
    assert!(ts.is_string());
    assert_eq!(vm.lookup_str(ts).as_deref(), Some("123"));
    assert_eq!(run_source(&mut vm, "Number(5n)"), JsValue::int(5));
}

#[test]
fn bigint_mixed_type_throws() {
    let mut vm = Vm::new();
    let te = run_source(&mut vm, "try { 1n + 1 } catch(e) { e.name }");
    assert_eq!(vm.lookup_str(te).as_deref(), Some("TypeError"));
    let re = run_source(&mut vm, "try { 1n / 0n } catch(e) { e.name }");
    assert_eq!(vm.lookup_str(re).as_deref(), Some("RangeError"));
    let ne = run_source(&mut vm, "try { new BigInt(1) } catch(e) { e.name }");
    assert_eq!(vm.lookup_str(ne).as_deref(), Some("TypeError"));
}

#[test]
fn bigint_survives_reset_and_gc() {
    let mut vm = Vm::new();
    let result = run_source(&mut vm, "100n + 23n");
    assert!(result.is_bigint());
    assert_eq!(vm.bigint_value(result), &num_bigint::BigInt::from(123));
    vm.reset();
    // reset 保留 session 字符串/bigint box：值仍可读。
    assert_eq!(vm.bigint_value(result), &num_bigint::BigInt::from(123));
}

#[test]
fn bigint_wrapped_and_number_comparison() {
    let mut vm = Vm::new();
    // 包装对象 coerce 后双 BigInt 运算。
    assert_eq!(run_source(&mut vm, "Object(2n) / 2n"), run_source(&mut vm, "1n"));
    assert_eq!(run_source(&mut vm, "Object(2n) * 3n"), run_source(&mut vm, "6n"));
    assert_eq!(run_source(&mut vm, "2n + Object(3n)"), run_source(&mut vm, "5n"));
    // BigInt 与 Number 精确关系比较（超出 f64 精度仍精确）。
    assert_eq!(run_source(&mut vm, "9007199254740993n > 9007199254740992"), JsValue::bool(true));
    assert_eq!(run_source(&mut vm, "9007199254740993n < 9007199254740994"), JsValue::bool(true));
    assert_eq!(run_source(&mut vm, "2n < 3"), JsValue::bool(true));
    assert_eq!(run_source(&mut vm, "3n >= 3"), JsValue::bool(true));
    // NaN 关系比较为 false。
    assert_eq!(run_source(&mut vm, "0n < NaN"), JsValue::bool(false));
    // 混合算术抛 TypeError。
    let te = run_source(&mut vm, "try { Object(1n) - 1 } catch(e) { e.name }");
    assert_eq!(vm.lookup_str(te).as_deref(), Some("TypeError"));
}

#[test]
fn reset_clears_runtime_state_like_rerun() {
    let mut vm = Vm::new();
    vm.regs[1] = JsValue::int(7);
    vm.pc = 3;
    vm.frames.push(crate::vm::CallFrame {
        return_addr: 1,
        function_name: 0,
        caller_reg_limit: 2,
        caller_active_reg_limit: 2,
        saved_reg_offset: 0,
        spill_offset: 0,
        arguments_base: 0,
        arguments_count: 0,
        saved_this: JsValue::undefined(),
        saved_new_target: JsValue::undefined(),
        callee: JsValue::undefined(),
        construct_result_reg: None,
        strict: false,
        constructed_this: None,
        is_derived_constructor: false,
        super_called: false,
        continuation: crate::vm::FrameContinuation::None,
    });
    vm.save_stack.push(JsValue::undefined());
    vm.iters
        .for_in_iters
        .push(std::ptr::dangling_mut::<crate::vm::ForInIter<'static>>());
    vm.iters.for_of_iters.push(crate::vm_state::ForOfEntry {
        iterator: JsValue::undefined(),
        last_result: JsValue::undefined(),
        is_async: false,
    });
    vm.saved_bytecode_stack.push(Arc::from(vec![oxide_bytecode::opcode::encode(
        oxide_bytecode::opcode::OpCode::HALT,
        0,
        0,
        0,
    )]));
    vm.saved_immutables_stack
        .push(std::ptr::slice_from_raw_parts(std::ptr::null(), 0));
    vm.try_stack.push(crate::vm::TryHandler {
        catch_pc: Some(1),
        finally_pc: None,
        finally_active: false,
        frame_depth: 0,
        for_of_depth: 0,
    });
    vm.exception_value = Some(JsValue::int(2));
    vm.pending_exception = Some(JsValue::int(3));
    vm.pending_error_kind = Some("TypeError");

    vm.reset();

    assert_eq!(vm.pc, 0);
    assert!(vm.frames.is_empty());
    assert!(vm.save_stack.is_empty());
    assert!(vm.iters.for_in_iters.is_empty());
    assert!(vm.iters.for_of_iters.is_empty());
    assert!(vm.saved_bytecode_stack.is_empty());
    assert!(vm.saved_immutables_stack.is_empty());
    assert!(vm.try_stack.is_empty());
    assert!(vm.exception_value.is_none());
    assert!(vm.pending_exception.is_none());
    assert!(vm.pending_error_kind.is_none());
    assert!(vm.bytecode.is_empty());
    assert!(vm.immutables().is_empty());
}

#[test]
fn full_reset_clears_symbol_state() {
    let mut vm = Vm::new();
    vm.symbols.intern(Some("shared".to_string()));

    vm.full_reset();

    assert_eq!(vm.symbols.symbol_counter, 0);
    assert!(vm.symbols.symbol_descriptions.is_empty());
    assert!(vm.symbols.symbol_registry.is_empty());
}
