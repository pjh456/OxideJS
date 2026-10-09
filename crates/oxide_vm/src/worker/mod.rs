//! worker 线程基础设施：WorkerMail 消息通道、WorkerHandle 句柄与 worker 事件循环。
//!
//! 关键约定：
//! - 每 worker 是独立 OS 线程，线程内经 `Vm::with_kernel_core` 建专属 Vm（新 realm），
//!   线程退出即 drop（`Drop for Vm` → `Drop for Realm`）。
//! - `Arc<KernelCore>` 跨线程共享（Send/Sync），字节码缓存跨 worker 命中。
//! - 跨线程消息经 `MessageValue`（Send 中间表示，detach/rehydrate），`JsValue` 不
//!   跨线程（session 堆指针是 realm 局部地址，跨线程无效）。
//! - 主线程 → worker 通道载 `WorkerMail`（数据 / 错误 / 关停三面）；worker → 主线程
//!   通道载 `MessageValue`，主线程轮询时 rehydrate 进主 realm。
//! - worker 脚本编译失败不 panic：经 worker → 主线程通道上报错误串，worker 继续
//!   事件循环（可被干净终止）。

pub mod bindings;

use std::collections::HashSet;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use oxide_builtins::message_value::{detach_message, rehydrate_message, MessageValue};
use oxide_kernel::kernel::KernelCore;
use oxide_kernel::message_queue::{channel, Receiver, Sender, Timeout};
use oxide_runtime_api::CompilerService;
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

/// worker 句柄：主线程对单个 worker 的持有。
///
/// `id` 是单调递增的 worker 编号（主线程分配）；`tx` 是主线程 → worker 通道
/// （发 `WorkerMail`）；`rx_out` 是 worker → 主线程消息通道（收 `MessageValue`，
/// 轮询时 rehydrate 进主 realm）；`handle` 是 OS 线程句柄（终止时 join）。
pub struct WorkerHandle {
    /// worker 编号（主线程分配，单调递增）。
    pub id: u64,
    /// 主线程 → worker 通道（发 `WorkerMail`）。
    pub tx: Sender<WorkerMail>,
    /// worker → 主线程消息通道（收 `MessageValue`）。
    pub(crate) rx_out: Receiver<MessageValue>,
    /// OS 线程句柄（终止时 join）。
    pub(crate) handle: JoinHandle<()>,
}

/// worker 事件循环：在 worker 线程内运行。
///
/// # 步骤
/// 1. 经 `Vm::with_kernel_core` 建专属 Vm（新 realm）并注入编译服务。
/// 2. 编译 worker 脚本（worker 的程序）；失败则经 worker → 主线程通道上报错误串，
///    不 panic，worker 继续事件循环（可被干净终止）。
/// 3. 编译成功则运行脚本（worker 的程序）。
/// 4. 循环 `recv_timeout(100ms)`：处理 `Message` / `Error` / `Terminate` / 超时。
///
/// # 边界与前提
/// - `script` 是 worker 的程序源码（普通脚本），编译与运行均在 worker 线程内完成。
/// - 超时后查 `rx.is_disconnected()`：主线程全部发送端 drop 即断开，worker 退出。
///
/// # 副作用
/// - 创建并 drop 一个 Vm（退出时 drop 触发 `Drop for Vm` → `Drop for Realm`）。
/// - 每条 `Message` 经 rehydrate → execute_task → drain_microtasks 处理后，把处理
///   值 detach 回 worker → 主线程通道（935.3 的回显语义，935.4 的 onmessage 替换之）。
fn worker_event_loop(
    core: Arc<KernelCore>, compiler: Arc<dyn CompilerService>, script: String, rx: Receiver<WorkerMail>,
    out_tx: Sender<MessageValue>,
) {
    let mut vm = Vm::with_kernel_core(core);
    vm.set_compiler_service(Arc::clone(&compiler));
    // 注入 worker → 主线程输出通道（self.postMessage 经 thread-local 发回主线程）。
    bindings::set_worker_out_tx(out_tx.clone());

    // 编译 worker 脚本（worker 的程序）。失败不 panic：上报错误串后继续循环。
    let script_module = match compiler.compile_script(&script) {
        Ok(module) => Some(module),
        Err(err) => {
            vm_warn!("worker: script compile failed: {err}");
            let units: Box<[u16]> = err.encode_utf16().collect::<Vec<u16>>().into();
            let _ = out_tx.send(MessageValue::String(units));
            None
        }
    };

    // 运行脚本（worker 的程序）。运行期错误记入 last_uncaught_value，不中断循环。
    if let Some(module) = script_module {
        if let Err(err) = vm.run(&Arc::new(module)) {
            vm.last_uncaught_value = Some(vm.new_string(&err));
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
                // rehydrate → execute_task → drain_microtasks，再回显处理值。
                let value = rehydrate_message(&mut vm, &value);
                let _ = vm.execute_task(|_vm| Ok(value));
                vm.drain_microtasks();
                if let Ok(mv) = detach_message(&mut vm, value, &HashSet::new()) {
                    let _ = out_tx.send(mv);
                }
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
        let (out_tx, out_rx) = channel::<MessageValue>();

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
    ///
    /// # 边界与前提
    /// - `id` 不存在时返回 `Err`。
    /// - 线程 panic 时返回 `Err`。
    ///
    /// # 副作用
    /// - 移除一个 `WorkerHandle`；join 一个 OS 线程。
    pub fn worker_terminate(&mut self, id: u64) -> Result<(), String> {
        // 发关停信号（worker 收到即退出事件循环）。
        if let Some(worker) = self.worker_registry.get_mut(&id) {
            let _ = worker.tx.send(WorkerMail::Terminate);
        }
        // 从注册表移除并 join 线程（句柄 drop 前必须 join）。
        let worker = self.worker_registry.remove(&id).ok_or_else(|| format!("worker {id} 不存在"))?;
        worker.handle.join().map_err(|_| format!("worker {id} 线程异常退出"))?;
        Ok(())
    }

    /// 轮询 worker 消息：排空 worker → 主线程通道，rehydrate 进主 realm。
    ///
    /// # 步骤
    /// 1. 取 `WorkerHandle`（`id` 不存在时返回空列表）。
    /// 2. `try_recv` 循环排空 `rx_out`，每条 rehydrate 进主 realm。
    ///
    /// # 返回值
    /// 本批排空的消息值列表（无消息时为空列表）。
    ///
    /// # 边界与前提
    /// - 非阻塞（`try_recv`），不等待新消息。
    pub fn poll_worker_messages(&mut self, id: u64) -> Vec<JsValue> {
        // 先排空 worker → 主线程通道到本地列表（注册表借用与 rehydrate 借用
        // 不重叠，避免对 self 的双重可变借用）。
        let drained: Vec<MessageValue> = {
            let Some(worker) = self.worker_registry.get_mut(&id) else {
                return Vec::new();
            };
            let mut drained = Vec::new();
            while let Ok(mv) = worker.rx_out.try_recv() {
                drained.push(mv);
            }
            drained
        };
        let mut out = Vec::new();
        for mv in &drained {
            out.push(rehydrate_message(self, mv));
        }
        out
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use oxide_compiler::DefaultCompilerService;
    use oxide_kernel::kernel::KernelConfig;

    /// 建一个带真实编译服务的 Vm（worker 脚本编译需要）。
    fn vm_with_compiler() -> Vm {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut vm = Vm::with_kernel_core(core);
        vm.set_compiler_service(Arc::new(DefaultCompilerService));
        vm
    }

    /// 轮询至 worker 回显一条消息（带截止，防 flaky）。
    fn poll_until_message(vm: &mut Vm, id: u64) -> Vec<JsValue> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let messages = vm.poll_worker_messages(id);
            if !messages.is_empty() || Instant::now() >= deadline {
                return messages;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 端到端：spawn → post → 回显 → terminate，验证 worker 基础设施闭环。
    #[test]
    fn worker_round_trip() {
        let mut vm = vm_with_compiler();

        let id = vm.spawn_worker("1 + 1").expect("worker 应派生成功");
        assert_eq!(vm.active_workers(), vec![id], "应有唯一活跃 worker");

        vm.worker_post_message(id, MessageValue::Number(42.0)).expect("投递应成功");
        let messages = poll_until_message(&mut vm, id);
        assert_eq!(messages.len(), 1, "应回显一条消息");
        // 整数值经 number_to_js 归为 Int 表示。
        assert_eq!(messages[0], JsValue::int(42), "回显值应为 42");

        vm.worker_terminate(id).expect("终止应成功");
        assert!(vm.active_workers().is_empty(), "终止后无活跃 worker");
    }

    /// 编译失败不 panic：worker 上报错误串，主线程可经轮询取回。
    #[test]
    fn worker_script_compile_failure_reports_error() {
        let mut vm = vm_with_compiler();

        let id = vm.spawn_worker("function { 语法错误").expect("worker 应派生成功");
        let messages = poll_until_message(&mut vm, id);
        assert!(!messages.is_empty(), "编译失败应上报错误串");
        assert!(messages[0].is_string(), "错误串应为字符串值");

        vm.worker_terminate(id).expect("终止应成功");
    }

    /// 多 worker 隔离：各 worker 独立 realm，消息不串扰。
    #[test]
    fn multiple_workers_isolated() {
        let mut vm = vm_with_compiler();

        let id_a = vm.spawn_worker("1").expect("worker A 应派生成功");
        let id_b = vm.spawn_worker("2").expect("worker B 应派生成功");
        assert_eq!(vm.active_workers(), vec![id_a, id_b], "应有两个活跃 worker");

        vm.worker_post_message(id_a, MessageValue::Number(1.0)).expect("投递 A 应成功");
        vm.worker_post_message(id_b, MessageValue::Number(2.0)).expect("投递 B 应成功");

        let messages_a = poll_until_message(&mut vm, id_a);
        assert_eq!(messages_a[0], JsValue::int(1), "A 应回显 1");
        let messages_b = poll_until_message(&mut vm, id_b);
        assert_eq!(messages_b[0], JsValue::int(2), "B 应回显 2");

        vm.shutdown_workers();
        assert!(vm.active_workers().is_empty(), "shutdown 后无活跃 worker");
    }
}
