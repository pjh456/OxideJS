use oxide_kernel::shape_forge::{ShapeForge, StringIndex, EMPTY_SHAPE_ID};
use std::sync::{Arc, Barrier};

#[test]
fn empty_shape_exists() {
    let forge = ShapeForge::new();
    let s = forge.get_shape(EMPTY_SHAPE_ID);
    assert!(s.is_some());
    let s = s.unwrap();
    assert_eq!(s.id, EMPTY_SHAPE_ID);
    assert!(s.parent.is_none());
    assert_eq!(s.depth, 0);
}

#[test]
fn make_shape_creates_new_id() {
    let forge = ShapeForge::new();
    let key: StringIndex = 1_000_000;
    let s1 = forge.make_shape(EMPTY_SHAPE_ID, key);
    assert!(s1 > EMPTY_SHAPE_ID);
    let shape = forge.get_shape(s1).unwrap();
    assert_eq!(shape.property_name, key);
    assert_eq!(shape.parent, Some(EMPTY_SHAPE_ID));
    assert_eq!(shape.depth, 1);
}

#[test]
fn hash_cons_returns_same_id() {
    let forge = ShapeForge::new();
    let key: StringIndex = 1_000_001;
    let a = forge.make_shape(EMPTY_SHAPE_ID, key);
    let b = forge.make_shape(EMPTY_SHAPE_ID, key);
    assert_eq!(a, b);
}

#[test]
fn different_props_different_ids() {
    let forge = ShapeForge::new();
    let a = forge.make_shape(EMPTY_SHAPE_ID, 1_000_002);
    let b = forge.make_shape(EMPTY_SHAPE_ID, 1_000_003);
    assert_ne!(a, b);
}

#[test]
fn chain_of_three() {
    let forge = ShapeForge::new();
    let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_010);
    let s2 = forge.make_shape(s1, 1_000_020);
    let s3 = forge.make_shape(s2, 1_000_030);

    assert_eq!(forge.shape_prop_count(s3), 3);

    assert_eq!(forge.lookup_position(s3, 1_000_030), Some(2));
    assert_eq!(forge.lookup_position(s3, 1_000_020), Some(1));
    assert_eq!(forge.lookup_position(s3, 1_000_010), Some(0));
    assert_eq!(forge.lookup_position(s3, 99), None);
}

#[test]
fn lookup_position_cached_second_call() {
    let forge = ShapeForge::new();
    let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_010);
    let s2 = forge.make_shape(s1, 1_000_020);
    let s3 = forge.make_shape(s2, 1_000_030);

    // 首次调用填充缓存。
    assert_eq!(forge.lookup_position(s3, 1_000_020), Some(1));
    // 二次调用命中缓存（验证无回归）。
    assert_eq!(forge.lookup_position(s3, 1_000_020), Some(1));
    assert_eq!(forge.lookup_position(s3, 1_000_030), Some(2));
    assert_eq!(forge.lookup_position(s3, 1_000_030), Some(2));
}

#[test]
fn clear_transient_clears_positions() {
    let forge = ShapeForge::new();
    let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_010);
    assert_eq!(forge.lookup_position(s1, 1_000_010), Some(0));
    forge.clear_transient();
    // 清理后只剩 EMPTY_SHAPE，其余全部移除。
    assert_eq!(forge.len(), 1);
}
#[test]
fn two_branches_share_ancestor() {
    let forge = ShapeForge::new();
    let base = forge.make_shape(EMPTY_SHAPE_ID, 1_000_040);
    let branch_a = forge.make_shape(base, 1_000_050);
    let branch_b = forge.make_shape(base, 1_000_060);
    assert_ne!(branch_a, branch_b);
    let a_shape = forge.get_shape(branch_a).unwrap();
    let b_shape = forge.get_shape(branch_b).unwrap();
    assert_eq!(a_shape.parent, Some(base));
    assert_eq!(b_shape.parent, Some(base));
}

#[test]
fn edge_same_structure_different_names() {
    let forge = ShapeForge::new();
    let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_070);
    let s2 = forge.make_shape(s1, 1_000_080);
    let s3 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_090);
    let s4 = forge.make_shape(s3, 1_000_100);
    assert_ne!(s1, s3);
    assert_ne!(s2, s4);
    assert_eq!(forge.lookup_position(s2, 1_000_070), Some(0));
    assert_eq!(forge.lookup_position(s4, 1_000_090), Some(0));
}

#[test]
fn concurrent_make_same_key() {
    let forge = Arc::new(ShapeForge::new());
    let key: StringIndex = 1_000_200;
    let barrier = Arc::new(Barrier::new(2));

    let f1 = Arc::clone(&forge);
    let b1 = Arc::clone(&barrier);
    let h1 = std::thread::spawn(move || {
        b1.wait();
        f1.make_shape(EMPTY_SHAPE_ID, key)
    });

    let f2 = Arc::clone(&forge);
    let b2 = Arc::clone(&barrier);
    let h2 = std::thread::spawn(move || {
        b2.wait();
        f2.make_shape(EMPTY_SHAPE_ID, key)
    });

    let id1 = h1.join().unwrap();
    let id2 = h2.join().unwrap();
    assert_eq!(id1, id2);
    assert!(id1 > EMPTY_SHAPE_ID);
}

#[test]
fn has_property_works() {
    let forge = ShapeForge::new();
    let s1 = forge.make_shape(EMPTY_SHAPE_ID, 1_000_300);
    let s2 = forge.make_shape(s1, 1_000_310);
    assert!(forge.has_property(s2, 1_000_300));
    assert!(forge.has_property(s2, 1_000_310));
    assert!(!forge.has_property(s2, 99));
}

#[test]
fn private_band_keys_are_shape_properties() {
    let forge = ShapeForge::new();
    let private_key = oxide_types::private_key::make_private_name_id(12);
    let shape = forge.make_shape(EMPTY_SHAPE_ID, private_key);
    assert!(oxide_types::private_key::is_private_name_key(private_key));
    assert!(forge.has_property(shape, private_key));
    assert_eq!(forge.lookup_position(shape, private_key), Some(0));
}
