//! 内置对象 id 枚举（`BuiltinId` 96 变体 + `ALL` 顺序钉表）、世代快照
//! （`BuiltinSnapshot`）与按家族划分的脏标记位集（`BuiltinDirtySet`）；
//! `NUM_BUILTINS` 文档承载"新增 BuiltinWorld 字段须同步"四处约束注记。

use oxide_types::mem::P;
use oxide_types::object::JsObject;

use crate::builtin::BuiltinWorld;
/// 供 reset 脏检查用的世代快照。
///
/// 维护注意：每个新增的 `BuiltinWorld` 对象字段都必须加到这里以及
/// `KernelSession::dirty_since_snapshot()`，以便选择性重置重建正确的
/// builtin 家族。
pub const NUM_BUILTINS: usize = 97;

/// 内置对象枚举 id，与 `BuiltinWorld` 中的存储槽一一对应。
///
/// 覆盖各构造器/原型、Error 家族（含 SuppressedError 原型）、集合类型、
/// TypedArray 家族与 well-known symbols；`repr(u8)` 使其可直接作为数组下标
/// （`u8` 值即下标）。
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
    SymAsyncDispose = 86,
    SymDispose = 87,
    SuppressedErrorProto = 88,
    Console = 89,
    PlainMonthDayConstructor = 90,
    PlainMonthDayProto = 91,
    PlainYearMonthConstructor = 92,
    PlainYearMonthProto = 93,
    SharedArrayBufferProto = 94,
    SharedArrayBufferConstructor = 95,
    AtomicsObject = 96,
}

impl BuiltinId {
    /// 全部内置对象的 id 常量表，供快照/脏检查按序遍历。
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
        BuiltinId::SymAsyncDispose,
        BuiltinId::SymDispose,
        BuiltinId::SuppressedErrorProto,
        BuiltinId::Console,
        BuiltinId::PlainMonthDayConstructor,
        BuiltinId::PlainMonthDayProto,
        BuiltinId::PlainYearMonthConstructor,
        BuiltinId::PlainYearMonthProto,
        BuiltinId::SharedArrayBufferProto,
        BuiltinId::SharedArrayBufferConstructor,
        BuiltinId::AtomicsObject,
    ];
}

// 编译期自检：ALL 必须与判别值 0..NUM_BUILTINS 严格同序（无换序、无重复、无遗漏）；
// 错位即快照/脏检查按下标误读，const 求值使错位直接成为编译错误。
const _: () = {
    let mut i = 0;
    while i < BuiltinId::ALL.len() {
        if BuiltinId::ALL[i] as usize != i {
            panic!("BuiltinId::ALL 顺序与判别值不一致");
        }
        i += 1;
    }
};

/// 内置对象世代（generation）快照：记录构造时各对象及其 stub 的世代号与数量。
///
/// 供 [`crate::kernel::KernelSession::dirty_since_snapshot`] 对比，判断哪些 builtin 家族在运行期被污染。
#[derive(Clone, Debug)]
pub struct BuiltinSnapshot {
    pub generations: [u32; NUM_BUILTINS],
    pub global_object_generation: u32,
    pub stub_objects_len: usize,
    pub stub_object_generations: Vec<u32>,
}

impl BuiltinSnapshot {
    pub(crate) fn gen(obj: &P<JsObject>) -> u32 {
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
    pub shared_array_buffer: bool,
    pub atomics: bool,
    pub data_view: bool,
    pub typed_array_family: bool,
    pub temporal: bool,
    pub stubs: bool,
    pub global: bool,
    pub console: bool,
}

impl BuiltinDirtySet {
    /// 全部家族位与 global 均置脏的集合。
    ///
    /// wrapper 本体被写无法按世代定位到具体家族（写的是 wrapper 自身而非所属
    /// P 对象），脏检测收缩为全量重建，保证全部复用键都经过失效流程、无旧
    /// wrapper 被复用回新原型。
    pub fn all_dirty() -> Self {
        Self {
            object: true,
            array: true,
            function: true,
            string: true,
            number: true,
            boolean: true,
            error_family: true,
            symbol_family: true,
            math: true,
            json: true,
            date: true,
            set: true,
            map: true,
            regexp: true,
            array_buffer: true,
            shared_array_buffer: true,
            atomics: true,
            data_view: true,
            typed_array_family: true,
            temporal: true,
            stubs: true,
            global: true,
            console: true,
        }
    }

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
            || self.shared_array_buffer
            || self.atomics
            || self.data_view
            || self.typed_array_family
            || self.temporal
            || self.stubs
            || self.console
    }

    /// 是否存在任何污染（builtin world 或 global object）。
    pub fn any(&self) -> bool {
        self.any_builtin_dirty() || self.global
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 槽对齐面：表尾追加 Atomics 单例后总槽数 97，既有 96 个判别值零位移，
    /// 快照数组随 NUM_BUILTINS 自动扩维、逐槽对齐。
    #[test]
    fn builtin_snapshot_all_slots_aligned() {
        assert_eq!(NUM_BUILTINS, 97);
        assert_eq!(BuiltinId::ALL.len(), NUM_BUILTINS);
        // 前 96 项判别值 0-95 逐项不变（尾追加零位移）。
        for i in 0..96usize {
            assert_eq!(BuiltinId::ALL[i] as usize, i);
        }
        assert_eq!(BuiltinId::ALL[94], BuiltinId::SharedArrayBufferProto);
        assert_eq!(BuiltinId::ALL[95], BuiltinId::SharedArrayBufferConstructor);
        assert_eq!(BuiltinId::ALL[96], BuiltinId::AtomicsObject);

        // 快照经 session 全量构造路径采集，generations 数组维度 = 槽数。
        use crate::kernel::{KernelConfig, KernelCore, KernelSession};
        let core = KernelCore::new(KernelConfig::minimal());
        let session = KernelSession::new(&core);
        assert_eq!(session.builtin_snapshot.generations.len(), NUM_BUILTINS);
    }
}
