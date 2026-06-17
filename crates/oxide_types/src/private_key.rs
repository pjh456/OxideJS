pub const PRIVATE_NAME_BASE: u32 = 0x8000_0000;

#[inline]
pub const fn is_private_name_key(key: u32) -> bool {
    key >= PRIVATE_NAME_BASE
}

#[inline]
pub const fn make_private_name_id(local_id: u32) -> u32 {
    PRIVATE_NAME_BASE | (local_id & !PRIVATE_NAME_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_name_ids_use_high_band() {
        let id = make_private_name_id(7);
        assert!(is_private_name_key(id));
        assert_ne!(id, u32::MAX);
        assert!(!is_private_name_key(7));
    }
}
