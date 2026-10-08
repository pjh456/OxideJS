//! 结构化克隆核（structured clone algorithm）：递归遍历加 seen 映射保共享引用、
//! 逐类型分派、DataCloneError 报错路径。
//!
//! 本核以 `VmHost` 为接口、目标 realm 为分配上下文，做同 realm 克隆（源与目标
//! 同一 realm，符号映射为恒等）。可克隆类型白名单：原始值、数组、plain 对象、
//! Map、Set、Date、RegExp、Error 家族、缓冲区族（ArrayBuffer / SharedArrayBuffer /
//! TypedArray / DataView）；其余对象（function / DOM / 模块命名空间 / Promise /
//! 迭代器 / 特型对象）一律 DataCloneError。缓冲区克隆支持 transfer 转移：
//! 命中转移集合的 ArrayBuffer 移动载荷（源 detach），未命中克隆字节；
//! SharedArrayBuffer 克隆共享同一载荷盒。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::array::create_new_array;
use crate::map::MapInner;
use crate::object::{walk_own_keys, walk_own_symbol_keys};
use crate::set::{SetInner, SetKey};

/// 克隆状态：`seen` 映射保共享引用（同源对象二次出现复用同一克隆，循环引用不
/// 死循环）；`transfer` 集合枚举待转移的 ArrayBuffer（命中移动载荷，未命中
/// 克隆字节）。
struct CloneState {
    seen: HashMap<*const JsObject, *mut JsObject>,
    transfer: HashSet<*const JsObject>,
}

impl CloneState {
    fn new() -> Self {
        Self {
            seen: HashMap::new(),
            transfer: HashSet::new(),
        }
    }
}

/// `structuredClone(value, options)` 入口：解析 value 与 options.transfer，
/// 建克隆状态，调核心。
///
/// # 步骤
/// 1. 取第一实参 value（缺省 undefined）
/// 2. 取第二实参 options（缺省 undefined）；在场时解析 transfer 列表
/// 3. 建 `CloneState`
/// 4. 调 `clone_value` 递归克隆
///
/// # 边界与前提
/// - options 非对象 → TypeError；`transfer` 非数组 → TypeError；元素非
///   ArrayBuffer 或重复 → DataCloneError；
/// - 返回 `Ok(克隆值)` 或 `Err(TypeError / DataCloneError)`。
pub fn structured_clone_entry<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut state = CloneState::new();
    if !options.is_undefined() {
        if let Err(e) = parse_transfer_list(vm, options, &mut state) {
            return NativeResult::Err(e);
        }
    }
    match clone_value(vm, &mut state, value) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::Err(e),
    }
}

/// 解析 options.transfer 入 transfer 集合：options 须为对象，`transfer` 须为
/// 数组，元素须为不重复的 ArrayBuffer（缺省 `transfer` 为空集）。
///
/// # 边界与前提
/// - `transfer` 属性读取是用户代码窗口（getter 副作用与异常原值传播）；
/// - 转移集合只收 ArrayBuffer 对象指针（SAB 不可转移，非 AB 元素报
///   DataCloneError）。
fn parse_transfer_list<H: VmHost>(vm: &mut H, options: JsValue, state: &mut CloneState) -> Result<(), JsValue> {
    if !options.is_object() {
        return Err(crate::error::create_type_error(vm, "structuredClone options must be an object"));
    }
    let opt_ptr = options.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let opt = unsafe { &*opt_ptr };
    let si_transfer = vm.kernel_core().perm_interner().intern("transfer").0;
    let transfer_val = match vm.ordinary_get(opt, si_transfer, options) {
        Ok(v) => v,
        Err(e) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, &e));
        }
    };
    if transfer_val.is_undefined() {
        return Ok(());
    }
    if !transfer_val.is_object() || !unsafe { &*transfer_val.as_js_object_ptr() }.is_array() {
        return Err(crate::error::create_type_error(vm, "transfer list must be an array"));
    }
    let list = unsafe { &*transfer_val.as_js_object_ptr() };
    let count = list.logical_len() as usize;
    for i in 0..count {
        let e = list.get_prop_at(i);
        if !e.is_object() || !unsafe { &*e.as_js_object_ptr() }.is_array_buffer_obj() {
            return Err(data_clone_error(vm, "transfer list element is not an ArrayBuffer"));
        }
        let ptr = e.as_js_object_ptr() as *const JsObject;
        if !state.transfer.insert(ptr) {
            return Err(data_clone_error(vm, "duplicate buffer in transfer list"));
        }
    }
    Ok(())
}

/// 递归克隆一个值：原始值原样拷贝，对象按类型分派。
///
/// # 步骤
/// 1. 非对象（原始值）原样返回
/// 2. 对象查 `seen` 映射：命中返回既有克隆（保共享引用与循环）
/// 3. 按类型分配克隆空壳（不可克隆类型在此报 DataCloneError）
/// 4. 登记 `seen`（先于递归填充，循环引用不重入）
/// 5. 递归填充克隆内容
///
/// # 边界与前提
/// - 源对象由调用方寄存器保活；克隆经 `alloc_object` 入目标 realm 对象表即成根；
/// - 可克隆白名单：数组 / plain / Map / Set / Date / RegExp / Error / 缓冲区族，
///   其余对象（function / DOM / 模块命名空间 / Promise / 迭代器 / 特型对象）
///   报 DataCloneError。
fn clone_value<H: VmHost>(vm: &mut H, state: &mut CloneState, value: JsValue) -> Result<JsValue, JsValue> {
    if !value.is_object() {
        return Ok(value);
    }
    let src_ptr = value.as_js_object_ptr();
    if let Some(&existing) = state.seen.get(&(src_ptr as *const JsObject)) {
        return Ok(JsValue::from_js_object(existing));
    }
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let src = unsafe { &*src_ptr };
    let clone_ptr = if src.is_array() {
        alloc_array_clone(vm, src)
    } else if src.is_map() {
        alloc_map_clone(vm)
    } else if src.is_set() {
        alloc_set_clone(vm)
    } else if src.is_date_obj() {
        alloc_date_clone(vm, src)
    } else if src.is_regexp_obj() {
        alloc_regexp_clone(vm, src)?
    } else if src.is_error_obj() {
        alloc_error_clone(vm, src)
    } else if src.is_array_buffer_obj() {
        alloc_array_buffer_clone(vm, state, src, src_ptr as *const JsObject)?
    } else if src.is_shared_array_buffer_obj() {
        alloc_shared_array_buffer_clone(vm, src)?
    } else if src.is_typed_array_obj() {
        alloc_typed_array_clone(vm, state, src)?
    } else if src.is_data_view_obj() {
        alloc_data_view_clone(vm, state, src)?
    } else if is_plain_object(src) {
        alloc_plain_clone(vm)
    } else {
        return Err(data_clone_error(vm, "value is not cloneable"));
    };
    state.seen.insert(src_ptr as *const JsObject, clone_ptr);
    fill_clone(vm, state, src, clone_ptr)?;
    Ok(JsValue::from_js_object(clone_ptr))
}

/// 填充克隆内容：Map / Set 走各自条目复制，数组 / plain / Error 统一走自有属性复制。
/// Date / RegExp / 缓冲区族在分配时已填充（时间戳 / 源与标志 / 载荷与视图状态盒），
/// 此处无操作。
fn fill_clone<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, clone_ptr: *mut JsObject,
) -> Result<(), JsValue> {
    if src.is_map() {
        return fill_map(vm, state, src, clone_ptr);
    }
    if src.is_set() {
        return fill_set(vm, state, src, clone_ptr);
    }
    // Date / RegExp 在分配时已填充（时间戳 / 源与标志与 lastIndex），不再复制自有属性。
    if src.is_date_obj() || src.is_regexp_obj() {
        return Ok(());
    }
    // 缓冲区族（AB / SAB / TypedArray / DataView）在分配时已填充载荷与视图
    // 状态盒，无自有属性可复制。
    if is_buffer(src) {
        return Ok(());
    }
    fill_object_props(vm, state, src, clone_ptr)
}

/// 复制自有属性（数组元素 / 命名属性 / Symbol 键）到克隆：逐键读源值、克隆、
/// 按源描述符定义到克隆。
///
/// # 边界与前提
/// - 访问器属性经 getter 取值（用户代码窗口），数据属性直读槽值；
/// - 数组元素键经 `define_data_property` 路由到元素区，命名 / Symbol 键路由到
///   命名属性区；数组 hole 在克隆中落为在位 undefined（稀疏数组边界，后续补齐）。
fn fill_object_props<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, clone_ptr: *mut JsObject,
) -> Result<(), JsValue> {
    let src_val = JsValue::from_js_object(src as *const JsObject as *mut JsObject);
    let str_keys = walk_own_keys(vm, src);
    let sym_keys = walk_own_symbol_keys(vm, src);
    for (si, store) in str_keys {
        let val = read_own_value(vm, src, src_val, si, store)?;
        let cloned = clone_value(vm, state, val)?;
        let attrs = prop_attributes_of(src, store);
        define_on_clone(vm, clone_ptr, si, cloned, attrs)?;
    }
    for (si, store) in sym_keys {
        let val = read_own_value(vm, src, src_val, si, store)?;
        let cloned = clone_value(vm, state, val)?;
        let attrs = prop_attributes_of(src, store);
        define_on_clone(vm, clone_ptr, si, cloned, attrs)?;
    }
    Ok(())
}

/// 读源对象自有属性的值：数据属性直读槽值，访问器属性经 getter 取值。
///
/// # 边界与前提
/// - getter 抛出的用户异常经 `take_uncaught_value` 原值恢复；
/// - 无元数据槽（默认数据属性）按 `get_prop_at` 取值。
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

/// 取源对象在 store 槽的属性描述符标志；无元数据时回退默认数据描述符。
fn prop_attributes_of(src: &JsObject, store: u32) -> PropAttributes {
    src.prop_meta_at(store)
        .map(|m| m.attributes)
        .unwrap_or(PropAttributes::DEFAULT_DATA)
}

/// 把克隆值按描述符定义到克隆对象。
///
/// # 副作用
/// - 更新克隆对象的 shape 链与属性表。
fn define_on_clone<H: VmHost>(
    vm: &mut H, clone_ptr: *mut JsObject, si: u32, val: JsValue, attrs: PropAttributes,
) -> Result<(), JsValue> {
    // SAFETY: clone_ptr 是本克隆核新分配的对象，存活且本段无别名。
    let clone = unsafe { &mut *clone_ptr };
    match vm.define_data_property(clone, si, val, attrs) {
        Ok(()) => Ok(()),
        Err(e) => Err(crate::error::create_type_error(vm, &e)),
    }
}

/// 复制 Map 条目到克隆：逐条克隆键值后插入克隆的 `MapInner`。
fn fill_map<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, clone_ptr: *mut JsObject,
) -> Result<(), JsValue> {
    let src_inner = src.native_data() as *const MapInner;
    if src_inner.is_null() {
        return Ok(());
    }
    // SAFETY: clone_ptr 是本克隆核新分配的 Map 对象，native_data 槽已写入非空盒。
    let clone_inner = unsafe { (*clone_ptr).native_data() } as *mut MapInner;
    if clone_inner.is_null() {
        return Ok(());
    }
    // 先收集条目（迭代器持有 &MapInner 借用，克隆递归前须结束借用）。
    let entries: Vec<(SetKey, JsValue)> = unsafe { (*src_inner).iter() }.collect();
    for (key, value) in entries {
        let cloned_key = clone_value(vm, state, key.0)?;
        let cloned_value = clone_value(vm, state, value)?;
        // SAFETY: clone_inner 为本克隆核新分配的 MapInner，存活且单线程独占。
        unsafe { (*clone_inner).insert(SetKey(cloned_key), cloned_value) };
    }
    Ok(())
}

/// 复制 Set 元素到克隆：逐个克隆后插入克隆的 `SetInner`。
fn fill_set<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, clone_ptr: *mut JsObject,
) -> Result<(), JsValue> {
    let src_inner = src.native_data() as *const SetInner;
    if src_inner.is_null() {
        return Ok(());
    }
    // SAFETY: clone_ptr 是本克隆核新分配的 Set 对象，native_data 槽已写入非空盒。
    let clone_inner = unsafe { (*clone_ptr).native_data() } as *mut SetInner;
    if clone_inner.is_null() {
        return Ok(());
    }
    // 先收集元素（迭代器持有 &SetInner 借用，克隆递归前须结束借用）。
    let entries: Vec<SetKey> = unsafe { (*src_inner).iter() }.copied().collect();
    for key in entries {
        let cloned_key = clone_value(vm, state, key.0)?;
        // SAFETY: clone_inner 为本克隆核新分配的 SetInner，存活且单线程独占。
        unsafe { (*clone_inner).insert(SetKey(cloned_key)) };
    }
    Ok(())
}

/// 分配 plain 对象克隆（proto = %Object.prototype%）。
fn alloc_plain_clone<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)))
}

/// 分配数组克隆（同长度，元素区初始为在位 undefined）。
fn alloc_array_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> *mut JsObject {
    create_new_array(vm, src.logical_len() as usize)
}

/// 分配 Map 克隆（空 `MapInner`，proto = %Map.prototype%）。
fn alloc_map_clone<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let map_proto = vm.session().builtin_world().map_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
    obj.set_map(true);
    let inner = Box::into_raw(Box::new(MapInner::new()));
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

/// 分配 Set 克隆（空 `SetInner`，proto = %Set.prototype%）。
fn alloc_set_clone<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let set_proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    obj.set_set(true);
    let inner = Box::into_raw(Box::new(SetInner::new()));
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

/// 分配 Date 克隆（同时间戳，proto = %Date.prototype%）。
fn alloc_date_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> *mut JsObject {
    let proto = vm.session().builtin_world().date_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_DATE;
    obj.set_prop_at(0, src.get_prop_at(0));
    vm.alloc_object(obj)
}

/// 分配 RegExp 克隆（同源与标志重编译，proto = %RegExp.prototype%）。
///
/// # 边界与前提
/// - 源模式非法（理论上不发生，源已是合法 RegExp）时抛 SyntaxError；
/// - lastIndex 初始化为 0（writable / 非枚举 / 非可配置）。
fn alloc_regexp_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> Result<*mut JsObject, JsValue> {
    let proto = vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    let source_val = src.get_regexp_source();
    let flags_val = src.get_regexp_flags();
    let source_text = vm.lookup_str(source_val).unwrap_or_default();
    let flags_text = vm.lookup_str(flags_val).unwrap_or_default();
    match regress::Regex::with_flags(source_text.as_str(), flags_text.as_str()) {
        Ok(re) => {
            let re_ptr = Box::into_raw(Box::new(re));
            // SAFETY: re_ptr 是 `Box<regress::Regex>` 指针，与 RegExp 构造器同形态，
            // 对象存活期间有效，由 `drop_regexp_native` 释放。
            obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(re_ptr as *const ()) }));
        }
        Err(e) => {
            return Err(crate::error::create_syntax_error(vm, &format!("Invalid regular expression: {e}")));
        }
    }
    obj.type_tag = JsObject::OBJ_TYPE_REGEXP;
    obj.set_regexp_source(source_val);
    obj.set_regexp_flags(flags_val);
    let si_lastindex = vm.kernel_core().perm_interner().intern("lastIndex").0;
    let _ = vm.define_data_property(&mut obj, si_lastindex, JsValue::int(0), PropAttributes::new(true, false, false));
    Ok(vm.alloc_object(obj))
}

/// 分配 Error 克隆（原型与源一致 → 同类型新 Error，标记 Error 家族标签）。
fn alloc_error_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> *mut JsObject {
    let proto = src.proto();
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_ERROR;
    vm.alloc_object(obj)
}

/// 分配 ArrayBuffer 克隆：transfer 命中移动载荷（源 `data → None` 即 detach），
/// 未命中克隆字节；存储态上限与 immutable 标志原样拷贝，detached 源产 detached
/// 克隆。
fn alloc_array_buffer_clone<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, src_ptr: *const JsObject,
) -> Result<*mut JsObject, JsValue> {
    let Some(payload_ptr) = crate::array_buffer::array_buffer_payload_ptr(src) else {
        return Err(data_clone_error(vm, "ArrayBuffer internal state invalid"));
    };
    let transferred = state.transfer.contains(&src_ptr);
    // SAFETY: payload_ptr 经 array_buffer_payload_ptr 校验为存活载荷盒；标量
    // 拷出后借用即结束，不跨 JS 调用。
    let (data, max_byte_length, immutable) = unsafe {
        let p = &mut *payload_ptr;
        // 转移命中移动载荷（源 data → None）；未命中克隆字节。
        let data = if transferred { p.data.take() } else { p.data.clone() };
        (data, p.max_byte_length, p.immutable)
    };
    let proto = crate::array_buffer::default_array_buffer_proto(vm);
    let clone_ptr = alloc_buffer_object(vm, data, max_byte_length, immutable, proto);
    Ok(clone_ptr)
}

/// 分配携给定载荷形态（`data` 为 `None` 即 detached）的 ArrayBuffer 对象。
fn alloc_buffer_object<H: VmHost>(
    vm: &mut H, data: Option<Vec<u8>>, max_byte_length: usize, immutable: bool, proto: JsValue,
) -> *mut JsObject {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
    let payload = crate::array_buffer::ArrayBufferPayload { data, max_byte_length, immutable };
    let payload_ptr = Arc::into_raw(Arc::new(payload));
    // SAFETY: ArrayBuffer 对象不可调用，native_fn 槽复用为不透明载荷盒指针，
    // 与 new_array_buffer 的存储形态一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
    vm.alloc_object(obj)
}

/// 分配 SharedArrayBuffer 克隆：共享同一载荷盒（引用计数加一），两对象共享
/// 字节缓冲；SAB 无 detach 路径。
fn alloc_shared_array_buffer_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> Result<*mut JsObject, JsValue> {
    let Some(payload_ptr) = crate::array_buffer::shared_array_buffer_payload_ptr(src) else {
        return Err(data_clone_error(vm, "SharedArrayBuffer internal state invalid"));
    };
    // SAFETY: payload_ptr 经 shared_array_buffer_payload_ptr 校验为存活载荷盒；
    // 克隆共享同一载荷（引用计数加一），释放经计数归零统一收口。
    unsafe { Arc::increment_strong_count(payload_ptr) };
    let proto = crate::array_buffer::default_shared_array_buffer_proto(vm);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_SHARED_ARRAY_BUFFER;
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
    Ok(vm.alloc_object(obj))
}

/// 分配 TypedArray 克隆：递归克隆（或转移）底层缓冲，再按同类型 / 同偏移 /
/// 同长度构造新视图。
fn alloc_typed_array_clone<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject,
) -> Result<*mut JsObject, JsValue> {
    let Some(data_ptr) = src
        .native_fn()
        .map(|p| p.as_ptr() as *const crate::typed_array::TypedArrayData)
        .filter(|p| !p.is_null())
    else {
        return Err(data_clone_error(vm, "TypedArray internal state invalid"));
    };
    // SAFETY: data_ptr 经对象类型标签校验为存活状态盒；Copy 语义，不跨 JS 调用持借用。
    let data = unsafe { *data_ptr };
    let cloned_buffer = clone_value(vm, state, data.buffer)?;
    let proto = JsValue::from_js_object(crate::typed_array::typed_array_proto_ptr(vm, data.kind));
    let clone_ptr =
        crate::typed_array::create_typed_array(vm, data.kind, cloned_buffer, data.byte_offset, data.length, data.auto_length, proto);
    Ok(clone_ptr)
}

/// 分配 DataView 克隆：递归克隆（或转移）底层缓冲，再按同偏移 / 同长度构造
/// 新视图。
fn alloc_data_view_clone<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject,
) -> Result<*mut JsObject, JsValue> {
    let Some(data_ptr) = src
        .native_fn()
        .map(|p| p.as_ptr() as *const crate::data_view::DataViewData)
        .filter(|p| !p.is_null())
    else {
        return Err(data_clone_error(vm, "DataView internal state invalid"));
    };
    // SAFETY: data_ptr 经对象类型标签校验为存活状态盒；Copy 语义，不跨 JS 调用持借用。
    let data = unsafe { *data_ptr };
    let cloned_buffer = clone_value(vm, state, data.buffer)?;
    let proto = JsValue::from_js_object(vm.session().builtin_world().data_view_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_DATA_VIEW;
    let new_data = Box::into_raw(Box::new(crate::data_view::DataViewData {
        buffer: cloned_buffer,
        byte_offset: data.byte_offset,
        byte_length: data.byte_length,
        length_is_auto: data.length_is_auto,
    }));
    // SAFETY: DataView 对象不可调用，native_fn 槽存不透明 `Box<DataViewData>`
    // 指针，与构造器存储形态一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(new_data as *const ()) }));
    Ok(vm.alloc_object(obj))
}

/// 是否缓冲区对象（ArrayBuffer / SharedArrayBuffer / DataView / TypedArray）。
fn is_buffer(src: &JsObject) -> bool {
    src.is_array_buffer_obj() || src.is_shared_array_buffer_obj() || src.is_data_view_obj() || src.is_typed_array_obj()
}

/// 是否 plain 对象：类型标签为 PLAIN 且非函数、非模块命名空间（Map / Set 等
/// 特型对象在分派前已被拦截，不会到达本判定）。
fn is_plain_object(src: &JsObject) -> bool {
    src.type_tag == JsObject::OBJ_TYPE_PLAIN && !src.is_function() && !src.is_module_namespace()
}

/// 构造 DataCloneError 对象（统一报错入口）。
fn data_clone_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_kind_error(vm, "DataCloneError", msg)
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

    /// 把 value 置入寄存器并调克隆入口，返回克隆值或 DataCloneError。
    fn clone(vm: &mut Vm, value: JsValue) -> Result<JsValue, JsValue> {
        vm.set_reg(1, value);
        let args = [0u8, 1];
        match structured_clone_entry(vm, &args) {
            NativeResult::Ok(v) => Ok(v),
            NativeResult::Err(e) => Err(e),
            NativeResult::TailCall { .. } => unreachable!("structuredClone 不返回 TailCall"),
        }
    }

    /// 读对象自身属性值（按名称），无则 undefined。
    fn obj_prop(vm: &Vm, obj: &JsObject, name: &str) -> JsValue {
        let si = vm.kernel_core().perm_interner().intern(name).0;
        match vm.get_own_property_slot(obj, si) {
            Some(idx) => obj.get_prop_at(idx),
            None => JsValue::undefined(),
        }
    }

    #[test]
    fn clone_primitives_identity() {
        for src in ["42", "'hello'", "true", "null", "undefined", "1.5", "10n"] {
            let (mut vm, v) = eval(src).unwrap();
            let c = clone(&mut vm, v).unwrap();
            assert_eq!(c, v, "原始值应原样拷贝: {src}");
        }
    }

    #[test]
    fn clone_plain_object() {
        let (mut vm, v) = eval("var o = {a: 1, b: 'x', c: null}; o").unwrap();
        let c = clone(&mut vm, v).unwrap();
        assert!(c.is_object());
        assert!(!std::ptr::eq(v.as_js_object_ptr(), c.as_js_object_ptr()), "克隆应为新对象");
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(!cl.is_function());
        assert_eq!(obj_prop(&vm, cl, "a"), JsValue::int(1));
        assert_eq!(vm.lookup_str(obj_prop(&vm, cl, "b")).unwrap(), "x");
        assert!(obj_prop(&vm, cl, "c").is_null());
    }

    #[test]
    fn clone_array() {
        let (mut vm, v) = eval("var a = [1, 'two', null]; a").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_array());
        assert_eq!(cl.logical_len(), 3);
        assert_eq!(cl.get_prop_at(0), JsValue::int(1));
        assert_eq!(vm.lookup_str(cl.get_prop_at(1)).unwrap(), "two");
        assert!(cl.get_prop_at(2).is_null());
    }

    #[test]
    fn clone_map() {
        let (mut vm, v) = eval("var m = new Map(); m.set('k', 1); m.set(2, 'v'); m").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_map());
        let inner = cl.native_data() as *const MapInner;
        assert!(!inner.is_null());
        let entries: Vec<(SetKey, JsValue)> = unsafe { (*inner).iter() }.collect();
        assert_eq!(entries.len(), 2);
        // 键 'k' → 1，键 2 → 'v'（SameValueZero 键语义）。
        let mut found = [false; 2];
        for (key, val) in &entries {
            if key.0.is_string() && vm.lookup_str(key.0).as_deref() == Some("k") {
                found[0] = true;
                assert_eq!(*val, JsValue::int(1));
            }
            if key.0.is_int() && key.0.as_int() == 2 {
                found[1] = true;
                assert_eq!(vm.lookup_str(*val).unwrap(), "v");
            }
        }
        assert!(found[0] && found[1], "两个条目均应克隆: {entries:?}");
    }

    #[test]
    fn clone_set() {
        let (mut vm, v) = eval("var s = new Set([1, 'two', 3]); s").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_set());
        let inner = cl.native_data() as *const SetInner;
        assert!(!inner.is_null());
        let entries: Vec<SetKey> = unsafe { (*inner).iter() }.copied().collect();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn clone_date() {
        let (mut vm, v) = eval("var d = new Date(1234567890); d").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_date_obj());
        assert_eq!(cl.get_prop_at(0), JsValue::float(1234567890.0));
    }

    #[test]
    fn clone_regexp() {
        let (mut vm, v) = eval("var r = /abc/gi; r").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_regexp_obj());
        assert_eq!(vm.lookup_str(cl.get_regexp_source()).unwrap(), "abc");
        let flags = vm.lookup_str(cl.get_regexp_flags()).unwrap();
        assert!(flags.contains('g') && flags.contains('i'), "flags 应含 g 与 i: {flags}");
    }

    #[test]
    fn clone_error() {
        let (mut vm, v) = eval("var e = new TypeError('boom'); e").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_error_obj());
        assert!(!std::ptr::eq(v.as_js_object_ptr(), c.as_js_object_ptr()));
        assert_eq!(vm.lookup_str(obj_prop(&vm, cl, "message")).unwrap(), "boom");
        // 原型与源一致（同类型 Error）。
        assert!(std::ptr::eq(unsafe { &*v.as_js_object_ptr() }.proto().as_ptr(), cl.proto().as_ptr()));
    }

    #[test]
    fn clone_shared_reference() {
        let (mut vm, v) = eval("var a = {x: 1}; var b = {ref: a, ref2: a}; b").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_ref = vm.kernel_core().perm_interner().intern("ref").0;
        let si_ref2 = vm.kernel_core().perm_interner().intern("ref2").0;
        let ref1 = cl.get_prop_at(vm.get_own_property_slot(cl, si_ref).unwrap());
        let ref2 = cl.get_prop_at(vm.get_own_property_slot(cl, si_ref2).unwrap());
        assert!(ref1.is_object() && ref2.is_object());
        assert!(std::ptr::eq(ref1.as_js_object_ptr(), ref2.as_js_object_ptr()), "共享引用应映射到同一克隆");
    }

    #[test]
    fn clone_cycle() {
        let (mut vm, v) = eval("var a = {}; a.self = a; a").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_self = vm.kernel_core().perm_interner().intern("self").0;
        let self_ref = cl.get_prop_at(vm.get_own_property_slot(cl, si_self).unwrap());
        assert!(std::ptr::eq(self_ref.as_js_object_ptr(), c.as_js_object_ptr()), "循环引用应指向克隆自身");
    }

    #[test]
    fn clone_function_data_clone_error() {
        let (mut vm, v) = eval("function f() {}; f").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
        assert!(vm.lookup_str(obj_prop(&vm, err, "message")).is_some());
    }

    /// 把 value 与 options 置入寄存器并调克隆入口，返回克隆值或错误。
    fn clone_with_options(vm: &mut Vm, value: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
        vm.set_reg(1, value);
        vm.set_reg(2, options);
        let args = [0u8, 1, 2];
        match structured_clone_entry(vm, &args) {
            NativeResult::Ok(v) => Ok(v),
            NativeResult::Err(e) => Err(e),
            NativeResult::TailCall { .. } => unreachable!("structuredClone 不返回 TailCall"),
        }
    }

    /// 读 ArrayBuffer 载荷字节序列（detached 返回 None）。
    fn ab_bytes(ab: &JsObject) -> Option<Vec<u8>> {
        let ptr = crate::array_buffer::array_buffer_payload_ptr(ab)?;
        // SAFETY: ptr 经 array_buffer_payload_ptr 校验为存活载荷盒。
        unsafe { &*ptr }.data.as_ref().cloned()
    }

    /// 读 TypedArray 视图状态盒（Copy）。
    fn ta_data(ta: &JsObject) -> crate::typed_array::TypedArrayData {
        let ptr = ta.native_fn().map(|p| p.as_ptr() as *const crate::typed_array::TypedArrayData).unwrap();
        // SAFETY: ptr 经对象类型标签校验为存活状态盒。
        unsafe { *ptr }
    }

    #[test]
    fn clone_arraybuffer() {
        let (mut vm, v) = eval("var ab = new Uint8Array([1, 2, 3, 4]).buffer; ab").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_array_buffer_obj());
        assert!(!std::ptr::eq(v.as_js_object_ptr(), c.as_js_object_ptr()), "克隆应为新对象");
        assert_eq!(ab_bytes(cl), Some(vec![1, 2, 3, 4]));
        // 源缓冲未 detach。
        assert_eq!(ab_bytes(unsafe { &*v.as_js_object_ptr() }), Some(vec![1, 2, 3, 4]));
    }

    #[test]
    fn clone_arraybuffer_transfer() {
        let (mut vm, v) =
            eval("var ab = new Uint8Array([9, 8, 7]).buffer; var opts = {transfer: [ab]}; ({ab: ab, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ab = vm.kernel_core().perm_interner().intern("ab").0;
        let si_opts = vm.kernel_core().perm_interner().intern("opts").0;
        let ab = holder.get_prop_at(vm.get_own_property_slot(holder, si_ab).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let c = clone_with_options(&mut vm, ab, opts).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_array_buffer_obj());
        assert_eq!(ab_bytes(cl), Some(vec![9, 8, 7]));
        // 源缓冲已 detach。
        assert!(ab_bytes(unsafe { &*ab.as_js_object_ptr() }).is_none());
    }

    #[test]
    fn clone_typed_array() {
        let (mut vm, v) = eval("var ta = new Uint8Array([5, 6, 7]); ta").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_typed_array_obj());
        let data = ta_data(cl);
        let src_data = ta_data(unsafe { &*v.as_js_object_ptr() });
        assert!(!std::ptr::eq(data.buffer.as_js_object_ptr(), src_data.buffer.as_js_object_ptr()), "克隆缓冲应为新对象");
        assert_eq!(data.length, 3);
        assert_eq!(data.byte_offset, 0);
        let buf = unsafe { &*data.buffer.as_js_object_ptr() };
        assert_eq!(ab_bytes(buf), Some(vec![5, 6, 7]));
    }

    #[test]
    fn clone_typed_array_with_transfer() {
        let (mut vm, v) =
            eval("var ta = new Uint8Array([1, 2, 3]); var opts = {transfer: [ta.buffer]}; ({ta: ta, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ta = vm.kernel_core().perm_interner().intern("ta").0;
        let si_opts = vm.kernel_core().perm_interner().intern("opts").0;
        let ta = holder.get_prop_at(vm.get_own_property_slot(holder, si_ta).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let c = clone_with_options(&mut vm, ta, opts).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_typed_array_obj());
        let data = ta_data(cl);
        let buf = unsafe { &*data.buffer.as_js_object_ptr() };
        assert_eq!(ab_bytes(buf), Some(vec![1, 2, 3]));
        // 源缓冲已 detach。
        let src_data = ta_data(unsafe { &*ta.as_js_object_ptr() });
        let src_buf = unsafe { &*src_data.buffer.as_js_object_ptr() };
        assert!(ab_bytes(src_buf).is_none());
    }

    #[test]
    fn clone_data_view() {
        let (mut vm, v) = eval("var dv = new DataView(new Uint8Array([10, 20, 30, 40]).buffer); dv").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_data_view_obj());
        let data_ptr = cl.native_fn().map(|p| p.as_ptr() as *const crate::data_view::DataViewData).unwrap();
        // SAFETY: data_ptr 经对象类型标签校验为存活状态盒。
        let data = unsafe { *data_ptr };
        assert_eq!(data.byte_length, 4);
        assert_eq!(data.byte_offset, 0);
        let buf = unsafe { &*data.buffer.as_js_object_ptr() };
        assert_eq!(ab_bytes(buf), Some(vec![10, 20, 30, 40]));
    }

    #[test]
    fn clone_shared_array_buffer_shares_payload() {
        let (mut vm, v) = eval("var sab = new SharedArrayBuffer(4); sab").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_shared_array_buffer_obj());
        // 两对象共享同一载荷盒（克隆侧写入对源侧可见）。
        let src_payload = crate::array_buffer::shared_array_buffer_payload_ptr(unsafe { &*v.as_js_object_ptr() }).unwrap();
        let cl_payload = crate::array_buffer::shared_array_buffer_payload_ptr(cl).unwrap();
        assert!(std::ptr::eq(src_payload, cl_payload), "克隆应共享同一载荷");
        // SAFETY: 载荷盒存活。
        unsafe { (*cl_payload).data.as_mut().unwrap()[0] = 0xAB };
        assert_eq!(unsafe { &*src_payload }.data.as_ref().unwrap()[0], 0xAB);
    }

    #[test]
    fn transfer_non_arraybuffer_data_clone_error() {
        let (mut vm, v) =
            eval("var sab = new SharedArrayBuffer(4); var opts = {transfer: [sab]}; ({v: sab, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.kernel_core().perm_interner().intern("v").0;
        let si_opts = vm.kernel_core().perm_interner().intern("opts").0;
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn transfer_duplicate_buffer_data_clone_error() {
        let (mut vm, v) =
            eval("var ab = new ArrayBuffer(4); var opts = {transfer: [ab, ab]}; ({v: ab, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.kernel_core().perm_interner().intern("v").0;
        let si_opts = vm.kernel_core().perm_interner().intern("opts").0;
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn transfer_options_not_object_type_error() {
        let (mut vm, v) = eval("1").unwrap();
        let e = clone_with_options(&mut vm, v, JsValue::int(42)).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn transfer_list_not_array_type_error() {
        let (mut vm, v) = eval("var opts = {transfer: 'x'}; ({v: 1, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.kernel_core().perm_interner().intern("v").0;
        let si_opts = vm.kernel_core().perm_interner().intern("opts").0;
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_promise_data_clone_error() {
        let (mut vm, v) = eval("Promise.resolve(1)").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }
}
