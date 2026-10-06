//! worker 池：固定 N 个常驻 worker 线程，各持自己线程上建的 VM 池，
//! 执行请求经 mpsc 路由到 worker 执行。
//!
//! 关键约定：
//! - `Vm` 不是 Send，池在 worker 线程闭包内创建、永不跨线程移动；
//!   `Arc<KernelCore>` 是 Send/Sync，由全体 worker 共享（字节码缓存跨 worker 命中）。
//! - 标准库 mpsc 的接收端不是 Send，不能跨线程共享给多个 worker，
//!   因此每 worker 一条独立通道，发送端收进 `WorkerRouter` 按轮询投递。
//! - 关闭序列固定：`close` 清空发送端 → worker 的 recv 断开退出 →
//!   主线程逐一 join → 再 drop 内核。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use oxide_kernel::kernel::KernelCore;
use oxide_vm::vm_pool::{PoolCounters, VmPool};

use super::eval;
use super::protocol::ServerResponse;

/// worker 任务：执行请求加一次性回复通道。
#[derive(Debug)]
pub enum WorkerTask {
    /// 执行请求：源码、可选最大指令数、一次性回复通道（worker 执行完送回响应帧）。
    Eval {
        code: String,
        max_steps: Option<u64>,
        reply: mpsc::Sender<ServerResponse>,
    },
}

/// worker 路由器：N 条独立任务通道的发送端加轮询计数。
///
/// 发送端收进互斥锁：连接线程可能存活超过主线程的关闭序列（在途排水超时后
/// 仍有余留连接），`close` 清空发送端后它们的投递立即失败，不挂起、不悬垂。
pub struct WorkerRouter {
    senders: Mutex<Vec<mpsc::Sender<WorkerTask>>>,
    next: AtomicUsize,
}

impl WorkerRouter {
    fn from_senders(senders: Vec<mpsc::Sender<WorkerTask>>) -> Self {
        WorkerRouter {
            senders: Mutex::new(senders),
            next: AtomicUsize::new(0),
        }
    }

    /// 轮询投递任务到 worker。
    ///
    /// # 步骤
    /// 1. 计数加一取模 N 选发送端。
    /// 2. 投递；发送端已清空（关闭序列进行中）或接收端已断开时归还任务。
    ///
    /// # 边界与前提
    /// - 返回 `Err` 时任务未被任何 worker 接收，调用方须以错误帧应答，不得阻塞等待。
    pub fn route(&self, task: WorkerTask) -> Result<(), WorkerTask> {
        let senders = self.senders.lock().unwrap();
        if senders.is_empty() {
            return Err(task);
        }
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % senders.len();
        match senders[idx].send(task) {
            Ok(()) => Ok(()),
            Err(sent) => Err(sent.0),
        }
    }

    /// 关闭序列第一步：清空全部发送端，worker 的 recv 断开退出。
    pub fn close(&self) {
        self.senders.lock().unwrap().clear();
    }
}

/// 派生 N 个常驻 worker 线程：各线程在自己线程上建自有 VM 池（共享聚合计数器）
/// 并进入任务循环。
///
/// # 步骤
/// 1. 建 N 对通道，接收端各移入一个 worker 线程。
/// 2. worker 线程内按内核配置的池规模建池（共享聚合计数器）。
/// 3. 各 worker 进入任务循环：recv → 执行 → 经回复通道送回响应帧。
///
/// # 边界与前提
/// - `worker_count` 下限钳制为 1。
///
/// # 副作用
/// - 派生 N 个常驻线程；发送端全部清空后线程退出。
///
/// # 注意事项
/// - 返回的句柄必须在关闭序列中逐一 join（drop 发送端 → join → drop 内核）。
pub fn spawn_workers(
    kernel: Arc<KernelCore>,
    counters: Arc<PoolCounters>,
    min_pool_size: usize,
    max_pool_size: Option<usize>,
    worker_count: usize,
) -> (WorkerRouter, Vec<JoinHandle<()>>) {
    let n = worker_count.max(1);
    let mut senders = Vec::with_capacity(n);
    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let (tx, rx) = mpsc::channel();
        let kernel = Arc::clone(&kernel);
        let counters = Arc::clone(&counters);
        handles.push(std::thread::spawn(move || {
            // 池在本线程上创建，永不跨线程移动（Vm 不是 Send）。
            let pool = VmPool::with_counters(Arc::clone(&kernel), min_pool_size, max_pool_size, counters);
            worker_loop(rx, kernel, pool);
        }));
        senders.push(tx);
    }
    (WorkerRouter::from_senders(senders), handles)
}

/// worker 任务循环：recv 任务 → 执行 → 经回复通道送回响应帧；发送端断开即退出。
fn worker_loop(receiver: mpsc::Receiver<WorkerTask>, kernel: Arc<KernelCore>, pool: Arc<VmPool>) {
    for task in receiver {
        match task {
            WorkerTask::Eval {
                code,
                max_steps,
                reply,
            } => {
                let response = eval::handle_eval(&code, max_steps, &kernel, &pool);
                let _ = reply.send(response);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::kernel::KernelConfig;

    /// 建一个哑任务（回复通道只挂不读）。
    fn dummy_task() -> WorkerTask {
        let (reply, _rx) = mpsc::channel();
        WorkerTask::Eval {
            code: "1".into(),
            max_steps: None,
            reply,
        }
    }

    /// 抽干接收端，返回收到的任务数。
    fn drain_count(receiver: &mpsc::Receiver<WorkerTask>) -> usize {
        let mut n = 0;
        while receiver.try_recv().is_ok() {
            n += 1;
        }
        n
    }

    /// 轮询分发：N 等于 2，投 4 个任务，两个 worker 各收到 2 个。
    #[test]
    fn round_robin_distribution() {
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let router = WorkerRouter::from_senders(vec![tx1, tx2]);
        for _ in 0..4 {
            router.route(dummy_task()).expect("投递应成功");
        }
        assert_eq!(drain_count(&rx1), 2, "第一个 worker 应收到 2 个任务");
        assert_eq!(drain_count(&rx2), 2, "第二个 worker 应收到 2 个任务");
    }

    /// 关闭序列：清空发送端后 worker 线程退出，join 在有界时间内返回。
    #[test]
    fn workers_exit_when_senders_cleared() {
        let kernel = KernelCore::new(KernelConfig::minimal());
        let counters = PoolCounters::shared();
        let (router, handles) = spawn_workers(kernel, counters, 1, Some(2), 2);
        router.close();
        for handle in handles {
            handle.join().expect("worker 线程应正常退出");
        }
    }

    /// 发送端清空后投递归还任务：调用方不挂起，可立即以错误帧应答。
    #[test]
    fn route_after_close_returns_task() {
        let (tx, _rx) = mpsc::channel();
        let router = WorkerRouter::from_senders(vec![tx]);
        router.close();
        assert!(router.route(dummy_task()).is_err(), "清空后投递应归还任务");
    }

    /// 端到端：真实 worker 执行 eval 并经回复通道送回响应帧。
    #[test]
    fn worker_executes_eval_and_replies() {
        let kernel = KernelCore::new(KernelConfig::minimal());
        let counters = PoolCounters::shared();
        let (router, handles) = spawn_workers(kernel, counters, 1, Some(2), 1);
        let (reply_tx, reply_rx) = mpsc::channel();
        router
            .route(WorkerTask::Eval {
                code: "1 + 1".into(),
                max_steps: None,
                reply: reply_tx,
            })
            .expect("投递应成功");
        let response = reply_rx.recv().expect("回复通道应收到响应");
        assert!(
            matches!(response, ServerResponse::EvalResult { ref value, .. } if value.as_deref() == Some("2")),
            "应得完成值为 2 的响应：{response:?}"
        );
        router.close();
        for handle in handles {
            handle.join().expect("worker 线程应正常退出");
        }
    }
}
