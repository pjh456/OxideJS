#![allow(clippy::arc_with_non_send_sync)]

use std::sync::{Arc, Mutex};

use crate::builtin::BuiltinWorld;
use crate::code_forge::CodeForge;
use crate::prop_forge::PropForge;
use crate::shape_forge::ShapeForge;
use crate::string_forge::Interner;
use crate::vm_pool::{VmGuard, VmPool};

pub struct KernelConfig {
    pub min_pool_size: usize,
    pub max_pool_size: Option<usize>,
    pub max_dead_strings: Option<usize>,
    pub warmup_builtin_shapes: bool,
    pub warmup_builtin_code: bool,
    pub warmup_builtin_ic: bool,
}

impl KernelConfig {
    pub fn minimal() -> Self {
        Self {
            min_pool_size: 4,
            max_pool_size: Some(8),
            max_dead_strings: Some(10_000),
            warmup_builtin_shapes: true,
            warmup_builtin_code: false,
            warmup_builtin_ic: false,
        }
    }

    pub fn standard() -> Self {
        Self {
            min_pool_size: 8,
            max_pool_size: Some(32),
            max_dead_strings: Some(10_000),
            warmup_builtin_shapes: true,
            warmup_builtin_code: true,
            warmup_builtin_ic: false,
        }
    }

    pub fn full() -> Self {
        Self {
            min_pool_size: 16,
            max_pool_size: None,
            max_dead_strings: Some(5_000),
            warmup_builtin_shapes: true,
            warmup_builtin_code: true,
            warmup_builtin_ic: true,
        }
    }
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self::minimal()
    }
}

pub struct OxideKernel {
    pub config: KernelConfig,
    pub string_forge: Arc<Interner>,
    pub shape_forge: Arc<ShapeForge>,
    pub code_forge: Arc<CodeForge>,
    pub prop_forge: Arc<PropForge>,
    pub builtin_world: Arc<BuiltinWorld>,
    #[allow(dead_code)]
    vm_pool: Mutex<Option<Arc<VmPool>>>,
}

impl OxideKernel {
    pub fn new(config: KernelConfig) -> Self {
        let string_forge = Arc::new(Interner::new());
        let shape_forge = Arc::new(ShapeForge::new());
        let builtin_world = Arc::new(BuiltinWorld::new(&string_forge, &shape_forge));
        let code_forge = Arc::new(CodeForge::new());
        let prop_forge = Arc::new(PropForge::new());

        Self {
            config,
            string_forge,
            shape_forge,
            code_forge,
            prop_forge,
            builtin_world,
            vm_pool: Mutex::new(None),
        }
    }

    pub fn init_vm_pool(self: &Arc<Self>) {
        let pool = VmPool::new(
            Arc::clone(self),
            self.config.min_pool_size,
            self.config.max_pool_size,
        );
        *self.vm_pool.lock().unwrap() = Some(pool);
    }

    pub fn spawn(&self) -> VmGuard {
        self.vm_pool
            .lock()
            .unwrap()
            .as_ref()
            .expect("vm_pool not initialized — call init_vm_pool() first")
            .spawn()
    }

    pub fn string_forge(&self) -> &Arc<Interner> {
        &self.string_forge
    }

    pub fn shape_forge(&self) -> &Arc<ShapeForge> {
        &self.shape_forge
    }

    pub fn code_forge(&self) -> &Arc<CodeForge> {
        &self.code_forge
    }

    pub fn prop_forge(&self) -> &Arc<PropForge> {
        &self.prop_forge
    }

    pub fn builtin_world(&self) -> &Arc<BuiltinWorld> {
        &self.builtin_world
    }

    pub fn config(&self) -> &KernelConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_forge::EMPTY_SHAPE_ID;

    #[test]
    fn test_kernel_new() {
        let kernel = OxideKernel::new(KernelConfig::minimal());
        let (idx, _) = kernel.string_forge().intern("test");
        assert!(idx > 0);
    }

    #[test]
    fn test_kernel_builtins_accessible() {
        let kernel = OxideKernel::new(KernelConfig::minimal());
        assert!(!kernel.builtin_world().object_proto.is_function());
        assert!(kernel.builtin_world().object_constructor.is_function());
    }

    #[test]
    fn test_kernel_shape_forge() {
        let kernel = OxideKernel::new(KernelConfig::minimal());
        assert!(kernel.shape_forge().get_shape(EMPTY_SHAPE_ID).is_some());
    }

    #[test]
    fn test_kernel_string_forge() {
        let kernel = OxideKernel::new(KernelConfig::minimal());
        let (i1, _) = kernel.string_forge().intern("hello");
        let (i2, _) = kernel.string_forge().intern("hello");
        assert_eq!(i1, i2);
    }

    #[test]
    fn test_kernel_config_presets() {
        assert_eq!(KernelConfig::minimal().max_pool_size, Some(8));
        assert_eq!(KernelConfig::standard().max_pool_size, Some(32));
        assert!(!KernelConfig::minimal().warmup_builtin_ic);
        assert!(KernelConfig::full().warmup_builtin_ic);
        assert_eq!(KernelConfig::full().max_pool_size, None);
    }

    #[test]
    fn test_kernel_spawn() {
        let kernel = Arc::new(OxideKernel::new(KernelConfig::minimal()));
        kernel.init_vm_pool();
        let guard = kernel.spawn();
        drop(guard);
        let guard2 = kernel.spawn();
        drop(guard2);
    }
}
