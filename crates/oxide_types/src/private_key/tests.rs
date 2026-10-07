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

#[test]
fn symbol_key_realm_511_in_range() {
    // 511 是 512 上界内的最后一个 realm 编号：realm 位落 bit 20 至 28，与基址
    // 不重叠，编码落 29 位符号空间内，往返自洽且与相邻 realm 不碰撞。
    for idx in [0u32, 14, 15, 16] {
        let key = encode_symbol_key(511, idx);
        assert!(is_symbol_key(key));
        assert_eq!(decode_symbol_key(key), (511, idx));
        assert_ne!(key, encode_symbol_key(0, idx));
        assert_ne!(key, encode_symbol_key(510, idx));
    }
}

#[test]
fn symbol_key_realm_512_collides_with_realm_zero() {
    // 512 是第一个出界 realm 编号：realm 位 bit 9 左移 20 位落 bit 29，与
    // SYMBOL_KEY_BASE 的 bit 29 重叠，编码结果与 realm 0 逐字节一致（碰撞）。
    for idx in [0u32, 14, 15, 16] {
        assert_eq!(encode_symbol_key(512, idx), encode_symbol_key(0, idx));
    }
}

#[test]
fn symbol_key_realm_1024_out_of_range_still_injective() {
    // 1024 出界但 realm 位 bit 10 左移 20 位落 bit 30，与基址不重叠：键仍单射
    // （解码自洽回 1024），仅出文档声明的 29 位符号空间，不与 realm 0 碰撞。
    for idx in [0u32, 15, 16] {
        let key = encode_symbol_key(1024, idx);
        assert!(is_symbol_key(key));
        assert_ne!(key, encode_symbol_key(0, idx));
        assert_eq!(symbol_realm_id_from_key(key), 1024);
        assert_eq!(symbol_local_index_from_key(key), idx);
    }
}

#[test]
fn symbol_key_realm_2048_collides_with_realm_zero() {
    // 2048 是第二个碰撞环：realm 位 bit 11 左移 20 位落 bit 31，与
    // SYMBOL_KEY_BASE 的 bit 31 重叠，编码结果与 realm 0 逐字节一致。
    for idx in [0u32, 15, 16] {
        assert_eq!(encode_symbol_key(2048, idx), encode_symbol_key(0, idx));
    }
}
