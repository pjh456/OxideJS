use std::sync::Arc;

use oxide_kernel::kernel::{KernelConfig, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_log::{Level, SUBSYSTEM_COUNT};
use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

#[test]
fn test_kernel_new() {
    let core = KernelCore::new(KernelConfig::minimal());
    let (i1, _) = core.perm_interner().intern("test");
    let (i2, _) = core.perm_interner().intern("test");
    assert_eq!(i1, i2);
}

#[test]
fn test_kernel_builtins_accessible() {
    let core = KernelCore::new(KernelConfig::minimal());
    let session = KernelSession::new(&core);
    assert!(!session.builtin_world().object_proto.is_function());
    assert!(session.builtin_world().object_constructor.is_function());
}

#[test]
fn test_kernel_shape_forge() {
    let core = KernelCore::new(KernelConfig::minimal());
    assert!(core.shape_forge().get_shape(EMPTY_SHAPE_ID).is_some());
}

#[test]
fn test_kernel_string_forge() {
    let core = KernelCore::new(KernelConfig::minimal());
    let (i1, _) = core.perm_interner().intern("hello");
    let (i2, _) = core.perm_interner().intern("hello");
    assert_eq!(i1, i2);
}

#[test]
fn test_kernel_config_presets() {
    assert_eq!(KernelConfig::minimal().max_pool_size, Some(8));
    assert_eq!(KernelConfig::standard().max_pool_size, Some(32));
    assert_eq!(KernelConfig::minimal().max_steps, None);
    assert_eq!(KernelConfig::standard().max_steps, None);
    assert_eq!(KernelConfig::full().max_steps, None);
    assert_eq!(KernelConfig::minimal().log_levels, [Level::Off; SUBSYSTEM_COUNT]);
    assert!(!KernelConfig::minimal().warmup_builtin_ic);
    assert!(KernelConfig::full().warmup_builtin_ic);
    assert_eq!(KernelConfig::full().max_pool_size, None);
}

#[test]
fn should_rebuild_perm_default_off() {
    // 预设默认 None = 无阈值：任意键数下永不触发，钉死零漂移默认面。
    let core = KernelCore::new(KernelConfig::minimal());
    for i in 0..10 {
        core.perm_interner().intern(&format!("key{i}"));
    }
    assert_eq!(core.should_rebuild_perm(), None);
}

#[test]
fn should_rebuild_perm_exceeds_threshold() {
    let mut config = KernelConfig::minimal();
    config.perm_interner_max_entries = Some(2);
    let core = KernelCore::new(config);
    core.perm_interner().intern("a");
    core.perm_interner().intern("b");
    // 恰在阈值不触发（超阈值，非达到）。
    assert_eq!(core.should_rebuild_perm(), None);
    core.perm_interner().intern("c");
    // 建议上限 = 阈值取 2 的幂后加倍：2 -> 4。
    assert_eq!(core.should_rebuild_perm(), Some(4));
}

#[test]
fn should_rebuild_perm_below_threshold() {
    let mut config = KernelConfig::minimal();
    config.perm_interner_max_entries = Some(100);
    let core = KernelCore::new(config);
    core.perm_interner().intern("a");
    core.perm_interner().intern("b");
    assert_eq!(core.should_rebuild_perm(), None);
}

#[test]
fn test_session_rebuild_shares_forges() {
    let core = KernelCore::new(KernelConfig::minimal());
    let (i1, _) = core.perm_interner().intern("hello");
    let _s2 = KernelSession::new(&core);
    let (i2, _) = core.perm_interner().intern("hello");
    assert_eq!(i1, i2);
}

#[test]
fn snapshot_fresh_session_is_clean() {
    let core = KernelCore::new(KernelConfig::minimal());
    let session = KernelSession::new(&core);
    let dirty = session.dirty_since_snapshot();

    assert!(!dirty.any());
    assert!(!dirty.any_builtin_dirty());
    assert!(!session.is_dirty_since_snapshot());
}

#[test]
fn snapshot_detects_array_dirty() {
    let core = KernelCore::new(KernelConfig::minimal());
    let session = KernelSession::new(&core);

    unsafe { &mut *(session.builtin_world.array_proto.as_ptr() as *mut JsObject) }.bump_generation();
    let dirty = session.dirty_since_snapshot();

    assert!(dirty.array);
    assert!(dirty.any_builtin_dirty());
    assert!(session.is_dirty_since_snapshot());
    assert!(!dirty.global);
    assert!(!dirty.object);
}

#[test]
fn snapshot_detects_global_dirty() {
    let core = KernelCore::new(KernelConfig::minimal());
    let session = KernelSession::new(&core);

    unsafe { &mut *(session.global_object.as_ptr() as *mut JsObject) }.bump_generation();
    let dirty = session.dirty_since_snapshot();

    assert!(dirty.global);
    assert!(dirty.any());
    assert!(!dirty.any_builtin_dirty());
}

#[test]
fn snapshot_detects_stub_dirty() {
    let core = KernelCore::new(KernelConfig::minimal());
    let mut session = KernelSession::new(&core);
    Arc::get_mut(&mut session.builtin_world)
        .expect("fresh session owns its builtin world")
        .stub_objects
        .push(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())));
    session.record_snapshot();

    Arc::get_mut(&mut session.builtin_world)
        .expect("fresh session owns its builtin world")
        .stub_objects
        .push(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())));

    let dirty = session.dirty_since_snapshot();
    assert!(dirty.stubs);
    assert!(dirty.any_builtin_dirty());
    assert!(!dirty.global);
}

#[test]
fn selective_reset_clean_keeps_builtin_world() {
    let core = KernelCore::new(KernelConfig::minimal());
    let mut session = KernelSession::new(&core);
    let world_ptr = Arc::as_ptr(&session.builtin_world);

    let dirty = session.selective_reset(&core);

    assert!(!dirty.any());
    assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&session.builtin_world)));
}

#[test]
fn selective_reset_rebuilds_global_only_when_global_dirty() {
    let core = KernelCore::new(KernelConfig::minimal());
    let mut session = KernelSession::new(&core);
    let world_ptr = Arc::as_ptr(&session.builtin_world);
    let global_ptr = session.global_object.as_ptr();

    unsafe { &mut *(session.global_object.as_ptr() as *mut JsObject) }.bump_generation();
    let dirty = session.selective_reset(&core);

    assert!(dirty.global);
    assert!(!dirty.any_builtin_dirty());
    assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&session.builtin_world)));
    assert!(!std::ptr::eq(global_ptr, session.global_object.as_ptr()));
}

#[test]
fn selective_reset_rebuilds_dirty_builtin_group() {
    let core = KernelCore::new(KernelConfig::minimal());
    let mut session = KernelSession::new(&core);
    let object_proto = session.builtin_world.object_proto.as_ptr();
    let array_proto = session.builtin_world.array_proto.as_ptr();
    let world_ptr = Arc::as_ptr(&session.builtin_world);

    unsafe { &mut *(session.builtin_world.array_proto.as_ptr() as *mut JsObject) }.bump_generation();
    let dirty = session.selective_reset(&core);

    assert!(dirty.array);
    assert!(!std::ptr::eq(world_ptr, Arc::as_ptr(&session.builtin_world)));
    assert!(std::ptr::eq(object_proto, session.builtin_world.object_proto.as_ptr()));
    assert!(!std::ptr::eq(array_proto, session.builtin_world.array_proto.as_ptr()));

    let ctor_proto = session.builtin_world.array_constructor.get_prop_at(0).as_js_object_ptr();
    assert!(std::ptr::eq(ctor_proto, session.builtin_world.array_proto.as_ptr() as *mut JsObject));
}
