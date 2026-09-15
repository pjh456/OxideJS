use super::*;
use crate::shape::EMPTY_SHAPE_ID;

#[test]
fn object_size_bounds() {
    let sz = std::mem::size_of::<JsObject>();
    assert!(sz <= 256, "JsObject grew unexpectedly: {sz}B");
}

#[test]
fn new_empty_defaults() {
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert_eq!(obj.shape_id(), EMPTY_SHAPE_ID);
    assert_eq!(obj.prop_count(), 0);
    assert!(obj.is_extensible());
    assert!(!obj.is_array());
    assert!(!obj.is_function());
    assert!(!obj.is_session_epoch());
    assert_eq!(obj.generation(), 1);
    assert!(obj.hash_props_vec().is_none());
    assert!(!obj.has_prop_meta());
}

#[test]
fn session_epoch_marker_roundtrip() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert!(!obj.is_session_epoch());
    obj.set_session_epoch(true);
    assert!(obj.is_session_epoch());
    obj.set_session_epoch(false);
    assert!(!obj.is_session_epoch());
}

#[test]
fn session_epoch_marker_preserves_gc_mark_bit() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert!(!obj.is_gc_marked());
    obj.set_gc_mark(true);
    assert!(obj.is_gc_marked());
    assert!(!obj.is_session_epoch());

    obj.set_session_epoch(true);
    assert!(obj.is_session_epoch());
    assert!(obj.is_gc_marked());

    obj.set_gc_mark(false);
    assert!(!obj.is_gc_marked());
}

#[test]
fn session_epoch_marker_keeps_object_size_bound() {
    let sz = std::mem::size_of::<JsObject>();
    assert!(sz <= 256, "JsObject grew unexpectedly: {sz}B");
}

#[test]
fn clone_for_session_epoch_marks_clone_and_does_not_alias_hash_props() {
    let mut source = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    source.set_prop_at(0, JsValue::int(1));

    let mut clone = source.clone_for_session_epoch();
    assert!(clone.is_session_epoch());
    clone.set_prop_at(0, JsValue::int(2));

    assert_eq!(source.get_prop_at(0), JsValue::int(1));
    assert_eq!(clone.get_prop_at(0), JsValue::int(2));
}

#[test]
fn clone_for_session_epoch_does_not_alias_prop_meta() {
    let mut source = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    source.set_prop_at(0, JsValue::undefined());
    source.set_accessor_meta(0, JsValue::int(10), JsValue::int(11), PropAttributes::DEFAULT_DATA);

    let mut clone = source.clone_for_session_epoch();
    clone.set_accessor_meta(0, JsValue::int(20), JsValue::int(21), PropAttributes::DEFAULT_DATA);

    let source_meta = source.prop_meta_at(0).expect("source meta");
    let clone_meta = clone.prop_meta_at(0).expect("clone meta");
    assert_eq!(source_meta.get, JsValue::int(10));
    assert_eq!(source_meta.set, JsValue::int(11));
    assert_eq!(clone_meta.get, JsValue::int(20));
    assert_eq!(clone_meta.set, JsValue::int(21));
}

#[test]
fn shape_id_roundtrip() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.set_shape_id(0x00AB_CDEF);
    assert_eq!(obj.shape_id(), 0x00AB_CDEF);
}

#[test]
fn prop_count_roundtrip() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert_eq!(obj.prop_count(), 0);
    obj.ensure_hash_props().push(JsValue::int(17));
    assert_eq!(obj.prop_count(), 1);
}

#[test]
fn flags_individual() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert!(obj.is_extensible());
    obj.set_extensible(false);
    assert!(!obj.is_extensible());
}

#[test]
fn hash_prop_read_write() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.set_prop_at(0, JsValue::int(42));
    assert_eq!(obj.get_prop_at(0), JsValue::int(42));
}

#[test]
fn new_array_flags() {
    let bump = bumpalo::Bump::new();
    let obj = JsObject::new_array(5, JsValue::null(), 3, &bump);
    assert!(obj.is_array());
    assert_eq!(obj.shape_id(), 5);
    assert_eq!(obj.prop_count(), 3);
}

#[test]
fn generation_bump() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert_eq!(obj.generation(), 1);
    obj.bump_generation();
    assert_eq!(obj.generation(), 2);
}

#[test]
fn hash_props_lazy_init() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    assert!(obj.hash_props_vec().is_none());
    assert_eq!(obj.prop_count(), 0);
    obj.set_prop_at(0, JsValue::int(1));
    assert!(obj.hash_props_vec().is_some());
    assert_eq!(obj.prop_count(), 1);
}

#[test]
fn hash_props_flat_storage_roundtrip() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.set_prop_at(0, JsValue::int(100));
    obj.set_prop_at(1, JsValue::int(200));
    assert_eq!(obj.get_prop_at(0), JsValue::int(100));
    assert_eq!(obj.get_prop_at(1), JsValue::int(200));
}

#[test]
fn prop_meta_lazy_init() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.set_prop_at(0, JsValue::int(1));
    assert!(!obj.has_prop_meta());

    obj.set_data_meta(0, PropAttributes::new(false, true, false));
    assert!(obj.has_prop_meta());
    let meta = obj.prop_meta_at(0).expect("meta");
    assert!(!meta.is_accessor);
    assert!(!meta.attributes.writable());
    assert!(meta.attributes.enumerable());
    assert!(!meta.attributes.configurable());
}

#[test]
fn accessor_meta_roundtrip_and_alignment() {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.set_prop_at(0, JsValue::int(1));
    obj.set_accessor_meta(2, JsValue::int(10), JsValue::int(11), PropAttributes::new(false, false, true));

    assert_eq!(obj.prop_count(), 3);
    assert!(obj.is_accessor_meta(2));
    let meta = obj.prop_meta_at(2).expect("accessor meta");
    assert_eq!(meta.get, JsValue::int(10));
    assert_eq!(meta.set, JsValue::int(11));
    assert!(!meta.attributes.writable());
    assert!(!meta.attributes.enumerable());
    assert!(meta.attributes.configurable());

    obj.push_prop(JsValue::int(4));
    assert_eq!(obj.prop_meta_vec().expect("meta").len(), obj.prop_vec_len());
}

#[test]
fn array_element_write_preserves_props_after_element_growth() {
    // 先写属性再 push 元素：元素区增长必须整体搬移属性区，不覆盖属性。
    let bump = bumpalo::Bump::new();
    let mut obj = JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), 3, &bump);
    obj.set_prop_shape(0, JsValue::int(99));
    obj.set_prop_at(3, JsValue::int(4));
    assert_eq!(obj.prop_count(), 4);
    assert_eq!(obj.get_prop_at(3), JsValue::int(4));
    assert_eq!(obj.get_prop_shape(0), JsValue::int(99));
}

#[test]
fn array_element_write_beyond_count_relocates_prop_zone() {
    // 稀疏写入（越界索引）把属性区推到新元素之后，属性读取仍命中。
    let bump = bumpalo::Bump::new();
    let mut obj = JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), 2, &bump);
    obj.set_prop_shape(0, JsValue::int(7));
    obj.set_prop_at(5, JsValue::int(50));
    assert_eq!(obj.prop_count(), 6);
    assert_eq!(obj.get_prop_at(5), JsValue::int(50));
    assert_eq!(obj.get_prop_shape(0), JsValue::int(7));
}

#[test]
fn array_prop_count_truncate_keeps_prop_zone() {
    // pop 截断元素区时属性区不得被删（meta 同步 insert/drain 对齐）。
    let bump = bumpalo::Bump::new();
    let mut obj = JsObject::new_array(EMPTY_SHAPE_ID, JsValue::null(), 3, &bump);
    obj.set_prop_shape(0, JsValue::int(5));
    obj.set_data_meta(3, PropAttributes::new(true, false, true));
    obj.set_prop_count_fast(2);
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_shape(0), JsValue::int(5));
    assert!(obj.prop_meta_at(2).is_some());
}
