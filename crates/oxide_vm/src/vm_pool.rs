#![allow(clippy::arc_with_non_send_sync)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::vm::Vm;
use crate::{vm_debug, vm_trace, vm_warn};
use oxide_kernel::kernel::KernelCore;

struct VmPoolInner {
    available: Vec<Vm>,
    total_count: usize,
}

/// 池状态计数器：空闲数、在借数、峰值借出数与已创建总数的跨线程可读快照。
///
/// 池本体不是 Send/Sync（Vm 不跨线程），状态接口经独立原子暴露，
/// 与队列变更同步更新；空闲数恒不大于总数（总数只增不减）。
/// 安静态（无并发借出归还）下在借数恒等于总数减空闲数；峰值是每次
/// 借出后精确在借数的最大值，单调不降。
pub struct PoolCounters {
    available: AtomicUsize,
    total: AtomicUsize,
    in_use: AtomicUsize,
    peak: AtomicUsize,
}

impl PoolCounters {
    /// 创建新的聚合计数器（零初始化），供多个池共享。
    ///
    /// 共享后各池的原子增减自然累加到同一聚合，读取方拿到的是全体池的合计。
    pub fn shared() -> Arc<Self> {
        Arc::new(PoolCounters {
            available: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            in_use: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }

    /// 读取空闲队列长度。
    pub fn available(&self) -> usize {
        self.available.load(Ordering::Relaxed)
    }

    /// 读取已创建 VM 总数（含借出中的）。
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// 读取历史峰值借出数（高水位，单调不降）。
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// 记录一次借出：在借数加一，并抬高历史峰值。
    ///
    /// 峰值取借出后精确在借数的最大值，单调不降；归还路径不触碰峰值。
    /// 诊断计数器，`Relaxed` 序，不参与同步语义。
    fn record_borrow(&self) {
        let in_use = self.in_use.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak.fetch_max(in_use, Ordering::Relaxed);
    }

    /// 记录一次归还：在借数减一。
    fn record_return(&self) {
        self.in_use.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 共享 `Vm` 实例池：按需创建并复用 VM，避免每次执行重建 kernel 共享状态。
///
/// 空闲 VM 存放在 `available` 队列；达到 `max_size` 上限时 `spawn` 会阻塞等待
/// 归还（带 5 秒超时强制扩容兜底）。线程安全，可供多线程并发领取。
pub struct VmPool {
    kernel_core: Arc<KernelCore>,
    inner: Mutex<VmPoolInner>,
    condvar: Condvar,
    max_size: Option<usize>,
    counters: Arc<PoolCounters>,
}

/// 从池中借出的 VM 独占句柄（RAII）。
///
/// 持有期间独占访问 [`VmGuard::vm`] / [`VmGuard::vm_mut`]；`Drop` 时归还池：
/// 干净 VM 执行 `full_reset` 后复用，被标记 dirty 的 VM 直接丢弃并新建替补。
pub struct VmGuard {
    vm: Option<Vm>,
    pool: Arc<VmPool>,
    dirty: bool,
}

impl VmPool {
    /// 创建 VM 池并同步预热 `min_size` 个 Vm（数量受 `max_size` 钳制），
    /// 首次 `spawn` 直接命中池。`max_size` 为池上限，`None` 表示不限。
    ///
    /// 计数器为本池私有（不与其他池共享）。
    pub fn new(kernel_core: Arc<KernelCore>, min_size: usize, max_size: Option<usize>) -> Arc<Self> {
        Self::with_counters(kernel_core, min_size, max_size, PoolCounters::shared())
    }

    /// 创建共享外部聚合计数器的 VM 池：多个池的原子增减累加到同一聚合，
    /// 读取方（如 server 的状态响应）拿到的是全体池的合计。
    ///
    /// # 步骤
    /// 1. 以给定计数器建池结构。
    /// 2. 同步预热 `min_size` 个 Vm（受 `max_size` 钳制），计数器以累加方式同步
    ///    （共享聚合上不得覆盖写，各池各自累加）。
    ///
    /// # 边界与前提
    /// - 计数器通常经 `PoolCounters::shared` 跨线程共享；池本体不跨线程移动。
    ///
    /// # 副作用
    /// - 同步创建 `min_size` 个 Vm。
    pub fn with_counters(
        kernel_core: Arc<KernelCore>,
        min_size: usize,
        max_size: Option<usize>,
        counters: Arc<PoolCounters>,
    ) -> Arc<Self> {
        let warm = min_size.min(max_size.unwrap_or(min_size));
        let pool = Arc::new(Self {
            kernel_core: Arc::clone(&kernel_core),
            inner: Mutex::new(VmPoolInner {
                available: Vec::new(),
                total_count: 0,
            }),
            condvar: Condvar::new(),
            max_size,
            counters: Arc::clone(&counters),
        });
        let mut inner = pool.inner.lock().unwrap();
        for _ in 0..warm {
            inner.available.push(Self::new_vm(&kernel_core));
            inner.total_count += 1;
        }
        drop(inner);
        counters.available.fetch_add(warm, Ordering::Relaxed);
        counters.total.fetch_add(warm, Ordering::Relaxed);
        pool
    }

    fn new_vm(core: &Arc<KernelCore>) -> Vm {
        Vm::with_kernel_core(Arc::clone(core))
    }

    fn replace_vm(&self) -> Vm {
        Self::new_vm(&self.kernel_core)
    }

    /// 从池中借出一个 VM：优先复用空闲实例，否则在池未满时新建，池满则阻塞等待归还。
    pub fn spawn(self: &Arc<Self>) -> VmGuard {
        loop {
            let mut inner = self.inner.lock().unwrap();

            if let Some(vm) = inner.available.pop() {
                vm_trace!("pool: reused vm, {} available", inner.available.len());
                self.counters.available.fetch_sub(1, Ordering::Relaxed);
                self.counters.record_borrow();
                return VmGuard {
                    vm: Some(vm),
                    pool: Arc::clone(self),
                    dirty: false,
                };
            }

            let can_grow = match self.max_size {
                Some(max) => inner.total_count < max,
                None => true,
            };

            if can_grow {
                inner.total_count += 1;
                vm_debug!("pool: growing to {} vms", inner.total_count);
                drop(inner);
                self.counters.total.fetch_add(1, Ordering::Relaxed);
                self.counters.record_borrow();
                let vm = Self::new_vm(&self.kernel_core);
                return VmGuard {
                    vm: Some(vm),
                    pool: Arc::clone(self),
                    dirty: false,
                };
            }

            let (guard, wait) = self.condvar.wait_timeout(inner, Duration::from_secs(5)).unwrap();
            inner = guard;
            if wait.timed_out() {
                vm_warn!("pool: wait timeout, force-growing to {} vms", inner.total_count + 1);
                inner.total_count += 1;
                drop(inner);
                self.counters.total.fetch_add(1, Ordering::Relaxed);
                self.counters.record_borrow();
                let vm = Self::new_vm(&self.kernel_core);
                return VmGuard {
                    vm: Some(vm),
                    pool: Arc::clone(self),
                    dirty: false,
                };
            }
        }
    }

    /// 读取空闲队列长度（当前可借出的 VM 数）。
    ///
    /// 读原子快照，不锁池内互斥锁；与队列变更同步更新，
    /// 恒不大于总数。
    pub fn available_count(&self) -> usize {
        self.counters.available()
    }

    /// 读取已创建 VM 总数（含借出中的）。
    ///
    /// 读原子快照，不锁池内互斥锁；总数只增不减。
    pub fn total_count(&self) -> usize {
        self.counters.total()
    }

    /// 读取历史峰值借出数（高水位）。
    ///
    /// 读原子快照，不锁池内互斥锁；峰值单调不降。
    pub fn peak_count(&self) -> usize {
        self.counters.peak()
    }

    /// 跨线程可读的池状态句柄（原子快照，无锁）。
    pub fn counters(&self) -> Arc<PoolCounters> {
        Arc::clone(&self.counters)
    }

    /// 对池内全体空闲 VM 执行一次完整 session GC，返回收集的 VM 数。
    ///
    /// # 边界与前提
    /// - 只触及空闲队列（available）；在借 VM（在途 eval）不在范围（非安全点）。
    /// - 锁池内互斥锁，与 spawn 原子互斥，无竞态。
    ///
    /// # 副作用
    /// - 各空闲 VM 的 session_bytes_allocated 重置为清扫后存活字节。
    pub fn collect_idle_gc(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let n = inner.available.len();
        for vm in inner.available.iter_mut() {
            vm.collect_session_gc();
        }
        n
    }
}

impl VmGuard {
    /// 只读访问被借出的 VM。
    pub fn vm(&self) -> &Vm {
        self.vm.as_ref().expect("VmGuard has no VM")
    }

    /// 可变访问被借出的 VM。
    pub fn vm_mut(&mut self) -> &mut Vm {
        self.vm.as_mut().expect("VmGuard has no VM")
    }

    /// 显式标记被借出的 VM 为 dirty：`Drop` 时丢弃该 VM 并新建替补，
    /// 不对其执行 `full_reset`。供并发与未来显式弃用场景使用。
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

impl Drop for VmGuard {
    fn drop(&mut self) {
        let Some(mut vm) = self.vm.take() else {
            return;
        };

        let mut inner = self.pool.inner.lock().unwrap();

        // panic 展开期（thread::panicking 为真）被 drop 的 VM 一律按 dirty 丢弃：
        // 损坏 VM 上执行 full_reset 可能二次 panic，unwind 中二次 panic 即 abort。
        if self.dirty || std::thread::panicking() {
            vm_debug!("pool: discarding dirty vm");
            let new_vm = self.pool.replace_vm();
            inner.available.push(new_vm);
        } else {
            vm.full_reset();
            inner.available.push(vm);
            vm_trace!("pool: recycled clean vm, {} available", inner.available.len());
        }

        self.pool.counters.available.fetch_add(1, Ordering::Relaxed);
        self.pool.counters.record_return();
        self.pool.condvar.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::kernel::KernelConfig;

    fn test_kernel() -> Arc<KernelCore> {
        KernelCore::new(KernelConfig::minimal())
    }

    #[test]
    fn test_pool_spawn_returns_guard() {
        let kernel = test_kernel();
        let pool = VmPool::new(kernel, 1, None);
        let guard = pool.spawn();
        drop(guard);
    }

    #[test]
    fn test_pool_recycle_on_drop() {
        let kernel = test_kernel();
        let pool = VmPool::new(kernel, 1, None);
        let guard = pool.spawn();
        drop(guard);
        let guard2 = pool.spawn();
        drop(guard2);
    }

    #[test]
    fn test_pool_grows_if_empty() {
        let kernel = test_kernel();
        let pool = VmPool::new(Arc::clone(&kernel), 1, Some(3));
        let g1 = pool.spawn();
        let g2 = pool.spawn();
        drop(g1);
        drop(g2);
    }

    #[test]
    fn test_pool_warms_min_size_on_new() {
        let kernel = test_kernel();
        let pool = VmPool::new(kernel, 2, None);
        {
            let inner = pool.inner.lock().unwrap();
            assert_eq!(inner.available.len(), 2);
            assert_eq!(inner.total_count, 2);
        }
        let g1 = pool.spawn();
        let g2 = pool.spawn();
        {
            let inner = pool.inner.lock().unwrap();
            assert_eq!(inner.total_count, 2);
        }
        drop(g1);
        drop(g2);
    }

    #[test]
    fn test_pool_warmup_clamped_by_max_size() {
        let kernel = test_kernel();
        let pool = VmPool::new(kernel, 2, Some(1));
        let inner = pool.inner.lock().unwrap();
        assert_eq!(inner.available.len(), 1);
        assert_eq!(inner.total_count, 1);
    }

    #[test]
    fn test_with_counters_shared_aggregate() {
        let kernel = test_kernel();
        let counters = PoolCounters::shared();
        let pool_a = VmPool::with_counters(Arc::clone(&kernel), 1, Some(4), Arc::clone(&counters));
        let pool_b = VmPool::with_counters(Arc::clone(&kernel), 2, Some(4), Arc::clone(&counters));

        // 预热后聚合等于两池之和：总数 1 + 2 = 3，全部空闲。
        assert_eq!(counters.total(), 3, "聚合总数应为两池预热之和");
        assert_eq!(counters.available(), 3, "聚合空闲数应为两池预热之和");

        // 各池借出再归还：聚合空闲数与总数等于两池之和。
        let g1 = pool_a.spawn();
        let g2 = pool_b.spawn();
        let g3 = pool_b.spawn();
        assert_eq!(counters.available(), 0, "三笔借出后聚合空闲数应为零");
        drop(g1);
        drop(g2);
        drop(g3);
        assert_eq!(counters.available(), 3, "全部归还后聚合空闲数应复原");
        assert_eq!(counters.total(), 3, "聚合总数应不变");

        // 各池的读取面读同一聚合，两池读数一致。
        assert_eq!(pool_a.available_count(), counters.available());
        assert_eq!(pool_b.available_count(), counters.available());
    }

    #[test]
    fn test_peak_count_single_thread_exact() {
        let kernel = test_kernel();
        let pool = VmPool::new(kernel, 2, None);
        assert_eq!(pool.peak_count(), 0, "从未借出的池峰值应为零");

        let g1 = pool.spawn();
        let g2 = pool.spawn();
        assert_eq!(pool.peak_count(), 2, "连续借出两个后峰值应为 2");
        drop(g1);
        drop(g2);

        let g3 = pool.spawn();
        assert_eq!(pool.peak_count(), 2, "归还后再借一个峰值不回落");
        drop(g3);
    }

    #[test]
    fn test_peak_count_shared_aggregate() {
        let kernel = test_kernel();
        let counters = PoolCounters::shared();
        let pool_a = VmPool::with_counters(Arc::clone(&kernel), 1, Some(4), Arc::clone(&counters));
        let pool_b = VmPool::with_counters(Arc::clone(&kernel), 1, Some(4), Arc::clone(&counters));

        let g1 = pool_a.spawn();
        let g2 = pool_b.spawn();
        assert_eq!(counters.peak(), 2, "两池各借出一个后聚合峰值应为 2");
        drop(g1);
        drop(g2);

        let g3 = pool_a.spawn();
        assert_eq!(counters.peak(), 2, "归还后再借一个聚合峰值不回落");
        drop(g3);
    }

    #[test]
    fn test_peak_count_concurrent_bounded() {
        let kernel = test_kernel();
        let counters = PoolCounters::shared();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let kernel = Arc::clone(&kernel);
                let counters = Arc::clone(&counters);
                std::thread::spawn(move || {
                    // 池本体不跨线程（Vm 不 Send），每线程自建池、共享同一聚合计数器。
                    let pool = VmPool::with_counters(kernel, 0, Some(8), counters);
                    for _ in 0..50 {
                        let guard = pool.spawn();
                        drop(guard);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        // 并发下精确峰值是竞争结果，只断言界：4 线程各至多持 1 个在借。
        let peak = counters.peak();
        assert!((1..=4).contains(&peak), "并发峰值应落在 1 到 4 之间：{peak}");
    }

    #[test]
    fn test_collect_idle_gc_returns_idle_count() {
        let kernel = test_kernel();
        let pool = VmPool::new(kernel, 1, None);
        let guard = pool.spawn();
        drop(guard);
        assert_eq!(pool.collect_idle_gc(), 1, "归还一个空闲 VM 后收集数应为 1");
    }
}
