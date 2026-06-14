#![allow(clippy::arc_with_non_send_sync)]

use std::sync::{Arc, Condvar, Mutex};

use crate::vm::Vm;
use oxide_kernel::kernel::KernelCore;

struct VmPoolInner {
    available: Vec<Vm>,
    total_count: usize,
}

pub struct VmPool {
    kernel_core: Arc<KernelCore>,
    inner: Mutex<VmPoolInner>,
    condvar: Condvar,
    #[allow(dead_code)]
    min_size: usize,
    max_size: Option<usize>,
}

pub struct VmGuard {
    vm: Option<Vm>,
    pool: Arc<VmPool>,
    dirty: bool,
    interned_strings: Vec<u32>,
}

impl VmPool {
    pub fn new(
        kernel_core: Arc<KernelCore>, #[allow(dead_code)] min_size: usize, max_size: Option<usize>,
    ) -> Arc<Self> {
        let pool = Arc::new(Self {
            kernel_core,
            inner: Mutex::new(VmPoolInner {
                available: Vec::with_capacity(min_size),
                total_count: 0,
            }),
            condvar: Condvar::new(),
            min_size,
            max_size,
        });

        {
            let mut inner = pool.inner.lock().unwrap();
            for _ in 0..min_size {
                let vm = Self::new_vm(&pool.kernel_core);
                inner.available.push(vm);
                inner.total_count += 1;
            }
        }

        pool
    }

    fn new_vm(core: &Arc<KernelCore>) -> Vm {
        Vm::with_kernel_core(Arc::clone(core))
    }

    fn replace_vm(&self) -> Vm {
        Self::new_vm(&self.kernel_core)
    }

    pub fn spawn(self: &Arc<Self>) -> VmGuard {
        loop {
            let mut inner = self.inner.lock().unwrap();

            if let Some(vm) = inner.available.pop() {
                return VmGuard {
                    vm: Some(vm),
                    pool: Arc::clone(self),
                    dirty: false,
                    interned_strings: Vec::new(),
                };
            }

            let can_grow = match self.max_size {
                Some(max) => inner.total_count < max,
                None => true,
            };

            if can_grow {
                inner.total_count += 1;
                drop(inner);
                let vm = Self::new_vm(&self.kernel_core);
                return VmGuard {
                    vm: Some(vm),
                    pool: Arc::clone(self),
                    dirty: false,
                    interned_strings: Vec::new(),
                };
            }

            inner = self.condvar.wait(inner).unwrap();
        }
    }
}

impl VmGuard {
    pub fn vm(&self) -> &Vm {
        self.vm.as_ref().expect("VmGuard has no VM")
    }

    pub fn vm_mut(&mut self) -> &mut Vm {
        self.vm.as_mut().expect("VmGuard has no VM")
    }

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

        if self.dirty {
            let new_vm = self.pool.replace_vm();
            inner.available.push(new_vm);
        } else {
            for &idx in &self.interned_strings {
                self.pool.kernel_core.string_forge().decref(idx);
            }

            vm.full_reset();
            inner.available.push(vm);
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
