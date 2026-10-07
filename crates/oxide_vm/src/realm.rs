//! 每 VM 的 realm 组合：内核会话（builtin world 与 global 对象）、
//! session GC 簿记与 10 个内建原型槽。
//!
//! `Vm` 以单个 `realm: Arc<Realm>` 字段持有本组合。可变组（session、gc、
//! 10 个 P 字段）经 `RefCell` 内部可变性，使重置路径（full_reset / reset /
//! 收尾）在 `&Arc<Realm>` 下改写；读路径经 `borrow()` 透传。Arc 计数归零时
//! `Drop for Realm` 执行收尾（per-realm 消亡：多 Vm 共享同一 Realm 时最后
//! 一个 Vm 析构不触发收尾，计数归零才触发）。
//!
//! `kernel_core` 留在 `Vm` 上不进 Realm（`active_vms` 边界守卫计数是
//! per-VM 语义，`KernelCore` 是跨 realm 共享结构，多 realm 共享同一
//! kernel）。`iters` 同样留在 `Vm` 上：活跃 for-in / for-of 迭代器是
//! per-VM 执行态，两个共享 Realm 的 Vm 须有独立迭代器栈。

use std::cell::RefCell;

use oxide_kernel::kernel::KernelSession;
use oxide_types::mem::P;
use oxide_types::object::JsObject;

use crate::session_gc::SessionGc;
use crate::vm_state::{GcState, SymbolState};

/// 每 VM 的 realm 组合：内核会话（builtin world 与 global 对象）、
/// session GC 簿记与 10 个内建原型槽。
///
/// 可变组经 `RefCell` 内部可变性：重置路径（selective_reset / rebind /
/// record_snapshot / 整体替换、水位复位 / 表排空 / 账目清零、
/// object_prototype 重赋）在 `&Arc<Realm>` 下改写，读路径经 `borrow()`
/// 透传。
///
/// 字段顺序按原 `Vm` 声明顺序（session 先、10 个 P 字段次之、gc 最后），
/// 保 Drop 相对顺序。`realm_id` 是分配时固化的编号（无 Drop 关切）；
/// `symbols` 是 per-realm 符号表（经 `RefCell` 内部可变性，与 session/gc 同型）。
pub(crate) struct Realm {
    /// realm 编号：构造时经 `KernelCore::alloc_realm_id` 分配，进程内单调递增，
    /// 首个 realm 编号为 0（与旧符号编码一致）。符号身份 = (realm 编号, 局部下标)。
    pub(crate) realm_id: u32,
    pub(crate) session: RefCell<KernelSession>,
    /// `%Object.prototype%`：session 的 Object 原型（global 的 `[[Prototype]]` 挂它）。
    pub(crate) object_prototype: RefCell<P<JsObject>>,
    /// `%GeneratorPrototype%`：生成器实例的原型（next/return/throw 方法挂此）。
    pub(crate) generator_proto: RefCell<P<JsObject>>,
    /// `%GeneratorFunction.prototype%`：生成器函数对象的原型（`constructor` 指向
    /// `%GeneratorFunction%`，使 `g.constructor.name` 解析为 "GeneratorFunction"）。
    pub(crate) generator_function_proto: RefCell<P<JsObject>>,
    /// `%Promise%` 构造器（resolve/reject 静态方法挂此，global 的 Promise 槽指向它）。
    pub(crate) promise_constructor: RefCell<P<JsObject>>,
    /// `%Promise.prototype%`：Promise 实例的原型（then/catch/finally 方法挂此）。
    pub(crate) promise_proto: RefCell<P<JsObject>>,
    /// `%AggregateError%` 构造器（Promise.any 拒绝时构造 AggregateError 用）。
    pub(crate) aggregate_error_constructor: RefCell<P<JsObject>>,
    /// `%AggregateError.prototype%`（proto = %Error.prototype%）。
    pub(crate) aggregate_error_proto: RefCell<P<JsObject>>,
    /// `%AsyncFunction.prototype%`：异步函数对象的原型（`constructor` 指向 `%AsyncFunction%`）。
    pub(crate) async_function_proto: RefCell<P<JsObject>>,
    /// `%AsyncGeneratorPrototype%`：异步生成器实例的原型（next/return/throw/@@asyncIterator）。
    pub(crate) async_generator_proto: RefCell<P<JsObject>>,
    /// `%AsyncGeneratorFunction.prototype%`：异步生成器函数对象的原型。
    pub(crate) async_generator_function_proto: RefCell<P<JsObject>>,
    /// session 堆与 GC 簿记（对象/字符串/BigInt/cell 四表 + 水位与账目）。
    pub(crate) gc: RefCell<GcState>,
    /// per-realm 符号表：well-known 名表 + 用户符号描述 + `Symbol.for` 全局注册表。
    /// 只持 Rust `String` 与 `u32` 下标，无 GC 根，移入 realm 后 `Send` 性不变。
    pub(crate) symbols: RefCell<SymbolState>,
}

impl Drop for Realm {
    fn drop(&mut self) {
        // 直接 drop（test262 每测试新建即弃）不经 reset / full_reset 路径：
        // 统一收尾释放全部 session 堆数据与内建原型属性区，防逐测试累积泄漏。
        // per-realm 消亡：Arc 计数归零时恰好执行一次，多 Vm 共享同一 Realm
        // 时最后一个 Vm 析构不触发收尾（计数未归零），归零才触发。
        self.teardown_intrinsic_protos();
        self.teardown_session_heap_data();
        // session 字段随后按声明顺序 drop → KernelSession::drop →
        // teardown_builtins（幂等），与上两组释放集合不相交（oxide_kernel
        // 不变量：登记表与 retire_replaced 对象集不相交、恰好一次）。
    }
}

impl Realm {
    /// 释放本 realm 内建原型 P 对象（生成器/Promise/异步族与 Object 原型）的
    /// 堆外属性区。
    ///
    /// # 注意事项
    /// 对象本体随 Arc 引用归零释放（字段 drop）；属性区须在此显式释放一次
    /// （`JsObject` 无 Drop 口径）。仅释放本 realm 独占（强引用计数为 1）的
    /// 副本：`object_prototype` 与 session world 共享 Arc，session 存活期
    /// 下一测试仍经 world 引用同一对象，其属性区归 session 收尾
    /// （`teardown_builtins`）释放。
    pub(crate) fn teardown_intrinsic_protos(&self) {
        for p in [
            &self.object_prototype,
            &self.generator_proto,
            &self.generator_function_proto,
            &self.promise_constructor,
            &self.promise_proto,
            &self.aggregate_error_constructor,
            &self.aggregate_error_proto,
            &self.async_function_proto,
            &self.async_generator_proto,
            &self.async_generator_function_proto,
        ] {
            let p = p.borrow();
            if p.strong_count() != 1 {
                continue;
            }
            // SAFETY: 独占副本的属性区仅此一处释放并置空（幂等）。
            unsafe {
                (&mut *p.as_mut_ptr()).release_raw_heap();
            }
        }
    }

    /// 释放全部 session 堆数据：session 对象（本体 + 堆数据 + upvalue 列表）
    /// + session 串 + BigInt box + upvalue cell box。
    ///
    /// 供 `full_reset` 与 `Drop` 共用——统一入口路径上对象本体全部为堆载体
    /// （`Box::from_raw` 恰好释放一次），堆数据与 upvalue 列表各恰好释放一次。
    /// 独占所有权免去重：死对象已出表不重复枚举。
    pub(crate) fn teardown_session_heap_data(&self) {
        // 对象逐条独占释放：本体 + 堆数据 + upvalue 列表，顺序由
        // drop_dead_session_object 收口。
        for ptr in self.gc.borrow_mut().session_object_ptrs.drain(..) {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 session 对象表登记，收尾时仍指向合法对象；
            // 表独占持有，无其他释放点。
            SessionGc::drop_dead_session_object(ptr);
        }
        self.free_session_string_heap_data();
        self.free_session_bigint_heap_data();
        self.gc.borrow_mut().free_cells();
    }

    /// 释放全部 session 堆 `JsString`。仅在完全隔离重置
    /// （`full_reset` / `clear_full_reset_state`）时调用，此时没有存活的
    /// session 对象会引用它们。较轻量的 `reset()` 刻意保留它们，与 session
    /// 串跨 eval 存活一致。
    fn free_session_string_heap_data(&self) {
        for ptr in self.gc.borrow_mut().session_string_ptrs.drain(..) {
            // SAFETY: 每个指针来自 new_string/new_cons_string 的 Box::into_raw，
            // 且只在这里（或 sweep）恰好释放一次；内部连带释放 rope 扁平化产物。
            unsafe {
                SessionGc::drop_session_string_box(ptr);
            }
        }
    }

    /// 释放全部 session 堆 BigInt box。仅在完全隔离重置（`full_reset`）时
    /// 调用，此时没有存活的 session 对象/寄存器会引用它们。
    fn free_session_bigint_heap_data(&self) {
        for ptr in self.gc.borrow_mut().session_bigint_ptrs.borrow_mut().drain(..) {
            // SAFETY: 每个指针来自 new_bigint 的 Box::into_raw(Box::new(BigInt))，
            // 且只在这里恰好释放一次。
            unsafe {
                drop(Box::from_raw(ptr));
            }
        }
    }
}
