#![allow(clippy::arc_with_non_send_sync)]

use std::sync::{Arc, Condvar, Mutex};

use oxide_vm::mem::P;
use oxide_vm::vm::Vm;

use crate::kernel::OxideKernel;

struct VmPoolInner {
    available: Vec<Vm>,
    total_count: usize,
}

pub struct VmPool {
    kernel: Arc<OxideKernel>,
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
    pub fn new(kernel: Arc<OxideKernel>, min_size: usize, max_size: Option<usize>) -> Arc<Self> {
        let pool = Arc::new(Self {
            kernel,
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
                let vm = Self::new_vm(&pool.kernel);
                inner.available.push(vm);
                inner.total_count += 1;
            }
        }

        pool
    }

    fn new_vm(kernel: &Arc<OxideKernel>) -> Vm {
        let mut vm = Vm::new();
        vm.object_prototype = P::clone(&kernel.builtin_world().object_proto);
        vm
    }

    fn replace_vm(&self) -> Vm {
        Self::new_vm(&self.kernel)
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
                let vm = Self::new_vm(&self.kernel);
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
        let Some(vm) = self.vm.take() else {
            return;
        };

        let mut inner = self.pool.inner.lock().unwrap();

        if self.dirty {
            let new_vm = self.pool.replace_vm();
            inner.available.push(new_vm);
        } else {
            for &idx in &self.interned_strings {
                self.pool.kernel.string_forge().decref(idx);
            }

            let mut returned_vm = vm;
            returned_vm.reset();
            inner.available.push(returned_vm);
        }

        self.pool.condvar.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::KernelConfig;

    fn test_kernel() -> Arc<OxideKernel> {
        Arc::new(OxideKernel::new(KernelConfig::minimal()))
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
