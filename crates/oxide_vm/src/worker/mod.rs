//! worker 线程基础设施：WorkerMail 消息通道、WorkerHandle 句柄与 worker 事件循环。
//!
//! 关键约定：
//! - 每 worker 是独立 OS 线程，线程内经 `Vm::with_kernel_core` 建专属 Vm（新 realm），
//!   线程退出即 drop（`Drop for Vm` → `Drop for Realm`）。
//! - `Arc<KernelCore>` 跨线程共享（Send/Sync），字节码缓存跨 worker 命中。
//! - 跨线程消息经 `MessageValue`（Send 中间表示，detach/rehydrate），`JsValue` 不
//!   跨线程（session 堆指针是 realm 局部地址，跨线程无效）。
//! - 主线程 → worker 通道载 `WorkerMail`（数据 / 错误 / 关停三面）；worker → 主线程
//!   通道载 `WorkerOutMail`（数据 / 错误两面），主线程轮询时 rehydrate 进主 realm。
//! - worker 脚本编译或执行失败不 panic：经 worker → 主线程通道错误面上报错误串
//!   （主线程交付 onerror），worker 继续事件循环（可被干净终止）。

pub mod bindings;

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use oxide_builtins::message_value::{rehydrate_message, MessageValue};
use oxide_kernel::kernel::KernelCore;
use oxide_kernel::message_queue::{channel, Receiver, Sender, Timeout};
use oxide_runtime_api::{CompilerService, NativeResult};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::vm::Vm;
use crate::vm_debug;
use crate::vm_warn;

/// worker 邮件（主线程 → worker 线程）：数据 / 错误 / 关停三面。
///
/// `Send`（`MessageValue` 与 `String` 均 Send），可经 `Sender<WorkerMail>` 跨线程投递。
pub enum WorkerMail {
    /// 数据面：一条结构化克隆消息值，worker rehydrate 后处理。
    Message(MessageValue),
    /// 错误面：主线程向 worker 上报错误（worker 记入 `last_uncaught_value`）。
    Error(String),
    /// 关停信号：worker 退出事件循环。
    Terminate,
}

/// worker 出站邮件（worker 线程 → 主线程）：数据面与错误面分离。
///
/// `Send`（`MessageValue` 与 `String` 均 Send），可经 `Sender<WorkerOutMail>` 跨线程投递。
pub enum WorkerOutMail {
    /// 数据面：一条结构化克隆消息值，主线程 rehydrate 后交付 onmessage。
    Message(MessageValue),
    /// 错误面：worker 脚本编译或执行失败错误串，主线程交付 onerror。
    Error(String),
}

/// 轮询结果：数据面（rehydrate 后的消息值）、错误面（错误串）与消息交付失败面分离。
pub struct WorkerPoll {
    /// 数据面：rehydrate 进主 realm 的消息值列表。
    pub messages: Vec<JsValue>,
    /// 错误面：worker 脚本编译或执行失败错误串列表（交付 onerror）。
    pub errors: Vec<String>,
    /// 消息交付失败面：rehydrate 失败的消息描述列表（交付 onmessageerror）。
    /// 当前架构 rehydrate 不可失败，此面恒空（SAB 真传递的前向钩子）。
    pub message_errors: Vec<String>,
}

/// worker 句柄：主线程对单个 worker 的持有。
///
/// `id` 是单调递增的 worker 编号（主线程分配）；`tx` 是主线程 → worker 通道
/// （发 `WorkerMail`）；`rx_out` 是 worker → 主线程消息通道（收 `WorkerOutMail`，
/// 轮询时 rehydrate 进主 realm）；`handle` 是 OS 线程句柄（终止时 join）。
pub struct WorkerHandle {
    /// worker 编号（主线程分配，单调递增）。
    pub id: u64,
    /// 主线程 → worker 通道（发 `WorkerMail`）。
    pub tx: Sender<WorkerMail>,
    /// worker → 主线程消息通道（收 `WorkerOutMail`）。
    pub(crate) rx_out: Receiver<WorkerOutMail>,
    /// OS 线程句柄（终止时 join）。
    pub(crate) handle: JoinHandle<()>,
}

/// worker 事件循环：在 worker 线程内运行。
///
/// # 步骤
/// 1. 经 `Vm::with_kernel_core` 建专属 Vm（新 realm）并注入编译服务。
/// 2. 编译 worker 脚本（worker 的程序）；失败则经 worker → 主线程通道错误面上报
///    错误串，不 panic，worker 继续事件循环（可被干净终止）。
/// 3. 编译成功则运行脚本（worker 的程序）；运行期未捕获异常同样经错误面上报。
/// 4. 循环 `recv_timeout(100ms)`：处理 `Message` / `Error` / `Terminate` / 超时。
///
/// # 边界与前提
/// - `script` 是 worker 的程序源码（普通脚本），编译与运行均在 worker 线程内完成。
/// - 超时后查 `rx.is_disconnected()`：主线程全部发送端 drop 即断开，worker 退出。
///
/// # 副作用
/// - 创建并 drop 一个 Vm（退出时 drop 触发 `Drop for Vm` → `Drop for Realm`）。
/// - 每条 `Message` 经 rehydrate → onmessage 交付 → drain_microtasks 处理；
///   编译或执行失败经 `WorkerOutMail::Error` 上报主线程（主线程交付 onerror）。
fn worker_event_loop(
    core: Arc<KernelCore>, compiler: Arc<dyn CompilerService>, script: String, rx: Receiver<WorkerMail>,
    out_tx: Sender<WorkerOutMail>,
) {
    let mut vm = Vm::with_kernel_core(core);
    vm.set_compiler_service(Arc::clone(&compiler));
    // 注入 worker → 主线程输出通道（self.postMessage 经 thread-local 发回主线程）。
    bindings::set_worker_out_tx(out_tx.clone());

    // 编译 worker 脚本（worker 的程序）。失败不 panic：经错误面上报后继续循环。
    let script_module = match compiler.compile_script(&script) {
        Ok(module) => Some(module),
        Err(err) => {
            vm_warn!("worker: script compile failed: {err}");
            let _ = out_tx.send(WorkerOutMail::Error(err));
            None
        }
    };

    // 运行脚本（worker 的程序）。运行期未捕获异常经错误面上报，不中断循环。
    if let Some(module) = script_module {
        if let Err(err) = vm.run(&Arc::new(module)) {
            let _ = out_tx.send(WorkerOutMail::Error(err));
        }
    }

    // 事件循环：recv_timeout 驱动，超时查断开。
    loop {
        // 自关停请求（self.close()）：事件循环每轮检查，置位即退出。
        if bindings::worker_close_requested() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(WorkerMail::Message(value)) => {
                // rehydrate → 建 MessageEvent → 交付 self.onmessage → drain 微任务
                // （onmessage 语义替换早期回显语义）。
                vm.deliver_self_message(value);
            }
            Ok(WorkerMail::Error(err)) => {
                // 主线程上报的错误记入 last_uncaught_value。
                vm.last_uncaught_value = Some(vm.new_string(&err));
            }
            Ok(WorkerMail::Terminate) => break,
            Err(Timeout) => {
                // 超时：主线程全部发送端 drop 即断开，worker 退出。
                if rx.is_disconnected() {
                    break;
                }
            }
        }
    }

    // 退出：drop(vm) 触发 Drop for Vm → Drop for Realm（per-realm 收尾）。
    drop(vm);
    vm_debug!("worker: event loop exited");
}

impl Vm {
    /// 派生一个 worker：建 `WorkerHandle` 并 spawn 一个 OS 线程运行事件循环。
    ///
    /// # 步骤
    /// 1. 分配 worker 编号（`worker_next_id` 自增）。
    /// 2. 建主线程 → worker 与 worker → 主线程两条通道。
    /// 3. spawn 线程，线程内建专属 Vm 并进入事件循环。
    /// 4. 把 `WorkerHandle` 登记进 `worker_registry`。
    ///
    /// # 边界与前提
    /// - `script` 是 worker 的程序源码（普通脚本）；编译在 worker 线程内完成，
    ///   编译失败经 worker → 主线程通道上报，不 panic。
    /// - realm 计数受 512 上界约束（每 worker 一个 realm），worker 数须远低于 512。
    ///
    /// # 返回值
    /// 新 worker 的编号。
    ///
    /// # 副作用
    /// - 派生一个 OS 线程；登记一个 `WorkerHandle`。
    pub fn spawn_worker(&mut self, script: &str) -> Result<u64, String> {
        let id = self.worker_next_id;
        self.worker_next_id += 1;

        let (tx, rx) = channel::<WorkerMail>();
        let (out_tx, out_rx) = channel::<WorkerOutMail>();

        let core = Arc::clone(&self.kernel_core);
        let compiler = Arc::clone(&self.compiler);
        let script_owned = script.to_string();

        let handle = std::thread::spawn(move || {
            worker_event_loop(core, compiler, script_owned, rx, out_tx);
        });

        let worker = WorkerHandle { id, tx, rx_out: out_rx, handle };
        self.worker_registry.insert(id, worker);
        Ok(id)
    }

    /// 向 worker 投递一条消息（经主线程 → worker 通道的 `WorkerMail::Message`）。
    ///
    /// # 边界与前提
    /// - `id` 不存在时返回 `Err`。
    /// - worker 通道已断开（worker 线程退出）时返回 `Err`，消息归还调用方。
    ///
    /// # 返回值
    /// 投递成功 `Ok(())`，失败 `Err`（含错误描述）。
    pub fn worker_post_message(&mut self, id: u64, msg: MessageValue) -> Result<(), String> {
        let worker = self.worker_registry.get_mut(&id).ok_or_else(|| format!("worker {id} 不存在"))?;
        worker
            .tx
            .send(WorkerMail::Message(msg))
            .map_err(|_| format!("worker {id} 通道已断开"))
    }

    /// 终止 worker：发 `Terminate` 并 join 线程。
    ///
    /// # 步骤
    /// 1. 发 `WorkerMail::Terminate`（worker 收到即退出事件循环）。
    /// 2. 从 `worker_registry` 移除（取走 `WorkerHandle`）。
    /// 3. join 线程（须在句柄 drop 前完成，防线程泄漏）。
    /// 4. 从 `worker_objects` 移除 Worker 对象注册表条目（GC 根解除）。
    ///
    /// # 边界与前提
    /// - `id` 不存在时返回 `Err`。
    /// - 线程 panic 时返回 `Err`。
    ///
    /// # 副作用
    /// - 移除一个 `WorkerHandle` 与一条 Worker 对象注册表条目；join 一个 OS 线程。
    pub fn worker_terminate(&mut self, id: u64) -> Result<(), String> {
        // 发关停信号（worker 收到即退出事件循环）。
        if let Some(worker) = self.worker_registry.get_mut(&id) {
            let _ = worker.tx.send(WorkerMail::Terminate);
        }
        // 从注册表移除并 join 线程（句柄 drop 前必须 join）。
        let worker = self.worker_registry.remove(&id).ok_or_else(|| format!("worker {id} 不存在"))?;
        worker.handle.join().map_err(|_| format!("worker {id} 线程异常退出"))?;
        // 解除 Worker 对象注册表条目（GC 根解除，对象可被回收）。
        self.worker_objects.remove(&id);
        Ok(())
    }

    /// 轮询 worker 消息：排空 worker → 主线程通道，数据面 rehydrate 进主 realm。
    ///
    /// # 步骤
    /// 1. 取 `WorkerHandle`（`id` 不存在时返回空轮询结果）。
    /// 2. `try_recv` 循环排空 `rx_out`，逐条按信封分面：
    ///    - `Message` 臂 rehydrate 进主 realm 归数据面。
    ///    - `Error` 臂错误串原样透传归错误面。
    ///
    /// # 返回值
    /// 本批排空的轮询结果（数据 / 错误 / 消息交付失败三面，无消息时各面为空）。
    ///
    /// # 边界与前提
    /// - 非阻塞（`try_recv`），不等待新消息。
    /// - rehydrate 当前架构不可失败；若未来变可失败，失败面进 `message_errors`
    ///   （SAB 真传递的前向钩子）。
    pub fn poll_worker_messages(&mut self, id: u64) -> WorkerPoll {
        let mut poll = WorkerPoll {
            messages: Vec::new(),
            errors: Vec::new(),
            message_errors: Vec::new(),
        };
        // 先排空 worker → 主线程通道到本地列表（注册表借用与 rehydrate 借用
        // 不重叠，避免对 self 的双重可变借用）。
        let drained: Vec<WorkerOutMail> = {
            let Some(worker) = self.worker_registry.get_mut(&id) else {
                return poll;
            };
            let mut drained = Vec::new();
            while let Ok(mail) = worker.rx_out.try_recv() {
                drained.push(mail);
            }
            drained
        };
        for mail in &drained {
            match mail {
                WorkerOutMail::Message(mv) => {
                    poll.messages.push(rehydrate_message(self, mv));
                }
                WorkerOutMail::Error(err) => {
                    poll.errors.push(err.clone());
                }
            }
        }
        poll
    }

    /// 列出活跃 worker 编号（升序）。
    pub fn active_workers(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.worker_registry.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// 终止全部 worker 并 join（`Drop for Vm` 调用，防线程泄漏）。
    ///
    /// # 步骤
    /// 1. 收集全部活跃 worker 编号。
    /// 2. 逐一 `worker_terminate`（发 `Terminate` + join）。
    ///
    /// # 副作用
    /// - 清空 `worker_registry`；join 全部 worker 线程。
    pub fn shutdown_workers(&mut self) {
        let ids = self.active_workers();
        for id in ids {
            let _ = self.worker_terminate(id);
        }
    }

    /// 按 worker 编号反查 Worker 对象（注册表查找，GC 根）。
    ///
    /// # 返回值
    /// 注册表中该编号的 Worker 对象值；编号不存在时 `None`。
    pub(crate) fn worker_object(&self, id: u64) -> Option<JsValue> {
        self.worker_objects.get(&id).copied()
    }

    /// 主线程事件循环消息交付：排空 worker → 主线程通道，数据面 rehydrate 进主
    /// realm 交付到 Worker 对象的 `onmessage`，错误面交付到 `onerror`，消息交付
    /// 失败面交付到 `onmessageerror`。
    ///
    /// # 步骤
    /// 1. 遍历活跃 worker 编号。
    /// 2. 对每个 worker `poll_worker_messages` 排空通道（数据面 rehydrate 进主 realm）。
    /// 3. 反查 Worker 对象（GC 根）。
    /// 4. 错误面逐条交付 `onerror`；消息交付失败面逐条交付 `onmessageerror`。
    /// 5. 数据面读 `onmessage` 属性，可调用时逐条建 MessageEvent 经
    ///    `execute_task` + `call_function_sync` 触发，后 `drain_microtasks`。
    ///
    /// # 返回值
    /// 本轮是否交付了任何面（false 时调用方 1ms 轮询，避免忙等）。
    ///
    /// # 边界与前提
    /// - 各 handler 缺失或非可调用时静默跳过（邮件已消费，符合浏览器
    ///   "无 handler 即丢弃"语义）。
    /// - 交付前二次 `worker_object` 校验存活（GC 防护）：worker 在事件循环中
    ///   被终止（注册表条目移除）即停止交付。
    ///
    /// # 副作用
    /// - 消费 worker → 主线程通道邮件；触发 `onmessage` / `onerror` /
    ///   `onmessageerror` 回调与微任务 drain。
    pub fn deliver_worker_messages(&mut self) -> bool {
        let mut any = false;
        for id in self.active_workers() {
            let poll = self.poll_worker_messages(id);
            if poll.messages.is_empty() && poll.errors.is_empty() && poll.message_errors.is_empty() {
                continue;
            }
            any = true;
            // Worker 对象注册表反查（GC 根，注册即存活）。
            let Some(worker_obj) = self.worker_object(id) else {
                continue;
            };
            // 错误面：逐条经 dispatchEvent 交付 error 事件（编译 / 执行失败）。
            for err in &poll.errors {
                if self.worker_object(id).is_none() {
                    break;
                }
                self.deliver_error_event(worker_obj, "error", err);
            }
            // 消息交付失败面：逐条经 dispatchEvent 交付 messageerror 事件
            // （当前架构不可达，前向钩子）。
            for msg_err in &poll.message_errors {
                if self.worker_object(id).is_none() {
                    break;
                }
                self.deliver_error_event(worker_obj, "messageerror", msg_err);
            }
            // 数据面：经 dispatchEvent 交付（注册表查监听器，addEventListener 与
            // 属性 handler 同权；首版属性 handler 尚不支持，8.7 完成）。
            for data in poll.messages {
                // 交付前二次校验存活（GC 防护）：worker 在事件循环中被终止
                // （注册表条目移除）即停止交付。
                if self.worker_object(id).is_none() {
                    break;
                }
                let event = self.build_message_event(data, "message", None);
                self.dispatch_event_on(worker_obj, event);
                self.drain_microtasks();
            }
        }
        any
    }

    /// 经 `dispatchEvent` 交付事件到目标对象（注册表查监听器，按登记序调用）。
    ///
    /// # 步骤
    /// 1. 目标与事件钉入寄存器 0 / 1（GC 根，防交付窗口内被回收）。
    /// 2. 调 `event_target_dispatch_event`（this = 目标，实参 = 事件）。
    ///
    /// # 边界与前提
    /// - 监听器回调的异常原值上抛（不吞）；无监听器时 no-op（返回 true）。
    ///
    /// # 副作用
    /// - 触发监听器回调与 `once` 条目移除；写事件载荷盒（target / 阶段）。
    fn dispatch_event_on(&mut self, target: JsValue, event: JsValue) {
        self.set_reg(0, target);
        self.set_reg(1, event);
        let _ = oxide_builtins::event_target::event_target_dispatch_event(self, &[0, 1]);
    }

    /// 交付错误事件：建事件、经 `dispatchEvent` 交付（注册表查监听器）。
    ///
    /// # 步骤
    /// 1. 建错误事件（`type` = `event_type`、`message` = `message`、`data` = undefined）。
    /// 2. 经 `dispatch_event_on` 交付到 `worker_obj`，后 `drain_microtasks`。
    ///
    /// # 边界与前提
    /// - `event_type` 是 `"error"` 或 `"messageerror"`。
    /// - `worker_obj` 是 `JsValue`（Copy），不跨 `&mut self` 持引用。
    /// - 无监听器时 `dispatchEvent` 返回 true（no-op）。
    ///
    /// # 副作用
    /// - 触发监听器回调与微任务 drain。
    fn deliver_error_event(&mut self, worker_obj: JsValue, event_type: &str, message: &str) {
        let event = self.build_message_event(JsValue::undefined(), event_type, Some(message));
        self.dispatch_event_on(worker_obj, event);
        self.drain_microtasks();
    }

    /// 建事件对象（`data` + `type` + `message` 属性），委托 `message_event_constructor`
    /// 构造辅助。消息事件 `message` 为 `None`（undefined），错误事件 `data` 为
    /// undefined、`message` 为错误串。
    ///
    /// # 步骤
    /// 1. `type` 与 `message` 串物化为 session 串（`message` 为 `None` 时 undefined）。
    /// 2. `data` / `type` / `message` 写入寄存器 1 / 2 / 3，委托
    ///    `message_event_constructor`（按寄存器下标读取）建对象。
    ///
    /// # 返回值
    /// 新建的事件对象值（构造失败时返回错误对象值）。
    ///
    /// # 副作用
    /// - 新建一个事件对象（经 `alloc_object` 入对象表）。
    fn build_message_event(&mut self, data: JsValue, type_str: &str, message: Option<&str>) -> JsValue {
        let type_val = self.new_string(type_str);
        let message_val = match message {
            Some(m) => self.new_string(m),
            None => JsValue::undefined(),
        };
        self.set_reg(1, data);
        self.set_reg(2, type_val);
        self.set_reg(3, message_val);
        match bindings::message_event_constructor(self, &[0, 1, 2, 3]) {
            NativeResult::Ok(v) => v,
            NativeResult::Err(e) => e,
            // 构造辅助只建对象，不产生尾调用（防御臂，不可达）。
            NativeResult::TailCall { .. } => JsValue::undefined(),
        }
    }

    /// 清理已断开 worker：通道断开即 worker 线程已退出，join 并移除。
    ///
    /// # 步骤
    /// 1. 遍历活跃 worker 编号。
    /// 2. 对每个 worker 查 `rx_out.is_disconnected()`（worker 线程退出后其
    ///    发送端 drop，接收端转断开）。
    /// 3. 断开者 `worker_terminate`（发 `Terminate` + join + 移除注册表条目）。
    ///
    /// # 副作用
    /// - 移除已断开 worker 的注册表条目；join 其 OS 线程。
    pub fn cleanup_disconnected_workers(&mut self) {
        for id in self.active_workers() {
            let disconnected = self.worker_registry.get(&id).is_some_and(|w| w.rx_out.is_disconnected());
            if disconnected {
                let _ = self.worker_terminate(id);
            }
        }
    }

    /// worker 侧消息交付：rehydrate 进 worker realm、建 MessageEvent、经
    /// `dispatchEvent` 交付到 worker global（注册表查监听器）。
    ///
    /// # 步骤
    /// 1. rehydrate 消息值进 worker realm。
    /// 2. 建 MessageEvent（data + type "message"）。
    /// 3. 取 worker global 对象，经 `dispatch_event_on` 交付，后 `drain_microtasks`。
    ///
    /// # 边界与前提
    /// - 无监听器时 `dispatchEvent` 返回 true（消息已消费，符合浏览器
    ///   "无 handler 即丢弃"语义）。
    ///
    /// # 副作用
    /// - 触发监听器回调与微任务 drain。
    pub(crate) fn deliver_self_message(&mut self, value: MessageValue) {
        let data = rehydrate_message(self, &value);
        let event = self.build_message_event(data, "message", None);
        // 取 worker global 对象指针（Ref 守卫在语句块内消费，不跨 &mut 借用长存）。
        let global_ptr = {
            let session = self.realm.session.borrow();
            session.global_object().as_ptr() as *mut JsObject
        };
        // SAFETY: global_ptr 是当前 session 的 global 对象，存活。
        let global_val = JsValue::from_js_object(global_ptr);
        // 经 dispatchEvent 交付（注册表查监听器，属性 handler 与 addEventListener 同权）。
        self.dispatch_event_on(global_val, event);
        self.drain_microtasks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use oxide_compiler::compiler::Compiler;
    use oxide_compiler::DefaultCompilerService;
    use oxide_kernel::kernel::KernelConfig;
    use oxide_parser::Allocator;

    /// 建一个带真实编译服务的 Vm（worker 脚本编译需要）。
    fn vm_with_compiler() -> Vm {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut vm = Vm::with_kernel_core(core);
        vm.set_compiler_service(Arc::new(DefaultCompilerService));
        vm
    }

    /// 轮询至 worker 上报任意一面邮件（带截止，防 flaky）。
    fn poll_until_mail(vm: &mut Vm, id: u64) -> WorkerPoll {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let poll = vm.poll_worker_messages(id);
            if !poll.messages.is_empty() || !poll.errors.is_empty() || Instant::now() >= deadline {
                return poll;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 端到端：spawn → post → onmessage 回显 → terminate，验证 worker 基础设施闭环。
    #[test]
    fn worker_round_trip() {
        let mut vm = vm_with_compiler();

        // worker 脚本设 addEventListener 回显（注册表交付替换早期 onmessage 属性语义）。
        let id = vm
            .spawn_worker("self.addEventListener('message', function(e) { self.postMessage(e.data); });")
            .expect("worker 应派生成功");
        assert_eq!(vm.active_workers(), vec![id], "应有唯一活跃 worker");

        vm.worker_post_message(id, MessageValue::Number(42.0)).expect("投递应成功");
        let poll = poll_until_mail(&mut vm, id);
        assert_eq!(poll.messages.len(), 1, "应回显一条消息");
        // 整数值经 number_to_js 归为 Int 表示。
        assert_eq!(poll.messages[0], JsValue::int(42), "回显值应为 42");

        vm.worker_terminate(id).expect("终止应成功");
        assert!(vm.active_workers().is_empty(), "终止后无活跃 worker");
    }

    /// 编译失败不 panic：worker 经错误面上报错误串，主线程可经轮询取回。
    #[test]
    fn worker_script_compile_failure_reports_error() {
        let mut vm = vm_with_compiler();

        let id = vm.spawn_worker("function { 语法错误").expect("worker 应派生成功");
        let poll = poll_until_mail(&mut vm, id);
        assert!(!poll.errors.is_empty(), "编译失败应经错误面上报错误串");
        assert!(!poll.errors[0].is_empty(), "错误串应非空");
        // 编译失败不再产数据面消息（错误串不伪装成数据）。
        assert!(poll.messages.is_empty(), "编译失败不应产数据面消息");

        vm.worker_terminate(id).expect("终止应成功");
    }

    /// 多 worker 隔离：各 worker 独立 realm，消息不串扰。
    #[test]
    fn multiple_workers_isolated() {
        let mut vm = vm_with_compiler();

        // 两 worker 各设 addEventListener 回显（注册表交付语义）。
        let id_a = vm
            .spawn_worker("self.addEventListener('message', function(e) { self.postMessage(e.data); });")
            .expect("worker A 应派生成功");
        let id_b = vm
            .spawn_worker("self.addEventListener('message', function(e) { self.postMessage(e.data); });")
            .expect("worker B 应派生成功");
        assert_eq!(vm.active_workers(), vec![id_a, id_b], "应有两个活跃 worker");

        vm.worker_post_message(id_a, MessageValue::Number(1.0)).expect("投递 A 应成功");
        vm.worker_post_message(id_b, MessageValue::Number(2.0)).expect("投递 B 应成功");

        let poll_a = poll_until_mail(&mut vm, id_a);
        assert_eq!(poll_a.messages[0], JsValue::int(1), "A 应回显 1");
        let poll_b = poll_until_mail(&mut vm, id_b);
        assert_eq!(poll_b.messages[0], JsValue::int(2), "B 应回显 2");

        vm.shutdown_workers();
        assert!(vm.active_workers().is_empty(), "shutdown 后无活跃 worker");
    }

    /// 编译主脚本源码为 `CompiledModule`（测试驱动主 Vm 执行）。
    fn compile_script(source: &str) -> oxide_bytecode::module::CompiledModule {
        let allocator = Allocator::default();
        let program = oxide_parser::parse(&allocator, source).expect("parse 应成功");
        Compiler::new().compile(&program).expect("compile 应成功")
    }

    /// 写 worker 脚本到临时文件（进程 id 加纳秒唯一名防并发单测冲突），返回路径。
    fn write_worker_script(content: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let name = format!("oxide_worker_{}_{}.js", std::process::id(), nanos);
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, content).expect("应能写 worker 脚本临时文件");
        path
    }

    /// 读全局属性（缺失返 None）。
    fn global_value_opt(vm: &Vm, name: &str) -> Option<JsValue> {
        let session = vm.realm.session.borrow();
        let global = session.global_object();
        let si = vm.kernel_core().perm_interner().intern(name).0;
        vm.resolve_property(global, si)
    }

    /// 数值值转 f64（int 与 double 两表示）。
    fn as_number(v: &JsValue) -> f64 {
        if v.is_int() {
            v.as_int() as f64
        } else {
            v.as_double()
        }
    }

    /// 端到端：主 → worker → main 双向消息（onmessage 交付、e.data 值）。
    ///
    /// 主脚本建 Worker、设 onmessage、投递 21；worker 脚本收到翻倍回发 42；
    /// 事件循环交付到主 onmessage 置 `globalThis.received`。
    #[test]
    fn worker_e2e_onmessage_round_trip() {
        let mut vm = vm_with_compiler();

        // worker 脚本：收到消息翻倍后回发。
        let worker_path =
            write_worker_script("self.addEventListener('message', function(e) { self.postMessage(e.data * 2); });");
        // 主脚本：建 Worker、设 addEventListener、投递 21。
        let main_script = format!(
            "var w = new Worker('{}'); w.addEventListener('message', function(e) {{ globalThis.received = e.data; }}); w.postMessage(21);",
            worker_path.display()
        );
        vm.run(&Arc::new(compile_script(&main_script))).expect("主脚本应运行");

        // 事件循环轮询至 globalThis.received === 42（带截止，防 flaky）。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            vm.deliver_worker_messages();
            vm.cleanup_disconnected_workers();
            if let Some(v) = global_value_opt(&vm, "received") {
                if as_number(&v) == 42.0 {
                    break;
                }
            }
            if Instant::now() >= deadline {
                panic!("5 秒内未收到回显消息 42");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            as_number(&global_value_opt(&vm, "received").expect("received 应被设置")),
            42.0,
            "回显值应为 42"
        );

        vm.shutdown_workers();
        let _ = std::fs::remove_file(&worker_path);
    }

    /// 端到端：terminate 后注册表清空、二次 postMessage 返 Err。
    #[test]
    fn worker_e2e_terminate() {
        let mut vm = vm_with_compiler();

        let id = vm.spawn_worker("1").expect("worker 应派生成功");
        assert_eq!(vm.active_workers(), vec![id], "应有唯一活跃 worker");

        vm.worker_terminate(id).expect("终止应成功");
        assert!(vm.active_workers().is_empty(), "终止后无活跃 worker");

        let result = vm.worker_post_message(id, MessageValue::Number(1.0));
        assert!(result.is_err(), "二次 postMessage 应返 Err");
    }

    /// 端到端：realm 回收计数配对（spawn +1、terminate 回基线）。
    #[test]
    fn worker_e2e_realm_reclaimed() {
        let mut vm = vm_with_compiler();
        // 克隆 Arc 解耦借用（kernel_core 返回 &Arc，长存会阻塞后续 &mut 借用）。
        let core = Arc::clone(vm.kernel_core());
        let baseline = core.active_vms();

        let id = vm.spawn_worker("1").expect("worker 应派生成功");
        // worker 线程异步建 Vm：轮询至 realm 计数 +1（带截止，防 flaky）。
        let deadline = Instant::now() + Duration::from_secs(5);
        while core.active_vms() < baseline + 1 {
            if Instant::now() >= deadline {
                panic!("5 秒内 worker realm 未创建");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(core.active_vms(), baseline + 1, "spawn 后应 +1 realm");

        vm.worker_terminate(id).expect("终止应成功");
        assert_eq!(core.active_vms(), baseline, "terminate 后应回基线");
    }

    /// 端到端：postMessage 不可克隆值抛 DataCloneError、事件循环不挂死。
    #[test]
    fn worker_e2e_data_clone_error() {
        let mut vm = vm_with_compiler();

        let worker_path = write_worker_script("1");
        // 主脚本：建 Worker、postMessage 函数（不可克隆），捕获 DataCloneError。
        let main_script = format!(
            "var w = new Worker('{}'); try {{ w.postMessage(function(){{}}); globalThis.cloneError = false; }} catch (e) {{ globalThis.cloneError = true; }}",
            worker_path.display()
        );
        vm.run(&Arc::new(compile_script(&main_script))).expect("主脚本应运行");

        assert_eq!(
            global_value_opt(&vm, "cloneError"),
            Some(JsValue::bool(true)),
            "postMessage 函数应抛 DataCloneError"
        );
        // worker 仍存活（postMessage 失败不影响 worker）。
        assert!(!vm.active_workers().is_empty(), "worker 应仍存活");

        vm.shutdown_workers();
        let _ = std::fs::remove_file(&worker_path);
    }

    /// 端到端：worker 脚本编译失败，主线程 onerror 被调用（message 非空、type 为 "error"）。
    #[test]
    fn worker_e2e_onerror_compile_failure() {
        let mut vm = vm_with_compiler();

        // worker 脚本：语法错误（编译失败）。
        let worker_path = write_worker_script("function { 语法错误");
        // 主脚本：建 Worker、设 addEventListener('error')，记录 e.message 与 e.type 是否为 "error"。
        let main_script = format!(
            "var w = new Worker('{}'); w.addEventListener('error', function(e) {{ globalThis.err = e.message; globalThis.errIsErrorType = (e.type === \"error\"); }});",
            worker_path.display()
        );
        vm.run(&Arc::new(compile_script(&main_script))).expect("主脚本应运行");

        // 事件循环轮询至 globalThis.err 为字符串（带截止，防 flaky）。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            vm.deliver_worker_messages();
            vm.cleanup_disconnected_workers();
            if let Some(v) = global_value_opt(&vm, "err") {
                if v.is_string() {
                    break;
                }
            }
            if Instant::now() >= deadline {
                panic!("5 秒内 onerror 未被调用");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // message 非空。
        let err = global_value_opt(&vm, "err").expect("err 应被设置");
        assert!(err.is_string(), "err 应为字符串");
        let err_text = vm.lookup_str(err).expect("err 应可读");
        assert!(!err_text.is_empty(), "err 应非空");
        // type 为 "error"。
        assert_eq!(
            global_value_opt(&vm, "errIsErrorType"),
            Some(JsValue::bool(true)),
            "事件 type 应为 \"error\""
        );

        vm.shutdown_workers();
        let _ = std::fs::remove_file(&worker_path);
    }

    /// 端到端：worker 脚本顶层 throw（执行失败），主线程 onerror 被调用（message 含错误文本）。
    #[test]
    fn worker_e2e_onerror_runtime_failure() {
        let mut vm = vm_with_compiler();

        // worker 脚本：顶层 throw（执行失败）。
        let worker_path = write_worker_script("throw new Error('boom');");
        // 主脚本：建 Worker、设 addEventListener('error')，记录 e.message。
        let main_script = format!(
            "var w = new Worker('{}'); w.addEventListener('error', function(e) {{ globalThis.err = e.message; }});",
            worker_path.display()
        );
        vm.run(&Arc::new(compile_script(&main_script))).expect("主脚本应运行");

        // 事件循环轮询至 globalThis.err 为字符串（带截止，防 flaky）。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            vm.deliver_worker_messages();
            vm.cleanup_disconnected_workers();
            if let Some(v) = global_value_opt(&vm, "err") {
                if v.is_string() {
                    break;
                }
            }
            if Instant::now() >= deadline {
                panic!("5 秒内 onerror 未被调用");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // message 含错误文本 "boom"。
        let err = global_value_opt(&vm, "err").expect("err 应被设置");
        let err_text = vm.lookup_str(err).expect("err 应可读");
        assert!(err_text.contains("boom"), "err 应含错误文本 \"boom\"，实际为 {err_text}");

        vm.shutdown_workers();
        let _ = std::fs::remove_file(&worker_path);
    }

    /// onmessageerror 交付辅助：无自然触发，直接单测交付辅助（handler 被调用、
    /// 事件 type 为 "messageerror"）。不钉触发可达性（当前架构不可达）。
    #[test]
    fn onmessageerror_delivery_helper() {
        let mut vm = vm_with_compiler();

        // 建 Worker 对象、设 addEventListener('messageerror')（记录 e.message 与 e.type）。
        let worker_path = write_worker_script("1");
        let main_script = format!(
            "var w = new Worker('{}'); w.addEventListener('messageerror', function(e) {{ globalThis.me = e.message; globalThis.meIsMessageErrorType = (e.type === \"messageerror\"); }}); globalThis.w = w;",
            worker_path.display()
        );
        vm.run(&Arc::new(compile_script(&main_script))).expect("主脚本应运行");

        // 取 Worker 对象，直接调交付辅助（模拟 rehydrate 失败，经 dispatchEvent 交付）。
        let worker_obj = global_value_opt(&vm, "w").expect("w 应被设置");
        vm.deliver_error_event(worker_obj, "messageerror", "合成 rehydrate 失败");

        // 断言 handler 被调用、message 为错误串、type 为 "messageerror"。
        let me = global_value_opt(&vm, "me").expect("me 应被设置");
        let me_text = vm.lookup_str(me).expect("me 应可读");
        assert_eq!(me_text, "合成 rehydrate 失败", "message 应为错误串");
        assert_eq!(
            global_value_opt(&vm, "meIsMessageErrorType"),
            Some(JsValue::bool(true)),
            "事件 type 应为 \"messageerror\""
        );

        vm.shutdown_workers();
        let _ = std::fs::remove_file(&worker_path);
    }
}
