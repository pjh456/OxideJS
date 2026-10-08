//! MessageValue 跨 realm 值传递抽象：`detach_message`（源 realm 侧）与
//! `rehydrate_message`（目标 realm 侧）两函数，加 `MessageValue` 中间表示。
//!
//! 跨线程消息不能直接传 `JsValue`（对象/字符串/BigInt 指针是 realm 局部的
//! session 堆地址，跨线程无效），须转成 `Send` 的 `MessageValue` 中间表示，
//! worker 侧再 rehydrate 进目标 realm。本模块复用克隆核的类型分派知识（哪些
//! 类型可克隆、如何按类型分派）与 `data_clone_error` 助手；跨 realm 符号映射
//! 为 well-known 符号映目标 realm 同 local_index、用户符号经描述重新 intern。
//!
//! 边界：
//! - 两函数为独立函数，刻意不挂 `VmHost` trait：trait 是单 realm 接口，跨 realm
//!   值传递不挂 trait（见 trait 文档边界条款），免污染单 realm 接口。
//! - `MessageValue` 是树形（无共享引用间接），值图内循环引用报 DataCloneError；
//!   共享引用（有向无环）复制为多份。
//! - 装箱对象（Boolean/Number/String 盒、BigInt 包装）传递时解箱为原始值。
//! - SharedArrayBuffer / TypedArray / DataView 报 DataCloneError（共享缓冲机制
//!   落地前的统一策略）。

use std::collections::HashSet;
use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::{decode_symbol_key, int_key_value, is_int_key, WELL_KNOWN_SYMBOL_COUNT};
use oxide_types::value::JsValue;

use oxide_runtime_api::{to_string_full, ProtoKind, VmHost};

use crate::array::create_new_array;
use crate::map::MapInner;
use crate::object::{walk_own_keys, walk_own_symbol_keys};
use crate::set::{SetInner, SetKey};
use crate::structured_clone::data_clone_error;

/// 跨 realm 值传递的中间表示：`Send` 枚举、无 realm 局部指针（无 `*mut JsObject`、
/// 无 session 字符串指针、无 `NativeFnPtr`），全部载荷 owned 或 `Arc` 共享。
///
/// 供 `detach_message`（源 realm 侧）产出、`rehydrate_message`（目标 realm 侧）
/// 消费。
#[derive(Debug, Clone)]
pub enum MessageValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(Box<[u16]>),
    BigInt(Box<num_bigint::BigInt>),
    /// ArrayBuffer：transfer 转移（源 detach）或未命中克隆（字节拷贝）。
    ArrayBuffer(Arc<Vec<u8>>),
    /// SharedArrayBuffer：预留臂，当前 detach/rehydrate 均报 DataCloneError。
    SharedArrayBuffer(Arc<Vec<u8>>),
    Array(Vec<MessageValue>),
    /// (key, value)，key 为 String（含整数键的文本形态）或 Symbol。
    Object(Vec<(MessageValue, MessageValue)>),
    Map(Vec<(MessageValue, MessageValue)>),
    Set(Vec<MessageValue>),
    Date(f64),
    RegExp {
        source: String,
        flags: String,
    },
    Error {
        name: String,
        message: Option<String>,
        cause: Option<Box<MessageValue>>,
    },
    /// 跨 realm 符号：`well_known` 为 well-known 局部下标（0..14），
    /// `description` 为用户符号描述。
    Symbol {
        well_known: Option<u32>,
        description: Option<String>,
    },
}

/// detach 状态：`on_stack` 记录正在 detach 中的源对象指针（递归栈），用于
/// 检测循环引用（循环报 DataCloneError）。
struct DetachState {
    on_stack: HashSet<*const JsObject>,
}

impl DetachState {
    fn new() -> Self {
        Self { on_stack: HashSet::new() }
    }
}

/// 源 realm 侧：遍历源值产出 `MessageValue`。
///
/// # 步骤
/// 1. 非对象值：按类型分派（undefined/null/bool/number/string/bigint/symbol）
/// 2. 对象值：查循环引用，按类型分派（数组/plain/Map/Set/Date/RegExp/Error/
///    ArrayBuffer 等）
/// 3. ArrayBuffer 查 transfer 集合（命中移动载荷、源 detach；未命中克隆字节）
///
/// # 边界与前提
/// - 源对象由调用方寄存器保活；
/// - 不可克隆类型（function/Promise/迭代器/模块命名空间/装箱 Symbol 盒/
///   SharedArrayBuffer/TypedArray/DataView/循环引用）报 DataCloneError；
/// - 装箱对象解箱为原始值。
///
/// # 返回值
/// 成功 `Ok(MessageValue)`，失败 `Err(DataCloneError)`。
pub fn detach_message<H: VmHost>(
    vm: &mut H, value: JsValue, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let mut state = DetachState::new();
    detach_value(vm, &mut state, value, transfer)
}

/// 递归 detach 一个值：非对象走原始值分派，对象先查循环再按类型分派。
fn detach_value<H: VmHost>(
    vm: &mut H, state: &mut DetachState, value: JsValue, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    if !value.is_object() {
        return detach_primitive(vm, value);
    }
    let src_ptr = value.as_js_object_ptr() as *const JsObject;
    // 循环引用检测：源对象已在递归栈上 → 循环，报 DataCloneError。
    if !state.on_stack.insert(src_ptr) {
        return Err(data_clone_error(vm, "circular structure is not cloneable"));
    }
    let result = detach_object(vm, state, value, transfer);
    state.on_stack.remove(&src_ptr);
    result
}

/// 非对象值按类型分派为 `MessageValue` 臂。
fn detach_primitive<H: VmHost>(vm: &mut H, value: JsValue) -> Result<MessageValue, JsValue> {
    if value.is_undefined() {
        return Ok(MessageValue::Undefined);
    }
    if value.is_null() {
        return Ok(MessageValue::Null);
    }
    if value.is_bool() {
        return Ok(MessageValue::Boolean(value.as_bool()));
    }
    if value.is_int() {
        return Ok(MessageValue::Number(value.as_int() as f64));
    }
    if value.is_double() {
        return Ok(MessageValue::Number(value.as_double()));
    }
    if value.is_string() {
        let units = vm.string_units(value);
        return Ok(MessageValue::String(units.to_vec().into_boxed_slice()));
    }
    if value.is_bigint() {
        let bigint = vm.bigint_value(value);
        return Ok(MessageValue::BigInt(Box::new(bigint.clone())));
    }
    if value.is_symbol() {
        return Ok(detach_symbol(vm, value));
    }
    Err(data_clone_error(vm, "value is not cloneable"))
}

/// symbol 原始值：well-known 记局部下标（跨 realm 同 local_index），用户符号
/// 记描述（目标 realm 重新 intern）。
fn detach_symbol<H: VmHost>(vm: &mut H, value: JsValue) -> MessageValue {
    let local_index = value.as_symbol_local_index();
    if local_index < WELL_KNOWN_SYMBOL_COUNT {
        MessageValue::Symbol {
            well_known: Some(local_index),
            description: None,
        }
    } else {
        let description = vm.symbol_description(local_index);
        MessageValue::Symbol { well_known: None, description }
    }
}

/// 对象值按类型分派：装箱解箱、缓冲区、容器递归、Date/RegExp/Error 专用。
fn detach_object<H: VmHost>(
    vm: &mut H, state: &mut DetachState, value: JsValue, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let src_ptr = value.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let src = unsafe { &*src_ptr };
    // 装箱对象解箱为原始值。
    if is_boxed(src) {
        return Ok(detach_boxed(vm, src));
    }
    if src.is_symbol_obj() {
        return Err(data_clone_error(vm, "symbol objects are not cloneable"));
    }
    if src.is_date_obj() {
        let ts = src.get_prop_at(0);
        let n = if ts.is_int() { ts.as_int() as f64 } else { ts.as_double() };
        return Ok(MessageValue::Date(n));
    }
    if src.is_regexp_obj() {
        let source = vm.lookup_str(src.get_regexp_source()).unwrap_or_default();
        let flags = vm.lookup_str(src.get_regexp_flags()).unwrap_or_default();
        return Ok(MessageValue::RegExp { source, flags });
    }
    if src.is_array_buffer_obj() {
        return detach_array_buffer(vm, src, transfer);
    }
    if src.is_shared_array_buffer_obj() {
        return Err(data_clone_error(vm, "SharedArrayBuffer is not cloneable"));
    }
    if src.is_typed_array_obj() || src.is_data_view_obj() {
        return Err(data_clone_error(vm, "typed array view is not cloneable"));
    }
    if src.is_map() {
        return detach_map(vm, state, src, transfer);
    }
    if src.is_set() {
        return detach_set(vm, state, src, transfer);
    }
    if src.is_error_obj() {
        return detach_error(vm, state, src, transfer);
    }
    if src.is_array() {
        return detach_array(vm, state, src, transfer);
    }
    if is_plain_object(src) {
        return detach_plain_object(vm, state, src, transfer);
    }
    Err(data_clone_error(vm, "value is not cloneable"))
}

/// 是否可解箱的装箱对象：Boolean/Number/String 盒，或 BigInt 包装（PLAIN 标签
/// 加 bigint 载荷）。
fn is_boxed(src: &JsObject) -> bool {
    src.is_boolean_obj()
        || src.is_number_obj()
        || src.is_string_obj()
        || (src.type_tag == JsObject::OBJ_TYPE_PLAIN && src.boxed_value().is_bigint())
}

/// 是否 plain 对象：类型标签为 PLAIN 且非函数、非模块命名空间（Map / Set 等
/// 特型对象在分派前已被拦截，不会到达本判定）。
fn is_plain_object(src: &JsObject) -> bool {
    src.type_tag == JsObject::OBJ_TYPE_PLAIN && !src.is_function() && !src.is_module_namespace()
}

/// 装箱对象解箱为原始值臂。
fn detach_boxed<H: VmHost>(vm: &mut H, src: &JsObject) -> MessageValue {
    let payload = src.boxed_value();
    if src.is_boolean_obj() {
        return MessageValue::Boolean(payload.as_bool());
    }
    if src.is_number_obj() {
        let n = if payload.is_int() { payload.as_int() as f64 } else { payload.as_double() };
        return MessageValue::Number(n);
    }
    if src.is_string_obj() {
        let units = vm.string_units(payload);
        return MessageValue::String(units.to_vec().into_boxed_slice());
    }
    // BigInt 包装。
    let bigint = vm.bigint_value(payload);
    MessageValue::BigInt(Box::new(bigint.clone()))
}

/// ArrayBuffer：transfer 命中移动载荷（源 detach），未命中克隆字节；detached
/// 源报 DataCloneError。
fn detach_array_buffer<H: VmHost>(
    vm: &mut H, src: &JsObject, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let Some(payload_ptr) = crate::array_buffer::array_buffer_payload_ptr(src) else {
        return Err(data_clone_error(vm, "ArrayBuffer internal state invalid"));
    };
    let src_ptr = src as *const JsObject;
    if transfer.contains(&src_ptr) {
        // 转移：移动源载荷入消息，源转 detached 态。
        // SAFETY: payload_ptr 经 array_buffer_payload_ptr 校验为存活载荷盒；
        // 移动为标量操作，不跨 JS 调用。
        let src_detached = unsafe { (*payload_ptr).detached };
        if src_detached {
            return Err(data_clone_error(vm, "detached buffer in transfer list"));
        };
        let data = unsafe { std::mem::take(&mut (*payload_ptr).bytes) };
        unsafe { (*payload_ptr).detached = true };
        return Ok(MessageValue::ArrayBuffer(Arc::new(data)));
    }
    // 未转移：克隆字节。
    // SAFETY: payload_ptr 经 array_buffer_payload_ptr 校验为存活载荷盒；标量
    // 拷出后借用即结束，不跨 JS 调用。
    let (data, detached) = unsafe {
        let p = &*payload_ptr;
        (p.bytes.clone(), p.detached)
    };
    if detached {
        return Err(data_clone_error(vm, "detached ArrayBuffer is not cloneable"));
    };
    Ok(MessageValue::ArrayBuffer(Arc::new(data)))
}

/// Map：逐条 detach 键值后组装 `MessageValue::Map`。
fn detach_map<H: VmHost>(
    vm: &mut H, state: &mut DetachState, src: &JsObject, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let src_inner = src.native_data() as *const MapInner;
    if src_inner.is_null() {
        return Ok(MessageValue::Map(Vec::new()));
    }
    // 先收集条目（迭代器持有 &MapInner 借用，递归前须结束借用）。
    // SAFETY: src_inner 是 Map native_data 槽写入的有效 `MapInner` 指针。
    let entries: Vec<(SetKey, JsValue)> = unsafe { (*src_inner).iter() }.collect();
    let mut result = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let detached_key = detach_value(vm, state, key.0, transfer)?;
        let detached_value = detach_value(vm, state, value, transfer)?;
        result.push((detached_key, detached_value));
    }
    Ok(MessageValue::Map(result))
}

/// Set：逐个 detach 元素后组装 `MessageValue::Set`。
fn detach_set<H: VmHost>(
    vm: &mut H, state: &mut DetachState, src: &JsObject, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let src_inner = src.native_data() as *const SetInner;
    if src_inner.is_null() {
        return Ok(MessageValue::Set(Vec::new()));
    }
    // 先收集元素（迭代器持有 &SetInner 借用，递归前须结束借用）。
    // SAFETY: src_inner 是 Set native_data 槽写入的有效 `SetInner` 指针。
    let entries: Vec<SetKey> = unsafe { (*src_inner).iter() }.copied().collect();
    let mut result = Vec::with_capacity(entries.len());
    for key in entries {
        let detached_key = detach_value(vm, state, key.0, transfer)?;
        result.push(detached_key);
    }
    Ok(MessageValue::Set(result))
}

/// Error：name 经原型链 Get 归一、message 仅取自有数据描述符槽值并 ToString、
/// cause 在场时递归 detach。
fn detach_error<H: VmHost>(
    vm: &mut H, state: &mut DetachState, src: &JsObject, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let src_val = JsValue::from_js_object(src as *const JsObject as *mut JsObject);
    let si_name = vm.perm_intern("name");
    let name_val = match vm.ordinary_get(src, si_name, src_val) {
        Ok(v) => v,
        Err(e) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, &e));
        }
    };
    let name = vm.lookup_str(name_val).unwrap_or_else(|| "Error".to_string());
    let si_msg = vm.perm_intern("message");
    let message = if let Some(store) = vm.get_own_property_slot(src, si_msg) {
        let is_accessor = src.prop_meta_at(store).is_some_and(|m| m.is_accessor);
        if !is_accessor {
            let slot_val = src.get_prop_at(store);
            to_string_full(slot_val, vm).ok()
        } else {
            None
        }
    } else {
        None
    };
    let si_cause = vm.perm_intern("cause");
    let cause = if let Some(store) = vm.get_own_property_slot(src, si_cause) {
        let val = read_own_value(vm, src, src_val, si_cause, store)?;
        Some(Box::new(detach_value(vm, state, val, transfer)?))
    } else {
        None
    };
    Ok(MessageValue::Error { name, message, cause })
}

/// 读源对象自有属性的值：数据属性直读槽值，访问器属性经 getter 取值。
fn read_own_value<H: VmHost>(
    vm: &mut H, src: &JsObject, src_val: JsValue, si: u32, store: u32,
) -> Result<JsValue, JsValue> {
    let is_accessor = src.prop_meta_at(store).is_some_and(|m| m.is_accessor);
    if !is_accessor {
        return Ok(src.get_prop_at(store));
    }
    match vm.ordinary_get(src, si, src_val) {
        Ok(v) => Ok(v),
        Err(e) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, &e))
        }
    }
}

/// 数组：逐元素 detach（元素区 0..length）。
fn detach_array<H: VmHost>(
    vm: &mut H, state: &mut DetachState, src: &JsObject, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let len = src.logical_len() as usize;
    let mut result = Vec::with_capacity(len);
    for i in 0..len {
        let val = src.get_prop_at(i as u32);
        let detached = detach_value(vm, state, val, transfer)?;
        result.push(detached);
    }
    Ok(MessageValue::Array(result))
}

/// plain 对象：复制可枚举自有属性（字符串键加 Symbol 键），键值均 detach。
fn detach_plain_object<H: VmHost>(
    vm: &mut H, state: &mut DetachState, src: &JsObject, transfer: &HashSet<*const JsObject>,
) -> Result<MessageValue, JsValue> {
    let src_val = JsValue::from_js_object(src as *const JsObject as *mut JsObject);
    let str_keys = walk_own_keys(vm, src);
    let sym_keys = walk_own_symbol_keys(vm, src);
    let mut result = Vec::new();
    for (si, store) in str_keys {
        if !is_enumerable_own(src, store) {
            continue;
        }
        let key = key_si_to_message_value(vm, si)?;
        let val = read_own_value(vm, src, src_val, si, store)?;
        let detached_val = detach_value(vm, state, val, transfer)?;
        result.push((key, detached_val));
    }
    for (si, store) in sym_keys {
        if !is_enumerable_own(src, store) {
            continue;
        }
        let key = symbol_key_to_message_value(vm, si);
        let val = read_own_value(vm, src, src_val, si, store)?;
        let detached_val = detach_value(vm, state, val, transfer)?;
        result.push((key, detached_val));
    }
    Ok(MessageValue::Object(result))
}

/// 键 si（整数键或字符串键）转 `MessageValue::String`（整数键反解数字串）。
fn key_si_to_message_value<H: VmHost>(vm: &mut H, si: u32) -> Result<MessageValue, JsValue> {
    let text = if is_int_key(si) {
        int_key_value(si).to_string()
    } else {
        vm.perm_lookup(si).unwrap_or_default().to_string()
    };
    let units = text.encode_utf16().collect::<Vec<u16>>();
    Ok(MessageValue::String(units.into_boxed_slice()))
}

/// Symbol 键 si 转 `MessageValue::Symbol`：well-known 记局部下标，用户符号
/// 记描述。
fn symbol_key_to_message_value<H: VmHost>(vm: &mut H, si: u32) -> MessageValue {
    let (_realm_id, local_index) = decode_symbol_key(si);
    if local_index < WELL_KNOWN_SYMBOL_COUNT {
        MessageValue::Symbol {
            well_known: Some(local_index),
            description: None,
        }
    } else {
        let description = vm.symbol_description(local_index);
        MessageValue::Symbol { well_known: None, description }
    }
}

/// 判定 store 槽的自有属性是否可枚举（无元数据槽时按默认数据属性，可枚举）。
fn is_enumerable_own(src: &JsObject, store: u32) -> bool {
    src.prop_meta_at(store).map(|m| m.attributes.enumerable()).unwrap_or(true)
}

/// 目标 realm 侧：遍历 `MessageValue` 产出目标 realm 的 `JsValue`。
///
/// # 步骤
/// 1. 原始值构造；String 经 `new_string_units_owned`；BigInt 经 `new_bigint`
/// 2. ArrayBuffer 分配新 AB 填字节
/// 3. Array/Object/Map/Set 递归分配
/// 4. Date 建 Date 写时间值；RegExp 建 RegExp；Error 建 Error
/// 5. Symbol 跨 realm 映射（well-known 同 local_index、用户符号重新 intern）
///
/// # 边界与前提
/// - 目标 realm 为当前 VM 的 realm；
/// - `SharedArrayBuffer` 臂不可达（detach 报 DataCloneError）。
///
/// # 返回值
/// 目标 realm 的 rehydrate 后 `JsValue`。
pub fn rehydrate_message<H: VmHost>(vm: &mut H, value: &MessageValue) -> JsValue {
    rehydrate_value(vm, value)
}

/// 递归 rehydrate 一个 `MessageValue` 臂。
fn rehydrate_value<H: VmHost>(vm: &mut H, value: &MessageValue) -> JsValue {
    match value {
        MessageValue::Undefined => JsValue::undefined(),
        MessageValue::Null => JsValue::null(),
        MessageValue::Boolean(b) => JsValue::bool(*b),
        MessageValue::Number(n) => number_to_js(*n),
        MessageValue::String(units) => vm.new_string_units_owned(units.to_vec()),
        MessageValue::BigInt(bigint) => vm.new_bigint((**bigint).clone()),
        MessageValue::ArrayBuffer(bytes) => rehydrate_array_buffer(vm, bytes),
        MessageValue::SharedArrayBuffer(_) => JsValue::undefined(),
        MessageValue::Array(elems) => {
            let ptr = create_new_array(vm, elems.len());
            // SAFETY: ptr 是本函数新分配的数组对象，存活且本段无别名。
            let arr = unsafe { &mut *ptr };
            for (i, elem) in elems.iter().enumerate() {
                let val = rehydrate_value(vm, elem);
                arr.set_prop_at(i as u32, val);
            }
            JsValue::from_js_object(ptr)
        }
        MessageValue::Object(props) => {
            let proto = vm.builtin_proto(ProtoKind::ObjectProto);
            let ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
            // SAFETY: ptr 是本函数新分配的对象，存活且本段无别名。
            let obj = unsafe { &mut *ptr };
            for (key, val) in props {
                let key_val = rehydrate_value(vm, key);
                let val_val = rehydrate_value(vm, val);
                let si = vm.property_key_si(key_val);
                let _ = vm.define_data_property(obj, si, val_val, PropAttributes::DEFAULT_DATA);
            }
            JsValue::from_js_object(ptr)
        }
        MessageValue::Map(entries) => {
            let map_proto = vm.builtin_proto(ProtoKind::MapProto);
            let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
            obj.set_map(true);
            let inner = Box::into_raw(Box::new(MapInner::new()));
            obj.set_native_data(inner as *mut u8);
            let ptr = vm.alloc_object(obj);
            // SAFETY: ptr 是本函数新分配的 Map 对象，native_data 槽已写入非空盒。
            let map_inner = unsafe { (*ptr).native_data() } as *mut MapInner;
            for (key, val) in entries {
                let key_val = rehydrate_value(vm, key);
                let val_val = rehydrate_value(vm, val);
                // SAFETY: map_inner 为本函数新分配的 MapInner，存活且单线程独占。
                unsafe { (*map_inner).insert(SetKey(key_val), val_val) };
            }
            JsValue::from_js_object(ptr)
        }
        MessageValue::Set(elems) => {
            let set_proto = vm.builtin_proto(ProtoKind::SetProto);
            let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
            obj.set_set(true);
            let inner = Box::into_raw(Box::new(SetInner::new()));
            obj.set_native_data(inner as *mut u8);
            let ptr = vm.alloc_object(obj);
            // SAFETY: ptr 是本函数新分配的 Set 对象，native_data 槽已写入非空盒。
            let set_inner = unsafe { (*ptr).native_data() } as *mut SetInner;
            for elem in elems {
                let elem_val = rehydrate_value(vm, elem);
                // SAFETY: set_inner 为本函数新分配的 SetInner，存活且单线程独占。
                unsafe { (*set_inner).insert(SetKey(elem_val)) };
            }
            JsValue::from_js_object(ptr)
        }
        MessageValue::Date(ts) => {
            let proto = vm.builtin_proto(ProtoKind::DateProto);
            let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
            obj.type_tag = JsObject::OBJ_TYPE_DATE;
            obj.set_prop_at(0, JsValue::float(*ts));
            let ptr = vm.alloc_object(obj);
            JsValue::from_js_object(ptr)
        }
        MessageValue::RegExp { source, flags } => rehydrate_regexp(vm, source, flags),
        MessageValue::Error { name, message, cause } => rehydrate_error(vm, name, message, cause),
        MessageValue::Symbol { well_known, description } => rehydrate_symbol(vm, well_known, description),
    }
}

/// Number 臂 rehydrate：有限整值且落在 i32 范围回 int 表示，其余 float。
fn number_to_js(n: f64) -> JsValue {
    if n.is_finite() && n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        JsValue::int(n as i32)
    } else {
        JsValue::float(n)
    }
}

/// ArrayBuffer 臂 rehydrate：分配新定长 AB 填字节。
fn rehydrate_array_buffer<H: VmHost>(vm: &mut H, bytes: &Arc<Vec<u8>>) -> JsValue {
    let proto = crate::array_buffer::default_array_buffer_proto(vm);
    let data = (**bytes).clone();
    let ptr = crate::array_buffer::new_array_buffer(vm, data, 0, proto);
    JsValue::from_js_object(ptr)
}

/// RegExp 臂 rehydrate：按源与标志重编译，建 RegExp 对象（lastIndex 初始 0）。
fn rehydrate_regexp<H: VmHost>(vm: &mut H, source: &str, flags: &str) -> JsValue {
    let proto = vm.builtin_proto(ProtoKind::RegExpProto);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    let source_val = vm.new_string(source);
    let flags_val = vm.new_string(flags);
    // 源来自合法 RegExp，重编译失败仅防御性兜底（不建引擎盒）。
    if let Ok(re) = regress::Regex::with_flags(source, flags) {
        let re_ptr = Box::into_raw(Box::new(re));
        // SAFETY: re_ptr 是 `Box<regress::Regex>` 指针，与 RegExp 构造器同形态，
        // 对象存活期间有效，由 `drop_regexp_native` 释放。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(re_ptr as *const ()) }));
    }
    obj.type_tag = JsObject::OBJ_TYPE_REGEXP;
    obj.set_regexp_source(source_val);
    obj.set_regexp_flags(flags_val);
    let si_lastindex = vm.perm_intern("lastIndex");
    let _ = vm.define_data_property(&mut obj, si_lastindex, JsValue::int(0), PropAttributes::new(true, false, false));
    let ptr = vm.alloc_object(obj);
    JsValue::from_js_object(ptr)
}

/// Error 臂 rehydrate：按归一化 name 选标准原型，定义 message（非枚举）与
/// cause（在场时全真数据属性）。
fn rehydrate_error<H: VmHost>(
    vm: &mut H, name: &str, message: &Option<String>, cause: &Option<Box<MessageValue>>,
) -> JsValue {
    let proto = match name {
        "EvalError" => vm.builtin_proto(ProtoKind::EvalErrorProto),
        "RangeError" => vm.builtin_proto(ProtoKind::RangeErrorProto),
        "ReferenceError" => vm.builtin_proto(ProtoKind::ReferenceErrorProto),
        "SyntaxError" => vm.builtin_proto(ProtoKind::SyntaxErrorProto),
        "TypeError" => vm.builtin_proto(ProtoKind::TypeErrorProto),
        "URIError" => vm.builtin_proto(ProtoKind::UriErrorProto),
        _ => vm.builtin_proto(ProtoKind::ErrorProto),
    };
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_ERROR;
    let ptr = vm.alloc_object(obj);
    // SAFETY: ptr 是本函数新分配的 Error 对象，存活且本段无别名。
    let err = unsafe { &mut *ptr };
    if let Some(msg) = message {
        let si_msg = vm.perm_intern("message");
        let msg_val = vm.new_string(msg);
        let _ = vm.define_data_property(err, si_msg, msg_val, PropAttributes::new(true, false, true));
    }
    if let Some(cause_val) = cause {
        let si_cause = vm.perm_intern("cause");
        let cause_js = rehydrate_value(vm, cause_val);
        let _ = vm.define_data_property(err, si_cause, cause_js, PropAttributes::DEFAULT_DATA);
    }
    JsValue::from_js_object(ptr)
}

/// Symbol 臂 rehydrate：well-known 映目标 realm 同 local_index，用户符号经
/// 描述重新 intern 得目标 realm 新局部下标。
fn rehydrate_symbol<H: VmHost>(vm: &mut H, well_known: &Option<u32>, description: &Option<String>) -> JsValue {
    let realm_id = vm.realm_id();
    match well_known {
        Some(idx) => JsValue::symbol_realm(realm_id, *idx),
        None => {
            let idx = vm.symbol_intern(description.clone());
            JsValue::symbol_realm(realm_id, idx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use oxide_vm::vm::Vm;

    /// 编译并执行单脚本，返回 VM 与完成值。
    fn eval(source: &str) -> Result<(Vm, JsValue), String> {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse: {:?}", e))?;
        let module = oxide_compiler::compiler::Compiler::new()
            .compile(&program)
            .map_err(|e| format!("compile: {e}"))?;
        let mut vm = Vm::new();
        let result = vm.run(&Arc::new(module))?;
        Ok((vm, result))
    }

    /// 在源 realm detach 一个值（空 transfer 集合）。
    fn detach(vm: &mut Vm, value: JsValue) -> Result<MessageValue, JsValue> {
        let empty: HashSet<*const JsObject> = HashSet::new();
        detach_message(vm, value, &empty)
    }

    /// 在目标 realm rehydrate 一个 `MessageValue`。
    fn rehydrate(vm: &mut Vm, value: &MessageValue) -> JsValue {
        rehydrate_message(vm, value)
    }

    /// 读对象自身属性值（按名称），无则 undefined。
    fn obj_prop(vm: &Vm, obj: &JsObject, name: &str) -> JsValue {
        let si = vm.perm_intern(name);
        match vm.get_own_property_slot(obj, si) {
            Some(idx) => obj.get_prop_at(idx),
            None => JsValue::undefined(),
        }
    }

    #[test]
    fn detach_rehydrate_primitives() {
        for src in ["42", "'hello'", "true", "null", "undefined", "1.5", "10n"] {
            let (mut vm, v) = eval(src).unwrap();
            let mv = detach(&mut vm, v).unwrap();
            let mut target = Vm::new();
            let r = rehydrate(&mut target, &mv);
            // rehydrate 产值与源值同类型（跨 realm 原始值恒等）。
            assert_eq!(r.js_type(), v.js_type(), "rehydrate 类型应与源一致: {src}");
            match (src, &mv) {
                ("42", MessageValue::Number(n)) => assert_eq!(*n, 42.0),
                ("'hello'", MessageValue::String(u)) => {
                    assert_eq!(String::from_utf16(u).unwrap(), "hello")
                }
                ("true", MessageValue::Boolean(b)) => assert!(*b),
                ("null", MessageValue::Null) => {}
                ("undefined", MessageValue::Undefined) => {}
                ("1.5", MessageValue::Number(n)) => assert_eq!(*n, 1.5),
                ("10n", MessageValue::BigInt(b)) => assert_eq!(b.to_string(), "10"),
                _ => panic!("unexpected: {src} => {mv:?}"),
            }
        }
    }

    #[test]
    fn detach_rehydrate_object() {
        let (mut vm, v) = eval("var o = {a: 1, b: 'x', c: null}; o").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        assert!(r.is_object());
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert_eq!(obj_prop(&target, cl, "a"), JsValue::int(1));
        assert_eq!(target.lookup_str(obj_prop(&target, cl, "b")).unwrap(), "x");
        assert!(obj_prop(&target, cl, "c").is_null());
    }

    #[test]
    fn detach_rehydrate_array() {
        let (mut vm, v) = eval("var a = [1, 'two', null]; a").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_array());
        assert_eq!(cl.logical_len(), 3);
        assert_eq!(cl.get_prop_at(0), JsValue::int(1));
        assert_eq!(target.lookup_str(cl.get_prop_at(1)).unwrap(), "two");
        assert!(cl.get_prop_at(2).is_null());
    }

    #[test]
    fn detach_rehydrate_map() {
        let (mut vm, v) = eval("var m = new Map(); m.set('k', 1); m.set(2, 'v'); m").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_map());
        let inner = cl.native_data() as *const MapInner;
        assert!(!inner.is_null());
        let entries: Vec<(SetKey, JsValue)> = unsafe { (*inner).iter() }.collect();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn detach_rehydrate_set() {
        let (mut vm, v) = eval("var s = new Set([1, 'two', 3]); s").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_set());
        let inner = cl.native_data() as *const SetInner;
        assert!(!inner.is_null());
        let entries: Vec<SetKey> = unsafe { (*inner).iter() }.copied().collect();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn detach_rehydrate_date() {
        let (mut vm, v) = eval("var d = new Date(1234567890); d").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_date_obj());
        assert_eq!(cl.get_prop_at(0), JsValue::float(1234567890.0));
    }

    #[test]
    fn detach_rehydrate_regexp() {
        let (mut vm, v) = eval("var r = /abc/gi; r").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_regexp_obj());
        assert_eq!(target.lookup_str(cl.get_regexp_source()).unwrap(), "abc");
        let flags = target.lookup_str(cl.get_regexp_flags()).unwrap();
        assert!(flags.contains('g') && flags.contains('i'), "flags 应含 g 与 i: {flags}");
    }

    #[test]
    fn detach_rehydrate_error() {
        let (mut vm, v) = eval("var e = new TypeError('boom'); e").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_error_obj());
        assert_eq!(target.lookup_str(obj_prop(&target, cl, "message")).unwrap(), "boom");
    }

    #[test]
    fn detach_rehydrate_arraybuffer_clone() {
        let (mut vm, v) = eval("var ab = new Uint8Array([1, 2, 3, 4]).buffer; ab").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        assert!(cl.is_array_buffer_obj());
        let ptr = crate::array_buffer::array_buffer_payload_ptr(cl).unwrap();
        // SAFETY: ptr 经 array_buffer_payload_ptr 校验为存活载荷盒。
        assert_eq!(unsafe { &*ptr }.bytes.as_slice(), &[1, 2, 3, 4]);
    }

    #[test]
    fn detach_rehydrate_arraybuffer_transfer() {
        let (mut vm, v) = eval("var ab = new Uint8Array([9, 8, 7]).buffer; ab").unwrap();
        let src_ptr = v.as_js_object_ptr() as *const JsObject;
        let mut transfer: HashSet<*const JsObject> = HashSet::new();
        transfer.insert(src_ptr);
        let mv = detach_message(&mut vm, v, &transfer).unwrap();
        // 源缓冲已 detach。
        let src = unsafe { &*v.as_js_object_ptr() };
        let src_ptr2 = crate::array_buffer::array_buffer_payload_ptr(src).unwrap();
        // SAFETY: src_ptr2 经校验为存活载荷盒。
        assert!(unsafe { &*src_ptr2 }.detached, "transfer 后源应 detach");
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        let cl = unsafe { &*r.as_js_object_ptr() };
        let ptr = crate::array_buffer::array_buffer_payload_ptr(cl).unwrap();
        // SAFETY: ptr 经校验为存活载荷盒。
        assert_eq!(unsafe { &*ptr }.bytes.as_slice(), &[9, 8, 7]);
    }

    #[test]
    fn detach_symbol_well_known() {
        let (mut vm, v) = eval("Symbol.iterator").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        assert!(r.is_symbol());
        // well-known 符号映目标 realm 同 local_index。
        assert_eq!(r.as_symbol_local_index(), 0);
    }

    #[test]
    fn detach_symbol_user() {
        let (mut vm, v) = eval("Symbol('desc')").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        let mut target = Vm::new();
        let r = rehydrate(&mut target, &mv);
        assert!(r.is_symbol());
        // 用户符号经描述重新 intern（目标 realm 新局部下标）。
        assert!(r.as_symbol_local_index() >= WELL_KNOWN_SYMBOL_COUNT);
    }

    #[test]
    fn detach_function_data_clone_error() {
        let (mut vm, v) = eval("function f() {}; f").unwrap();
        let e = detach(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn detach_promise_data_clone_error() {
        let (mut vm, v) = eval("Promise.resolve(1)").unwrap();
        let e = detach(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn detach_shared_array_buffer_data_clone_error() {
        let (mut vm, v) = eval("var sab = new SharedArrayBuffer(4); sab").unwrap();
        let e = detach(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn detach_cycle_data_clone_error() {
        let (mut vm, v) = eval("var a = {}; a.self = a; a").unwrap();
        let e = detach(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn detach_boxed_unboxed() {
        let (mut vm, v) = eval("new Boolean(true)").unwrap();
        let mv = detach(&mut vm, v).unwrap();
        assert!(matches!(mv, MessageValue::Boolean(true)));
    }

    #[test]
    fn message_value_is_send() {
        // 跨线程传递的前提：`MessageValue` 须满足 `Send`（编译期断言）。
        fn assert_send<T: Send>() {}
        assert_send::<MessageValue>();
    }
}
