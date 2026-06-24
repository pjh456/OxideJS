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
    pub session_gc_threshold: usize,
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
            session_gc_threshold: 8_388_608,
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
            session_gc_threshold: 8_388_608,
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
            session_gc_threshold: 8_388_608,
            log_levels: [LogLevel::Off; SUBSYSTEM_COUNT],
            warmup_builtin_shapes: true,
            warmup_builtin_code: true,
            warmup_builtin_ic: true,
        }
    }

    pub fn session_gc_threshold(&self) -> usize {
        self.session_gc_threshold
    }

    pub fn set_session_gc_threshold(&mut self, bytes: usize) {
        self.session_gc_threshold = bytes;
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

    pub fn session_gc_threshold(&self) -> usize {
        self.config.session_gc_threshold
    }

    pub fn set_session_gc_threshold(&mut self, bytes: usize) {
        self.config.session_gc_threshold = bytes;
    }

    pub fn sweep_runner_forges(&self) {
        self.string_forge.maybe_sweep(self.config.max_dead_strings.or(Some(1)));
        // test262 creates a fresh VM/session per test. At this boundary no JS object
        // from prior tests may retain transient shapes/templates.
        if self.shape_forge.len() > 50_000 {
            self.shape_forge.clear_transient();
            self.prop_forge.clear();
        } else if self.prop_forge.len() > 50_000 {
            self.prop_forge.clear();
        }
    }
}

/// Per-session mutable state: builtin prototype objects and the global object.
/// Rebuilt on full_reset() to achieve complete isolation between JS executions.
pub struct KernelSession {
    pub builtin_world: Arc<BuiltinWorld>,
    pub global_object: P<JsObject>,
    pub builtin_snapshot: BuiltinSnapshot,
}

/// Generation snapshot for reset dirty checks.
///
/// Maintenance: every new `BuiltinWorld` object field must be added here and to
/// `KernelSession::dirty_since_snapshot()` so selective reset can rebuild the
/// correct builtin family.
#[derive(Clone, Debug)]
pub struct BuiltinSnapshot {
    pub object_proto_generation: u32,
    pub array_proto_generation: u32,
    pub function_proto_generation: u32,
    pub string_proto_generation: u32,
    pub number_proto_generation: u32,
    pub boolean_proto_generation: u32,
    pub error_proto_generation: u32,
    pub symbol_proto_generation: u32,
    pub object_constructor_generation: u32,
    pub array_constructor_generation: u32,
    pub function_constructor_generation: u32,
    pub string_constructor_generation: u32,
    pub number_constructor_generation: u32,
    pub boolean_constructor_generation: u32,
    pub error_constructor_generation: u32,
    pub symbol_constructor_generation: u32,
    pub type_error_proto_generation: u32,
    pub reference_error_proto_generation: u32,
    pub range_error_proto_generation: u32,
    pub syntax_error_proto_generation: u32,
    pub uri_error_proto_generation: u32,
    pub eval_error_proto_generation: u32,
    pub math_object_generation: u32,
    pub json_object_generation: u32,
    pub date_constructor_generation: u32,
    pub date_proto_generation: u32,
    pub set_constructor_generation: u32,
    pub set_proto_generation: u32,
    pub map_constructor_generation: u32,
    pub map_proto_generation: u32,
    pub regexp_constructor_generation: u32,
    pub regexp_proto_generation: u32,
    pub array_buffer_constructor_generation: u32,
    pub array_buffer_proto_generation: u32,
    pub data_view_constructor_generation: u32,
    pub data_view_proto_generation: u32,
    pub typed_array_proto_generation: u32,
    pub int8array_constructor_generation: u32,
    pub int8array_proto_generation: u32,
    pub uint8array_constructor_generation: u32,
    pub uint8array_proto_generation: u32,
    pub uint8clampedarray_constructor_generation: u32,
    pub uint8clampedarray_proto_generation: u32,
    pub int16array_constructor_generation: u32,
    pub int16array_proto_generation: u32,
    pub uint16array_constructor_generation: u32,
    pub uint16array_proto_generation: u32,
    pub int32array_constructor_generation: u32,
    pub int32array_proto_generation: u32,
    pub uint32array_constructor_generation: u32,
    pub uint32array_proto_generation: u32,
    pub float32array_constructor_generation: u32,
    pub float32array_proto_generation: u32,
    pub float64array_constructor_generation: u32,
    pub float64array_proto_generation: u32,
    pub bigint64array_constructor_generation: u32,
    pub bigint64array_proto_generation: u32,
    pub biguint64array_constructor_generation: u32,
    pub biguint64array_proto_generation: u32,
    pub sym_match_generation: u32,
    pub sym_replace_generation: u32,
    pub sym_search_generation: u32,
    pub sym_split_generation: u32,
    pub sym_iterator_generation: u32,
    pub global_object_generation: u32,
    pub stub_objects_len: usize,
    pub stub_object_generations: Vec<u32>,
}

impl BuiltinSnapshot {
    fn gen(obj: &P<JsObject>) -> u32 {
        obj.generation()
    }

    pub fn new(world: &BuiltinWorld, global_object: &P<JsObject>) -> Self {
        Self {
            object_proto_generation: Self::gen(&world.object_proto),
            array_proto_generation: Self::gen(&world.array_proto),
            function_proto_generation: Self::gen(&world.function_proto),
            string_proto_generation: Self::gen(&world.string_proto),
            number_proto_generation: Self::gen(&world.number_proto),
            boolean_proto_generation: Self::gen(&world.boolean_proto),
            error_proto_generation: Self::gen(&world.error_proto),
            symbol_proto_generation: Self::gen(&world.symbol_proto),
            object_constructor_generation: Self::gen(&world.object_constructor),
            array_constructor_generation: Self::gen(&world.array_constructor),
            function_constructor_generation: Self::gen(&world.function_constructor),
            string_constructor_generation: Self::gen(&world.string_constructor),
            number_constructor_generation: Self::gen(&world.number_constructor),
            boolean_constructor_generation: Self::gen(&world.boolean_constructor),
            error_constructor_generation: Self::gen(&world.error_constructor),
            symbol_constructor_generation: Self::gen(&world.symbol_constructor),
            type_error_proto_generation: Self::gen(&world.type_error_proto),
            reference_error_proto_generation: Self::gen(&world.reference_error_proto),
            range_error_proto_generation: Self::gen(&world.range_error_proto),
            syntax_error_proto_generation: Self::gen(&world.syntax_error_proto),
            uri_error_proto_generation: Self::gen(&world.uri_error_proto),
            eval_error_proto_generation: Self::gen(&world.eval_error_proto),
            math_object_generation: Self::gen(&world.math_object),
            json_object_generation: Self::gen(&world.json_object),
            date_constructor_generation: Self::gen(&world.date_constructor),
            date_proto_generation: Self::gen(&world.date_proto),
            set_constructor_generation: Self::gen(&world.set_constructor),
            set_proto_generation: Self::gen(&world.set_proto),
            map_constructor_generation: Self::gen(&world.map_constructor),
            map_proto_generation: Self::gen(&world.map_proto),
            regexp_constructor_generation: Self::gen(&world.regexp_constructor),
            regexp_proto_generation: Self::gen(&world.regexp_proto),
            array_buffer_constructor_generation: Self::gen(&world.array_buffer_constructor),
            array_buffer_proto_generation: Self::gen(&world.array_buffer_proto),
            data_view_constructor_generation: Self::gen(&world.data_view_constructor),
            data_view_proto_generation: Self::gen(&world.data_view_proto),
            typed_array_proto_generation: Self::gen(&world.typed_array_proto),
            int8array_constructor_generation: Self::gen(&world.int8array_constructor),
            int8array_proto_generation: Self::gen(&world.int8array_proto),
            uint8array_constructor_generation: Self::gen(&world.uint8array_constructor),
            uint8array_proto_generation: Self::gen(&world.uint8array_proto),
            uint8clampedarray_constructor_generation: Self::gen(&world.uint8clampedarray_constructor),
            uint8clampedarray_proto_generation: Self::gen(&world.uint8clampedarray_proto),
            int16array_constructor_generation: Self::gen(&world.int16array_constructor),
            int16array_proto_generation: Self::gen(&world.int16array_proto),
            uint16array_constructor_generation: Self::gen(&world.uint16array_constructor),
            uint16array_proto_generation: Self::gen(&world.uint16array_proto),
            int32array_constructor_generation: Self::gen(&world.int32array_constructor),
            int32array_proto_generation: Self::gen(&world.int32array_proto),
            uint32array_constructor_generation: Self::gen(&world.uint32array_constructor),
            uint32array_proto_generation: Self::gen(&world.uint32array_proto),
            float32array_constructor_generation: Self::gen(&world.float32array_constructor),
            float32array_proto_generation: Self::gen(&world.float32array_proto),
            float64array_constructor_generation: Self::gen(&world.float64array_constructor),
            float64array_proto_generation: Self::gen(&world.float64array_proto),
            bigint64array_constructor_generation: Self::gen(&world.bigint64array_constructor),
            bigint64array_proto_generation: Self::gen(&world.bigint64array_proto),
            biguint64array_constructor_generation: Self::gen(&world.biguint64array_constructor),
            biguint64array_proto_generation: Self::gen(&world.biguint64array_proto),
            sym_match_generation: Self::gen(&world.sym_match),
            sym_replace_generation: Self::gen(&world.sym_replace),
            sym_search_generation: Self::gen(&world.sym_search),
            sym_split_generation: Self::gen(&world.sym_split),
            sym_iterator_generation: Self::gen(&world.sym_iterator),
            global_object_generation: Self::gen(global_object),
            stub_objects_len: world.stub_objects.len(),
            stub_object_generations: world.stub_objects.iter().map(Self::gen).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuiltinDirtySet {
    pub object: bool,
    pub array: bool,
    pub function: bool,
    pub string: bool,
    pub number: bool,
    pub boolean: bool,
    pub error_family: bool,
    pub symbol_family: bool,
    pub math: bool,
    pub json: bool,
    pub date: bool,
    pub set: bool,
    pub map: bool,
    pub regexp: bool,
    pub array_buffer: bool,
    pub data_view: bool,
    pub typed_array_family: bool,
    pub stubs: bool,
    pub global: bool,
}

impl BuiltinDirtySet {
    pub fn any_builtin_dirty(&self) -> bool {
        self.object
            || self.array
            || self.function
            || self.string
            || self.number
            || self.boolean
            || self.error_family
            || self.symbol_family
            || self.math
            || self.json
            || self.date
            || self.set
            || self.map
            || self.regexp
            || self.array_buffer
            || self.data_view
            || self.typed_array_family
            || self.stubs
    }

    pub fn any(&self) -> bool {
        self.any_builtin_dirty() || self.global
    }
}

impl KernelSession {
    fn new_global_object(core: &KernelCore) -> P<JsObject> {
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

        P::new(global_obj)
    }

    /// Build a fresh session from a KernelCore. All string/shape intern calls hit
    /// cache on second and subsequent calls — net cost is < 0.5 ms.
    pub fn new(core: &KernelCore) -> Self {
        let builtin_world = Arc::new(BuiltinWorld::new(&core.string_forge, &core.shape_forge));
        let global_object = Self::new_global_object(core);
        let builtin_snapshot = BuiltinSnapshot::new(&builtin_world, &global_object);

        Self {
            builtin_world,
            global_object,
            builtin_snapshot,
        }
    }

    pub fn builtin_world(&self) -> &Arc<BuiltinWorld> {
        &self.builtin_world
    }

    pub fn global_object(&self) -> &P<JsObject> {
        &self.global_object
    }

    pub fn record_snapshot(&mut self) {
        self.builtin_snapshot = BuiltinSnapshot::new(&self.builtin_world, &self.global_object);
    }

    pub fn dirty_since_snapshot(&self) -> BuiltinDirtySet {
        let world = self.builtin_world.as_ref();
        let snapshot = &self.builtin_snapshot;
        let stub_generations_dirty = world
            .stub_objects
            .iter()
            .zip(snapshot.stub_object_generations.iter())
            .any(|(obj, generation)| BuiltinSnapshot::gen(obj) != *generation);

        BuiltinDirtySet {
            object: BuiltinSnapshot::gen(&world.object_proto) != snapshot.object_proto_generation
                || BuiltinSnapshot::gen(&world.object_constructor) != snapshot.object_constructor_generation,
            array: BuiltinSnapshot::gen(&world.array_proto) != snapshot.array_proto_generation
                || BuiltinSnapshot::gen(&world.array_constructor) != snapshot.array_constructor_generation,
            function: BuiltinSnapshot::gen(&world.function_proto) != snapshot.function_proto_generation
                || BuiltinSnapshot::gen(&world.function_constructor) != snapshot.function_constructor_generation,
            string: BuiltinSnapshot::gen(&world.string_proto) != snapshot.string_proto_generation
                || BuiltinSnapshot::gen(&world.string_constructor) != snapshot.string_constructor_generation,
            number: BuiltinSnapshot::gen(&world.number_proto) != snapshot.number_proto_generation
                || BuiltinSnapshot::gen(&world.number_constructor) != snapshot.number_constructor_generation,
            boolean: BuiltinSnapshot::gen(&world.boolean_proto) != snapshot.boolean_proto_generation
                || BuiltinSnapshot::gen(&world.boolean_constructor) != snapshot.boolean_constructor_generation,
            error_family: BuiltinSnapshot::gen(&world.error_proto) != snapshot.error_proto_generation
                || BuiltinSnapshot::gen(&world.error_constructor) != snapshot.error_constructor_generation
                || BuiltinSnapshot::gen(&world.type_error_proto) != snapshot.type_error_proto_generation
                || BuiltinSnapshot::gen(&world.reference_error_proto) != snapshot.reference_error_proto_generation
                || BuiltinSnapshot::gen(&world.range_error_proto) != snapshot.range_error_proto_generation
                || BuiltinSnapshot::gen(&world.syntax_error_proto) != snapshot.syntax_error_proto_generation
                || BuiltinSnapshot::gen(&world.uri_error_proto) != snapshot.uri_error_proto_generation
                || BuiltinSnapshot::gen(&world.eval_error_proto) != snapshot.eval_error_proto_generation,
            symbol_family: BuiltinSnapshot::gen(&world.symbol_proto) != snapshot.symbol_proto_generation
                || BuiltinSnapshot::gen(&world.symbol_constructor) != snapshot.symbol_constructor_generation
                || BuiltinSnapshot::gen(&world.sym_match) != snapshot.sym_match_generation
                || BuiltinSnapshot::gen(&world.sym_replace) != snapshot.sym_replace_generation
                || BuiltinSnapshot::gen(&world.sym_search) != snapshot.sym_search_generation
                || BuiltinSnapshot::gen(&world.sym_split) != snapshot.sym_split_generation
                || BuiltinSnapshot::gen(&world.sym_iterator) != snapshot.sym_iterator_generation,
            math: BuiltinSnapshot::gen(&world.math_object) != snapshot.math_object_generation,
            json: BuiltinSnapshot::gen(&world.json_object) != snapshot.json_object_generation,
            date: BuiltinSnapshot::gen(&world.date_constructor) != snapshot.date_constructor_generation
                || BuiltinSnapshot::gen(&world.date_proto) != snapshot.date_proto_generation,
            set: BuiltinSnapshot::gen(&world.set_constructor) != snapshot.set_constructor_generation
                || BuiltinSnapshot::gen(&world.set_proto) != snapshot.set_proto_generation,
            map: BuiltinSnapshot::gen(&world.map_constructor) != snapshot.map_constructor_generation
                || BuiltinSnapshot::gen(&world.map_proto) != snapshot.map_proto_generation,
            regexp: BuiltinSnapshot::gen(&world.regexp_constructor) != snapshot.regexp_constructor_generation
                || BuiltinSnapshot::gen(&world.regexp_proto) != snapshot.regexp_proto_generation,
            array_buffer: BuiltinSnapshot::gen(&world.array_buffer_constructor)
                != snapshot.array_buffer_constructor_generation
                || BuiltinSnapshot::gen(&world.array_buffer_proto) != snapshot.array_buffer_proto_generation,
            data_view: BuiltinSnapshot::gen(&world.data_view_constructor) != snapshot.data_view_constructor_generation
                || BuiltinSnapshot::gen(&world.data_view_proto) != snapshot.data_view_proto_generation,
            typed_array_family: BuiltinSnapshot::gen(&world.typed_array_proto) != snapshot.typed_array_proto_generation
                || BuiltinSnapshot::gen(&world.int8array_constructor) != snapshot.int8array_constructor_generation
                || BuiltinSnapshot::gen(&world.int8array_proto) != snapshot.int8array_proto_generation
                || BuiltinSnapshot::gen(&world.uint8array_constructor) != snapshot.uint8array_constructor_generation
                || BuiltinSnapshot::gen(&world.uint8array_proto) != snapshot.uint8array_proto_generation
                || BuiltinSnapshot::gen(&world.uint8clampedarray_constructor)
                    != snapshot.uint8clampedarray_constructor_generation
                || BuiltinSnapshot::gen(&world.uint8clampedarray_proto) != snapshot.uint8clampedarray_proto_generation
                || BuiltinSnapshot::gen(&world.int16array_constructor) != snapshot.int16array_constructor_generation
                || BuiltinSnapshot::gen(&world.int16array_proto) != snapshot.int16array_proto_generation
                || BuiltinSnapshot::gen(&world.uint16array_constructor) != snapshot.uint16array_constructor_generation
                || BuiltinSnapshot::gen(&world.uint16array_proto) != snapshot.uint16array_proto_generation
                || BuiltinSnapshot::gen(&world.int32array_constructor) != snapshot.int32array_constructor_generation
                || BuiltinSnapshot::gen(&world.int32array_proto) != snapshot.int32array_proto_generation
                || BuiltinSnapshot::gen(&world.uint32array_constructor) != snapshot.uint32array_constructor_generation
                || BuiltinSnapshot::gen(&world.uint32array_proto) != snapshot.uint32array_proto_generation
                || BuiltinSnapshot::gen(&world.float32array_constructor)
                    != snapshot.float32array_constructor_generation
                || BuiltinSnapshot::gen(&world.float32array_proto) != snapshot.float32array_proto_generation
                || BuiltinSnapshot::gen(&world.float64array_constructor)
                    != snapshot.float64array_constructor_generation
                || BuiltinSnapshot::gen(&world.float64array_proto) != snapshot.float64array_proto_generation
                || BuiltinSnapshot::gen(&world.bigint64array_constructor)
                    != snapshot.bigint64array_constructor_generation
                || BuiltinSnapshot::gen(&world.bigint64array_proto) != snapshot.bigint64array_proto_generation
                || BuiltinSnapshot::gen(&world.biguint64array_constructor)
                    != snapshot.biguint64array_constructor_generation
                || BuiltinSnapshot::gen(&world.biguint64array_proto) != snapshot.biguint64array_proto_generation,
            stubs: world.stub_objects.len() != snapshot.stub_objects_len || stub_generations_dirty,
            global: BuiltinSnapshot::gen(&self.global_object) != snapshot.global_object_generation,
        }
    }

    pub fn is_dirty_since_snapshot(&self) -> bool {
        self.dirty_since_snapshot().any()
    }

    pub fn selective_reset(&mut self, core: &Arc<KernelCore>) -> BuiltinDirtySet {
        let dirty = self.dirty_since_snapshot();
        if dirty.global {
            self.global_object = Self::new_global_object(core);
        }
        if dirty.any_builtin_dirty() {
            self.builtin_world = Arc::new(BuiltinWorld::rebuild_with_dirty(
                &self.builtin_world,
                core.string_forge.as_ref(),
                core.shape_forge.as_ref(),
                &dirty,
            ));
        }
        dirty
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

    #[test]
    fn snapshot_fresh_session_is_clean() {
        let core = KernelCore::new(KernelConfig::minimal());
        let session = KernelSession::new(&core);
        let dirty = session.dirty_since_snapshot();

        assert!(!dirty.any());
        assert!(!dirty.any_builtin_dirty());
        assert!(!session.is_dirty_since_snapshot());
    }

    #[test]
    fn snapshot_detects_array_dirty() {
        let core = KernelCore::new(KernelConfig::minimal());
        let session = KernelSession::new(&core);

        unsafe { &mut *(session.builtin_world.array_proto.as_ptr() as *mut JsObject) }.bump_generation();
        let dirty = session.dirty_since_snapshot();

        assert!(dirty.array);
        assert!(dirty.any_builtin_dirty());
        assert!(session.is_dirty_since_snapshot());
        assert!(!dirty.global);
        assert!(!dirty.object);
    }

    #[test]
    fn snapshot_detects_global_dirty() {
        let core = KernelCore::new(KernelConfig::minimal());
        let session = KernelSession::new(&core);

        unsafe { &mut *(session.global_object.as_ptr() as *mut JsObject) }.bump_generation();
        let dirty = session.dirty_since_snapshot();

        assert!(dirty.global);
        assert!(dirty.any());
        assert!(!dirty.any_builtin_dirty());
    }

    #[test]
    fn snapshot_detects_stub_dirty() {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        Arc::get_mut(&mut session.builtin_world)
            .expect("fresh session owns its builtin world")
            .stub_objects
            .push(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())));
        session.record_snapshot();

        Arc::get_mut(&mut session.builtin_world)
            .expect("fresh session owns its builtin world")
            .stub_objects
            .push(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())));

        let dirty = session.dirty_since_snapshot();
        assert!(dirty.stubs);
        assert!(dirty.any_builtin_dirty());
        assert!(!dirty.global);
    }

    #[test]
    fn selective_reset_clean_keeps_builtin_world() {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        let world_ptr = Arc::as_ptr(&session.builtin_world);

        let dirty = session.selective_reset(&core);

        assert!(!dirty.any());
        assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&session.builtin_world)));
    }

    #[test]
    fn selective_reset_rebuilds_global_only_when_global_dirty() {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        let world_ptr = Arc::as_ptr(&session.builtin_world);
        let global_ptr = session.global_object.as_ptr();

        unsafe { &mut *(session.global_object.as_ptr() as *mut JsObject) }.bump_generation();
        let dirty = session.selective_reset(&core);

        assert!(dirty.global);
        assert!(!dirty.any_builtin_dirty());
        assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&session.builtin_world)));
        assert!(!std::ptr::eq(global_ptr, session.global_object.as_ptr()));
    }

    #[test]
    fn selective_reset_rebuilds_dirty_builtin_group() {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        let object_proto = session.builtin_world.object_proto.as_ptr();
        let array_proto = session.builtin_world.array_proto.as_ptr();
        let world_ptr = Arc::as_ptr(&session.builtin_world);

        unsafe { &mut *(session.builtin_world.array_proto.as_ptr() as *mut JsObject) }.bump_generation();
        let dirty = session.selective_reset(&core);

        assert!(dirty.array);
        assert!(!std::ptr::eq(world_ptr, Arc::as_ptr(&session.builtin_world)));
        assert!(std::ptr::eq(object_proto, session.builtin_world.object_proto.as_ptr()));
        assert!(!std::ptr::eq(array_proto, session.builtin_world.array_proto.as_ptr()));

        let ctor_proto = session.builtin_world.array_constructor.get_prop_at(0).as_js_object_ptr();
        assert!(std::ptr::eq(ctor_proto, session.builtin_world.array_proto.as_ptr() as *mut JsObject));
    }
}
