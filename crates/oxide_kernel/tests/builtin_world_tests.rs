use oxide_kernel::builtin::BuiltinWorld;
use oxide_kernel::kernel::BuiltinDirtySet;
use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::PermInterner;
use oxide_types::object::JsObject;

fn make_world() -> BuiltinWorld {
    let sf = PermInterner::new();
    let sh = ShapeForge::new();
    BuiltinWorld::new(&sf, &sh)
}

#[test]
fn test_all_protos_valid() {
    let w = make_world();
    let protos = [
        &w.object_proto,
        &w.array_proto,
        &w.function_proto,
        &w.string_proto,
        &w.number_proto,
        &w.boolean_proto,
        &w.error_proto,
        &w.symbol_proto,
    ];
    for p in protos {
        assert!(p.shape_id() > EMPTY_SHAPE_ID, "proto should have a non-empty shape");
    }
}

#[test]
fn test_all_constructors_valid() {
    let w = make_world();
    assert!(w.object_constructor.is_function());
    assert!(w.array_constructor.is_function());
    assert!(w.function_constructor.is_function());
    assert!(w.string_constructor.is_function());
    assert!(w.number_constructor.is_function());
    assert!(w.boolean_constructor.is_function());
    assert!(w.error_constructor.is_function());
    assert!(w.symbol_constructor.is_function());
}

#[test]
fn test_prototypes_are_not_functions() {
    let w = make_world();
    assert!(!w.object_proto.is_function());
    assert!(!w.array_proto.is_function());
    assert!(!w.function_proto.is_function());
    assert!(!w.string_proto.is_function());
    assert!(!w.number_proto.is_function());
    assert!(!w.boolean_proto.is_function());
    assert!(!w.error_proto.is_function());
    assert!(!w.symbol_proto.is_function());
}

#[test]
fn test_protos_have_null_proto() {
    let w = make_world();
    // Object.prototype 是根——其 __proto__ 为 null。
    assert!(w.object_proto.proto().is_null());
    // 其余构造器原型均继承自 Object.prototype。
    assert!(w.array_proto.proto().is_object());
    assert!(w.function_proto.proto().is_object());
    assert!(w.string_proto.proto().is_object());
    assert!(w.number_proto.proto().is_object());
    assert!(w.boolean_proto.proto().is_object());
    assert!(w.error_proto.proto().is_object());
    assert!(w.symbol_proto.proto().is_object());
}

#[test]
fn test_shapes_populated() {
    let w = make_world();
    assert!(
        w.object_constructor.shape_id() > EMPTY_SHAPE_ID,
        "constructor should have prototype + name shape"
    );
    assert!(w.object_proto.shape_id() > EMPTY_SHAPE_ID, "prototype should have constructor shape");
}

#[test]
fn builtin_rebuild_with_dirty_reuses_clean_fields() {
    let sf = PermInterner::new();
    let sh = ShapeForge::new();
    let w = BuiltinWorld::new(&sf, &sh);
    let rebuilt = BuiltinWorld::rebuild_with_dirty(&w, &sf, &sh, &BuiltinDirtySet::default());

    assert!(std::ptr::eq(w.object_proto.as_ptr(), rebuilt.object_proto.as_ptr()));
    assert!(std::ptr::eq(w.array_proto.as_ptr(), rebuilt.array_proto.as_ptr()));
    assert!(std::ptr::eq(w.function_proto.as_ptr(), rebuilt.function_proto.as_ptr()));
}

#[test]
fn builtin_rebuild_with_dirty_replaces_only_dirty_group() {
    let sf = PermInterner::new();
    let sh = ShapeForge::new();
    let w = BuiltinWorld::new(&sf, &sh);
    let dirty = BuiltinDirtySet {
        array: true,
        ..Default::default()
    };
    let rebuilt = BuiltinWorld::rebuild_with_dirty(&w, &sf, &sh, &dirty);

    assert!(!std::ptr::eq(w.array_proto.as_ptr(), rebuilt.array_proto.as_ptr()));
    assert!(!std::ptr::eq(w.array_constructor.as_ptr(), rebuilt.array_constructor.as_ptr()));
    assert!(std::ptr::eq(w.object_proto.as_ptr(), rebuilt.object_proto.as_ptr()));
    assert!(std::ptr::eq(w.function_proto.as_ptr(), rebuilt.function_proto.as_ptr()));
}

#[test]
fn builtin_rebuild_with_dirty_repairs_ctor_proto_links() {
    let sf = PermInterner::new();
    let sh = ShapeForge::new();
    let w = BuiltinWorld::new(&sf, &sh);
    let dirty = BuiltinDirtySet {
        array: true,
        ..Default::default()
    };
    let rebuilt = BuiltinWorld::rebuild_with_dirty(&w, &sf, &sh, &dirty);

    let ctor_proto = rebuilt.array_constructor.get_prop_at(0).as_js_object_ptr();
    let proto_ctor = rebuilt.array_proto.get_prop_at(0).as_js_object_ptr();
    assert!(std::ptr::eq(ctor_proto, rebuilt.array_proto.as_ptr() as *mut JsObject));
    assert!(std::ptr::eq(proto_ctor, rebuilt.array_constructor.as_ptr() as *mut JsObject));
}
