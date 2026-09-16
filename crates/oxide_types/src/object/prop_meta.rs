//! 属性元数据：描述符标志、数据/访问器属性条目与属性存储下标转换。
//!
//! `PropAttributes` 把三个描述符标志压缩进单个 `u8` 位集；`PropMetaEntry` 承载
//! 单个属性的元数据（数据 / 访问器两形态 + 数组空洞标记）；`PropIndex` 统一
//! 可作属性存储下标的整数类型。

use crate::value::JsValue;

/// 属性描述符标志位集合，压缩在单个 `u8` 中。
///
/// 位定义：bit0 = writable，bit1 = enumerable，bit2 = configurable。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropAttributes(pub u8);

impl PropAttributes {
    /// writable 标志位（bit0）。
    pub const WRITABLE: u8 = 0b001;
    /// enumerable 标志位（bit1）。
    pub const ENUMERABLE: u8 = 0b010;
    /// configurable 标志位（`0b100`）。
    pub const CONFIGURABLE: u8 = 0b100;
    /// 数据属性默认描述符：三标志全开。
    pub const DEFAULT_DATA: Self = Self(Self::WRITABLE | Self::ENUMERABLE | Self::CONFIGURABLE);

    /// 由三个布尔标志构造描述符。
    pub const fn new(writable: bool, enumerable: bool, configurable: bool) -> Self {
        let mut bits = 0;
        if writable {
            bits |= Self::WRITABLE;
        }
        if enumerable {
            bits |= Self::ENUMERABLE;
        }
        if configurable {
            bits |= Self::CONFIGURABLE;
        }
        Self(bits)
    }

    /// 是否 writable。
    pub const fn writable(self) -> bool {
        self.0 & Self::WRITABLE != 0
    }

    /// 是否 enumerable。
    pub const fn enumerable(self) -> bool {
        self.0 & Self::ENUMERABLE != 0
    }

    /// 是否 configurable。
    pub const fn configurable(self) -> bool {
        self.0 & Self::CONFIGURABLE != 0
    }
}

/// 单个属性的元数据条目。
///
/// 数据属性仅用 `attributes`；访问器属性额外携带 getter / setter
/// 的 [`JsValue`] 与 `is_accessor = true` 标记。`hole` 标记数组元素被删除
/// 后保留的稀疏空洞（数组元素区存在性判定依据）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropMetaEntry {
    pub attributes: PropAttributes,
    pub get: JsValue,
    pub set: JsValue,
    pub is_accessor: bool,
    pub hole: bool,
}

impl PropMetaEntry {
    /// 构造数据属性条目。
    pub fn data(attributes: PropAttributes) -> Self {
        Self {
            attributes,
            get: JsValue::undefined(),
            set: JsValue::undefined(),
            is_accessor: false,
            hole: false,
        }
    }

    /// 构造访问器属性条目（getter/setter 可为 `undefined`）。
    pub fn accessor(get: JsValue, set: JsValue, attributes: PropAttributes) -> Self {
        Self {
            attributes,
            get,
            set,
            is_accessor: true,
            hole: false,
        }
    }

    /// 构造数组元素删除后的 hole 标记条目。
    pub fn hole() -> Self {
        Self {
            attributes: PropAttributes::DEFAULT_DATA,
            get: JsValue::undefined(),
            set: JsValue::undefined(),
            is_accessor: false,
            hole: true,
        }
    }

    /// 是否数组元素 hole 标记（删除后保留的稀疏空洞）。
    pub fn is_hole(&self) -> bool {
        self.hole
    }
}

/// 可转换为属性存储下标的值。
///
/// 为 `u8` / `u16` / `u32` / `usize` / 非负 `i32` 实现，统一属性向量
/// 下标参数的类型。
pub trait PropIndex {
    fn to_u32(self) -> u32;
}

impl PropIndex for u8 {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for u16 {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for u32 {
    fn to_u32(self) -> u32 {
        self
    }
}

impl PropIndex for usize {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for i32 {
    fn to_u32(self) -> u32 {
        debug_assert!(self >= 0, "property index must be non-negative");
        self.max(0) as u32
    }
}
