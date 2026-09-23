//! 每会话可变状态：内置原型对象（BuiltinWorld）与全局对象；世代快照 +
//! 家族位脏集合计算，两条重置路径同走选择性重建，只重建脏家族。

use std::sync::Arc;

use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtin::BuiltinWorld;
use crate::shape_forge::EMPTY_SHAPE_ID;

use super::{BuiltinDirtySet, BuiltinId, BuiltinSnapshot, KernelCore};
/// 每会话可变状态：内置原型对象与全局对象。
/// full_reset() 为宿主全隔离重置入口：清空全部执行状态与内存，内置对象
/// 经下述选择性重建（未污染者保留原指针），并重新绑定被污染的内置对象
/// 与重采世代快照；selective_reset() 为选择性重建本体，按家族位脏集合
/// 只重建脏家族，保留未污染的内置指针与世代。
pub struct KernelSession {
    pub builtin_world: Arc<BuiltinWorld>,
    pub global_object: P<JsObject>,
    pub builtin_snapshot: BuiltinSnapshot,
}

impl KernelSession {
    /// 创建全局对象本体：`[[Prototype]]` 挂 Object.prototype（无代理分层
    /// 引擎的最简等价形态），并预置 NaN/undefined/Infinity 三个全局常量。
    fn new_global_object(core: &KernelCore, object_proto: JsValue) -> P<JsObject> {
        let mut global_obj = JsObject::new_empty(EMPTY_SHAPE_ID, object_proto);

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
    /// 第二次及后续调用时命中缓存。
    pub fn new(core: &KernelCore) -> Self {
        let builtin_world = Arc::new(BuiltinWorld::new(&core.perm_interner, &core.shape_forge));
        let object_proto_val = JsValue::from_js_object(builtin_world.object_proto.as_ptr() as *mut JsObject);
        let global_object = Self::new_global_object(core, object_proto_val);
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
        // 重建收尾的原型槽重指与重绑内部写会推进 wrapper 世代：先以当前世代
        // 重刷释放登记表洁净基线，避免下一次判脏误报 wrapper 本体被写。
        self.builtin_world.refresh_leaked_object_baselines();
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

        let mut dirty = BuiltinDirtySet {
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
            shared_array_buffer: gen(BuiltinId::SharedArrayBufferProto) != snap(BuiltinId::SharedArrayBufferProto)
                || gen(BuiltinId::SharedArrayBufferConstructor) != snap(BuiltinId::SharedArrayBufferConstructor),
            atomics: gen(BuiltinId::AtomicsObject) != snap(BuiltinId::AtomicsObject),
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
        };
        // wrapper 本体被写不落在任何家族位上（写的是 wrapper 自身而非所属 P
        // 对象）：收缩为全脏，强制重建全部家族与 global，使被写的可复用
        // wrapper 经失效流程清键、不被复用回新原型。
        if world.has_dirty_leaked_objects() {
            dirty = BuiltinDirtySet::all_dirty();
        }
        dirty
    }

    /// 是否自上次快照以来存在任何污染。
    pub fn is_dirty_since_snapshot(&self) -> bool {
        self.dirty_since_snapshot().any()
    }

    /// 收尾释放 session 拥有的手工堆数据：builtin world（方法 wrapper 释放登记表 +
    /// 全部 P 对象属性区）与 global 对象属性区。对象本体随 Arc 引用归零释放。
    ///
    /// # 注意事项
    /// 幂等（释放登记表按值取走、属性区释放后置空），session 生命周期内可安全重入；
    /// 仅应在 session 真正终止时调用——选择性重置替换的旧 world/global 不走本路径。
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
        // 被写的可复用 wrapper 复用键先失效：随后重建的 rebind 走 miss 分支
        // 新建 wrapper，旧 wrapper（含用户新增属性与悬垂对象槽）不被复用回新
        // 原型，本体滞留至 session 收尾统一释放。
        self.builtin_world.invalidate_dirty_leaked_objects();
        if dirty.global {
            // 旧 global 的属性区在替换前释放，避免旧引用长期持有该内存。
            let old_global = unsafe { &mut *(self.global_object.as_ptr() as *mut JsObject) };
            old_global.release_raw_heap();
            // proto 取当前（可能旧）world 的 ObjectProto；object 家族脏重建后
            // 由下方收尾重指修正。
            let object_proto_val = JsValue::from_js_object(self.builtin_world.object_proto.as_ptr() as *mut JsObject);
            self.global_object = Self::new_global_object(core, object_proto_val);
        }
        if dirty.any_builtin_dirty() {
            let old_world = self.builtin_world.clone();
            let new_world = BuiltinWorld::rebuild_with_dirty(
                &old_world,
                core.perm_interner.as_ref(),
                core.shape_forge.as_ref(),
                &dirty,
            );
            // 旧 world 的释放登记表并入新 world：存活 wrapper（未污染家族别名）
            // 仍须在 session 收尾统一释放；已弃 wrapper 随之恰好释放一次，
            // 不二次持有。
            new_world.inherit_leaked_objects(&old_world);
            // 选择性重建收尾：保留对象（释放登记表 wrapper + 共用保留 P 字段）
            // 原型槽改写到新指针，随后被替换旧 P 对象属性区恰好释放一次
            // （Function/Object 4 个对象本体永久保留、属性区同批释放）；原型
            // 槽改写须先于释放、先于旧 world 被替换完成。
            new_world.retire_replaced(&old_world);
            // 保留 global 的 proto 槽重指：object 家族脏重建替换 ObjectProto，
            // global 不在 BuiltinWorld 内，retire_replaced 不覆盖。
            let new_obj_proto = new_world.object_proto.as_ptr();
            let old_obj_proto = old_world.object_proto.as_ptr();
            self.builtin_world = Arc::new(new_world);
            if new_obj_proto != old_obj_proto {
                let global = unsafe { &mut *(self.global_object.as_ptr() as *mut JsObject) };
                global.set_proto(JsValue::from_js_object(new_obj_proto as *mut JsObject)).ok();
            }
        }
        dirty
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        self.teardown_builtins();
    }
}
