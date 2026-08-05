//! 私有名（private name）键编码。
//!
//! ECMAScript 私有字段（`#x`）与 `Symbol` 使用全局注册符号以外的独立命名空间。
//! 本模块用 `u32` 键的高半区（`>= 0x8000_0000`）表示私有名，与普通属性名区分，
//! 避免与用户可见的字符串键冲突。

/// 私有名键区间的起始值。
///
/// 键值 `>= PRIVATE_NAME_BASE` 一律视为私有名。
pub const PRIVATE_NAME_BASE: u32 = 0x8000_0000;

/// 判断键是否为私有名键（即位于高半区）。
#[inline]
pub const fn is_private_name_key(key: u32) -> bool {
    key >= PRIVATE_NAME_BASE
}

/// 把类内的局部私有名序号编码为全局唯一键。
///
/// 将 `local_id` 置入高半区（按位或 `PRIVATE_NAME_BASE`），
/// 保留低 31 位原始值。
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
