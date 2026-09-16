#![allow(clippy::arc_with_non_send_sync)]

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
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

mod builtin_id;
mod config;

pub use builtin_id::{BuiltinDirtySet, BuiltinId, BuiltinSnapshot, NUM_BUILTINS};
pub use config::KernelConfig;

/// 不可变、跨所有 VM 实例永久共享的状态。
/// 构造后从不原地重建——forge 表为 append-only；宿主可经
/// [`KernelCore::should_rebuild_perm`] 阈值在安全边界（无存活 VM）整体
/// 重建（换 `Arc`，非原地清表）。
pub struct KernelCore {
    pub config: KernelConfig,
    pub perm_interner: Arc<PermInterner>,
    pub shape_forge: Arc<ShapeForge>,
    pub code_forge: Arc<CodeForge>,
    pub prop_forge: Arc<PropForge>,
    /// 边界不变量守卫计数：当前持有本 kernel `Arc` 的 Vm 数。
    /// `Vm` 构造器增、`Drop for Vm` 减（恰好一次），供 sweep / kernel 重建 /
    /// kernel drop 处断言"无存活 VM"边界。
    active_vms: AtomicUsize,
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
            active_vms: AtomicUsize::new(0),
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

    /// 批边界清理瞬时 shape/prop 缓存，防止跨测试累积膨胀。
    ///
    /// 瞬时 forge（非根 shape/transition/position + prop 模板）的 id 空间只在两类
    /// 边界复位：（1）kernel 整体重建——新核构造即空（结构性必清；宿主义务 =
    /// 重建前旧核无存活 VM，否则旧核连同其 forge 永久驻留）；（2）批内兜底 sweep
    /// ——本函数，数据依赖：仅当 shape 或 prop 表超过 50k 阈值时才执行
    /// [`ShapeForge::clear_transient`] 与 [`PropForge::clear`]（阈值是增长闸门，
    /// 宿主的检查节奏只是采样点）。字符串 intern 表 append-only、CodeForge LRU
    /// 自管理，均不碰。
    ///
    /// # 边界与前提
    /// - 调用点须无存活 VM：对象头与 IC 词持 shape id，清空使 id 空间复位，跨复位
    ///   存活的 VM 会因 id 复用碰撞静默错槽；该前提由 debug_assert 守卫（开发期
    ///   fail-fast），release 构建无断言，宿主可经 [`Self::active_vms`] 自检；
    /// - 引擎不自动重建：id 空间的另一类复位（kernel 整体重建）由宿主在
    ///   "无存活 VM" 边界驱动（runner 由循环结构满足：VM 每测试新建即弃，
    ///   重建/sweep 点都在 VM 作用域外）。
    pub fn sweep_runner_forges(&self) {
        // id 空间复位在有存活 VM 时是静默错槽隐患，守卫先于阈值判断。
        debug_assert!(
            self.active_vms.load(Ordering::Relaxed) == 0,
            "sweep_runner_forges requires no live VMs (shape id space reset)"
        );
        // 键 interner 是 append-only（无逐次清理）；仅当瞬时 shape/prop 表超阈值
        // 时清表（批内兜底）。
        if self.shape_forge.len() > 50_000 {
            self.shape_forge.clear_transient();
            self.prop_forge.clear();
        } else if self.prop_forge.len() > 50_000 {
            self.prop_forge.clear();
        }
    }

    /// 宿主是否应整体重建本 kernel，并返回重建后的建议上限（advisory 信号）。
    ///
    /// perm interner 唯一键数 `entry_count` **超过**配置的
    /// [`KernelConfig::perm_interner_max_entries`] 阈值时返回
    /// `Some(建议上限)`——建议上限 = 阈值取 2 的幂后加倍，宿主应在整体重建
    /// 后把它写入新 kernel 的 `perm_interner_max_entries`（增长不立即再次
    /// 触顶）；阈值未设或键数未超阈值时返回 `None`。纯 advisory 契约：引擎
    /// 不自动重建——重建须由宿主在"无存活 VM"边界驱动：归还全部 `VmGuard`
    /// （池排空）→ 旧 `Arc<KernelCore>` 归零整体释放（PermInterner/ShapeForge/
    /// CodeForge/PropForge，含全部泄漏键文本与物化串）→ 新建 kernel + 新池/
    /// VM。存活 VM 的代际表注册表（`immutables`）与 P 对象持有旧 kernel 的物化串裸
    /// 指针与 shape id，VM 存活期间重建会使其悬垂。三个预设默认 None = 永不
    /// 触发（有意裁定：CLI eval/run/REPL/bench/test262 宿主均未接重建边界，
    /// None 保证零行为漂移）。
    ///
    /// # 边界与前提
    /// - 仅在宿主边界（run 间 / 测试间 / 迭代间）调用，勿入 dispatch 热路径：
    ///   `entry_count` 为 O(1) 读锁，intern 路径零加码；
    /// - 仅具备安全重建边界的宿主（test262 runner 批边界、嵌入宿主迭代
    ///   之间）应启用旋钮；REPL 类宿主（单持久 VM，重建 = 丢失顶层状态）
    ///   不应启用。
    ///
    /// # 副作用
    /// 无：只读查询。
    pub fn should_rebuild_perm(&self) -> Option<u32> {
        let cap = self.config.perm_interner_max_entries?;
        (self.perm_interner.entry_count() > cap).then_some(cap.next_power_of_two().saturating_mul(2))
    }

    /// 边界不变量守卫计数：登记一个 Vm 出生（`Vm` 两条构造路径完整构造后调用）。
    pub fn note_vm_started(&self) {
        self.active_vms.fetch_add(1, Ordering::Relaxed);
    }

    /// 边界不变量守卫计数：注销一个 Vm 死亡（`Drop for Vm` 恰好调用一次）。
    pub fn note_vm_ended(&self) {
        self.active_vms.fetch_sub(1, Ordering::Relaxed);
    }

    /// 读取当前持有本 kernel 的存活 Vm 数；release 宿主可在 id 空间复位边界自检。
    pub fn active_vms(&self) -> usize {
        self.active_vms.load(Ordering::Relaxed)
    }
}

impl Drop for KernelCore {
    fn drop(&mut self) {
        // kernel 能 drop 说明持 Arc 的 Vm 已全部归零；计数非零 = 纯计数漂移
        // （note_vm_started/note_vm_ended 配对挂接回归探测）。
        debug_assert_eq!(
            self.active_vms.load(Ordering::Relaxed),
            0,
            "KernelCore dropped with live VMs (note_vm_ended counter drift)"
        );
    }
}

/// 每会话可变状态：内置原型对象与全局对象。
/// 在 full_reset() 时重建，实现 JS 执行之间的完全隔离。
pub struct KernelSession {
    pub builtin_world: Arc<BuiltinWorld>,
    pub global_object: P<JsObject>,
    pub builtin_snapshot: BuiltinSnapshot,
}

impl KernelSession {
    fn new_global_object(core: &KernelCore) -> P<JsObject> {
        let mut global_obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

        let si_nan = core.perm_interner.intern("NaN").0;
        let si_undef = core.perm_interner.intern("undefined").0;
        let si_infinity = core.perm_interner.intern("Infinity").0;

        // 全局三常量描述符：{ writable:false, enumerable:false, configurable:false }。
        // e:false 是枚举面守卫（Object.keys / for-in 由 enumerable 决定）；
        // c:false 是 delete / defineProperty 可观察的规范值（delete globalThis.undefined
        // 返回 false、属性不可删，后续声明/定义均不得 reconfigure）。
        let nan_shape = core.shape_forge.make_shape(EMPTY_SHAPE_ID, si_nan);
        global_obj.set_shape_id(nan_shape);
        global_obj.ensure_hash_props().push(JsValue::float(f64::NAN));
        global_obj.set_data_meta(
            global_obj.prop_vec_len().saturating_sub(1) as u32,
            oxide_types::object::PropAttributes::new(false, false, false),
        );

        let undef_shape = core.shape_forge.make_shape(nan_shape, si_undef);
        global_obj.set_shape_id(undef_shape);
        global_obj.ensure_hash_props().push(JsValue::undefined());
        global_obj.set_data_meta(
            global_obj.prop_vec_len().saturating_sub(1) as u32,
            oxide_types::object::PropAttributes::new(false, false, false),
        );

        let inf_shape = core.shape_forge.make_shape(undef_shape, si_infinity);
        global_obj.set_shape_id(inf_shape);
        global_obj.ensure_hash_props().push(JsValue::float(f64::INFINITY));
        global_obj.set_data_meta(
            global_obj.prop_vec_len().saturating_sub(1) as u32,
            oxide_types::object::PropAttributes::new(false, false, false),
        );

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
                || gen(BuiltinId::EvalErrorProto) != snap(BuiltinId::EvalErrorProto)
                || gen(BuiltinId::SuppressedErrorProto) != snap(BuiltinId::SuppressedErrorProto),
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
                || gen(BuiltinId::SymSpecies) != snap(BuiltinId::SymSpecies)
                || gen(BuiltinId::SymAsyncDispose) != snap(BuiltinId::SymAsyncDispose)
                || gen(BuiltinId::SymDispose) != snap(BuiltinId::SymDispose),
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
                || gen(BuiltinId::PlainDateTimeProto) != snap(BuiltinId::PlainDateTimeProto)
                || gen(BuiltinId::PlainMonthDayConstructor) != snap(BuiltinId::PlainMonthDayConstructor)
                || gen(BuiltinId::PlainMonthDayProto) != snap(BuiltinId::PlainMonthDayProto)
                || gen(BuiltinId::PlainYearMonthConstructor) != snap(BuiltinId::PlainYearMonthConstructor)
                || gen(BuiltinId::PlainYearMonthProto) != snap(BuiltinId::PlainYearMonthProto),
            stubs: world.stub_objects.len() != snapshot.stub_objects_len
                || stub_generations_dirty
                || gen(BuiltinId::BigIntConstructor) != snap(BuiltinId::BigIntConstructor)
                || gen(BuiltinId::BigIntProto) != snap(BuiltinId::BigIntProto),
            global: BuiltinSnapshot::gen(&self.global_object) != snapshot.global_object_generation,
            console: gen(BuiltinId::Console) != snap(BuiltinId::Console),
        }
    }

    /// 是否自上次快照以来存在任何污染。
    pub fn is_dirty_since_snapshot(&self) -> bool {
        self.dirty_since_snapshot().any()
    }

    /// 收尾释放 session 拥有的手工堆数据：builtin world（方法 wrapper 登记表 +
    /// 全部 P 对象属性区）与 global 对象属性区。对象本体随 Arc 引用归零释放。
    ///
    /// # 注意事项
    /// 幂等（登记表按值取走、属性区释放后置空），session 生命周期内可安全重入；
    /// 仅应在 session 真正终止时调用——选择性重置换出的旧 world/global 不走本路径。
    pub fn teardown_builtins(&mut self) {
        self.builtin_world.teardown_heap_data();
        // SAFETY: global 归本 session 所有，收尾时恰好释放其属性区一次。
        let global = unsafe { &mut *(self.global_object.as_ptr() as *mut JsObject) };
        global.release_raw_heap();
    }

    /// 选择性重置：只重建被污染的对象（global 或相应 builtin 家族），并返回脏集合。
    ///
    /// 相比全量重建，可保留未污染的内置对象指针与世代，减少隔离成本。
    pub fn selective_reset(&mut self, core: &Arc<KernelCore>) -> BuiltinDirtySet {
        let dirty = self.dirty_since_snapshot();
        if dirty.global {
            // 旧 global 的属性区在替换前释放，避免随旧引用永久泄漏。
            let old_global = unsafe { &mut *(self.global_object.as_ptr() as *mut JsObject) };
            old_global.release_raw_heap();
            self.global_object = Self::new_global_object(core);
        }
        if dirty.any_builtin_dirty() {
            let old_world = self.builtin_world.clone();
            let new_world = BuiltinWorld::rebuild_with_dirty(
                &old_world,
                core.perm_interner.as_ref(),
                core.shape_forge.as_ref(),
                &dirty,
            );
            // 旧 world 的登记表并入新 world：存活 wrapper（未污染家族别名）仍须在
            // session 收尾统一释放；已弃 wrapper 随之恰好释放一次，不二次持有。
            new_world.inherit_leaked_objects(&old_world);
            // 重建收尾：保留对象（登记表 wrapper + 共用保留 P 字段）proto 槽
            // 重指新指针，随后被替换旧对象属性区恰好释放一次（含保活钉住
            // 对的属性区，本体钉保留）；重指须先于释放、先于换出完成。
            new_world.retire_replaced(&old_world);
            self.builtin_world = Arc::new(new_world);
        }
        dirty
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        self.teardown_builtins();
    }
}
