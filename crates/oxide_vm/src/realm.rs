//! 每 VM 的 realm 组合：内核会话（builtin world 与 global 对象）、
//! session GC 簿记与 10 个内建原型槽。
//!
//! `Vm` 以单个 `realm: Realm` 字段持有本组合。`kernel_core`
//! 留在 `Vm` 上不进 Realm（`active_vms` 边界守卫计数是 per-VM 语义，
//! `KernelCore` 是跨 realm 共享结构，多 realm 共享同一 kernel）。

use oxide_kernel::kernel::KernelSession;
use oxide_types::mem::P;
use oxide_types::object::JsObject;

use crate::vm_state::GcState;

/// 每 VM 的 realm 组合：内核会话（builtin world 与 global 对象）、
/// session GC 簿记与 10 个内建原型槽。
///
/// 字段顺序按原 `Vm` 声明顺序（session 先、10 个 P 字段次之、gc 最后），
/// 保 Drop 相对顺序。
pub(crate) struct Realm {
    pub(crate) session: KernelSession,
    /// `%Object.prototype%`：session 的 Object 原型（global 的 `[[Prototype]]` 挂它）。
    pub(crate) object_prototype: P<JsObject>,
    /// `%GeneratorPrototype%`：生成器实例的原型（next/return/throw 方法挂此）。
    pub(crate) generator_proto: P<JsObject>,
    /// `%GeneratorFunction.prototype%`：生成器函数对象的原型（`constructor` 指向
    /// `%GeneratorFunction%`，使 `g.constructor.name` 解析为 "GeneratorFunction"）。
    pub(crate) generator_function_proto: P<JsObject>,
    /// `%Promise%` 构造器（resolve/reject 静态方法挂此，global 的 Promise 槽指向它）。
    pub(crate) promise_constructor: P<JsObject>,
    /// `%Promise.prototype%`：Promise 实例的原型（then/catch/finally 方法挂此）。
    pub(crate) promise_proto: P<JsObject>,
    /// `%AggregateError%` 构造器（Promise.any 拒绝时构造 AggregateError 用）。
    pub(crate) aggregate_error_constructor: P<JsObject>,
    /// `%AggregateError.prototype%`（proto = %Error.prototype%）。
    pub(crate) aggregate_error_proto: P<JsObject>,
    /// `%AsyncFunction.prototype%`：异步函数对象的原型（`constructor` 指向 `%AsyncFunction%`）。
    pub(crate) async_function_proto: P<JsObject>,
    /// `%AsyncGeneratorPrototype%`：异步生成器实例的原型（next/return/throw/@@asyncIterator）。
    pub(crate) async_generator_proto: P<JsObject>,
    /// `%AsyncGeneratorFunction.prototype%`：异步生成器函数对象的原型。
    pub(crate) async_generator_function_proto: P<JsObject>,
    /// session 堆与 GC 簿记（对象/字符串/BigInt/cell 四表 + 水位与账目）。
    pub(crate) gc: GcState,
}
