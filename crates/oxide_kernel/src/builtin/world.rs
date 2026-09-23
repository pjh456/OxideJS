//! 注册表职责：BuiltinWorld 结构（106 固定 P 字段 + stub 族 + Box::into_raw
//! 登记表）、get_by_id 派发、all_p_fields 104 元组枚举（新增 P 字段四处同步
//! 约束载体）与登记表 track/find/inherit/teardown。

use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::kernel::BuiltinId;

/// 全部内置对象（原型、构造器、全局单例 Math/JSON、well-known symbol 与 stub 对象）的持有者。
///
/// 每个 session 独立持有自己的 `BuiltinWorld`，保证 session 间内置对象隔离；
/// 由 [`BuiltinWorld::new`] 全量构造，或 [`BuiltinWorld::rebuild_with_dirty`] 按脏标记部分重建。
pub struct BuiltinWorld {
    pub object_proto: P<JsObject>,
    pub array_proto: P<JsObject>,
    pub function_proto: P<JsObject>,
    pub string_proto: P<JsObject>,
    pub number_proto: P<JsObject>,
    pub boolean_proto: P<JsObject>,
    pub error_proto: P<JsObject>,
    pub symbol_proto: P<JsObject>,
    pub object_constructor: P<JsObject>,
    pub array_constructor: P<JsObject>,
    pub function_constructor: P<JsObject>,
    pub string_constructor: P<JsObject>,
    pub number_constructor: P<JsObject>,
    pub boolean_constructor: P<JsObject>,
    pub error_constructor: P<JsObject>,
    pub symbol_constructor: P<JsObject>,
    pub type_error_proto: P<JsObject>,
    pub reference_error_proto: P<JsObject>,
    pub range_error_proto: P<JsObject>,
    pub syntax_error_proto: P<JsObject>,
    pub uri_error_proto: P<JsObject>,
    pub eval_error_proto: P<JsObject>,
    pub suppressed_error_proto: P<JsObject>,
    pub math_object: P<JsObject>,
    pub json_object: P<JsObject>,
    pub date_constructor: P<JsObject>,
    pub date_proto: P<JsObject>,
    pub set_constructor: P<JsObject>,
    pub set_proto: P<JsObject>,
    pub map_constructor: P<JsObject>,
    pub map_proto: P<JsObject>,
    pub regexp_constructor: P<JsObject>,
    pub regexp_proto: P<JsObject>,
    pub array_buffer_constructor: P<JsObject>,
    pub array_buffer_proto: P<JsObject>,
    /// `SharedArrayBuffer.prototype` 与 `SharedArrayBuffer` 构造器：经
    /// `make_named_pair` 成对构造，方法/访问器由绑定层安装。
    pub shared_array_buffer_proto: P<JsObject>,
    pub shared_array_buffer_constructor: P<JsObject>,
    pub data_view_constructor: P<JsObject>,
    pub data_view_proto: P<JsObject>,
    pub typed_array_proto: P<JsObject>,
    pub typed_array_constructor: P<JsObject>,
    pub int8array_constructor: P<JsObject>,
    pub int8array_proto: P<JsObject>,
    pub uint8array_constructor: P<JsObject>,
    pub uint8array_proto: P<JsObject>,
    pub uint8clampedarray_constructor: P<JsObject>,
    pub uint8clampedarray_proto: P<JsObject>,
    pub int16array_constructor: P<JsObject>,
    pub int16array_proto: P<JsObject>,
    pub uint16array_constructor: P<JsObject>,
    pub uint16array_proto: P<JsObject>,
    pub int32array_constructor: P<JsObject>,
    pub int32array_proto: P<JsObject>,
    pub uint32array_constructor: P<JsObject>,
    pub uint32array_proto: P<JsObject>,
    pub float32array_constructor: P<JsObject>,
    pub float32array_proto: P<JsObject>,
    pub float64array_constructor: P<JsObject>,
    pub float64array_proto: P<JsObject>,
    pub bigint64array_constructor: P<JsObject>,
    pub bigint64array_proto: P<JsObject>,
    pub biguint64array_constructor: P<JsObject>,
    pub biguint64array_proto: P<JsObject>,
    pub sym_match: P<JsObject>,
    pub sym_replace: P<JsObject>,
    pub sym_search: P<JsObject>,
    pub sym_split: P<JsObject>,
    pub sym_iterator: P<JsObject>,
    pub sym_to_primitive: P<JsObject>,
    pub sym_has_instance: P<JsObject>,
    pub sym_match_all: P<JsObject>,
    pub sym_async_iterator: P<JsObject>,
    pub sym_to_string_tag: P<JsObject>,
    pub sym_species: P<JsObject>,
    pub sym_async_dispose: P<JsObject>,
    pub sym_dispose: P<JsObject>,
    pub temporal_object: P<JsObject>,
    pub temporal_now_object: P<JsObject>,
    pub instant_constructor: P<JsObject>,
    pub instant_proto: P<JsObject>,
    pub plain_date_constructor: P<JsObject>,
    pub plain_date_proto: P<JsObject>,
    pub plain_time_constructor: P<JsObject>,
    pub plain_time_proto: P<JsObject>,
    pub duration_constructor: P<JsObject>,
    pub duration_proto: P<JsObject>,
    pub zoned_date_time_constructor: P<JsObject>,
    pub zoned_date_time_proto: P<JsObject>,
    pub plain_date_time_constructor: P<JsObject>,
    pub plain_date_time_proto: P<JsObject>,
    pub plain_month_day_constructor: P<JsObject>,
    pub plain_month_day_proto: P<JsObject>,
    pub plain_year_month_constructor: P<JsObject>,
    pub plain_year_month_proto: P<JsObject>,
    pub bigint_constructor: P<JsObject>,
    pub bigint_proto: P<JsObject>,
    /// `%IteratorPrototype%`：各集合迭代器原型的公共祖先，持有 `@@iterator`（返回自身）。
    pub iterator_proto: P<JsObject>,
    /// `%ArrayIteratorPrototype%`：Array 与 TypedArray 迭代器共享（`next` 挂其上）。
    pub array_iterator_proto: P<JsObject>,
    /// `%MapIteratorPrototype%`：Map 的 values/keys/entries 迭代器共享。
    pub map_iterator_proto: P<JsObject>,
    /// `%SetIteratorPrototype%`：Set 的 values/keys/entries 迭代器共享。
    pub set_iterator_proto: P<JsObject>,
    /// `%StringIteratorPrototype%`：`String.prototype[@@iterator]` 返回的迭代器。
    pub string_iterator_proto: P<JsObject>,
    /// `String.prototype[@@iterator]` 默认迭代器函数对象的裸指针，绑定层安装方法时
    /// 捕获写入。wrapper 本体归 `leaked_objects` 释放登记表所有，session 收尾统一释放。
    ///
    /// 字符串迭代器协议判定（判断某函数是否就是该默认迭代器）按指针与
    /// 本值比较：默认迭代器正是 @@iterator 槽存储的值本身（槽值与迭代器函数
    /// 同一对象），无法像集合迭代器那样复用"与迭代器原型槽比较"的判法，
    /// 故独立存一份指针。`Cell` 供绑定层经 `&Arc<BuiltinWorld>` 共享引用写入。
    pub string_default_iterator: std::cell::Cell<*const JsObject>,
    /// `%RegExpStringIteratorPrototype%`：matchAll 返回的迭代器。
    pub regexp_string_iterator_proto: P<JsObject>,
    /// `%IteratorHelperPrototype%`：Iterator helpers 结果对象的共享原型，链到 %IteratorPrototype%。
    pub iterator_helper_proto: P<JsObject>,
    /// `DisposableStack.prototype`：同步资源栈原型（链到 Object.prototype），
    /// 方法/别名/@@toStringTag 由绑定层安装。
    pub disposable_stack_proto: P<JsObject>,
    /// `AsyncDisposableStack.prototype`：异步资源栈原型（形状与同步栈一致）。
    pub async_disposable_stack_proto: P<JsObject>,
    pub stub_objects: Vec<P<JsObject>>,
    pub console_object: P<JsObject>,
    /// 释放登记表（`Box::into_raw` 对象的清单，session 收尾统一释放，非内存泄漏）：
    /// 绑定层经 `Box::into_raw` 持有的函数/宿主对象（方法 wrapper、访问器、
    /// 错误构造器、Reflect/Iterator、内建原型构造器、`$262` 宿主等）。
    /// 这些对象本体在堆上、不属任何 arena，session 收尾时按表统一释放
    /// （属性区 + 本体）；选择性重建替换 world 时本表整体并入新 world
    /// （`inherit_leaked_objects`），仍由 session 收尾统一释放，不悬垂、不双放。
    ///
    /// 可复用 native 函数 wrapper 带复用键（[`FnWrapperKey`]）：选择性重建
    /// 重绑按键命中旧 wrapper，迁移到重建 P 对象槽位，登记表跨重建不增长。
    ///
    /// 每条目另记洁净世代基线：wrapper 本体被用户写会递增其世代（写的是
    /// wrapper 自身而非所属 P 对象），基线偏离即视为污染，选择性重建据此
    /// 失效复用键、强制新建 wrapper。
    pub(crate) leaked_objects: std::cell::RefCell<Vec<LeakedSlot>>,
}

/// native 函数 wrapper 的复用键：（目标家族，目标站点标签，属性槽位键，wrapper 名）。
///
/// `family` 字段：目标对象属于本 world 固定 P 字段时取该字段在 `all_p_fields`
/// 枚举中的下标加 1（字段序跨重建不变），否则为 0；`label` 字段：非 P 目标
/// 的绑定站点标识（站点名的 intern 键），用于区分 Generator/AsyncGenerator
/// 原型上同名的 next/return/throw 槽。选择性重建重绑按键命中旧 wrapper 并
/// 迁移，避免跨重建无界累积登记表；同键重复登记意味着复用键设计缺陷（debug 断言）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FnWrapperKey {
    family: u16,
    label: u32,
    slot: u32,
    name: u32,
}

impl FnWrapperKey {
    /// 按（family, label, slot, name）四元组构造复用键。
    pub const fn new(family: u16, label: u32, slot: u32, name: u32) -> Self {
        Self { family, label, slot, name }
    }

    /// 是否属于 VM 内建原型（generator/async）的站点标签键。
    ///
    /// 这些站点绑定在 VM 自有的原型 Box 对象上（非本 world 固定 P 字段），
    /// 故家族为 0 而站点标签非零。其 wrapper 由 VM 内建原型持有，选择性重建
    /// 不可清键失效：清键会让原型仍引用旧 wrapper 而另建新 wrapper，留下
    /// 悬垂引用。普通方法 wrapper（P 目标家族非零，或 global/构造器等无标签
    /// 站点）不在此列，正常参与失效。
    const fn is_vm_intrinsic_site(&self) -> bool {
        self.family == 0 && self.label != 0
    }
}

/// 登记表条目：对象指针 + 可选复用键（`None` = 不可复用对象）。
///
/// `clean_generation` 是登记（或重建收尾经
/// [`BuiltinWorld::refresh_leaked_object_baselines`] 刷新）时的对象世代；
/// 属性写使其偏离即视为 wrapper 本体污染。`key = None` 除不可复用对象外，
/// 还可能是被失效的旧可复用 wrapper——复用键已清空、不再参与重建复用，但
/// 本体仍可能被旧引用持有，须滞留至 session 收尾（`teardown_heap_data`）
/// 统一释放，不得提前释放。
pub(crate) struct LeakedSlot {
    pub(crate) ptr: *mut JsObject,
    key: Option<FnWrapperKey>,
    clean_generation: u32,
}

impl BuiltinWorld {
    /// 把 Function.prototype 包装为 `JsValue` 返回，供方法 wrapper 取原型。
    pub fn fn_proto_val(&self) -> JsValue {
        JsValue::from_js_object(self.function_proto.as_ptr() as *mut JsObject)
    }

    /// 登记一个绑定层经 `Box::into_raw` 持有的函数/宿主对象（不可复用对象），
    /// 供 [`Self::teardown_heap_data`] 在 session 收尾时统一释放；可复用
    /// native 函数 wrapper 走 [`Self::track_fn_wrapper`]。
    ///
    /// 登记点同时读取对象世代作洁净基线（属性写使其偏离即视为本体污染）。
    #[expect(clippy::not_unsafe_ptr_arg_deref)] // 指针由绑定层保证存活，此函数仅读世代、不转移所有权
    pub fn track_leaked_object(&self, obj_ptr: *mut JsObject) {
        // SAFETY: obj_ptr 是刚 Box::into_raw 的存活对象，登记点读取其世代作洁净基线。
        let clean_generation = unsafe { (*obj_ptr).generation() };
        self.leaked_objects.borrow_mut().push(LeakedSlot {
            ptr: obj_ptr,
            key: None,
            clean_generation,
        });
    }

    /// 登记一个可复用 native 函数 wrapper（带复用键），随登记表在 session
    /// 收尾统一释放。
    ///
    /// # 注意事项
    /// 同键重复登记意味着复用键设计缺陷（同家族槽位对应两个不同 wrapper
    /// 对象）——debug 断言立即失败。
    #[expect(clippy::not_unsafe_ptr_arg_deref)] // 指针由绑定层保证存活，此函数仅读世代、不转移所有权
    pub fn track_fn_wrapper(&self, obj_ptr: *mut JsObject, key: FnWrapperKey) {
        debug_assert!(
            !self.leaked_objects.borrow().iter().any(|s| s.key == Some(key)),
            "同键 native 函数 wrapper 重复登记"
        );
        // SAFETY: obj_ptr 是刚装好 length/name 的存活 wrapper，此时世代即洁净基线。
        let clean_generation = unsafe { (*obj_ptr).generation() };
        self.leaked_objects.borrow_mut().push(LeakedSlot {
            ptr: obj_ptr,
            key: Some(key),
            clean_generation,
        });
    }

    /// 查找复用键相同且 native 函数/参数个数匹配的既有 wrapper（选择性重建
    /// 重绑的复用入口）。
    ///
    /// # 边界与前提
    /// 登记表指针 session 存活期内有效（full_reset 是无存活 VM 的时刻，
    /// 无并发读者）；native 函数与参数个数一并校验，防绑定表漂移时误换旧实现。
    pub fn find_fn_wrapper(
        &self, key: FnWrapperKey, native_fn_ptr: NativeFnPtr, arg_count: u8,
    ) -> Option<*mut JsObject> {
        self.leaked_objects
            .borrow()
            .iter()
            .find(|s| {
                s.key == Some(key) && {
                    // SAFETY: 登记表指针 session 存活期内有效。
                    let obj = unsafe { &*s.ptr };
                    obj.native_fn().map(|p| p.0) == Some(native_fn_ptr.0) && obj.native_arg_count() == arg_count
                }
            })
            .map(|s| s.ptr)
    }

    /// 释放登记表中是否存在世代偏离洁净基线的条目（wrapper 本体被写）。
    ///
    /// # 边界与前提
    /// 登记表指针 session 存活期内有效。不可复用宿主对象（`Reflect`/`Iterator`
    /// /`$262`）与其上方法 wrapper 一并纳入检测——它们同样可被用户写，且其
    /// 所属 global 需要随重建整批换新。
    pub fn has_dirty_leaked_objects(&self) -> bool {
        self.leaked_objects.borrow().iter().any(|slot| {
            // SAFETY: 登记表指针 session 存活期内有效。
            unsafe { (*slot.ptr).generation() != slot.clean_generation }
        })
    }

    /// 使偏离洁净基线的可复用 wrapper 复用键失效（清为 `None`）：重建时
    /// [`Self::find_fn_wrapper`] 落空、改走新建分支，旧 wrapper 不再被复用回
    /// 新原型槽。
    ///
    /// # 注意事项
    /// - 只清键不释放本体：旧 P 对象在重建收尾前仍可能引用它，本体随 session
    ///   收尾统一释放；提前释放会双放。
    /// - keyless 单例本就不可复用，无需清键；VM 内建 generator/async 原型
    ///   wrapper（[`FnWrapperKey::is_vm_intrinsic_site`]）由 VM 内建原型持有，
    ///   清键会遗留悬垂引用，同样跳过。
    pub fn invalidate_dirty_leaked_objects(&self) {
        for slot in self.leaked_objects.borrow_mut().iter_mut() {
            let Some(key) = slot.key else { continue };
            if key.is_vm_intrinsic_site() {
                continue;
            }
            // SAFETY: 登记表指针 session 存活期内有效。
            if unsafe { (*slot.ptr).generation() != slot.clean_generation } {
                slot.key = None;
            }
        }
    }

    /// 以各条目当前世代重刷洁净基线。
    ///
    /// # 副作用
    /// 覆盖全部条目的 `clean_generation`；由 [`crate::kernel::KernelSession::record_snapshot`]
    /// 在重建收尾（原型槽重指与重绑内部写完成）后调用，避免这些内部写在下一次
    /// 判脏时被误报为 wrapper 本体污染。
    pub fn refresh_leaked_object_baselines(&self) {
        for slot in self.leaked_objects.borrow_mut().iter_mut() {
            // SAFETY: 登记表指针 session 存活期内有效。
            slot.clean_generation = unsafe { (*slot.ptr).generation() };
        }
    }

    /// wrapper 复用键的目标家族标签：目标对象是本 world 固定 P 字段时返回
    /// 其枚举下标 + 1（`all_p_fields` 顺序跨重建不变），非 P 目标返回 0。
    pub fn wrapper_family_of(&self, obj: *const JsObject) -> u16 {
        for (i, p) in self.all_p_fields().iter().enumerate() {
            if std::ptr::eq(p.as_ptr(), obj) {
                return (i + 1) as u16;
            }
        }
        0
    }

    /// 返回释放登记表的对象数；选择性重建继承登记表时，宿主基准用例以该数
    /// 为采样点验证登记表无界增长。
    pub fn leaked_object_count(&self) -> usize {
        self.leaked_objects.borrow().len()
    }

    /// 选择性重建时把旧 world 的登记表整体并入新 world（见
    /// [`crate::kernel::KernelSession::selective_reset`]）。
    pub fn inherit_leaked_objects(&self, from: &BuiltinWorld) {
        self.leaked_objects.borrow_mut().append(&mut from.leaked_objects.borrow_mut());
    }

    /// 枚举本 world 全部固定 P 对象字段（按结构体字段序，含迭代器原型族、
    /// stub 之外的全部命名空间对象与 console）。
    ///
    /// # 注意事项
    /// session 收尾（`teardown_heap_data`）与选择性重建收尾（`retire_replaced`）
    /// 的 P 字段枚举唯一入口：`BuiltinWorld` 新增 P 字段须在此同步补一行，否则
    /// 收尾时该字段属性区无法释放、重建原型槽改写/释放漏掉该字段。
    pub(crate) fn all_p_fields(&self) -> [&P<JsObject>; 106] {
        [
            &self.object_proto,
            &self.array_proto,
            &self.function_proto,
            &self.string_proto,
            &self.number_proto,
            &self.boolean_proto,
            &self.error_proto,
            &self.symbol_proto,
            &self.object_constructor,
            &self.array_constructor,
            &self.function_constructor,
            &self.string_constructor,
            &self.number_constructor,
            &self.boolean_constructor,
            &self.error_constructor,
            &self.symbol_constructor,
            &self.type_error_proto,
            &self.reference_error_proto,
            &self.range_error_proto,
            &self.syntax_error_proto,
            &self.uri_error_proto,
            &self.eval_error_proto,
            &self.suppressed_error_proto,
            &self.math_object,
            &self.json_object,
            &self.date_constructor,
            &self.date_proto,
            &self.set_constructor,
            &self.set_proto,
            &self.map_constructor,
            &self.map_proto,
            &self.regexp_constructor,
            &self.regexp_proto,
            &self.array_buffer_constructor,
            &self.array_buffer_proto,
            &self.shared_array_buffer_proto,
            &self.shared_array_buffer_constructor,
            &self.data_view_constructor,
            &self.data_view_proto,
            &self.typed_array_proto,
            &self.typed_array_constructor,
            &self.int8array_constructor,
            &self.int8array_proto,
            &self.uint8array_constructor,
            &self.uint8array_proto,
            &self.uint8clampedarray_constructor,
            &self.uint8clampedarray_proto,
            &self.int16array_constructor,
            &self.int16array_proto,
            &self.uint16array_constructor,
            &self.uint16array_proto,
            &self.int32array_constructor,
            &self.int32array_proto,
            &self.uint32array_constructor,
            &self.uint32array_proto,
            &self.float32array_constructor,
            &self.float32array_proto,
            &self.float64array_constructor,
            &self.float64array_proto,
            &self.bigint64array_constructor,
            &self.bigint64array_proto,
            &self.biguint64array_constructor,
            &self.biguint64array_proto,
            &self.sym_match,
            &self.sym_replace,
            &self.sym_search,
            &self.sym_split,
            &self.sym_iterator,
            &self.sym_to_primitive,
            &self.sym_has_instance,
            &self.sym_match_all,
            &self.sym_async_iterator,
            &self.sym_to_string_tag,
            &self.sym_species,
            &self.sym_async_dispose,
            &self.sym_dispose,
            &self.temporal_object,
            &self.temporal_now_object,
            &self.instant_constructor,
            &self.instant_proto,
            &self.plain_date_constructor,
            &self.plain_date_proto,
            &self.plain_time_constructor,
            &self.plain_time_proto,
            &self.duration_constructor,
            &self.duration_proto,
            &self.zoned_date_time_constructor,
            &self.zoned_date_time_proto,
            &self.plain_date_time_constructor,
            &self.plain_date_time_proto,
            &self.plain_month_day_constructor,
            &self.plain_month_day_proto,
            &self.plain_year_month_constructor,
            &self.plain_year_month_proto,
            &self.bigint_constructor,
            &self.bigint_proto,
            &self.iterator_proto,
            &self.array_iterator_proto,
            &self.map_iterator_proto,
            &self.set_iterator_proto,
            &self.string_iterator_proto,
            &self.regexp_string_iterator_proto,
            &self.iterator_helper_proto,
            &self.disposable_stack_proto,
            &self.async_disposable_stack_proto,
            &self.console_object,
        ]
    }

    /// 释放本 world 拥有的全部手工堆数据。
    ///
    /// # 释放范围
    /// 1. `Box::into_raw` 持有的函数/宿主对象（登记表）：先释放其堆外属性区，
    ///    再释放对象本体；
    /// 2. 全部 P 对象字段（`all_p_fields` 枚举 + stub 族）的堆外属性区——
    ///    对象本体随 Arc 引用归零释放。
    ///
    /// # 注意事项
    /// 仅由 session 收尾调用（`KernelSession` 的 `Drop` 与 session 替换前），
    /// 幂等：登记表按值取走，属性区释放后置空。选择性重建不走本路径：
    /// 登记表整体并入新 world（`inherit_leaked_objects`），仍由
    /// session 收尾统一释放；被替换家族的旧 P 字段属性区在重建收尾
    /// （`retire_replaced`）恰好释放一次，与本路径对象集不相交，不双放。
    pub fn teardown_heap_data(&self) {
        for slot in self.leaked_objects.borrow_mut().drain(..) {
            let ptr = slot.ptr;
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 是绑定层 Box::into_raw 产物，session 存活期内有效，
            // 此处恰好释放一次（登记表按值取走，重入时表已空）。
            unsafe {
                let obj = &mut *ptr;
                obj.release_raw_heap();
                drop(Box::from_raw(ptr));
            }
        }
        for p in self.all_p_fields() {
            // SAFETY: p 是本 world 的 P 对象，属性区仅在此释放并置空（幂等）。
            unsafe {
                (&mut *p.as_mut_ptr()).release_raw_heap();
            }
        }
        for p in &self.stub_objects {
            // SAFETY: 同上，stub 对象归本 world 所有。
            unsafe {
                (&mut *p.as_mut_ptr()).release_raw_heap();
            }
        }
    }

    /// 按 [`BuiltinId`] 取对应内置对象的指针引用。
    pub fn get_by_id(&self, id: BuiltinId) -> &P<JsObject> {
        match id {
            BuiltinId::ObjectProto => &self.object_proto,
            BuiltinId::ArrayProto => &self.array_proto,
            BuiltinId::FunctionProto => &self.function_proto,
            BuiltinId::StringProto => &self.string_proto,
            BuiltinId::NumberProto => &self.number_proto,
            BuiltinId::BooleanProto => &self.boolean_proto,
            BuiltinId::ErrorProto => &self.error_proto,
            BuiltinId::SymbolProto => &self.symbol_proto,
            BuiltinId::ObjectConstructor => &self.object_constructor,
            BuiltinId::ArrayConstructor => &self.array_constructor,
            BuiltinId::FunctionConstructor => &self.function_constructor,
            BuiltinId::StringConstructor => &self.string_constructor,
            BuiltinId::NumberConstructor => &self.number_constructor,
            BuiltinId::BooleanConstructor => &self.boolean_constructor,
            BuiltinId::ErrorConstructor => &self.error_constructor,
            BuiltinId::SymbolConstructor => &self.symbol_constructor,
            BuiltinId::TypeErrorProto => &self.type_error_proto,
            BuiltinId::ReferenceErrorProto => &self.reference_error_proto,
            BuiltinId::RangeErrorProto => &self.range_error_proto,
            BuiltinId::SyntaxErrorProto => &self.syntax_error_proto,
            BuiltinId::UriErrorProto => &self.uri_error_proto,
            BuiltinId::EvalErrorProto => &self.eval_error_proto,
            BuiltinId::SuppressedErrorProto => &self.suppressed_error_proto,
            BuiltinId::MathObject => &self.math_object,
            BuiltinId::JsonObject => &self.json_object,
            BuiltinId::DateConstructor => &self.date_constructor,
            BuiltinId::DateProto => &self.date_proto,
            BuiltinId::SetConstructor => &self.set_constructor,
            BuiltinId::SetProto => &self.set_proto,
            BuiltinId::MapConstructor => &self.map_constructor,
            BuiltinId::MapProto => &self.map_proto,
            BuiltinId::RegExpConstructor => &self.regexp_constructor,
            BuiltinId::RegExpProto => &self.regexp_proto,
            BuiltinId::ArrayBufferConstructor => &self.array_buffer_constructor,
            BuiltinId::ArrayBufferProto => &self.array_buffer_proto,
            BuiltinId::SharedArrayBufferProto => &self.shared_array_buffer_proto,
            BuiltinId::SharedArrayBufferConstructor => &self.shared_array_buffer_constructor,
            BuiltinId::DataViewConstructor => &self.data_view_constructor,
            BuiltinId::DataViewProto => &self.data_view_proto,
            BuiltinId::TypedArrayProto => &self.typed_array_proto,
            BuiltinId::Int8ArrayConstructor => &self.int8array_constructor,
            BuiltinId::Int8ArrayProto => &self.int8array_proto,
            BuiltinId::Uint8ArrayConstructor => &self.uint8array_constructor,
            BuiltinId::Uint8ArrayProto => &self.uint8array_proto,
            BuiltinId::Uint8ClampedArrayConstructor => &self.uint8clampedarray_constructor,
            BuiltinId::Uint8ClampedArrayProto => &self.uint8clampedarray_proto,
            BuiltinId::Int16ArrayConstructor => &self.int16array_constructor,
            BuiltinId::Int16ArrayProto => &self.int16array_proto,
            BuiltinId::Uint16ArrayConstructor => &self.uint16array_constructor,
            BuiltinId::Uint16ArrayProto => &self.uint16array_proto,
            BuiltinId::Int32ArrayConstructor => &self.int32array_constructor,
            BuiltinId::Int32ArrayProto => &self.int32array_proto,
            BuiltinId::Uint32ArrayConstructor => &self.uint32array_constructor,
            BuiltinId::Uint32ArrayProto => &self.uint32array_proto,
            BuiltinId::Float32ArrayConstructor => &self.float32array_constructor,
            BuiltinId::Float32ArrayProto => &self.float32array_proto,
            BuiltinId::Float64ArrayConstructor => &self.float64array_constructor,
            BuiltinId::Float64ArrayProto => &self.float64array_proto,
            BuiltinId::BigInt64ArrayConstructor => &self.bigint64array_constructor,
            BuiltinId::BigInt64ArrayProto => &self.bigint64array_proto,
            BuiltinId::BigUint64ArrayConstructor => &self.biguint64array_constructor,
            BuiltinId::BigUint64ArrayProto => &self.biguint64array_proto,
            BuiltinId::SymMatch => &self.sym_match,
            BuiltinId::SymReplace => &self.sym_replace,
            BuiltinId::SymSearch => &self.sym_search,
            BuiltinId::SymSplit => &self.sym_split,
            BuiltinId::SymIterator => &self.sym_iterator,
            BuiltinId::SymToPrimitive => &self.sym_to_primitive,
            BuiltinId::SymHasInstance => &self.sym_has_instance,
            BuiltinId::SymMatchAll => &self.sym_match_all,
            BuiltinId::SymAsyncIterator => &self.sym_async_iterator,
            BuiltinId::SymToStringTag => &self.sym_to_string_tag,
            BuiltinId::SymSpecies => &self.sym_species,
            BuiltinId::SymAsyncDispose => &self.sym_async_dispose,
            BuiltinId::SymDispose => &self.sym_dispose,
            BuiltinId::TemporalObject => &self.temporal_object,
            BuiltinId::TemporalNowObject => &self.temporal_now_object,
            BuiltinId::InstantConstructor => &self.instant_constructor,
            BuiltinId::InstantProto => &self.instant_proto,
            BuiltinId::PlainDateConstructor => &self.plain_date_constructor,
            BuiltinId::PlainDateProto => &self.plain_date_proto,
            BuiltinId::PlainTimeConstructor => &self.plain_time_constructor,
            BuiltinId::PlainTimeProto => &self.plain_time_proto,
            BuiltinId::DurationConstructor => &self.duration_constructor,
            BuiltinId::DurationProto => &self.duration_proto,
            BuiltinId::ZonedDateTimeConstructor => &self.zoned_date_time_constructor,
            BuiltinId::ZonedDateTimeProto => &self.zoned_date_time_proto,
            BuiltinId::PlainDateTimeConstructor => &self.plain_date_time_constructor,
            BuiltinId::PlainDateTimeProto => &self.plain_date_time_proto,
            BuiltinId::PlainMonthDayConstructor => &self.plain_month_day_constructor,
            BuiltinId::PlainMonthDayProto => &self.plain_month_day_proto,
            BuiltinId::PlainYearMonthConstructor => &self.plain_year_month_constructor,
            BuiltinId::PlainYearMonthProto => &self.plain_year_month_proto,
            BuiltinId::BigIntConstructor => &self.bigint_constructor,
            BuiltinId::BigIntProto => &self.bigint_proto,
            BuiltinId::Console => &self.console_object,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_forge::EMPTY_SHAPE_ID;

    /// 选择性重建的释放 + 原型槽改写面动态自测：被替换旧 P 对象属性区恰好释放
    /// 一次（置空可断言，含经 `mem::forget` 抬升 Arc 计数的 Function 家族属性区、
    /// 本体永久保留），保留字段指针不变且 proto 槽改写到新指针，登记表并入新 world。
    #[test]
    fn selective_reset_releases_replaced_family_heap() {
        use crate::kernel::{KernelConfig, KernelCore, KernelSession};
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        // 持有旧 world Arc：被替换对象本体在断言期仍可读（属性区指针可检查）。
        let old_world = std::sync::Arc::clone(&session.builtin_world);
        let array_proto = old_world.array_proto.as_ptr() as *mut JsObject;
        let object_proto = old_world.object_proto.as_ptr() as *mut JsObject;
        let fn_proto = old_world.function_proto.as_ptr() as *mut JsObject;

        // 旧原型各造一个命名属性区（绑定安装属性区后的状态）；登记表放一个
        // proto 槽指向旧 fn_proto 的 wrapper（槽值绑定时写入）。
        let wrapper = Box::into_raw(Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())));
        unsafe {
            (*wrapper).set_proto(JsValue::from_js_object(fn_proto)).ok();
            (&mut *array_proto).ensure_hash_props().push(JsValue::int(1));
            (&mut *object_proto).ensure_hash_props().push(JsValue::int(2));
            (&mut *fn_proto).ensure_hash_props().push(JsValue::int(3));
        }
        old_world.track_leaked_object(wrapper);

        unsafe {
            (&mut *array_proto).bump_generation();
            (&mut *fn_proto).bump_generation();
        }
        let dirty = session.selective_reset(&core);
        assert!(dirty.array);
        assert!(dirty.function);

        // 被替换家族（array）：四处堆外属性区已释放置空。
        let old_array = unsafe { &*array_proto };
        assert!(old_array.hash_props_raw().is_null());
        assert!(old_array.array_elements_raw().is_null());
        assert!(old_array.array_elements_meta_raw().is_null());
        assert!(old_array.prop_meta_raw().is_null());
        // 经 `mem::forget` 抬升 Arc 计数的 Function 家族对象对：本体永久保留（仍可读），
        // 属性区于原型槽改写完成后同样释放——改写后旧对象无读者。
        assert!(unsafe { &*fn_proto }.hash_props_raw().is_null());
        // 未脏家族（object）：沿用同一对象，字段指针与属性区均不受影响。
        assert!(!unsafe { &*object_proto }.hash_props_raw().is_null());
        assert!(std::ptr::eq(object_proto, session.builtin_world.object_proto.as_ptr() as *mut JsObject));
        // 原型槽改写：保留字段（object_constructor）与保留 wrapper 的 proto 槽
        // 均改写到新 fn_proto，无残留旧指针。
        let new_fn_proto = session.builtin_world.function_proto.as_ptr() as *mut JsObject;
        let object_ctor = old_world.object_constructor.as_ptr() as *mut JsObject;
        assert!(std::ptr::eq(
            object_ctor,
            session.builtin_world.object_constructor.as_ptr() as *mut JsObject
        ));
        assert!(std::ptr::eq(unsafe { (*object_ctor).proto().as_js_object_ptr() }, new_fn_proto));
        assert!(std::ptr::eq(unsafe { (*wrapper).proto().as_js_object_ptr() }, new_fn_proto));
        // 登记表并入新 world：保留 wrapper 仍须由 session 收尾统一释放。
        assert!(session.builtin_world.leaked_objects.borrow().iter().any(|s| s.ptr == wrapper));
    }

    /// SAB 家族脏线冒烟：仅 SAB 对世代漂移 → 选择性重置只判 SAB 家族脏，
    /// ArrayBuffer 家族指针原样保留（钉 dirty_since_snapshot 家族线接线）。
    #[test]
    fn sab_family_selective_rebuild() {
        use crate::kernel::{KernelConfig, KernelCore, KernelSession};
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        let old_world = std::sync::Arc::clone(&session.builtin_world);
        let sab_proto = old_world.shared_array_buffer_proto.as_ptr() as *mut JsObject;
        let sab_ctor = old_world.shared_array_buffer_constructor.as_ptr() as *mut JsObject;
        let ab_proto = old_world.array_buffer_proto.as_ptr() as *mut JsObject;

        unsafe {
            (&mut *sab_proto).bump_generation();
            (&mut *sab_ctor).bump_generation();
        }
        let dirty = session.selective_reset(&core);
        assert!(dirty.shared_array_buffer);
        assert!(!dirty.array_buffer);
        assert!(!dirty.data_view);

        // 脏家族换新对象对，未脏家族指针原样保留。
        assert!(!std::ptr::eq(
            sab_proto,
            session.builtin_world.shared_array_buffer_proto.as_ptr() as *mut JsObject
        ));
        assert!(!std::ptr::eq(
            sab_ctor,
            session.builtin_world.shared_array_buffer_constructor.as_ptr() as *mut JsObject
        ));
        assert!(std::ptr::eq(ab_proto, session.builtin_world.array_buffer_proto.as_ptr() as *mut JsObject));
    }
}
