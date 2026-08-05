#![allow(clippy::arc_with_non_send_sync)]

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::vm::Vm;
use crate::{vm_debug, vm_trace, vm_warn};
use oxide_kernel::kernel::KernelCore;

struct VmPoolInner {
    available: Vec<Vm>,
    total_count: usize,
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
    /// 创建空池。`min_size` 当前仅作预热预留（未使用），`max_size` 为池上限，`None` 表示不限。
    pub fn new(kernel_core: Arc<KernelCore>, _min_size: usize, max_size: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            kernel_core,
            inner: Mutex::new(VmPoolInner {
                available: Vec::new(),
                total_count: 0,
            }),
            condvar: Condvar::new(),
            max_size,
        })
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
                let vm = Self::new_vm(&self.kernel_core);
                return VmGuard {
                    vm: Some(vm),
                    pool: Arc::clone(self),
                    dirty: false,
                };
            }
        }
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
}

impl Drop for VmGuard {
    fn drop(&mut self) {
        let Some(mut vm) = self.vm.take() else {
            return;
        };

        let mut inner = self.pool.inner.lock().unwrap();

        if self.dirty {
            vm_debug!("pool: discarding dirty vm");
            let new_vm = self.pool.replace_vm();
            inner.available.push(new_vm);
        } else {
            vm.full_reset();
            inner.available.push(vm);
            vm_trace!("pool: recycled clean vm, {} available", inner.available.len());
        }

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
}
