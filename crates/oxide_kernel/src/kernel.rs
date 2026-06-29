#![allow(clippy::arc_with_non_send_sync)]

use std::num::NonZeroUsize;
use std::sync::Arc;

use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtin::BuiltinWorld;
use crate::code_forge::CodeForge;
use crate::kernel_info;
use crate::prop_forge::PropForge;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;
use oxide_log;
use oxide_log::{Level, SUBSYSTEM_COUNT};

#[derive(Clone)]
pub struct KernelConfig {
    pub min_pool_size: usize,
    pub max_pool_size: Option<usize>,
    pub max_dead_strings: Option<usize>,
    pub max_steps: Option<u64>,
    pub max_call_depth: usize,
    pub session_gc_threshold: usize,
    pub max_cached_modules: usize,
    pub log_levels: [Level; SUBSYSTEM_COUNT],
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
            session_gc_threshold: 33_554_432,
            max_cached_modules: 512,
            log_levels: [Level::Off; SUBSYSTEM_COUNT],
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
            session_gc_threshold: 33_554_432,
            max_cached_modules: 512,
            log_levels: [Level::Off; SUBSYSTEM_COUNT],
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
            session_gc_threshold: 33_554_432,
            max_cached_modules: 512,
            log_levels: [Level::Off; SUBSYSTEM_COUNT],
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

    pub fn max_cached_modules(&self) -> usize {
        self.max_cached_modules
    }

    pub fn set_max_cached_modules(&mut self, cap: usize) {
        self.max_cached_modules = cap;
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
    pub perm_interner: Arc<PermInterner>,
    pub shape_forge: Arc<ShapeForge>,
    pub code_forge: Arc<CodeForge>,
    pub prop_forge: Arc<PropForge>,
}

impl KernelCore {
    pub fn new(config: KernelConfig) -> Arc<Self> {
        oxide_log::init(&oxide_log::LogConfig {
            output: oxide_log::Output::Stderr,
            levels: config.log_levels,
        });
        let perm_interner = Arc::new(PermInterner::new());
        let shape_forge = Arc::new(ShapeForge::new());
        let code_forge = Arc::new(CodeForge::new(
            NonZeroUsize::new(config.max_cached_modules).expect("max_cached_modules must be greater than zero"),
        ));
        let prop_forge = Arc::new(PropForge::new());
        let max_cached = config.max_cached_modules;
        let min_pool = config.min_pool_size;
        let core = Arc::new(Self {
            config,
            perm_interner,
            shape_forge,
            code_forge,
            prop_forge,
        });
        kernel_info!("KernelCore initialized: max_cached_modules={}, min_pool={}", max_cached, min_pool);
        core
    }

    pub fn perm_interner(&self) -> &Arc<PermInterner> {
        &self.perm_interner
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

    pub fn max_cached_modules(&self) -> usize {
        self.config.max_cached_modules
    }

    pub fn set_max_cached_modules(&mut self, cap: usize) {
        self.config.max_cached_modules = cap;
    }

    pub fn sweep_runner_forges(&self) {
        // The key interner is append-only (no per-run sweep); only the transient
        // shape/prop tables need bounding at the test262 per-test boundary.
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
pub const NUM_BUILTINS: usize = 66;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BuiltinId {
    ObjectProto = 0,
    ArrayProto = 1,
    FunctionProto = 2,
    StringProto = 3,
    NumberProto = 4,
    BooleanProto = 5,
    ErrorProto = 6,
    SymbolProto = 7,
    ObjectConstructor = 8,
    ArrayConstructor = 9,
    FunctionConstructor = 10,
    StringConstructor = 11,
    NumberConstructor = 12,
    BooleanConstructor = 13,
    ErrorConstructor = 14,
    SymbolConstructor = 15,
    TypeErrorProto = 16,
    ReferenceErrorProto = 17,
    RangeErrorProto = 18,
    SyntaxErrorProto = 19,
    UriErrorProto = 20,
    EvalErrorProto = 21,
    MathObject = 22,
    JsonObject = 23,
    DateConstructor = 24,
    DateProto = 25,
    SetConstructor = 26,
    SetProto = 27,
    MapConstructor = 28,
    MapProto = 29,
    RegExpConstructor = 30,
    RegExpProto = 31,
    ArrayBufferConstructor = 32,
    ArrayBufferProto = 33,
    DataViewConstructor = 34,
    DataViewProto = 35,
    TypedArrayProto = 36,
    Int8ArrayConstructor = 37,
    Int8ArrayProto = 38,
    Uint8ArrayConstructor = 39,
    Uint8ArrayProto = 40,
    Uint8ClampedArrayConstructor = 41,
    Uint8ClampedArrayProto = 42,
    Int16ArrayConstructor = 43,
    Int16ArrayProto = 44,
    Uint16ArrayConstructor = 45,
    Uint16ArrayProto = 46,
    Int32ArrayConstructor = 47,
    Int32ArrayProto = 48,
    Uint32ArrayConstructor = 49,
    Uint32ArrayProto = 50,
    Float32ArrayConstructor = 51,
    Float32ArrayProto = 52,
    Float64ArrayConstructor = 53,
    Float64ArrayProto = 54,
    BigInt64ArrayConstructor = 55,
    BigInt64ArrayProto = 56,
    BigUint64ArrayConstructor = 57,
    BigUint64ArrayProto = 58,
    SymMatch = 59,
    SymReplace = 60,
    SymSearch = 61,
    SymSplit = 62,
    SymIterator = 63,
    SymToPrimitive = 64,
    SymHasInstance = 65,
}

impl BuiltinId {
    pub const ALL: [BuiltinId; NUM_BUILTINS] = [
        BuiltinId::ObjectProto,
        BuiltinId::ArrayProto,
        BuiltinId::FunctionProto,
        BuiltinId::StringProto,
        BuiltinId::NumberProto,
        BuiltinId::BooleanProto,
        BuiltinId::ErrorProto,
        BuiltinId::SymbolProto,
        BuiltinId::ObjectConstructor,
        BuiltinId::ArrayConstructor,
        BuiltinId::FunctionConstructor,
        BuiltinId::StringConstructor,
        BuiltinId::NumberConstructor,
        BuiltinId::BooleanConstructor,
        BuiltinId::ErrorConstructor,
        BuiltinId::SymbolConstructor,
        BuiltinId::TypeErrorProto,
        BuiltinId::ReferenceErrorProto,
        BuiltinId::RangeErrorProto,
        BuiltinId::SyntaxErrorProto,
        BuiltinId::UriErrorProto,
        BuiltinId::EvalErrorProto,
        BuiltinId::MathObject,
        BuiltinId::JsonObject,
        BuiltinId::DateConstructor,
        BuiltinId::DateProto,
        BuiltinId::SetConstructor,
        BuiltinId::SetProto,
        BuiltinId::MapConstructor,
        BuiltinId::MapProto,
        BuiltinId::RegExpConstructor,
        BuiltinId::RegExpProto,
        BuiltinId::ArrayBufferConstructor,
        BuiltinId::ArrayBufferProto,
        BuiltinId::DataViewConstructor,
        BuiltinId::DataViewProto,
        BuiltinId::TypedArrayProto,
        BuiltinId::Int8ArrayConstructor,
        BuiltinId::Int8ArrayProto,
        BuiltinId::Uint8ArrayConstructor,
        BuiltinId::Uint8ArrayProto,
        BuiltinId::Uint8ClampedArrayConstructor,
        BuiltinId::Uint8ClampedArrayProto,
        BuiltinId::Int16ArrayConstructor,
        BuiltinId::Int16ArrayProto,
        BuiltinId::Uint16ArrayConstructor,
        BuiltinId::Uint16ArrayProto,
        BuiltinId::Int32ArrayConstructor,
        BuiltinId::Int32ArrayProto,
        BuiltinId::Uint32ArrayConstructor,
        BuiltinId::Uint32ArrayProto,
        BuiltinId::Float32ArrayConstructor,
        BuiltinId::Float32ArrayProto,
        BuiltinId::Float64ArrayConstructor,
        BuiltinId::Float64ArrayProto,
        BuiltinId::BigInt64ArrayConstructor,
        BuiltinId::BigInt64ArrayProto,
        BuiltinId::BigUint64ArrayConstructor,
        BuiltinId::BigUint64ArrayProto,
        BuiltinId::SymMatch,
        BuiltinId::SymReplace,
        BuiltinId::SymSearch,
        BuiltinId::SymSplit,
        BuiltinId::SymIterator,
        BuiltinId::SymToPrimitive,
        BuiltinId::SymHasInstance,
    ];
}

#[derive(Clone, Debug)]
pub struct BuiltinSnapshot {
    pub generations: [u32; NUM_BUILTINS],
    pub global_object_generation: u32,
    pub stub_objects_len: usize,
    pub stub_object_generations: Vec<u32>,
}

impl BuiltinSnapshot {
    fn gen(obj: &P<JsObject>) -> u32 {
        obj.generation()
    }

    pub fn new(world: &BuiltinWorld, global_object: &P<JsObject>) -> Self {
        let mut generations = [0u32; NUM_BUILTINS];
        for (i, id) in BuiltinId::ALL.iter().enumerate() {
            generations[i] = Self::gen(world.get_by_id(*id));
        }
        debug_assert_eq!(generations.len(), NUM_BUILTINS);
        Self {
            generations,
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

        let si_nan = core.perm_interner.intern("NaN").0;
        let si_undef = core.perm_interner.intern("undefined").0;
        let si_infinity = core.perm_interner.intern("Infinity").0;

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
        let builtin_world = Arc::new(BuiltinWorld::new(&core.perm_interner, &core.shape_forge));
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
        let gen = |id: BuiltinId| BuiltinSnapshot::gen(world.get_by_id(id));
        let snap = |id: BuiltinId| snapshot.generations[id as usize];

        BuiltinDirtySet {
            object: gen(BuiltinId::ObjectProto) != snap(BuiltinId::ObjectProto)
                || gen(BuiltinId::ObjectConstructor) != snap(BuiltinId::ObjectConstructor),
            array: gen(BuiltinId::ArrayProto) != snap(BuiltinId::ArrayProto)
                || gen(BuiltinId::ArrayConstructor) != snap(BuiltinId::ArrayConstructor),
            function: gen(BuiltinId::FunctionProto) != snap(BuiltinId::FunctionProto)
                || gen(BuiltinId::FunctionConstructor) != snap(BuiltinId::FunctionConstructor),
            string: gen(BuiltinId::StringProto) != snap(BuiltinId::StringProto)
                || gen(BuiltinId::StringConstructor) != snap(BuiltinId::StringConstructor),
            number: gen(BuiltinId::NumberProto) != snap(BuiltinId::NumberProto)
                || gen(BuiltinId::NumberConstructor) != snap(BuiltinId::NumberConstructor),
            boolean: gen(BuiltinId::BooleanProto) != snap(BuiltinId::BooleanProto)
                || gen(BuiltinId::BooleanConstructor) != snap(BuiltinId::BooleanConstructor),
            error_family: gen(BuiltinId::ErrorProto) != snap(BuiltinId::ErrorProto)
                || gen(BuiltinId::ErrorConstructor) != snap(BuiltinId::ErrorConstructor)
                || gen(BuiltinId::TypeErrorProto) != snap(BuiltinId::TypeErrorProto)
                || gen(BuiltinId::ReferenceErrorProto) != snap(BuiltinId::ReferenceErrorProto)
                || gen(BuiltinId::RangeErrorProto) != snap(BuiltinId::RangeErrorProto)
                || gen(BuiltinId::SyntaxErrorProto) != snap(BuiltinId::SyntaxErrorProto)
                || gen(BuiltinId::UriErrorProto) != snap(BuiltinId::UriErrorProto)
                || gen(BuiltinId::EvalErrorProto) != snap(BuiltinId::EvalErrorProto),
            symbol_family: gen(BuiltinId::SymbolProto) != snap(BuiltinId::SymbolProto)
                || gen(BuiltinId::SymbolConstructor) != snap(BuiltinId::SymbolConstructor)
                || gen(BuiltinId::SymMatch) != snap(BuiltinId::SymMatch)
                || gen(BuiltinId::SymReplace) != snap(BuiltinId::SymReplace)
                || gen(BuiltinId::SymSearch) != snap(BuiltinId::SymSearch)
                || gen(BuiltinId::SymSplit) != snap(BuiltinId::SymSplit)
                || gen(BuiltinId::SymIterator) != snap(BuiltinId::SymIterator)
                || gen(BuiltinId::SymToPrimitive) != snap(BuiltinId::SymToPrimitive)
                || gen(BuiltinId::SymHasInstance) != snap(BuiltinId::SymHasInstance),
            math: gen(BuiltinId::MathObject) != snap(BuiltinId::MathObject),
            json: gen(BuiltinId::JsonObject) != snap(BuiltinId::JsonObject),
            date: gen(BuiltinId::DateConstructor) != snap(BuiltinId::DateConstructor)
                || gen(BuiltinId::DateProto) != snap(BuiltinId::DateProto),
            set: gen(BuiltinId::SetConstructor) != snap(BuiltinId::SetConstructor)
                || gen(BuiltinId::SetProto) != snap(BuiltinId::SetProto),
            map: gen(BuiltinId::MapConstructor) != snap(BuiltinId::MapConstructor)
                || gen(BuiltinId::MapProto) != snap(BuiltinId::MapProto),
            regexp: gen(BuiltinId::RegExpConstructor) != snap(BuiltinId::RegExpConstructor)
                || gen(BuiltinId::RegExpProto) != snap(BuiltinId::RegExpProto),
            array_buffer: gen(BuiltinId::ArrayBufferConstructor) != snap(BuiltinId::ArrayBufferConstructor)
                || gen(BuiltinId::ArrayBufferProto) != snap(BuiltinId::ArrayBufferProto),
            data_view: gen(BuiltinId::DataViewConstructor) != snap(BuiltinId::DataViewConstructor)
                || gen(BuiltinId::DataViewProto) != snap(BuiltinId::DataViewProto),
            typed_array_family: gen(BuiltinId::TypedArrayProto) != snap(BuiltinId::TypedArrayProto)
                || gen(BuiltinId::Int8ArrayConstructor) != snap(BuiltinId::Int8ArrayConstructor)
                || gen(BuiltinId::Int8ArrayProto) != snap(BuiltinId::Int8ArrayProto)
                || gen(BuiltinId::Uint8ArrayConstructor) != snap(BuiltinId::Uint8ArrayConstructor)
                || gen(BuiltinId::Uint8ArrayProto) != snap(BuiltinId::Uint8ArrayProto)
                || gen(BuiltinId::Uint8ClampedArrayConstructor) != snap(BuiltinId::Uint8ClampedArrayConstructor)
                || gen(BuiltinId::Uint8ClampedArrayProto) != snap(BuiltinId::Uint8ClampedArrayProto)
                || gen(BuiltinId::Int16ArrayConstructor) != snap(BuiltinId::Int16ArrayConstructor)
                || gen(BuiltinId::Int16ArrayProto) != snap(BuiltinId::Int16ArrayProto)
                || gen(BuiltinId::Uint16ArrayConstructor) != snap(BuiltinId::Uint16ArrayConstructor)
                || gen(BuiltinId::Uint16ArrayProto) != snap(BuiltinId::Uint16ArrayProto)
                || gen(BuiltinId::Int32ArrayConstructor) != snap(BuiltinId::Int32ArrayConstructor)
                || gen(BuiltinId::Int32ArrayProto) != snap(BuiltinId::Int32ArrayProto)
                || gen(BuiltinId::Uint32ArrayConstructor) != snap(BuiltinId::Uint32ArrayConstructor)
                || gen(BuiltinId::Uint32ArrayProto) != snap(BuiltinId::Uint32ArrayProto)
                || gen(BuiltinId::Float32ArrayConstructor) != snap(BuiltinId::Float32ArrayConstructor)
                || gen(BuiltinId::Float32ArrayProto) != snap(BuiltinId::Float32ArrayProto)
                || gen(BuiltinId::Float64ArrayConstructor) != snap(BuiltinId::Float64ArrayConstructor)
                || gen(BuiltinId::Float64ArrayProto) != snap(BuiltinId::Float64ArrayProto)
                || gen(BuiltinId::BigInt64ArrayConstructor) != snap(BuiltinId::BigInt64ArrayConstructor)
                || gen(BuiltinId::BigInt64ArrayProto) != snap(BuiltinId::BigInt64ArrayProto)
                || gen(BuiltinId::BigUint64ArrayConstructor) != snap(BuiltinId::BigUint64ArrayConstructor)
                || gen(BuiltinId::BigUint64ArrayProto) != snap(BuiltinId::BigUint64ArrayProto),
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
                core.perm_interner.as_ref(),
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
        let (i1, _) = core.perm_interner().intern("test");
        let (i2, _) = core.perm_interner().intern("test");
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
        let (i1, _) = core.perm_interner().intern("hello");
        let (i2, _) = core.perm_interner().intern("hello");
        assert_eq!(i1, i2);
    }

    #[test]
    fn test_kernel_config_presets() {
        assert_eq!(KernelConfig::minimal().max_pool_size, Some(8));
        assert_eq!(KernelConfig::standard().max_pool_size, Some(32));
        assert_eq!(KernelConfig::minimal().max_steps, None);
        assert_eq!(KernelConfig::standard().max_steps, None);
        assert_eq!(KernelConfig::full().max_steps, None);
        assert_eq!(KernelConfig::minimal().log_levels, [Level::Off; SUBSYSTEM_COUNT]);
        assert!(!KernelConfig::minimal().warmup_builtin_ic);
        assert!(KernelConfig::full().warmup_builtin_ic);
        assert_eq!(KernelConfig::full().max_pool_size, None);
    }

    #[test]
    fn test_session_rebuild_shares_forges() {
        let core = KernelCore::new(KernelConfig::minimal());
        let (i1, _) = core.perm_interner().intern("hello");
        let _s2 = KernelSession::new(&core);
        let (i2, _) = core.perm_interner().intern("hello");
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
