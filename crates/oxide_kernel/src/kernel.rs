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

/// kernel 运行配置：VM 池规模、步数/调用深度上限、session GC 阈值、
/// 日志级别与内置对象预热开关。由三个预设构造器（[`KernelConfig::minimal`] /
/// [`KernelConfig::standard`] / [`KernelConfig::full`]）或默认值创建。
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
    /// 最小配置：小 VM 池、关闭 code/IC 预热，适合嵌入式或单次执行场景。
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

    /// 标准配置：默认 VM 池大小，开启内置对象 code 预热。
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

    /// 全量配置：无上限 VM 池，开启 shapes/code/IC 全量预热，为性能场景服务。
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

    /// 读取 session GC 阈值（字节数）。
    pub fn session_gc_threshold(&self) -> usize {
        self.session_gc_threshold
    }

    /// 设置 session GC 阈值（字节数），超过后触发一次 session 级 GC。
    pub fn set_session_gc_threshold(&mut self, bytes: usize) {
        self.session_gc_threshold = bytes;
    }

    /// 读取 code cache 的 module 数量上限。
    pub fn max_cached_modules(&self) -> usize {
        self.max_cached_modules
    }

    /// 设置 code cache 的 module 数量上限。
    pub fn set_max_cached_modules(&mut self, cap: usize) {
        self.max_cached_modules = cap;
    }
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self::minimal()
    }
}

/// 不可变、跨所有 VM 实例永久共享的状态。
/// 构造后从不重建——forge 表为 append-only。
pub struct KernelCore {
    pub config: KernelConfig,
    pub perm_interner: Arc<PermInterner>,
    pub shape_forge: Arc<ShapeForge>,
    pub code_forge: Arc<CodeForge>,
    pub prop_forge: Arc<PropForge>,
}

impl KernelCore {
    /// 按配置创建共享核心：初始化日志系统与四个共享 forge（interner/shape/code/prop）。
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

    /// 只读访问永久字符串 intern 表。
    pub fn perm_interner(&self) -> &Arc<PermInterner> {
        &self.perm_interner
    }

    /// 只读访问共享 hidden class（shape）存储。
    pub fn shape_forge(&self) -> &Arc<ShapeForge> {
        &self.shape_forge
    }

    /// 只读访问共享 bytecode cache。
    pub fn code_forge(&self) -> &Arc<CodeForge> {
        &self.code_forge
    }

    /// 只读访问共享属性模板缓存。
    pub fn prop_forge(&self) -> &Arc<PropForge> {
        &self.prop_forge
    }

    /// 只读访问构建时固定的配置。
    pub fn config(&self) -> &KernelConfig {
        &self.config
    }

    /// 读取 session GC 阈值（字节数）。
    pub fn session_gc_threshold(&self) -> usize {
        self.config.session_gc_threshold
    }

    /// 设置 session GC 阈值（字节数）。
    pub fn set_session_gc_threshold(&mut self, bytes: usize) {
        self.config.session_gc_threshold = bytes;
    }

    /// 读取 code cache 的 module 数量上限。
    pub fn max_cached_modules(&self) -> usize {
        self.config.max_cached_modules
    }

    /// 设置 code cache 的 module 数量上限。
    pub fn set_max_cached_modules(&mut self, cap: usize) {
        self.config.max_cached_modules = cap;
    }

    /// 在每次 runner（test262 等）边界清理瞬时 shape/prop 缓存，防止跨测试累积膨胀。
    ///
    /// 字符串 intern 表是 append-only，无需清理；仅当 shape 或 prop 表超过阈值时才执行
    /// [`ShapeForge::clear_transient`] 与 [`PropForge::clear`]。
    pub fn sweep_runner_forges(&self) {
        // 键 interner 是 append-only（无逐次清理）；只有瞬时 shape/prop 表
        // 需要在 test262 每测试边界做上限约束。
        // test262 每测试新建 VM/session。在该边界，此前测试产生的 JS 对象
        // 不应保留任何瞬时 shape/模板。
        if self.shape_forge.len() > 50_000 {
            self.shape_forge.clear_transient();
            self.prop_forge.clear();
        } else if self.prop_forge.len() > 50_000 {
            self.prop_forge.clear();
        }
    }
}

/// 每会话可变状态：内置原型对象与全局对象。
/// 在 full_reset() 时重建，实现 JS 执行之间的完全隔离。
pub struct KernelSession {
    pub builtin_world: Arc<BuiltinWorld>,
    pub global_object: P<JsObject>,
    pub builtin_snapshot: BuiltinSnapshot,
}

/// 供 reset 脏检查用的世代快照。
///
/// 维护注意：每个新增的 `BuiltinWorld` 对象字段都必须加到这里以及
/// `KernelSession::dirty_since_snapshot()`，以便选择性重置重建正确的
/// builtin 家族。
pub const NUM_BUILTINS: usize = 86;

/// 内置对象枚举 id，与 `BuiltinWorld` 中的存储槽一一对应。
///
/// 覆盖各构造器/原型、Error 家族、集合类型、TypedArray 家族与 well-known symbols；
/// `repr(u8)` 使其可直接作为数组下标（`u8` 值即下标）。
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
    SymMatchAll = 66,
    SymAsyncIterator = 67,
    TemporalObject = 68,
    TemporalNowObject = 69,
    InstantConstructor = 70,
    InstantProto = 71,
    PlainDateConstructor = 72,
    PlainDateProto = 73,
    PlainTimeConstructor = 74,
    PlainTimeProto = 75,
    BigIntConstructor = 76,
    BigIntProto = 77,
    DurationConstructor = 78,
    DurationProto = 79,
    SymToStringTag = 80,
    SymSpecies = 85,
    ZonedDateTimeConstructor = 81,
    ZonedDateTimeProto = 82,
    PlainDateTimeConstructor = 83,
    PlainDateTimeProto = 84,
}

impl BuiltinId {
    /// 全部 86 个内置对象的 id 常量表，供快照/脏检查按序遍历。
    /// 顺序必须与枚举判别值一致（`ALL[i]` 的 `u8` 值 == `i`）：
    /// 快照数组按下标填充，脏检查按下标回读，错位会误判 builtin 家族永久脏。
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
        BuiltinId::SymMatchAll,
        BuiltinId::SymAsyncIterator,
        BuiltinId::TemporalObject,
        BuiltinId::TemporalNowObject,
        BuiltinId::InstantConstructor,
        BuiltinId::InstantProto,
        BuiltinId::PlainDateConstructor,
        BuiltinId::PlainDateProto,
        BuiltinId::PlainTimeConstructor,
        BuiltinId::PlainTimeProto,
        BuiltinId::BigIntConstructor,
        BuiltinId::BigIntProto,
        BuiltinId::DurationConstructor,
        BuiltinId::DurationProto,
        BuiltinId::SymToStringTag,
        BuiltinId::ZonedDateTimeConstructor,
        BuiltinId::ZonedDateTimeProto,
        BuiltinId::PlainDateTimeConstructor,
        BuiltinId::PlainDateTimeProto,
        BuiltinId::SymSpecies,
    ];
}

/// 内置对象世代（generation）快照：记录构造时各对象及其 stub 的世代号与数量。
///
/// 供 [`KernelSession::dirty_since_snapshot`] 对比，判断哪些 builtin 家族在运行期被污染。
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

    /// 对给定 builtin world 与 global object 采集一份世代快照。
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

/// 按 builtin 家族划分的脏标记位集合：运行期哪些内置对象被用户代码修改过。
///
/// `any_builtin_dirty` 只关注 builtin world 内部对象，`any` 额外包含 global object。
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
    pub temporal: bool,
    pub stubs: bool,
    pub global: bool,
}

impl BuiltinDirtySet {
    /// 是否存在任何 builtin world 内部对象被污染（不含 global object）。
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
            || self.temporal
            || self.stubs
    }

    /// 是否存在任何污染（builtin world 或 global object）。
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

    /// 从 KernelCore 构建一个全新 session。所有 string/shape intern 调用在
    /// 第二次及后续调用时命中缓存——净开销 < 0.5 ms。
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

    /// 只读访问当前 session 的 builtin world。
    pub fn builtin_world(&self) -> &Arc<BuiltinWorld> {
        &self.builtin_world
    }

    /// 只读访问当前 session 的 global object。
    pub fn global_object(&self) -> &P<JsObject> {
        &self.global_object
    }

    /// 重新采集内置对象世代快照，作为下一次脏检查的基准。
    pub fn record_snapshot(&mut self) {
        self.builtin_snapshot = BuiltinSnapshot::new(&self.builtin_world, &self.global_object);
    }

    /// 对比当前世代与最近快照，返回各 builtin 家族是否被污染。
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
                || gen(BuiltinId::SymHasInstance) != snap(BuiltinId::SymHasInstance)
                || gen(BuiltinId::SymMatchAll) != snap(BuiltinId::SymMatchAll)
                || gen(BuiltinId::SymAsyncIterator) != snap(BuiltinId::SymAsyncIterator)
                || gen(BuiltinId::SymToStringTag) != snap(BuiltinId::SymToStringTag)
                || gen(BuiltinId::SymSpecies) != snap(BuiltinId::SymSpecies),
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
            temporal: gen(BuiltinId::TemporalObject) != snap(BuiltinId::TemporalObject)
                || gen(BuiltinId::TemporalNowObject) != snap(BuiltinId::TemporalNowObject)
                || gen(BuiltinId::InstantConstructor) != snap(BuiltinId::InstantConstructor)
                || gen(BuiltinId::InstantProto) != snap(BuiltinId::InstantProto)
                || gen(BuiltinId::PlainDateConstructor) != snap(BuiltinId::PlainDateConstructor)
                || gen(BuiltinId::PlainDateProto) != snap(BuiltinId::PlainDateProto)
                || gen(BuiltinId::PlainTimeConstructor) != snap(BuiltinId::PlainTimeConstructor)
                || gen(BuiltinId::PlainTimeProto) != snap(BuiltinId::PlainTimeProto)
                || gen(BuiltinId::DurationConstructor) != snap(BuiltinId::DurationConstructor)
                || gen(BuiltinId::DurationProto) != snap(BuiltinId::DurationProto)
                || gen(BuiltinId::ZonedDateTimeConstructor) != snap(BuiltinId::ZonedDateTimeConstructor)
                || gen(BuiltinId::ZonedDateTimeProto) != snap(BuiltinId::ZonedDateTimeProto)
                || gen(BuiltinId::PlainDateTimeConstructor) != snap(BuiltinId::PlainDateTimeConstructor)
                || gen(BuiltinId::PlainDateTimeProto) != snap(BuiltinId::PlainDateTimeProto),
            stubs: world.stub_objects.len() != snapshot.stub_objects_len
                || stub_generations_dirty
                || gen(BuiltinId::BigIntConstructor) != snap(BuiltinId::BigIntConstructor)
                || gen(BuiltinId::BigIntProto) != snap(BuiltinId::BigIntProto),
            global: BuiltinSnapshot::gen(&self.global_object) != snapshot.global_object_generation,
        }
    }

    /// 是否自上次快照以来存在任何污染。
    pub fn is_dirty_since_snapshot(&self) -> bool {
        self.dirty_since_snapshot().any()
    }

    /// 选择性重置：只重建被污染的对象（global 或相应 builtin 家族），并返回脏集合。
    ///
    /// 相比全量重建，可保留未污染的内置对象指针与世代，减少隔离成本。
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
