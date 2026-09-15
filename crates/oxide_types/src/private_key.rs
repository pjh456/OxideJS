//! 私有名（private name）、Symbol 与整数属性键编码。
//!
//! ECMAScript 私有字段（`#x`）与 `Symbol` 使用全局注册符号以外的独立命名空间。
//! 本模块用 `u32` 键的高半区表示它们，与普通属性名区分，避免与用户可见的字符串键冲突：
//! - 整数键：`[INT_KEY_BASE, PRIVATE_NAME_BASE)`，`make_int_key` 生成（非负小整数
//!   属性键直接编码，免字符串转换与 intern）；
//! - 私有名键：`[PRIVATE_NAME_BASE, SYMBOL_KEY_BASE)`，`make_private_name_id` 生成；
//! - Symbol 键：`>= SYMBOL_KEY_BASE`，其中最低 `WELL_KNOWN_SYMBOL_COUNT` 个槽位
//!   保留给 well-known symbol，其余编码用户 symbol 的 VM 内下标。
//!
//! Symbol 键与整数键不被字符串 interner 收录，枚举路径须按 `is_symbol_key` /
//! `is_int_key` 显式跳过或物化文本。

/// 私有名键区间的起始值。
///
/// 键值 `>= PRIVATE_NAME_BASE` 一律视为私有名。
pub const PRIVATE_NAME_BASE: u32 = 0x8000_0000;

/// 整数键区间的起始值。
///
/// 整数键编码 `INT_KEY_BASE + i`（`i < INT_KEY_COUNT`），落在
/// `[INT_KEY_BASE, PRIVATE_NAME_BASE)` 空档——与字符串 interner id
/// （自 0 递增，实际值远小于该起点）及 private/symbol 区间互不重叠。
pub const INT_KEY_BASE: u32 = 0x4000_0000;

/// 整数键可覆盖的索引个数（`2^30`，远大于 dense 数组上限）。
pub const INT_KEY_COUNT: u32 = 0x4000_0000;

/// 把非负小整数索引编码为整数属性键（与 [`int_key_value`] 互逆）。
///
/// 调用方须保证 `i < INT_KEY_COUNT`，否则溢出到 private 区间（debug 构建断言兜底）。
#[inline]
pub const fn make_int_key(i: u32) -> u32 {
    debug_assert!(i < INT_KEY_COUNT, "整数键索引超出 INT_KEY_COUNT");
    INT_KEY_BASE + i
}

/// 判断键是否为整数键（位于整数键区间）。
#[inline]
pub const fn is_int_key(key: u32) -> bool {
    key >= INT_KEY_BASE && key < PRIVATE_NAME_BASE
}

/// 从整数键反解原索引（与 [`make_int_key`] 互逆）。
#[inline]
pub const fn int_key_value(key: u32) -> u32 {
    key - INT_KEY_BASE
}

/// Symbol 键区间的起始值，避开私有名区间。
pub const SYMBOL_KEY_BASE: u32 = 0xA000_0000;

/// 保留给 well-known symbol 的键槽位数（位于 Symbol 键区间最低端）。
///
/// 键序与 TC39 well-known symbol 表一致：0-10 为既有项，11/12 分别为
/// `Symbol.asyncDispose` 与 `Symbol.dispose`（显式资源管理提案）。
pub const WELL_KNOWN_SYMBOL_COUNT: u32 = 13;

/// 用户 symbol 下标的保留位掩码（低 28 位）。
const SYMBOL_INDEX_MASK: u32 = 0x0FFF_FFFF;

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

/// 判断键是否为 Symbol 键（位于 Symbol 键区间）。
///
/// 私有名键与 Symbol 键互不重叠：私有名经 `make_private_name_id` 生成，
/// 其 `local_id` 来自字节码局部序号，远小于 `SYMBOL_KEY_BASE` 的偏移量。
#[inline]
pub const fn is_symbol_key(key: u32) -> bool {
    key >= SYMBOL_KEY_BASE
}

/// 把用户 symbol 的表下标编码为属性键。
///
/// 键 = Symbol 区间起点 + well-known 预留槽 + 下标（截断到 28 位防加法回绕）。
#[inline]
pub const fn make_symbol_key(idx: u32) -> u32 {
    SYMBOL_KEY_BASE + WELL_KNOWN_SYMBOL_COUNT + (idx & SYMBOL_INDEX_MASK)
}

/// 从用户 symbol 键反解表下标（与 [`make_symbol_key`] 互逆）。
#[inline]
pub const fn symbol_index_from_key(key: u32) -> u32 {
    (key - SYMBOL_KEY_BASE - WELL_KNOWN_SYMBOL_COUNT) & SYMBOL_INDEX_MASK
}

/// 把 well-known symbol 序号编码为属性键（Symbol 键区间最低槽位）。
#[inline]
pub const fn make_well_known_symbol_key(id: u32) -> u32 {
    SYMBOL_KEY_BASE + id
}

/// 若键是 well-known symbol 键，返回其序号；否则返回 `None`。
#[inline]
pub const fn well_known_symbol_id_from_key(key: u32) -> Option<u32> {
    let offset = key - SYMBOL_KEY_BASE;
    if offset < WELL_KNOWN_SYMBOL_COUNT {
        Some(offset)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
