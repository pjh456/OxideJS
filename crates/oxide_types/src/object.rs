//! 本模块是对象模型的门面与对象本体——`JsObject` 定长 header、类型标签、
//! 构造与 epoch/GC 位、堆外区生命周期与形状链接面；字符串、属性元数据、
//! 闭包 cell、原生函数指针与 TypedArray 种类已下沉子模块，经门面显式重导出。
//!
//! 子模块布局：
//! - `string`：字符串三形态与 rope（重导出 `JsString`、`ConsNode`）
//! - `prop_meta`：属性标志与元数据（重导出 `PropAttributes`、`PropMetaEntry`、`PropIndex`）
//! - `cell` / `native_fn` / `typed_array`：闭包 cell、原生函数指针、TypedArray
//!   种类（重导出 `Cell`、`NativeFnPtr`、`TypedArrayKind`）
//! - `props`：私有模块，`JsObject` 的属性槽读写 36 方法，无外部名称
//! - `tests`：`#[cfg(test)]` 对象测试
//!
//! 冻结公共面：外部可见名称为 11 名清单（8 名重导出 + 3 名原生定义
//! `JsObject` / `ShapeId` / `MAX_DENSE_PROPS`），显式名单防 `pub` 面无声扩大。
//!
//! 布局说明块位于 `JsObject` 定义上方（随结构体，不在模块头文档）；
//! 尺寸守护为 `object_size_bounds` 测试的 256B 上界，与布局块字段总字节数
//! 并存、不统一。

use crate::value::JsValue;

// —— 子模块声明与显式重导出：11 名冻结面，不用通配 ——
mod cell; // 闭包共享单元
mod native_fn; // 原生函数指针不透明包装
mod prop_meta; // 属性标志与元数据
mod props; // 属性槽读写（私有模块，无重导出）
mod string; // 字符串三形态与 rope
mod typed_array; // TypedArray 种类

pub use cell::Cell;
pub use native_fn::NativeFnPtr;
pub use prop_meta::{PropAttributes, PropIndex, PropMetaEntry};
pub use string::{ConsNode, JsString};
pub use typed_array::TypedArrayKind;

/// 形状标识符（对象 header 低位 24 位）。
pub type ShapeId = u32;

/// 属性存储索引的硬上限，防止索引失控导致内存膨胀。
pub const MAX_DENSE_PROPS: usize = 1_000_000;

/// 定长对象头 + 堆外数据指针的 JS 普通/外来对象。
///
/// 内联字段：`header`（shape_id + 一组标志位）、`type_tag`（外来对象种类）、
/// `proto`、`generation` 等；命名属性区（`hash_props`）、属性元数据、
/// native payload 与 upvalue cell 列表以裸指针挂在堆上，由 VM / GC 维护。对象可分配在
/// session arena（`Epoch`）或全局堆（`Arc`），通过
/// `is_session_epoch` 位区分。
///
/// 布局：
///   header: u32 位
///     \[0:23\]   shape_id
///     \[24\]     is_set
///     \[25\]     is_map
///     \[26\]     is_derived_constructor
///     \[27\]     is_class_constructor
///     \[28\]     is_arrow
///     \[29\]     is_array
///     \[30\]     is_extensible
///     \[31\]     is_function
///   native_arg_count: u8 (1 字节)
///   type_tag: u8 — 标识包装/外来对象种类的 OBJ_TYPE_* 常量 (1 字节)
///   is_session_epoch: u8 (1 字节)
///   _pad: u8 位
///     \[0\]     is_frozen
///     \[1\]     is_sealed
///     \[2\]     is_module_namespace
///     \[3\]     is_length_non_writable（数组 length 虚拟属性的数据属性 writable=false）
///   array_elements: *mut u8 (8 字节，数组对象元素区 Box\<Vec\<JsValue\>\>)
///   array_elements_meta: *mut u8 (8 字节，数组元素元数据 Box\<Vec\<Option\<PropMetaEntry\>\>\>)
///   hash_props: *mut u8 (8 字节，命名属性 Box\<Vec\<JsValue\>\>)
///   prop_meta: *mut u8 (8 字节，命名属性元数据 Box\<Vec\<Option\<PropMetaEntry\>\>\>)
///   native_data: *mut u8 (8 字节，VM 拥有的不透明 native/外来载荷)
///   proto: JsValue (8 字节)
///   generation: u32 (4 字节)
///   array_prop_count: u32 (4 字节，数组元素数，普通对象恒 0)
///   array_len_override: u32 (4 字节，逻辑长度覆盖，length 超 dense 上限时非 0)
///   native_fn: Option\<NativeFnPtr\> (16 字节 — 裸 `*const ()` 无法利用 Option\<NonNull\>
///              优化；因 repr(Rust) 布局规则存为包装 8 字节指针的 Option，带 8 字节
///              判别式填充)
///   sub_module_index: u32 (4 字节，子模块平表下标，与 table_gen 配对解析)
///   table_gen: u32 (4 字节，创建期所属子模块平表的表代际)
///   captured_this: JsValue (8 字节，箭头函数的词法 this)
///   home_object: JsValue (8 字节，\[\[HomeObject\]\]，供 super 查找)
///   boxed_value: JsValue (8 字节，装箱对象的被包基元载荷，见字段注释)
///   upvalues: *mut u8 (8 字节，指向闭包的 Box<Vec<*mut Cell>>)
///
///   字段自和：124 字节，另有 4 字节对齐填充（native_fn 之前）
///   总计：128 字节
///   对齐：8 字节
pub struct JsObject {
    header: u32,
    native_arg_count: u8,
    pub type_tag: u8,
    is_session_epoch: u8,
    _pad: u8,
    /// 数组元素区（仅数组对象）：`Vec<JsValue>`，`len == array_prop_count`。
    /// 经 `Box::into_raw` 创建（仅 `new_array` / `ensure_array_elements` 分配），
    /// 本对象持有期间有效，由 `release_raw_heap` 释放。
    array_elements: *mut u8,
    /// 数组元素元数据区（仅数组对象，懒分配）：`Vec<Option<PropMetaEntry>>`。
    /// 经 `Box::into_raw` 创建（仅 `ensure_array_elements_meta` 分配），
    /// 本对象持有期间有效，由 `release_raw_heap` 释放。
    array_elements_meta: *mut u8,
    /// 命名属性区（懒分配）：`Vec<JsValue>` 值向量。
    /// 经 `Box::into_raw` 创建（仅 `ensure_hash_props` 分配），
    /// 本对象持有期间有效，由 `release_raw_heap` 释放。
    hash_props: *mut u8,
    /// 命名属性元数据区（懒分配）：`Vec<Option<PropMetaEntry>>`。
    /// 经 `Box::into_raw` 创建（仅 `ensure_prop_meta` 分配），
    /// 本对象持有期间有效，由 `release_raw_heap` 释放。
    prop_meta: *mut u8,
    native_data: *mut u8,
    proto: JsValue,
    generation: u32,
    /// 数组元素数（数组对象）。普通对象恒 0。命名属性存储在 `hash_props`
    /// （与元素区分，JS 数组命名属性不影响 length）。
    pub array_prop_count: u32,
    /// 数组逻辑长度覆盖：仅当 length 超出 dense 存储上限 `MAX_DENSE_PROPS` 时非 0。
    /// `arr.length = 4294967295` 等超大长度时，元素区仍以 `MAX_DENSE_PROPS` 封顶，
    /// 超出部分视为稀疏空洞，逻辑长度单独记录供 `a.length` 读取。
    array_len_override: u32,
    native_fn: Option<NativeFnPtr>,
    sub_module_index: u32,
    /// 子模块平表的表代际：函数对象创建时所属的平表由该代际唯一定位，
    /// 调用期与 sub_module_index 配对解析（跨 run 换表后按创建期代际仍命中原表）。
    table_gen: u32,
    captured_this: JsValue,
    home_object: JsValue,
    /// 装箱基元对象（Number/String/Boolean/Symbol 盒与 BigInt 包装）的被包
    /// 基元载荷：构造期直接写入本字段，不占用命名属性区——属性区存储位与
    /// shape 槽位保持恒等映射，后续索引/命名属性写读不干扰被包值。
    /// 未装箱对象恒为 undefined。
    boxed_value: JsValue,
    /// RegExp 实例的 source 字符串（RegExp.prototype.source 访问器的数据源）。
    regexp_source: JsValue,
    /// RegExp 实例的 flags 字符串（RegExp.prototype.flags 访问器的数据源）。
    regexp_flags: JsValue,
    pub upvalues: *mut u8,
}

impl JsObject {
    /// 普通对象类型标签。
    pub const OBJ_TYPE_PLAIN: u8 = 0;
    /// Date 对象类型标签。
    pub const OBJ_TYPE_DATE: u8 = 1;
    /// RegExp 对象类型标签。
    pub const OBJ_TYPE_REGEXP: u8 = 2;
    /// 装箱 Boolean 对象类型标签。
    pub const OBJ_TYPE_BOOLEAN_OBJ: u8 = 3;
    /// 装箱 Number 对象类型标签。
    pub const OBJ_TYPE_NUMBER_OBJ: u8 = 4;
    /// 装箱 String 对象类型标签。
    pub const OBJ_TYPE_STRING_OBJ: u8 = 5;
    /// ArrayBuffer 对象类型标签。
    pub const OBJ_TYPE_ARRAY_BUFFER: u8 = 6;
    /// DataView 对象类型标签。
    pub const OBJ_TYPE_DATA_VIEW: u8 = 7;
    /// TypedArray 对象类型标签。
    pub const OBJ_TYPE_TYPED_ARRAY: u8 = 8;
    /// native 构造器（Set/Array/Object 等，可 [[Construct]]）；区别于不可构造的 native 方法。
    pub const OBJ_TYPE_CONSTRUCTOR: u8 = 9;
    /// 生成器迭代器对象（`function*` 调用返回）：状态快照存于 `native_data`。
    pub const OBJ_TYPE_GENERATOR: u8 = 10;
    /// Promise 对象：状态盒（Pending/Fulfilled/Rejected + reactions）存于 `native_data`。
    pub const OBJ_TYPE_PROMISE: u8 = 11;
    /// 异步函数执行上下文（隐藏对象）：`AsyncState` 快照存于 `native_data`。
    pub const OBJ_TYPE_ASYNC: u8 = 12;
    /// 异步生成器迭代器对象（`async function*` 调用返回）：`AsyncGeneratorState`
    /// 快照存于 `native_data`。
    pub const OBJ_TYPE_ASYNC_GENERATOR: u8 = 13;
    /// Temporal.Instant 对象：纪元纳秒存于 prop 0。
    pub const OBJ_TYPE_INSTANT: u8 = 14;
    /// Temporal.PlainDate 对象：ISO 年/月/日存于 prop 0-2。
    pub const OBJ_TYPE_PLAIN_DATE: u8 = 15;
    /// Temporal.PlainTime 对象：午夜后纳秒存于 prop 0。
    pub const OBJ_TYPE_PLAIN_TIME: u8 = 16;
    /// Temporal.Duration 对象：十个时长分量依次存于 prop 0-9。
    pub const OBJ_TYPE_DURATION: u8 = 17;
    /// Temporal.ZonedDateTime 对象：纪元纳秒、时区 ID、日历 ID 存于 prop 0-2。
    pub const OBJ_TYPE_ZONED_DATE_TIME: u8 = 18;
    /// Temporal.PlainDateTime 对象：ISO 年/月/日与午夜后纳秒存于 prop 0-3。
    pub const OBJ_TYPE_PLAIN_DATE_TIME: u8 = 19;
    /// 装箱 Symbol 对象类型标签。
    pub const OBJ_TYPE_SYMBOL_OBJ: u8 = 21;
    /// Arguments 对象：`Object.prototype.toString` 返回 `[object Arguments]`。
    pub const OBJ_TYPE_ARGUMENTS: u8 = 20;
    /// Error 家族对象（Error 及 NativeError 实例）：`Object.prototype.toString` 返回
    /// `[object Error]`，与 [[ErrorData]] 内部槽对应。
    pub const OBJ_TYPE_ERROR: u8 = 22;
    /// bound 函数包装（`Function.prototype.bind` 产物）：[[BoundTargetFunction]] /
    /// [[BoundThis]] / [[BoundArguments]] 存于 dense 槽 4/5/6+，构造与 instanceof
    /// 语义转发到 target。
    pub const OBJ_TYPE_BOUND: u8 = 23;
    /// DisposableStack 对象：状态盒（Pending/Disposed + entries）存于 `native_data`。
    pub const OBJ_TYPE_DISPOSABLE_STACK: u8 = 24;
    /// AsyncDisposableStack 对象（异步资源栈）：状态盒与同步栈同构，预留 type_tag。
    pub const OBJ_TYPE_ASYNC_DISPOSABLE_STACK: u8 = 25;
    /// Temporal.PlainMonthDay 对象：月/日/参考年/日历 ID 存于 prop 0-3。
    pub const OBJ_TYPE_PLAIN_MONTH_DAY: u8 = 26;
    /// Temporal.PlainYearMonth 对象：年/月/参考日/日历 ID 存于 prop 0-3。
    pub const OBJ_TYPE_PLAIN_YEAR_MONTH: u8 = 27;
    /// matchAll 迭代器内部载体对象：仅持有编译后的 `regress::Regex`（存于
    /// `native_fn` 槽），无 RegExp 可见属性面；`native_fn` 的释放与深拷贝
    /// 守卫与 RegExp 对象一致。
    pub const OBJ_TYPE_REGEX_STUB: u8 = 28;
    /// [[IsHTMLDDA]] 宿主对象（test262 `$262.IsHTMLDDA`）：ToBoolean、typeof
    /// 与宽松相等按 undefined 处理，Type 与其余强制转换按普通对象处理；无额外
    /// 内部载荷。
    pub const OBJ_TYPE_HTML_DDA: u8 = 29;
    /// SharedArrayBuffer 对象：与 ArrayBuffer 同构的字节载荷盒（`native_fn`
    /// 槽存 `ArrayBufferPayload`），但无 `[[ArrayBufferMaxByteLength]]`/定长
    /// 语义面；接收者品牌守卫按本 tag 拒绝，ArrayBuffer 方法族对其抛
    /// TypeError。释放与深拷贝守卫同 ArrayBuffer。
    pub const OBJ_TYPE_SHARED_ARRAY_BUFFER: u8 = 30;
    /// WeakMap 对象：条目表（弱键 → 强值）存于 `native_data`。键为弱边
    /// （GC mark 不产边），值为强边。
    pub const OBJ_TYPE_WEAK_MAP: u8 = 31;
    /// `is_session_epoch` 字段中的 session 标记位。
    pub const SESSION_EPOCH_BIT: u8 = 0x01;
    /// `is_session_epoch` 字段中的 GC 标记位。
    pub const GC_MARK_BIT: u8 = 0x02;
    /// `is_session_epoch` 字段中的 epoch 归属标记位。
    pub const EPOCH_BIT: u8 = 0x04;

    /// 是否 Date 外来对象。
    #[inline]
    pub fn is_date_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DATE
    }
    /// 是否 RegExp 外来对象。
    #[inline]
    pub fn is_regexp_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_REGEXP
    }
    /// 是否 matchAll 迭代器内部载体（`native_fn` 槽存编译后的 `regress::Regex`）。
    #[inline]
    pub fn is_regex_stub_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_REGEX_STUB
    }
    /// 是否持有编译正则：`native_fn` 槽存 `Box<regress::Regex>` 的两种对象形态
    /// （RegExp 对象 / matchAll 载体）——其 `native_fn` 槽的释放、深拷贝与字节
    /// 核算均由该谓词判定。
    #[inline]
    pub fn holds_compiled_regex(&self) -> bool {
        matches!(self.type_tag, Self::OBJ_TYPE_REGEXP | Self::OBJ_TYPE_REGEX_STUB)
    }
    /// 是否 [[IsHTMLDDA]] 宿主对象（ToBoolean/typeof/宽松相等按 undefined 处理）。
    #[inline]
    pub fn is_html_dda_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_HTML_DDA
    }
    /// 是否装箱 Boolean 对象。
    #[inline]
    pub fn is_boolean_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_BOOLEAN_OBJ
    }
    /// 是否装箱 Number 对象。
    #[inline]
    pub fn is_number_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_NUMBER_OBJ
    }
    /// 是否装箱 String 对象。
    #[inline]
    pub fn is_string_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_STRING_OBJ
    }

    /// 是否装箱 Symbol 对象。
    #[inline]
    pub fn is_symbol_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_SYMBOL_OBJ
    }
    /// 是否 ArrayBuffer 对象。
    #[inline]
    pub fn is_array_buffer_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ARRAY_BUFFER
    }
    /// 是否 SharedArrayBuffer 对象。
    #[inline]
    pub fn is_shared_array_buffer_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_SHARED_ARRAY_BUFFER
    }
    /// 是否 WeakMap 对象（条目表存于 `native_data`）。
    #[inline]
    pub fn is_weak_map_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_WEAK_MAP
    }
    /// 是否 DataView 对象。
    #[inline]
    pub fn is_data_view_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DATA_VIEW
    }
    /// 是否 TypedArray 对象。
    #[inline]
    pub fn is_typed_array_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_TYPED_ARRAY
    }
    /// 是否生成器迭代器对象。
    #[inline]
    pub fn is_generator_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_GENERATOR
    }
    /// 是否 Promise 对象。
    #[inline]
    pub fn is_promise_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PROMISE
    }
    /// 是否异步函数执行上下文对象。
    #[inline]
    pub fn is_async_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ASYNC
    }
    /// 是否异步生成器迭代器对象。
    #[inline]
    pub fn is_async_generator_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ASYNC_GENERATOR
    }
    /// 是否 Temporal.Instant 对象。
    #[inline]
    pub fn is_instant_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_INSTANT
    }
    /// 是否 Temporal.PlainDate 对象。
    #[inline]
    pub fn is_plain_date_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_DATE
    }
    /// 是否 Temporal.PlainTime 对象。
    #[inline]
    pub fn is_plain_time_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_TIME
    }
    /// 是否 Temporal.Duration 对象。
    #[inline]
    pub fn is_duration_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DURATION
    }
    /// 是否 Temporal.ZonedDateTime 对象。
    #[inline]
    pub fn is_zoned_date_time_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ZONED_DATE_TIME
    }
    /// 是否 Temporal.PlainDateTime 对象。
    #[inline]
    pub fn is_plain_date_time_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_DATE_TIME
    }
    /// 是否 Temporal.PlainMonthDay 对象。
    #[inline]
    pub fn is_plain_month_day_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_MONTH_DAY
    }
    /// 是否 Temporal.PlainYearMonth 对象。
    #[inline]
    pub fn is_plain_year_month_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_PLAIN_YEAR_MONTH
    }

    /// 是否 Arguments 对象（`type_tag` 为 `OBJ_TYPE_ARGUMENTS`）。
    #[inline]
    pub fn is_arguments_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ARGUMENTS
    }

    /// 是否 Error 家族对象（含 NativeError 实例）。
    #[inline]
    pub fn is_error_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ERROR
    }

    /// 是否 DisposableStack 对象（同步资源栈）。
    #[inline]
    pub fn is_disposable_stack_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_DISPOSABLE_STACK
    }

    /// 是否 AsyncDisposableStack 对象（异步资源栈）。
    #[inline]
    pub fn is_async_disposable_stack_obj(&self) -> bool {
        self.type_tag == Self::OBJ_TYPE_ASYNC_DISPOSABLE_STACK
    }

    /// 构造无属性、可扩展的空对象（`new Object()` 的基础对象）。
    pub fn new_empty(shape_id: ShapeId, proto: JsValue) -> Self {
        Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30),
            native_arg_count: 0,
            type_tag: 0,
            is_session_epoch: 0,
            _pad: 0,
            array_elements: std::ptr::null_mut(),
            array_elements_meta: std::ptr::null_mut(),
            hash_props: std::ptr::null_mut(),
            prop_meta: std::ptr::null_mut(),
            native_data: std::ptr::null_mut(),
            proto,
            generation: 1,
            array_prop_count: 0,
            array_len_override: 0,
            native_fn: None,
            sub_module_index: 0,
            table_gen: 0,
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
            boxed_value: JsValue::undefined(),
            regexp_source: JsValue::undefined(),
            regexp_flags: JsValue::undefined(),
            upvalues: std::ptr::null_mut(),
        }
    }

    /// 构造数组对象：预分配 `n_elements` 个 `undefined` 的独立元素区并置 array 标志。
    pub fn new_array(shape_id: ShapeId, proto: JsValue, n_elements: usize, _bump: &bumpalo::Bump) -> Self {
        let mut obj = Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30) | (1 << 29),
            native_arg_count: 0,
            type_tag: 0,
            is_session_epoch: 0,
            _pad: 0,
            array_elements: std::ptr::null_mut(),
            array_elements_meta: std::ptr::null_mut(),
            hash_props: std::ptr::null_mut(),
            prop_meta: std::ptr::null_mut(),
            native_data: std::ptr::null_mut(),
            proto,
            generation: 1,
            array_prop_count: 0,
            array_len_override: 0,
            native_fn: None,
            sub_module_index: 0,
            table_gen: 0,
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
            boxed_value: JsValue::undefined(),
            regexp_source: JsValue::undefined(),
            regexp_flags: JsValue::undefined(),
            upvalues: std::ptr::null_mut(),
        };
        let vec = Box::new(vec![JsValue::undefined(); n_elements.min(MAX_DENSE_PROPS)]);
        obj.array_elements = Box::into_raw(vec) as *mut u8;
        obj.array_prop_count = n_elements.min(MAX_DENSE_PROPS) as u32;
        obj.array_len_override = if n_elements > MAX_DENSE_PROPS { n_elements as u32 } else { 0 };
        obj
    }

    /// 是否 session-epoch 分配（调用期内分配，调用结束即失效）。
    #[inline]
    pub fn is_session_epoch(&self) -> bool {
        self.is_session_epoch & Self::SESSION_EPOCH_BIT != 0
    }

    /// 设置 / 清除 session-epoch 标记。
    #[inline]
    pub fn set_session_epoch(&mut self, value: bool) {
        if value {
            self.is_session_epoch |= Self::SESSION_EPOCH_BIT;
        } else {
            self.is_session_epoch &= !Self::SESSION_EPOCH_BIT;
        }
    }

    /// 是否被 GC 标记。
    #[inline]
    pub fn is_gc_marked(&self) -> bool {
        self.is_session_epoch & Self::GC_MARK_BIT != 0
    }

    /// 设置 / 清除 GC 标记。
    #[inline]
    pub fn set_gc_mark(&mut self, marked: bool) {
        if marked {
            self.is_session_epoch |= Self::GC_MARK_BIT;
        } else {
            self.is_session_epoch &= !Self::GC_MARK_BIT;
        }
    }

    /// 是否为 epoch 分配（非 session）的对象。
    #[inline]
    pub fn is_epoch(&self) -> bool {
        self.is_session_epoch & Self::EPOCH_BIT != 0
    }

    /// 设置 / 清除 epoch 归属标记。
    #[inline]
    pub fn set_is_epoch(&mut self, value: bool) {
        if value {
            self.is_session_epoch |= Self::EPOCH_BIT;
        } else {
            self.is_session_epoch &= !Self::EPOCH_BIT;
        }
    }

    /// 深拷贝本对象到 session epoch：复制属性向量与元数据，标记为新 session 对象。
    ///
    /// 用于把持久对象快照进当前调用上下文，修改不反向传播到源对象。
    pub fn clone_for_session_epoch(&self) -> Self {
        let hash_props = self
            .hash_props_vec()
            .map(|props| Box::into_raw(Box::new(props.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let prop_meta = self
            .prop_meta_vec()
            .map(|meta| Box::into_raw(Box::new(meta.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let array_elements = self
            .array_elements_vec()
            .map(|elems| Box::into_raw(Box::new(elems.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let array_elements_meta = self
            .array_elements_meta_vec()
            .map(|meta| Box::into_raw(Box::new(meta.clone())) as *mut u8)
            .unwrap_or(std::ptr::null_mut());

        Self {
            header: self.header,
            native_arg_count: self.native_arg_count,
            type_tag: self.type_tag,
            is_session_epoch: Self::SESSION_EPOCH_BIT,
            _pad: self._pad,
            array_elements,
            array_elements_meta,
            hash_props,
            prop_meta,
            native_data: self.native_data,
            proto: self.proto,
            generation: self.generation,
            array_prop_count: self.array_prop_count,
            array_len_override: self.array_len_override,
            native_fn: self.native_fn,
            sub_module_index: self.sub_module_index,
            table_gen: self.table_gen,
            captured_this: self.captured_this,
            home_object: self.home_object,
            boxed_value: self.boxed_value,
            regexp_source: self.regexp_source,
            regexp_flags: self.regexp_flags,
            upvalues: self.upvalues,
        }
    }

    /// 命名属性区底层指针（未分配时为空指针）。
    pub fn hash_props_raw(&self) -> *mut u8 {
        self.hash_props
    }

    /// 数组元素区底层指针（未分配时为空指针）。
    pub fn array_elements_raw(&self) -> *mut u8 {
        self.array_elements
    }

    /// 数组元素元数据区底层指针（未分配时为空指针）。
    pub fn array_elements_meta_raw(&self) -> *mut u8 {
        self.array_elements_meta
    }

    /// 属性元数据向量底层指针（未分配时为空指针）。
    pub fn prop_meta_raw(&self) -> *mut u8 {
        self.prop_meta
    }

    /// 释放四处堆外裸指针区（命名属性区、命名属性元数据区、数组元素区、
    /// 数组元素元数据区）并置空。
    ///
    /// 供 session 收尾调用：对象本体可能仍被 Arc 引用（此后属性区不再被读取），
    /// 每区至多释放一次、重复调用为 no-op。upvalue 列表不在本函数释放范围内
    /// （原件与晋升克隆间别名，须收尾时去重统一释放）。
    pub fn release_raw_heap(&mut self) {
        // SAFETY: 四个区指针归本对象所有（见字段声明）；回收后即刻置空。
        unsafe {
            if !self.array_elements.is_null() {
                drop(Box::from_raw(self.array_elements as *mut Vec<JsValue>));
                self.array_elements = std::ptr::null_mut();
            }
            if !self.array_elements_meta.is_null() {
                drop(Box::from_raw(self.array_elements_meta as *mut Vec<Option<PropMetaEntry>>));
                self.array_elements_meta = std::ptr::null_mut();
            }
            if !self.hash_props.is_null() {
                drop(Box::from_raw(self.hash_props as *mut Vec<JsValue>));
                self.hash_props = std::ptr::null_mut();
            }
            if !self.prop_meta.is_null() {
                drop(Box::from_raw(self.prop_meta as *mut Vec<Option<PropMetaEntry>>));
                self.prop_meta = std::ptr::null_mut();
            }
        }
    }

    /// 原生 / 外来对象 payload 指针。
    pub fn native_data(&self) -> *mut u8 {
        self.native_data
    }

    /// 设置原生 / 外来对象 payload 指针。
    pub fn set_native_data(&mut self, ptr: *mut u8) {
        self.native_data = ptr;
    }

    /// 闭包 upvalue cell 列表切片（未设置时为空切片）。
    pub fn upvalues_slice(&self) -> &[*mut Cell] {
        if self.upvalues.is_null() {
            &[]
        } else {
            unsafe { &*(self.upvalues as *const Vec<*mut Cell>) }
        }
    }

    /// 可变 upvalue cell 列表切片（未设置时为空切片）。
    pub fn upvalues_slice_mut(&mut self) -> &mut [*mut Cell] {
        if self.upvalues.is_null() {
            &mut []
        } else {
            unsafe { &mut *(self.upvalues as *mut Vec<*mut Cell>) }
        }
    }

    /// 替换 upvalue cell 列表（释放旧列表）。
    pub fn set_upvalues(&mut self, v: Box<Vec<*mut Cell>>) {
        if !self.upvalues.is_null() {
            unsafe {
                drop(Box::from_raw(self.upvalues as *mut Vec<*mut Cell>));
            }
        }
        self.upvalues = Box::into_raw(v) as *mut u8;
    }

    /// 用 `rewrite` 改写对象内引用的所有对象值。
    ///
    /// 用于 GC 移动 / 世代晋升：遍历数组元素区、命名属性区、访问器 getter/setter、
    /// `proto`、`captured_this`、`home_object` 与 upvalue cell 中的对象值，
    /// 原地替换为新地址。非对象值保持不变。
    pub fn rewrite_object_values<F>(&mut self, mut rewrite: F)
    where
        F: FnMut(JsValue) -> JsValue,
    {
        if let Some(elements) = self.array_elements_vec_mut() {
            for value in elements {
                if value.is_object() {
                    *value = rewrite(*value);
                }
            }
        }
        if let Some(meta) = self.array_elements_meta_vec_mut() {
            for entry in meta.iter_mut().flatten() {
                if entry.get.is_object() {
                    entry.get = rewrite(entry.get);
                }
                if entry.set.is_object() {
                    entry.set = rewrite(entry.set);
                }
            }
        }
        if let Some(props) = self.hash_props_vec_mut() {
            for value in props {
                if value.is_object() {
                    *value = rewrite(*value);
                }
            }
        }
        if let Some(meta) = self.prop_meta_vec_mut() {
            for entry in meta.iter_mut().flatten() {
                if entry.get.is_object() {
                    entry.get = rewrite(entry.get);
                }
                if entry.set.is_object() {
                    entry.set = rewrite(entry.set);
                }
            }
        }
        if self.proto.is_object() {
            self.proto = rewrite(self.proto);
        }
        if self.captured_this.is_object() {
            self.captured_this = rewrite(self.captured_this);
        }
        if self.home_object.is_object() {
            self.home_object = rewrite(self.home_object);
        }
        if self.boxed_value.is_object() {
            self.boxed_value = rewrite(self.boxed_value);
        }
        if self.regexp_source.is_object() {
            self.regexp_source = rewrite(self.regexp_source);
        }
        if self.regexp_flags.is_object() {
            self.regexp_flags = rewrite(self.regexp_flags);
        }
        if !self.upvalues.is_null() {
            let cells = unsafe { &mut *(self.upvalues as *mut Vec<*mut Cell>) };
            for cell_ptr in cells {
                let cell = unsafe { &mut **cell_ptr };
                if cell.value.is_object() {
                    cell.value = rewrite(cell.value);
                }
            }
        }
    }

    /// 当前形状 ID（header 低位 24 位）。
    pub fn shape_id(&self) -> ShapeId {
        self.header & 0x00FF_FFFF
    }

    /// 设置形状 ID（低位 24 位，保留其它标志位）。
    pub fn set_shape_id(&mut self, id: ShapeId) {
        self.header = (self.header & !0x00FF_FFFF) | (id & 0x00FF_FFFF);
    }

    /// 是否数组（header bit 29）。
    pub fn is_array(&self) -> bool {
        (self.header >> 29) & 1 != 0
    }

    /// 是否可扩展（header bit 30，与 `Object.preventExtensions` 对应）。
    pub fn is_extensible(&self) -> bool {
        (self.header >> 30) & 1 != 0
    }

    /// 设置可扩展标志。
    pub fn set_extensible(&mut self, ext: bool) {
        if ext {
            self.header |= 1 << 30;
        } else {
            self.header &= !(1 << 30);
        }
    }

    /// 是否冻结（`Object.freeze`，存在访问器/描述符时仍由属性级 configurable 约束）。
    pub fn is_frozen(&self) -> bool {
        (self._pad) & 1 != 0
    }

    /// 设置冻结标志。
    pub fn set_frozen(&mut self, frozen: bool) {
        if frozen {
            self._pad |= 1 << 0;
        } else {
            self._pad &= !(1 << 0);
        }
    }

    /// 是否密封（`Object.seal`）。
    pub fn is_sealed(&self) -> bool {
        (self._pad >> 1) & 1 != 0
    }

    /// 设置密封标志。
    pub fn set_sealed(&mut self, sealed: bool) {
        if sealed {
            self._pad |= 1 << 1;
        } else {
            self._pad &= !(1 << 1);
        }
    }

    /// 是否模块命名空间 exotic 对象（`[[Set]]`/`[[DefineOwnProperty]]` 有专属语义）。
    pub fn is_module_namespace(&self) -> bool {
        (self._pad >> 2) & 1 != 0
    }

    /// 设置模块命名空间 exotic 标志。
    pub fn set_module_namespace(&mut self, value: bool) {
        if value {
            self._pad |= 1 << 2;
        } else {
            self._pad &= !(1 << 2);
        }
    }

    /// 数组 length 虚拟属性是否可写。
    ///
    /// # 边界与前提
    /// - 仅对数组对象有语义；冻结数组（`Object.freeze`）恒返回 `false`，不依赖独立位。
    /// - 独立位只记录 `defineProperty` 显式设置 `writable:false` 的收窄。
    pub fn is_length_writable(&self) -> bool {
        !self.is_frozen() && (self._pad >> 3) & 1 == 0
    }

    /// 设置数组 length 虚拟属性的不可写标志。
    ///
    /// # 副作用
    /// - 写 `_pad` bit3；`Clone`/晋升复制 `_pad`，标志随之保留。
    pub fn set_length_non_writable(&mut self, non_writable: bool) {
        if non_writable {
            self._pad |= 1 << 3;
        } else {
            self._pad &= !(1 << 3);
        }
    }

    /// 是否 Set 实例（header bit 24）。
    pub fn is_set(&self) -> bool {
        (self.header >> 24) & 1 != 0
    }

    /// 设置 Set 实例标志。
    pub fn set_set(&mut self, s: bool) {
        if s {
            self.header |= 1 << 24;
        } else {
            self.header &= !(1 << 24);
        }
    }

    /// 是否 Map 实例（header bit 25）。
    pub fn is_map(&self) -> bool {
        (self.header >> 25) & 1 != 0
    }

    /// 设置 Map 实例标志。
    pub fn set_map(&mut self, m: bool) {
        if m {
            self.header |= 1 << 25;
        } else {
            self.header &= !(1 << 25);
        }
    }

    /// 是否函数对象（header bit 31）。
    pub fn is_function(&self) -> bool {
        (self.header >> 31) & 1 != 0
    }

    /// 设置函数对象标志。
    pub fn set_function(&mut self, f: bool) {
        if f {
            self.header |= 1 << 31;
        } else {
            self.header &= !(1 << 31);
        }
    }

    /// 当前 `[[Prototype]]` 值。
    pub fn proto(&self) -> JsValue {
        self.proto
    }

    /// 设置 `[[Prototype]]`，成功时递增 generation。
    ///
    /// 仅接受 `null` 或对象；沿新原型链检查是否构成环，成环返回
    /// `Err("cyclic __proto__ value")`。
    ///
    /// # 注意事项
    /// 本方法是裸写槽操作：既不检查 `[[Extensible]]`，也不比较新旧原型是否
    /// SameValue。实现 `[[SetPrototypeOf]]` 的调用方须先自行判定「新旧相同直接
    /// 成功」「不可扩展且新旧不同则失败」，再调用本方法；原型接线、世界重建等
    /// 内部路径不受可扩展标志约束，直接调用即可。
    pub fn set_proto(&mut self, proto: JsValue) -> Result<(), &'static str> {
        if proto.is_null() {
            self.proto = proto;
            self.generation = self.generation.wrapping_add(1);
            return Ok(());
        }
        if !proto.is_object() {
            return Err("__proto__ must be an object or null");
        }
        let mut cursor = proto;
        let self_ptr = self as *const JsObject;
        while cursor.is_object() {
            let cursor_ptr = cursor.as_js_object_ptr();
            if std::ptr::eq(cursor_ptr, self_ptr) {
                return Err("cyclic __proto__ value");
            }
            debug_assert!(!cursor_ptr.is_null(), "prototype cursor pointer must not be null");
            // SAFETY: cursor 已知为对象 JsValue，编码的是合法 JsObject 指针。
            let obj = unsafe { &*cursor_ptr };
            cursor = obj.proto;
        }
        self.proto = proto;
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }

    /// 当前 generation（对象结构/值变更计数器，脏检测与并发模板裁决依据）。
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// 递增 generation（`wrapping_add`）。新增属性、改写既有属性值与原型接线均调用，
    /// 快照仅做等值比较，数值大小无语义。
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// 关联的原生函数指针（若为内建/原生函数对象）。
    pub fn native_fn(&self) -> Option<NativeFnPtr> {
        self.native_fn
    }

    /// 设置 / 清除原生函数指针。
    pub fn set_native_fn(&mut self, ptr: Option<NativeFnPtr>) {
        self.native_fn = ptr;
    }

    /// 原生函数声明的参数个数。
    pub fn native_arg_count(&self) -> u8 {
        self.native_arg_count
    }

    /// 设置原生函数参数个数。
    pub fn set_native_arg_count(&mut self, n: u8) {
        self.native_arg_count = n;
    }

    /// 子模块下标（函数对象对应的 `CompiledModule` 在创建期平表中的下标；
    /// 0 = 原生函数哨兵）。
    pub fn sub_module_index(&self) -> u32 {
        self.sub_module_index
    }

    /// 设置子模块下标。
    pub fn set_sub_module_index(&mut self, idx: u32) {
        self.sub_module_index = idx;
    }

    /// 创建期所属子模块平表的表代际（与 sub_module_index 配对解析出唯一模块）。
    pub fn table_gen(&self) -> u32 {
        self.table_gen
    }

    /// 设置创建期表代际。
    pub fn set_table_gen(&mut self, gen: u32) {
        self.table_gen = gen;
    }

    /// 箭头函数标志（header bit 28）。
    /// 为 true 时，CALL 分发捕获创建时的词法 `this`。
    pub fn is_arrow(&self) -> bool {
        (self.header >> 28) & 1 != 0
    }

    /// 设置箭头函数标志（header bit 28）。
    pub fn set_arrow(&mut self, v: bool) {
        if v {
            self.header |= 1 << 28;
        } else {
            self.header &= !(1 << 28);
        }
    }

    /// 箭头函数创建时捕获的词法 `this`。
    /// 仅在 `is_arrow()` 返回 true 时有意义。
    pub fn captured_this(&self) -> JsValue {
        self.captured_this
    }

    /// 设置箭头函数捕获的词法 `this`。
    pub fn set_captured_this(&mut self, v: JsValue) {
        self.captured_this = v;
    }

    /// 类构造函数标志（header bit 27）。
    /// 普通 CALL 拒绝带此标志的对象；NEW_EXPRESSION 允许。
    pub fn is_class_constructor(&self) -> bool {
        (self.header >> 27) & 1 != 0
    }

    /// 设置类构造函数标志（header bit 27）。
    pub fn set_class_constructor(&mut self, v: bool) {
        if v {
            self.header |= 1 << 27;
        } else {
            self.header &= !(1 << 27);
        }
    }

    /// 是否派生类构造函数（有 `extends` 子句，header bit 26）。
    pub fn is_derived_constructor(&self) -> bool {
        (self.header >> 26) & 1 != 0
    }

    /// 设置派生类构造函数标志（header bit 26）。
    pub fn set_derived_constructor(&mut self, v: bool) {
        if v {
            self.header |= 1 << 26;
        } else {
            self.header &= !(1 << 26);
        }
    }

    /// 当前 `[[HomeObject]]`（供 `super` 属性访问解析）。
    pub fn home_object(&self) -> JsValue {
        self.home_object
    }

    /// 设置 `[[HomeObject]]`。
    pub fn set_home_object(&mut self, v: JsValue) {
        self.home_object = v;
    }

    /// 装箱对象的被包基元载荷（未装箱对象为 undefined）。
    pub fn boxed_value(&self) -> JsValue {
        self.boxed_value
    }

    /// 设置装箱对象的被包基元载荷。
    ///
    /// # 注意事项
    /// 仅供装箱构造点调用：写入前对象须尚未进入属性读写路径，
    /// 载荷不参与 shape/属性区，写后不得再经属性区预存同一值。
    pub fn set_boxed_value(&mut self, v: JsValue) {
        self.boxed_value = v;
    }

    /// RegExp 实例的 source 字符串（非 RegExp 实例恒为 undefined）。
    pub fn get_regexp_source(&self) -> JsValue {
        self.regexp_source
    }

    /// 设置 RegExp 实例的 source 字符串。
    pub fn set_regexp_source(&mut self, v: JsValue) {
        self.regexp_source = v;
    }

    /// RegExp 实例的 flags 字符串（非 RegExp 实例恒为 undefined）。
    pub fn get_regexp_flags(&self) -> JsValue {
        self.regexp_flags
    }

    /// 设置 RegExp 实例的 flags 字符串。
    pub fn set_regexp_flags(&mut self, v: JsValue) {
        self.regexp_flags = v;
    }
}

#[cfg(test)]
mod tests;
