//! Promise 运行时：Promise 状态盒 + resolve/reject 闭包 + 微任务队列。
//!
//! Promise 对象把 `PromiseState` 状态盒（Box）挂在 `native_data`；resolve/reject
//! 是携带目标 promise 的 native 闭包函数（函数对象 prop 存 promise 引用）。
//! 反应（reaction）与 thenable 委托以 `Microtask` 入队 `Vm::job_queue`，
//! 由 `run()` 末尾的 drain 循环 FIFO 执行。所有盒内 JsValue 由 session GC
//! 经 Promise 对象边追踪（见 `promise_native_edges` / `rewrite_promise_native` /
//! `clone_promise_native_with_rewrite` / `drop_promise_native`）。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

mod aggregate;
mod constructor;
mod gc_edges;
mod jobs;
mod reactions;
mod settlement;

pub(crate) use gc_edges::{
    clone_promise_native_with_rewrite, drop_promise_native, migrate_settlement_to_newest_clone, promise_native_edges,
    promise_native_size, repoint_promise_promoted_clone, rewrite_promise_native,
};
pub(crate) use jobs::{for_each_job_value, rewrite_job_values};
pub use reactions::promise_settled_value;

/// Promise 的 settled 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromiseStateKind {
    Pending,
    Fulfilled,
    Rejected,
}

/// `then` 注册的一条反应：派生 promise 的能力（promise + resolve/reject）+ 处理器。
#[derive(Clone)]
pub(crate) struct PromiseReaction {
    pub promise: JsValue,
    pub resolve: JsValue,
    pub reject: JsValue,
    pub handler: JsValue,
    pub is_fulfill: bool,
}

/// Promise 状态盒：存于 Promise 对象 `native_data`，所有 JsValue 是 GC 边。
pub(crate) struct PromiseState {
    pub state: PromiseStateKind,
    pub result: JsValue,
    pub reactions: Vec<PromiseReaction>,
    /// 本 promise 能力上的 resolve/reject 闭包（thenable 委托入队时取用）。
    pub resolve_fn: JsValue,
    pub reject_fn: JsValue,
    /// resolve/reject 是否已被调用过（Resolve Promise Functions 的 alreadyResolved）。
    /// 首次调用后置位，后续任何 resolve/reject（含 thenable 委托期间）均 no-op。
    pub already_resolved: bool,
    /// 结算传导链指针：非空时指向本 promise 晋升出的 session 克隆，原件结算时
    /// 结果沿它传导到克隆；再晋升场景旧克隆的残留反应被取回归并进新克隆，
    /// 指针更新为最新克隆。非 pending 原件恒为空指针。本指针在
    /// `promise_native_edges` 登记为 mark 边：克隆恒为 session 表成员，凭这条
    /// 边在原件仍存活时被同轮置活，结算不会读到悬垂克隆。
    pub promoted_clone: *mut JsObject,
}

/// 一条微任务（job）。
pub(crate) enum Microtask {
    /// 反应任务：调 `handler(argument)`，结果经能力 resolve/reject 交付；空处理器直通。
    Reaction {
        is_fulfill: bool,
        handler: JsValue,
        argument: JsValue,
        resolve: JsValue,
        reject: JsValue,
    },
    /// thenable 委托任务：调 `then(thenable, resolve, reject)`。
    Thenable {
        thenable: JsValue,
        then: JsValue,
        resolve: JsValue,
        reject: JsValue,
    },
}

/// 单次 drain 的微任务处理上限，防止无终止的自我产生链导致活锁。
const MAX_DRAIN_JOBS: usize = 1_000_000;

/// 闭包函数对象上存目标 promise 的属性名。
const PROMISE_PROP: &str = "__oxide_promise__";
/// thenable 委托结算代理标志：绕过 alreadyResolved（委托是真正结算路径）。
const DELEGATED_PROP: &str = "__oxide_delegated__";
/// finally 处理器上存 onFinally 回调的属性名。
const ON_FINALLY_PROP: &str = "__oxide_on_finally__";
/// finally 处理器上区分 reject 角色的属性名。
const FINALLY_REJECT_PROP: &str = "__oxide_finally_reject__";
/// 能力 executor 上捕获 resolve/reject 的属性名。
const CAP_RESOLVE_PROP: &str = "__oxide_cap_resolve__";
const CAP_REJECT_PROP: &str = "__oxide_cap_reject__";

// ── 聚合静态方法（all/race/allSettled/any）内部状态属性 ──
/// 元素处理器函数上存共享记录对象的属性名。
const AGG_RECORD_PROP: &str = "__oxide_agg_record__";
/// 元素处理器函数上存元素下标的属性名。
const AGG_INDEX_PROP: &str = "__oxide_agg_index__";
/// 元素处理器函数上存"已调用一次"标志的属性名。
const AGG_ALREADY_PROP: &str = "__oxide_agg_already__";
/// 记录对象上存剩余未结算元素计数（含尾部哨兵 1）。
const AGG_REMAINING_PROP: &str = "__oxide_agg_remaining__";
/// 记录对象上存结果数组（all 的 values，any 的 errors）。
const AGG_VALUES_PROP: &str = "__oxide_agg_values__";
/// 记录对象上存能力 resolve 闭包。
const AGG_RESOLVE_PROP: &str = "__oxide_agg_resolve__";
/// 记录对象上存能力 reject 闭包。
const AGG_REJECT_PROP: &str = "__oxide_agg_reject__";

// ── Promise.try 包装闭包（W）状态属性 ──
/// W 上存用户 executor 函数。
const TRY_EXECUTOR_PROP: &str = "__oxide_try_executor__";
/// W 上存转发实参数组（resolve/reject 经调用实参传入，不落属性）。
const TRY_ARGS_PROP: &str = "__oxide_try_args__";

/// 聚合静态方法的语义模式（决定元素处理器与结算方式）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum AggregateKind {
    All,
    Race,
    AllSettled,
    Any,
}

/// IsConstructor 近似判定：可调用、非 arrow，且 native 函数须带构造器 tag
/// （与 `construct_ctor` 的校验一致）；bound 包装恒接受（target 可构造性在
/// `dispatch_new_bound` 入口复核）。
fn is_constructor_value(c: JsValue) -> bool {
    if !c.is_object() {
        return false;
    }
    // SAFETY: is_object 保证指针非空且指向存活 arena 对象，此处只读函数标志并立即消费。
    let c_obj = unsafe { &*c.as_js_object_ptr() };
    c_obj.is_function()
        && !c_obj.is_arrow()
        && !(c_obj.native_fn().is_some()
            && c_obj.type_tag != JsObject::OBJ_TYPE_CONSTRUCTOR
            && c_obj.type_tag != JsObject::OBJ_TYPE_BOUND)
}

// ── session GC 支撑：状态盒中的 JsValues 作为 Promise 对象边追踪 ──

fn promise_state_ref(obj: &JsObject) -> Option<&PromiseState> {
    let ptr = obj.native_data() as *const PromiseState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 由 Box::into_raw 分配，生命周期与 Promise 对象一致。
        Some(unsafe { &*ptr })
    }
}

#[expect(clippy::mut_from_ref)]
fn promise_state_mut(obj: &JsObject) -> Option<&mut PromiseState> {
    let ptr = obj.native_data() as *mut PromiseState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: 同上，改写发生在 GC 移动/清扫期间，对象仍存活。
        Some(unsafe { &mut *ptr })
    }
}
