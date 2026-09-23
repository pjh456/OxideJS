//! Atomics.waitAsync waiter 核：per-(缓冲对象, 偏移) 的 promise FIFO 表、登记与
//! notify 唤醒（直调 `fulfill_promise` 结算 "ok"，反应入队随 run 收尾 FIFO drain）。
//!
//! 键 = 登记时刻视图的 `buffer` 指针：同 run 无 GC 时 notify 侧读同一指针恒匹配；
//! run 内 GC 晋升后指针漂移则唤醒退化 0（单线程语义，无界外观察钉）。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::vm::Vm;

impl Vm {
    /// 新建 pending Promise（waitAsync 异步臂的 value 槽）。
    ///
    /// resolve/reject 闭包弃用：结算路径是 notify 唤醒直调 [`Self::fulfill_promise`]，
    /// 不经闭包。
    pub(crate) fn atomics_new_waiter_promise(&mut self) -> JsValue {
        let (promise, _resolve, _reject) = self.new_promise_capability();
        promise
    }

    /// 登记 waitAsync waiter：键 = (缓冲对象指针, 元素字节偏移)，同键 FIFO 追加。
    ///
    /// # 注意事项
    /// 调用方须传登记时刻视图的 `buffer` 现指针（与 notify 侧同一读径）；表在
    /// run 边界清空（`clear_execution_state`），清位后 promise 无强根自然回收。
    pub(crate) fn atomics_register_waiter(&mut self, buffer: *mut JsObject, offset: usize, promise: JsValue) {
        self.atomics_waiters.entry((buffer as u64, offset)).or_default().push(promise);
    }

    /// 唤醒 (缓冲, 偏移) 处登记的前 `count` 个 waiter（`<= 0` 不唤醒、`+Inf` 全唤醒），
    /// 逐个直调 `fulfill_promise` 结算 "ok"（反应入队随 run 收尾 drain 执行）。
    ///
    /// # 返回
    /// 实际唤醒数（队列不存在或不足时为 0 / 队列长）。
    pub(crate) fn atomics_wake_waiters(&mut self, buffer: *mut JsObject, offset: usize, count: f64) -> usize {
        // 唤醒数先独立判定（借用即时结束），结算串创建不跨表借用。
        let n = {
            let Some(waiters) = self.atomics_waiters.get_mut(&(buffer as u64, offset)) else {
                return 0;
            };
            if count <= 0.0 {
                0
            } else if count.is_infinite() {
                waiters.len()
            } else {
                (count as usize).min(waiters.len())
            }
        };
        if n == 0 {
            return 0;
        }
        let ok = self.new_string("ok");
        let Some(waiters) = self.atomics_waiters.get_mut(&(buffer as u64, offset)) else {
            return 0;
        };
        let woken: Vec<JsValue> = waiters.drain(..n).collect();
        for promise in woken {
            let _ = self.fulfill_promise(promise, ok);
        }
        n
    }
}
