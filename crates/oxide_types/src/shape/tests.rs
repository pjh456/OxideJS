use super::*;

#[test]
fn empty_shape_exists() {
    let s = get_shape(EMPTY_SHAPE_ID);
    assert!(s.is_some());
    let s = s.unwrap();
    assert_eq!(s.id, EMPTY_SHAPE_ID);
    assert!(s.parent.is_none());
}

#[test]
fn make_shape_creates_new_id() {
    let key: StringIndex = 1_000_000;
    let s1 = make_shape(EMPTY_SHAPE_ID, key);
    assert!(s1 > EMPTY_SHAPE_ID);
    let shape = get_shape(s1).unwrap();
    assert_eq!(shape.property_name, key);
    assert_eq!(shape.parent, Some(EMPTY_SHAPE_ID));
}

#[test]
fn hash_cons_returns_same_id() {
    let key: StringIndex = 1_000_001;
    let a = make_shape(EMPTY_SHAPE_ID, key);
    let b = make_shape(EMPTY_SHAPE_ID, key);
    assert_eq!(a, b);
}

#[test]
fn different_props_different_ids() {
    let a = make_shape(EMPTY_SHAPE_ID, 1_000_002);
    let b = make_shape(EMPTY_SHAPE_ID, 1_000_003);
    assert_ne!(a, b);
}

#[test]
fn chain_of_three() {
    let s1 = make_shape(EMPTY_SHAPE_ID, 1_000_010);
    let s2 = make_shape(s1, 1_000_020);
    let s3 = make_shape(s2, 1_000_030);

    assert_eq!(shape_prop_count(s3), 3);

    assert_eq!(lookup_position(s3, 1_000_030), Some(2));
    assert_eq!(lookup_position(s3, 1_000_020), Some(1));
    assert_eq!(lookup_position(s3, 1_000_010), Some(0));
    assert_eq!(lookup_position(s3, 99), None);
}

#[test]
fn two_branches_share_ancestor() {
    let base = make_shape(EMPTY_SHAPE_ID, 1_000_040);
    let branch_a = make_shape(base, 1_000_050);
    let branch_b = make_shape(base, 1_000_060);
    assert_ne!(branch_a, branch_b);
    let a_shape = get_shape(branch_a).unwrap();
    let b_shape = get_shape(branch_b).unwrap();
    assert_eq!(a_shape.parent, Some(base));
    assert_eq!(b_shape.parent, Some(base));
}

#[test]
fn edge_same_structure_different_names() {
    let s1 = make_shape(EMPTY_SHAPE_ID, 1_000_070);
    let s2 = make_shape(s1, 1_000_080);
    let s3 = make_shape(EMPTY_SHAPE_ID, 1_000_090);
    let s4 = make_shape(s3, 1_000_100);
    assert_ne!(s1, s3);
    assert_ne!(s2, s4);
    assert_eq!(lookup_position(s2, 1_000_070), Some(0));
    assert_eq!(lookup_position(s4, 1_000_090), Some(0));
}

#[test]
fn has_property_works() {
    let s1 = make_shape(EMPTY_SHAPE_ID, 1_000_110);
    let s2 = make_shape(s1, 1_000_120);
    assert!(has_property(s2, 1_000_110));
    assert!(has_property(s2, 1_000_120));
    assert!(!has_property(s2, 99));
}
