#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtin::BuiltinWorld;
use crate::code_forge::CodeForge;
use crate::logging::{init_logging, LogLevel, SUBSYSTEM_COUNT};
use crate::prop_forge::PropForge;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::StringForge;

#[derive(Clone)]
pub struct KernelConfig {
    pub min_pool_size: usize,
    pub max_pool_size: Option<usize>,
    pub max_dead_strings: Option<usize>,
    pub max_steps: Option<u64>,
    pub max_call_depth: usize,
    pub log_levels: [LogLevel; SUBSYSTEM_COUNT],
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
            max_steps: None,
            max_call_depth: 1024,
            log_levels: [LogLevel::Off; SUBSYSTEM_COUNT],
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
            max_steps: None,
            max_call_depth: 1024,
            log_levels: [LogLevel::Off; SUBSYSTEM_COUNT],
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
            max_steps: None,
            max_call_depth: 1024,
            log_levels: [LogLevel::Off; SUBSYSTEM_COUNT],
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

/// Immutable, permanently shared state across all VM instances.
/// Never rebuilt after construction — forge tables are append-only.
pub struct KernelCore {
    pub config: KernelConfig,
    pub string_forge: Arc<StringForge>,
    pub shape_forge: Arc<ShapeForge>,
    pub code_forge: Arc<CodeForge>,
    pub prop_forge: Arc<PropForge>,
}

impl KernelCore {
    pub fn new(config: KernelConfig) -> Arc<Self> {
        init_logging(&config.log_levels);
        let string_forge = Arc::new(StringForge::new());
        let shape_forge = Arc::new(ShapeForge::new());
        let code_forge = Arc::new(CodeForge::new());
        let prop_forge = Arc::new(PropForge::new());
        Arc::new(Self {
            config,
            string_forge,
            shape_forge,
            code_forge,
            prop_forge,
        })
    }

    pub fn string_forge(&self) -> &Arc<StringForge> {
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

    pub fn config(&self) -> &KernelConfig {
        &self.config
    }
}

/// Per-session mutable state: builtin prototype objects and the global object.
/// Rebuilt on full_reset() to achieve complete isolation between JS executions.
pub struct KernelSession {
    pub builtin_world: Arc<BuiltinWorld>,
    pub global_object: P<JsObject>,
}

impl KernelSession {
    /// Build a fresh session from a KernelCore. All string/shape intern calls hit
    /// cache on second and subsequent calls — net cost is < 0.5 ms.
    pub fn new(core: &KernelCore) -> Self {
        let builtin_world = Arc::new(BuiltinWorld::new(&core.string_forge, &core.shape_forge));

        let mut global_obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

        let si_nan = core.string_forge.intern("NaN").0;
        let si_undef = core.string_forge.intern("undefined").0;
        let si_infinity = core.string_forge.intern("Infinity").0;

        let nan_shape = core.shape_forge.make_shape(EMPTY_SHAPE_ID, si_nan);
        global_obj.set_shape_id(nan_shape);
        global_obj.ensure_hash_props().push(JsValue::float(f64::NAN));

        let undef_shape = core.shape_forge.make_shape(nan_shape, si_undef);
        global_obj.set_shape_id(undef_shape);
        global_obj.ensure_hash_props().push(JsValue::undefined());

        let inf_shape = core.shape_forge.make_shape(undef_shape, si_infinity);
        global_obj.set_shape_id(inf_shape);
        global_obj.ensure_hash_props().push(JsValue::float(f64::INFINITY));

        Self {
            builtin_world,
            global_object: P::new(global_obj),
        }
    }

    pub fn builtin_world(&self) -> &Arc<BuiltinWorld> {
        &self.builtin_world
    }

    pub fn global_object(&self) -> &P<JsObject> {
        &self.global_object
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_forge::EMPTY_SHAPE_ID;

    #[test]
    fn test_kernel_new() {
        let core = KernelCore::new(KernelConfig::minimal());
        let (i1, _) = core.string_forge().intern("test");
        let (i2, _) = core.string_forge().intern("test");
        assert_eq!(i1, i2);
    }

    #[test]
    fn test_kernel_builtins_accessible() {
        let core = KernelCore::new(KernelConfig::minimal());
        let session = KernelSession::new(&core);
        assert!(!session.builtin_world().object_proto.is_function());
        assert!(session.builtin_world().object_constructor.is_function());
    }

    #[test]
    fn test_kernel_shape_forge() {
        let core = KernelCore::new(KernelConfig::minimal());
        assert!(core.shape_forge().get_shape(EMPTY_SHAPE_ID).is_some());
    }

    #[test]
    fn test_kernel_string_forge() {
        let core = KernelCore::new(KernelConfig::minimal());
        let (i1, _) = core.string_forge().intern("hello");
        let (i2, _) = core.string_forge().intern("hello");
        assert_eq!(i1, i2);
    }

    #[test]
    fn test_kernel_config_presets() {
        assert_eq!(KernelConfig::minimal().max_pool_size, Some(8));
        assert_eq!(KernelConfig::standard().max_pool_size, Some(32));
        assert_eq!(KernelConfig::minimal().max_steps, None);
        assert_eq!(KernelConfig::standard().max_steps, None);
        assert_eq!(KernelConfig::full().max_steps, None);
        assert_eq!(KernelConfig::minimal().log_levels, [LogLevel::Off; SUBSYSTEM_COUNT]);
        assert!(!KernelConfig::minimal().warmup_builtin_ic);
        assert!(KernelConfig::full().warmup_builtin_ic);
        assert_eq!(KernelConfig::full().max_pool_size, None);
    }

    #[test]
    fn test_session_rebuild_shares_forges() {
        let core = KernelCore::new(KernelConfig::minimal());
        let (i1, _) = core.string_forge().intern("hello");
        let _s2 = KernelSession::new(&core);
        let (i2, _) = core.string_forge().intern("hello");
        assert_eq!(i1, i2);
    }
}
