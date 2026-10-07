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

#[test]
fn realm_zero_symbol_keys_match_legacy() {
    // realm 编号为 0 时新编码与旧编码逐字节一致：well-known 与用户符号各取若干下标比对。
    for id in 0..WELL_KNOWN_SYMBOL_COUNT {
        assert_eq!(encode_symbol_key(0, id), make_well_known_symbol_key(id));
    }
    for idx in WELL_KNOWN_SYMBOL_COUNT..(WELL_KNOWN_SYMBOL_COUNT + 8) {
        assert_eq!(encode_symbol_key(0, idx), make_symbol_key(idx));
    }
}

#[test]
fn symbol_key_realm_roundtrip() {
    for realm in [0u32, 1, 7, 511] {
        for idx in [0u32, 14, 15, 16, 1000] {
            let key = encode_symbol_key(realm, idx);
            assert!(is_symbol_key(key));
            assert_eq!(decode_symbol_key(key), (realm, idx));
            assert_eq!(symbol_realm_id_from_key(key), realm);
            assert_eq!(symbol_local_index_from_key(key), idx);
        }
    }
}

#[test]
fn symbol_key_realm_dimensions_distinct() {
    // 同下标不同 realm 产生不同键；同 realm 不同下标产生不同键。
    assert_ne!(encode_symbol_key(0, 15), encode_symbol_key(1, 15));
    assert_ne!(encode_symbol_key(1, 15), encode_symbol_key(1, 16));
    // 跨 realm 同名 well-known 符号也产生不同键。
    assert_ne!(encode_symbol_key(0, WELL_KNOWN_SYMBOL_ITERATOR), encode_symbol_key(2, WELL_KNOWN_SYMBOL_ITERATOR));
}
