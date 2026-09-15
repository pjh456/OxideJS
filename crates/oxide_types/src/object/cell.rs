//! 闭包共享 cell：JS 值 + 已初始化 / GC 标记位，定长 16 字节。
//!
//! cell 组成闭包的 upvalue 数组（对象 `upvalues` 裸指针区），随闭包释放；
//! `flags` 为 1 字节位集加 7 字节固定填充，保证数组元素步长恒定。

use crate::value::JsValue;

#[repr(C)]
pub struct Cell {
    pub value: JsValue,
    pub flags: u8,
    pub _pad: [u8; 7],
}

impl Cell {
    /// 已初始化标志（TDZ / 未赋值检测）。
    pub const INITIALIZED: u8 = 0x01;
    /// GC 标记位。
    pub const GC_MARK: u8 = 0x02;

    /// 构造 cell，`initialized` 决定是否立即置 [`INITIALIZED`](Cell::INITIALIZED) 位。
    pub fn new(value: JsValue, initialized: bool) -> Self {
        Cell {
            value,
            flags: if initialized { Self::INITIALIZED } else { 0 },
            _pad: [0; 7],
        }
    }

    /// 是否已初始化。
    pub fn is_initialized(&self) -> bool {
        self.flags & Self::INITIALIZED != 0
    }

    /// 设置 / 清除已初始化标志。
    pub fn set_initialized(&mut self, val: bool) {
        if val {
            self.flags |= Self::INITIALIZED;
        } else {
            self.flags &= !Self::INITIALIZED;
        }
    }

    /// 是否被 GC 标记。
    pub fn is_gc_marked(&self) -> bool {
        self.flags & Self::GC_MARK != 0
    }

    /// 设置 / 清除 GC 标记。
    pub fn set_gc_mark(&mut self, marked: bool) {
        if marked {
            self.flags |= Self::GC_MARK;
        } else {
            self.flags &= !Self::GC_MARK;
        }
    }
}
