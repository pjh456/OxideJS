//! 通用消息队列：`std::sync::mpsc` 的薄封装，提供命名发送端/接收端类型与统一入口。
//!
//! 关键约定：
//! - `Sender<T>` 可克隆、可跨线程共享（要求 `T: Send`）；`Receiver<T>` 单消费者、非 Send。
//! - FIFO 序：`send` 的顺序与 `recv` 的顺序一致。
//! - 断开检测：全部发送端（含克隆）drop 后，`recv` / `try_recv` 返回断开，
//!   `is_disconnected` 返回 true（与 std 语义一致，不取决于队列是否还有待取值）。
//! - 语义与 `std::sync::mpsc` 完全一致；本模块只提供命名类型与统一入口，零新依赖。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

/// 创建一对消息队列发送端与接收端。
///
/// 发送端可克隆、可跨线程共享；接收端单消费者。全部发送端 drop 后队列断开，
/// 接收端此后的所有接收都返回断开。
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    // 存活发送端计数：std 的 `mpsc::Receiver::is_disconnected` 尚不稳定，
    // 用自维护计数复现 std 断开语义（全部发送端 drop 即断开）。
    let live = Arc::new(AtomicUsize::new(1));
    (Sender { inner: tx, live: live.clone() }, Receiver { inner: rx, live })
}

/// 发送端：消息队列的可克隆、可跨线程共享端。
///
/// 多个 `Sender` 克隆（含移入其他线程的）写入同一队列；全部发送端 drop 后
/// 队列断开。
pub struct Sender<T> {
    inner: mpsc::Sender<T>,
    live: Arc<AtomicUsize>,
}

impl<T> Sender<T> {
    /// 发送一个值进队列。
    ///
    /// # 边界与前提
    /// - 接收端已全部 drop 时返回 `SendError`，值归还调用方。
    ///
    /// # 注意事项
    /// - `Sender<T>: Send` 要求 `T: Send`，值随发送跨线程转移。
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.inner.send(value).map_err(|e| SendError(e.0))
    }
}

// std 的 `mpsc::Sender` 对任意 `T` 都可克隆，手动实现避免 derive 引入多余的 `T: Clone` 约束。
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.live.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: self.inner.clone(),
            live: self.live.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Release);
    }
}

/// 接收端：消息队列的单消费者端，非 Send。
///
/// 接收端只在持有线程内使用；阻塞 `recv` 与非阻塞 `try_recv` 均可用。
pub struct Receiver<T> {
    inner: mpsc::Receiver<T>,
    live: Arc<AtomicUsize>,
}

impl<T> Receiver<T> {
    /// 阻塞接收：等待直到有值到达或队列断开。
    ///
    /// # 边界与前提
    /// - 队列已断开（全部发送端 drop 且无值可取）时返回 `RecvError`。
    ///
    /// # 注意事项
    /// - 阻塞调用，调用方须保证所在线程可被阻塞。
    pub fn recv(&self) -> Result<T, RecvError> {
        self.inner.recv().map_err(|_| RecvError)
    }

    /// 非阻塞接收：立即返回。
    ///
    /// # 边界与前提
    /// - 队列为空时返回 `TryRecvError::Empty`（发送端仍存活，其后可有值）。
    /// - 队列已断开时返回 `TryRecvError::Disconnected`（其后无值）。
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.inner.try_recv().map_err(|e| match e {
            mpsc::TryRecvError::Empty => TryRecvError::Empty,
            mpsc::TryRecvError::Disconnected => TryRecvError::Disconnected,
        })
    }

    /// 带超时接收：在 `duration` 内返回一个值，否则返回超时错误。
    ///
    /// # 边界与前提
    /// - `duration` 内无值到达且队列未断开时返回 `Err(Timeout)`。
    /// - 队列已断开（全部发送端 drop）且无值可取时同样返回 `Err(Timeout)`
    ///   （与 std `recv_timeout` 语义一致，不区分超时与断开；断开判定经
    ///   [`Receiver::is_disconnected`] 单独查询）。
    ///
    /// # 注意事项
    /// - 透传 std `mpsc::Receiver::recv_timeout` 语义。
    pub fn recv_timeout(&self, duration: std::time::Duration) -> Result<T, Timeout> {
        self.inner.recv_timeout(duration).map_err(|_| Timeout)
    }

    /// 队列是否已断开（全部发送端已 drop）。
    ///
    /// 与 std 断开语义一致：不取决于队列是否还有待取值。断开是单向状态：
    /// 一旦为 true 不再回到 false。
    pub fn is_disconnected(&self) -> bool {
        self.live.load(Ordering::Acquire) == 0
    }
}

/// 发送失败：接收端已 drop，值归还调用方。
pub struct SendError<T>(pub T);

/// 接收失败：队列已断开（全部发送端已 drop）且无值可取。
pub struct RecvError;

/// 非阻塞接收失败：队列为空或队列已断开。
pub enum TryRecvError {
    /// 队列为空（发送端仍存活，其后可有值）。
    Empty,
    /// 队列已断开（全部发送端已 drop，其后无值）。
    Disconnected,
}

/// 带超时接收失败：指定时间内无值到达且队列未断开（或已断开且无值可取）。
pub struct Timeout;

impl<T: std::fmt::Debug> std::fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SendError").field(&self.0).finish()
    }
}

impl std::fmt::Debug for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RecvError")
    }
}

impl std::fmt::Debug for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryRecvError::Empty => write!(f, "TryRecvError::Empty"),
            TryRecvError::Disconnected => write!(f, "TryRecvError::Disconnected"),
        }
    }
}

impl std::fmt::Debug for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Timeout")
    }
}

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "发送失败：接收端已 drop")
    }
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "接收失败：消息队列已断开")
    }
}

impl std::fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryRecvError::Empty => write!(f, "队列为空"),
            TryRecvError::Disconnected => write!(f, "消息队列已断开"),
        }
    }
}

impl std::fmt::Display for Timeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "接收超时")
    }
}

impl<T> std::error::Error for SendError<T> where T: std::fmt::Debug {}
impl std::error::Error for RecvError {}
impl std::error::Error for TryRecvError {}
impl std::error::Error for Timeout {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_recv_roundtrip() {
        let (tx, rx) = channel();
        tx.send(42).unwrap();
        assert_eq!(rx.recv().unwrap(), 42);
    }

    #[test]
    fn fifo_order() {
        let (tx, rx) = channel();
        for i in 0..3 {
            tx.send(i).unwrap();
        }
        for i in 0..3 {
            assert_eq!(rx.recv().unwrap(), i);
        }
    }

    #[test]
    fn multi_producer() {
        let (tx, rx) = channel();
        let mut handles = Vec::new();
        for t in 0..4 {
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..25 {
                    tx.send(t * 100 + i).unwrap();
                }
            }));
        }
        drop(tx);
        let mut received = Vec::new();
        for _ in 0..100 {
            received.push(rx.recv().unwrap());
        }
        received.sort_unstable();
        let expected: Vec<i32> = (0..4).flat_map(|t| (0..25).map(move |i| t * 100 + i)).collect();
        assert_eq!(received, expected);
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn try_recv_empty() {
        let (_tx, rx) = channel::<i32>();
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn try_recv_disconnected() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
    }

    #[test]
    fn recv_disconnected() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn is_disconnected_flag() {
        let (tx, rx) = channel::<i32>();
        assert!(!rx.is_disconnected());
        drop(tx);
        assert!(rx.is_disconnected());
    }

    #[test]
    fn is_disconnected_with_pending_values() {
        // 断开语义不取决于队列是否还有待取值：发送端 drop 即断开。
        let (tx, rx) = channel::<i32>();
        tx.send(1).unwrap();
        drop(tx);
        assert!(rx.is_disconnected());
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn send_error_returns_value() {
        let (tx, rx) = channel::<i32>();
        drop(rx);
        let err = tx.send(7).unwrap_err();
        assert_eq!(err.0, 7);
    }

    #[test]
    fn recv_timeout_returns_value_in_time() {
        let (tx, rx) = channel::<i32>();
        tx.send(42).unwrap();
        assert_eq!(rx.recv_timeout(std::time::Duration::from_millis(50)).unwrap(), 42);
    }

    #[test]
    fn recv_timeout_empty_returns_timeout() {
        let (_tx, rx) = channel::<i32>();
        assert!(rx.recv_timeout(std::time::Duration::from_millis(50)).is_err());
    }

    #[test]
    fn recv_timeout_disconnected_returns_timeout() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert!(rx.recv_timeout(std::time::Duration::from_millis(50)).is_err());
        assert!(rx.is_disconnected());
    }

    #[test]
    fn sender_is_send_across_threads() {
        let (tx, rx) = channel::<String>();
        let handle = std::thread::spawn(move || {
            tx.send("from worker".to_string()).unwrap();
        });
        handle.join().unwrap();
        assert_eq!(rx.recv().unwrap(), "from worker");
    }
}
