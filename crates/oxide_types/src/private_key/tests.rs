use super::*;

#[test]
fn private_name_ids_use_high_band() {
    let id = make_private_name_id(7);
    assert!(is_private_name_key(id));
    assert_ne!(id, u32::MAX);
    assert!(!is_private_name_key(7));
}

#[test]
fn symbol_keys_roundtrip() {
    let key = make_symbol_key(0);
    assert!(is_symbol_key(key));
    assert!(is_private_name_key(key));
    assert_eq!(symbol_index_from_key(key), 0);
    assert!(well_known_symbol_id_from_key(key).is_none());
}

#[test]
fn symbol_keys_distinct_per_index() {
    let a = make_symbol_key(1);
    let b = make_symbol_key(2);
    assert_ne!(a, b);
    assert!(is_symbol_key(a));
    assert!(is_symbol_key(b));
}

#[test]
fn symbol_keys_avoid_private_band() {
    let id = make_private_name_id(12);
    let key = make_symbol_key(12);
    assert!(!is_symbol_key(id));
    assert!(is_symbol_key(key));
    assert_ne!(id, key);
}

#[test]
fn well_known_keys_reserve_low_slots() {
    for id in 0..WELL_KNOWN_SYMBOL_COUNT {
        let key = make_well_known_symbol_key(id);
        assert!(is_symbol_key(key));
        assert_eq!(well_known_symbol_id_from_key(key), Some(id));
    }
    let user_key = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_COUNT);
    assert!(well_known_symbol_id_from_key(user_key).is_none());
}

#[test]
fn symbol_index_mask_prevents_wrap() {
    let key = make_symbol_key(SYMBOL_INDEX_MASK);
    assert_eq!(symbol_index_from_key(key), SYMBOL_INDEX_MASK);
    assert!(is_symbol_key(key));
}

#[test]
fn int_keys_stay_in_own_band() {
    assert_eq!(make_int_key(0), INT_KEY_BASE);
    assert_eq!(int_key_value(INT_KEY_BASE), 0);
    assert_eq!(int_key_value(make_int_key(123)), 123);
    assert!(is_int_key(make_int_key(0)));
    assert!(!is_int_key(0));
    assert!(!is_int_key(PRIVATE_NAME_BASE));
    assert!(!is_private_name_key(make_int_key(0)));
    assert!(!is_symbol_key(make_int_key(0)));
    assert_ne!(make_int_key(5), make_private_name_id(5));
    assert_ne!(make_int_key(5), make_symbol_key(5));
}

#[test]
fn int_key_roundtrip_max() {
    let key = make_int_key(INT_KEY_COUNT - 1);
    assert!(is_int_key(key));
    assert!(key < PRIVATE_NAME_BASE);
    assert_eq!(int_key_value(key), INT_KEY_COUNT - 1);
}
