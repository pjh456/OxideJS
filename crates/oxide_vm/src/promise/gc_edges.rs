//! Promise 状态盒的 session GC 支撑：值边、改写、深拷贝迁移、
//! 结算链克隆链维护与字节核算 / 释放。
//!
//! 结算链指针是裸指针边、不在 JsValue 改写面：晋升场景由
//! `migrate_settlement_to_newest_clone` 接链，搬移场景按转发表重定位。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::{promise_state_mut, promise_state_ref, PromiseReaction, PromiseState, PromiseStateKind};

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

/// 用转发函数重写状态盒中的所有 JsValue（session GC 移动式清扫 / promote 用）。
///
/// 结算链指针是裸指针边、不在本函数改写面：晋升路径克隆新分配于 session
/// arena（原件地址不变），同 arena 改写场景指针天然有效；搬移换址场景由
/// 调用方在重写完成后经 `repoint_promise_promoted_clone` 按转发表重定位。
pub(crate) fn rewrite_promise_native(obj: &JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    let Some(state) = promise_state_mut(obj) else {
        return;
    };
    state.result = rewrite(state.result);
    state.resolve_fn = rewrite(state.resolve_fn);
    state.reject_fn = rewrite(state.reject_fn);
    for r in &mut state.reactions {
        r.promise = rewrite(r.promise);
        r.resolve = rewrite(r.resolve);
        r.reject = rewrite(r.reject);
        r.handler = rewrite(r.handler);
    }
}

/// 深拷贝状态盒到新对象（promote / sweep 用）：新对象持独立 Box，源盒可安全释放。
///
/// 源的反应**迁移**（非复制）进新对象：源盒随 epoch 释放，留在源上的反应会
/// 永久丢失、原件侧消费者永不触发；每条反应的 JsValue 随状态盒其余字段同一
/// pass 晋升/改写。结算链指针按原值保留（晋升场景由
/// `migrate_settlement_to_newest_clone` 接链，搬移场景按转发表重定位）。
pub(crate) fn clone_promise_native_with_rewrite(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    // 先取全部 Copy 字段，避免与随后的可变 take 别名校验冲突。
    let (state_kind, prev_clone, result, resolve_fn, reject_fn, already_resolved) = {
        let Some(state) = promise_state_ref(old) else {
            return;
        };
        (
            state.state,
            state.promoted_clone,
            state.result,
            state.resolve_fn,
            state.reject_fn,
            state.already_resolved,
        )
    };

    // 源反应迁入新对象：源盒随 epoch 释放，反应留源即永久丢失。
    let mut reactions: Vec<PromiseReaction> = Vec::new();
    if let Some(src) = promise_state_mut(old) {
        for r in std::mem::take(&mut src.reactions) {
            reactions.push(rewrite_reaction(r, &mut rewrite));
        }
    }

    let cloned = PromiseState {
        state: state_kind,
        result: rewrite(result),
        reactions,
        resolve_fn: rewrite(resolve_fn),
        reject_fn: rewrite(reject_fn),
        already_resolved,
        // 结算链指针按原值保留（不改写源指针）：晋升后继步
        // `migrate_settlement_to_newest_clone` 负责接链与更新源指针，
        // 搬移后继步按转发表重定位。
        promoted_clone: prev_clone,
    };
    new.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 晋升路径专属：新克隆取代前一克隆成为结算枢纽——前一克隆的残留反应取回归并
/// 进新克隆，旧克隆的结算改链到新克隆，原件的传导指针指向新克隆。任一结算
/// 入口（原件 / 旧克隆 / 新克隆）都沿链落到携全部反应的最新克隆，反应恰触发
/// 一次。
///
/// # 边界与前提
/// - 仅 epoch 原件晋升后调用（`promote_object_inner` 的 Promise 分支，紧随
///   `clone_promise_native_with_rewrite`）；session 搬移（GC sweep）不改结算
///   拓扑，不得调用。
/// - 源非 pending 时无事可做直接返回（已结算原件不再传导）。
pub(crate) fn migrate_settlement_to_newest_clone(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    // 链上的前克隆 = 源当前指向的克隆（克隆步只保留不改写源指针）。
    let (pending, prev) = {
        let Some(src) = promise_state_ref(old) else {
            return;
        };
        (src.state == PromiseStateKind::Pending, src.promoted_clone)
    };
    if !pending {
        return;
    }
    // 前克隆残留并入本克隆（旧克隆腾空，防双触发），并把它的结算改链到新克隆。
    if !prev.is_null() {
        // SAFETY: prev 是早前晋升产生的 session 克隆，session arena 存活期内有效。
        if let Some(prev_state) = promise_state_mut(unsafe { &*prev }) {
            if let Some(dst) = promise_state_mut(new) {
                for r in std::mem::take(&mut prev_state.reactions) {
                    dst.reactions.push(rewrite_reaction(r, &mut rewrite));
                }
            }
            prev_state.promoted_clone = new as *mut JsObject;
        }
    }
    // 新克隆即最新：自身无后继，原件传导指针指向它。
    if let Some(dst) = promise_state_mut(new) {
        dst.promoted_clone = std::ptr::null_mut();
    }
    if let Some(src) = promise_state_mut(old) {
        src.promoted_clone = new as *mut JsObject;
    }
}

/// 按给定转发改写结算链指针（裸指针边、非 JsValue），session GC 搬移后调用。
pub(crate) fn repoint_promise_promoted_clone(obj: &JsObject, forward: impl FnOnce(*mut JsObject) -> *mut JsObject) {
    let Some(state) = promise_state_mut(obj) else {
        return;
    };
    if !state.promoted_clone.is_null() {
        state.promoted_clone = forward(state.promoted_clone);
    }
}

/// 改写单条反应的四条 JsValue 引用（派生 promise 能力与处理器），源反应迁移
/// 与再晋升合并共用。
fn rewrite_reaction<F: FnMut(JsValue) -> JsValue>(r: PromiseReaction, rewrite: &mut F) -> PromiseReaction {
    PromiseReaction {
        promise: rewrite(r.promise),
        resolve: rewrite(r.resolve),
        reject: rewrite(r.reject),
        handler: rewrite(r.handler),
        is_fulfill: r.is_fulfill,
    }
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
