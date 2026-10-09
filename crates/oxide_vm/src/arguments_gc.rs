//! mapped arguments 同步状态盒的 GC 支撑：size 核算与 drop 释放。
//!
//! 状态盒存于对象 `native_data`（`Box::into_raw`），无引用边（位图与帧身份均为
//! 原始值），仅 size/drop 链消费。

use std::mem::size_of;

use oxide_types::arguments_map::ArgumentsMapState;
use oxide_types::object::JsObject;

/// 只读核算 mapped arguments 同步状态盒字节（不释放），供 GC 账目核算。
pub(crate) fn arguments_native_size(obj: &JsObject) -> u64 {
    if !obj.is_arguments_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut ArgumentsMapState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 通过裸指针读取 capacity 字段，不解引用整个 Box（不移动/释放）。
    unsafe { size_of::<ArgumentsMapState>() as u64 + (*ptr).mapped_mask.capacity() as u64 * size_of::<u64>() as u64 }
}

/// 释放 mapped arguments 同步状态盒（对象被 GC 回收时），返回释放字节数。
pub(crate) fn drop_arguments_native(obj: &JsObject) -> u64 {
    let bytes = arguments_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let ptr = obj.native_data() as *mut ArgumentsMapState;
    // SAFETY: ptr 非空（arguments_native_size 已验证），Box::from_raw 恰好释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    drop(state);
    bytes
}
