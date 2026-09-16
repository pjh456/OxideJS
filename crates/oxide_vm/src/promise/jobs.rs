//! 微任务队列：FIFO drain 执行与 session GC 的队列值遍历 / 改写。

use oxide_types::value::JsValue;

use crate::vm::Vm;
use crate::vm_warn;

use super::{Microtask, MAX_DRAIN_JOBS};

impl Vm {
    /// drain 微任务队列：FIFO 逐条处理直到清空或达到上限。
    ///
    /// # 副作用
    /// - 任务内调用 JS 回调（`call_function_sync`），可能入队新任务。
    /// - 单任务抛错不中断 drain（未处理拒绝按规范不可见）。
    pub(crate) fn drain_job_queue(&mut self) {
        let mut count = 0usize;
        while let Some(job) = self.job_queue.pop_front() {
            count += 1;
            if count > MAX_DRAIN_JOBS {
                vm_warn!("drain_job_queue: exceeded {MAX_DRAIN_JOBS} microtasks, aborting");
                break;
            }
            self.run_microtask(job);
        }
    }

    /// 执行单条微任务。
    fn run_microtask(&mut self, job: Microtask) {
        match job {
            Microtask::Reaction {
                is_fulfill,
                handler,
                argument,
                resolve,
                reject,
            } => {
                // 空处理器（undefined/非可调用）→ 值直通：fulfill 调 resolve，reject 调 reject。
                let handler_result = if oxide_builtins::iterator::is_callable(handler) {
                    self.call_function_sync(handler, JsValue::undefined(), &[argument])
                } else if is_fulfill {
                    Ok(argument)
                } else {
                    let _ = self.call_function_sync(reject, JsValue::undefined(), &[argument]);
                    return;
                };
                match handler_result {
                    Ok(x) => {
                        let _ = self.call_function_sync(resolve, JsValue::undefined(), &[x]);
                    }
                    Err(e) => {
                        let exc = self
                            .last_uncaught_value
                            .take()
                            .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                        let _ = self.call_function_sync(reject, JsValue::undefined(), &[exc]);
                    }
                }
            }
            Microtask::Thenable {
                thenable,
                then,
                resolve,
                reject,
            } => {
                // 委托调用；抛错则拒绝目标 promise。
                match self.call_function_sync(then, thenable, &[resolve, reject]) {
                    Ok(_) => {}
                    Err(e) => {
                        let exc = self
                            .last_uncaught_value
                            .take()
                            .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                        let _ = self.call_function_sync(reject, JsValue::undefined(), &[exc]);
                    }
                }
            }
        }
    }
}

/// 供外部遍历微任务队列（GC mark/rewrite 用）。
pub(crate) fn for_each_job_value(job: &Microtask, mut f: impl FnMut(JsValue)) {
    match job {
        Microtask::Reaction {
            handler,
            argument,
            resolve,
            reject,
            ..
        } => {
            f(*handler);
            f(*argument);
            f(*resolve);
            f(*reject);
        }
        Microtask::Thenable {
            thenable,
            then,
            resolve,
            reject,
        } => {
            f(*thenable);
            f(*then);
            f(*resolve);
            f(*reject);
        }
    }
}

/// 供外部改写微任务队列中的 JsValue（session GC sweep 用）。
pub(crate) fn rewrite_job_values(job: &mut Microtask, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    match job {
        Microtask::Reaction {
            handler,
            argument,
            resolve,
            reject,
            ..
        } => {
            *handler = rewrite(*handler);
            *argument = rewrite(*argument);
            *resolve = rewrite(*resolve);
            *reject = rewrite(*reject);
        }
        Microtask::Thenable {
            thenable,
            then,
            resolve,
            reject,
        } => {
            *thenable = rewrite(*thenable);
            *then = rewrite(*then);
            *resolve = rewrite(*resolve);
            *reject = rewrite(*reject);
        }
    }
}
