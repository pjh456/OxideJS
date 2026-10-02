//! Promise 状态盒的 session GC 支撑：值边、字节核算与释放三类接线。
//!
//! 结算链指针是裸指针边、不是 JsValue。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::{promise_state_ref, PromiseReaction, PromiseState};

/// Promise 状态盒内全部值边（GC mark 边）：对象/字符串/BigInt 均产出，
/// 消费侧按值类型分发到对象栈与存活集。
pub(crate) fn promise_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = promise_state_ref(obj) else {
        return Vec::new();
    };
    let mut edges = Vec::new();
    edges.push(state.result);
    edges.push(state.resolve_fn);
    edges.push(state.reject_fn);
    // 结算链指针：克隆恒为 session 表成员，凭这条边在原件/旧克隆仍存活时被
    // 同轮置活，结算传导不会读到悬垂克隆。
    if !state.promoted_clone.is_null() {
        edges.push(JsValue::from_js_object(state.promoted_clone));
    }
    for r in &state.reactions {
        edges.push(r.promise);
        edges.push(r.resolve);
        edges.push(r.reject);
        edges.push(r.handler);
    }
    edges
}

/// 只读核算 Promise 状态盒字节（不释放），供 GC 账目核算。
pub(crate) fn promise_native_size(obj: &JsObject) -> u64 {
    if !obj.is_promise_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut PromiseState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: !is_promise_obj 与 ptr.is_null 已早退，ptr 非空且指向存活 Box<PromiseState>，此处只读容量不释放。
    unsafe {
        std::mem::size_of::<PromiseState>() as u64
            + (*ptr).reactions.capacity() as u64 * std::mem::size_of::<PromiseReaction>() as u64
    }
}

/// 释放 Promise 状态盒（对象被回收时），返回释放字节数。
pub(crate) fn drop_promise_native(obj: &JsObject) -> u64 {
    let bytes = promise_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let ptr = obj.native_data() as *mut PromiseState;
    // SAFETY: ptr 非空（promise_native_size 已验证），Box::from_raw 恰好释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    drop(state);
    bytes
}
