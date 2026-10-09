//! 结构化克隆核（structured clone algorithm）：递归遍历加 seen 映射保共享引用、
//! 逐类型分派、DataCloneError 报错路径。
//!
//! 本核以 `VmHost` 为接口、目标 realm 为分配上下文，做同 realm 克隆（源与目标
//! 同一 realm，符号映射为恒等）。可克隆类型白名单：原始值（symbol 除外）、
//! 装箱对象（Boolean / Number / String 盒与 BigInt 包装）、数组、plain 对象、
//! Map、Set、Date、RegExp、Error 家族、缓冲区族（ArrayBuffer / SharedArrayBuffer /
//! TypedArray / DataView）；symbol 原始值、装箱 Symbol 盒及其余对象
//! （function / DOM / 模块命名空间 / Promise / 迭代器 / 特型对象）一律
//! DataCloneError。plain 对象与数组只复制可枚举自有属性，克隆侧描述符一律
//! writable / enumerable / configurable 全真数据属性。缓冲区克隆支持 transfer
//! 转移：转移条目在序列化前注册为 seen 映射占位（空载荷），整个序列化成功后
//! 才移动源载荷入克隆（源 detach）；值图外条目同样 detach；值图内 detached
//! 缓冲报 DataCloneError。未转移的 ArrayBuffer 克隆字节；SharedArrayBuffer
//! 克隆共享同一载荷盒。
//!
//! 全局入口（`structuredClone`）按 HTML 规范口径收口：options 经 ToObject
//! 容许（null 抛 TypeError、非 null 原始值装箱后无 transfer 即空转移）；
//! transfer 列表经 ToList 收一切可迭代（数组 / 字符串 / 生成器 / Map / Set）；
//! DataCloneError 的 `name` 归一为 "DataCloneError"；Error 臂克隆原型按归一化
//! name 回落标准 Error 族原型（非标准名取 %Error.prototype%）、message 仅取
//! 自有数据描述符槽值并 ToString（访问器不复制）；ArrayBuffer 克隆是新缓冲
//! （不继承 immutable 标志）。

use std::collections::HashMap;
use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use oxide_runtime_api::{to_string_full, NativeResult, ProtoKind, VmHost};

use crate::array::create_new_array;
use crate::map::MapInner;
use crate::object::{walk_own_keys, walk_own_symbol_keys};
use crate::set::{SetInner, SetKey};

/// 克隆状态：`seen` 映射保共享引用（同源对象二次出现复用同一克隆，循环引用不
/// 死循环）；`transfer_placeholders` 记录转移条目（源指针、克隆指针）——克隆体
/// 是空载荷占位，整个序列化成功后才移动源载荷入克隆（源 detach）。
struct CloneState {
    seen: HashMap<*const JsObject, *mut JsObject>,
    transfer_placeholders: Vec<(*const JsObject, *mut JsObject)>,
}

impl CloneState {
    fn new() -> Self {
        Self {
            seen: HashMap::new(),
            transfer_placeholders: Vec::new(),
        }
    }
}

/// `structuredClone(value, options)` 入口：解析 value 与 options.transfer，
/// 建克隆状态，调核心，成功后完成转移（源 detach 并填克隆载荷）。
///
/// # 步骤
/// 1. 取第一实参 value（缺省 undefined）
/// 2. 取第二实参 options（缺省 undefined）；在场时解析 transfer 列表（注册占位）
/// 3. 建 `CloneState`
/// 4. 调 `clone_value` 递归克隆
/// 5. 成功后对每个转移条目完成转移（源 detach 并填克隆载荷）
///
/// # 边界与前提
/// - options 经 ToObject 容许：undefined 无 transfer、null 抛 TypeError、
///   非 null 原始值装箱后无 transfer（空转移）、对象经 Get 读 transfer；
/// - `transfer` 经 ToList 收一切可迭代；元素非 ArrayBuffer 或重复 →
///   DataCloneError；不可迭代对象经 GetIterator 抛 TypeError；
/// - 序列化中值图内 detached 缓冲 → DataCloneError（源保持未 detach）；
/// - 序列化成功后转移条目 detached → DataCloneError；
/// - 返回 `Ok(克隆值)` 或 `Err(TypeError / DataCloneError)`。
pub fn structured_clone_entry<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut state = CloneState::new();
    // options 经 ToObject 容许：仅对象读 transfer；null 抛 TypeError；
    // 非 null 原始值装箱后无 transfer 自有属性，等价空转移。
    if options.is_object() {
        if let Err(e) = parse_transfer_list(vm, options, &mut state) {
            return NativeResult::Err(e);
        }
    } else if options.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "structuredClone options must be an object"));
    }
    let cloned = match clone_value(vm, &mut state, value) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if let Err(e) = finish_transfer(vm, &mut state) {
        return NativeResult::Err(e);
    }
    NativeResult::Ok(cloned)
}

/// 解析 options.transfer 并把转移条目注册进 seen 映射：options 须为对象，
/// `transfer` 经 ToList 收一切可迭代（数组 / 字符串 / 生成器 / Map / Set），
/// 元素须为不重复的 ArrayBuffer（缺省 `transfer` 为空集）。每个元素分配空载荷
/// 占位克隆（存储态上限继承源、immutable 恒 false），登记 seen 映射并记入占位
/// 列表（源、克隆），序列化成功后才移动载荷。
///
/// # 边界与前提
/// - `transfer` 属性读取是用户代码窗口（getter 副作用与异常原值传播）；
/// - `transfer` 经 ToList 迭代：不可迭代对象经 GetIterator 抛 TypeError（原值
///   透传）；迭代中途抛错经 IteratorClose 后透传原异常；
/// - 转移条目只收 ArrayBuffer 对象（SAB 不可转移，非 AB 元素报
///   DataCloneError）；重复经 seen 映射判定（占位注册即重复检查）。
fn parse_transfer_list<H: VmHost>(vm: &mut H, options: JsValue, state: &mut CloneState) -> Result<(), JsValue> {
    let opt_ptr = options.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针（入口已过滤）；对象在 native 执行期间被根保持、不被回收。
    let opt = unsafe { &*opt_ptr };
    let si_transfer = vm.perm_intern("transfer");
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
    // ToList：经迭代协议逐元素消费（GetIterator 加步进循环加异常时 IteratorClose）。
    crate::iterator::iterate_elements(vm, transfer_val, |vm, e| {
        if !e.is_object() || !unsafe { &*e.as_js_object_ptr() }.is_array_buffer_obj() {
            return Err(data_clone_error(vm, "transfer list element is not an ArrayBuffer"));
        }
        let src_ptr = e.as_js_object_ptr() as *const JsObject;
        if state.seen.contains_key(&src_ptr) {
            return Err(data_clone_error(vm, "duplicate buffer in transfer list"));
        }
        // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
        let src = unsafe { &*e.as_js_object_ptr() };
        let Some(payload_ptr) = crate::array_buffer::array_buffer_payload_ptr(src) else {
            return Err(data_clone_error(vm, "ArrayBuffer internal state invalid"));
        };
        // SAFETY: payload_ptr 经 array_buffer_payload_ptr 校验为存活载荷盒；标量
        // 拷出后借用即结束，不跨 JS 调用。
        let max_byte_length = unsafe { (*payload_ptr).max_byte_length };
        let proto = crate::array_buffer::default_array_buffer_proto(vm);
        let clone_ptr = alloc_buffer_object(vm, Vec::new(), max_byte_length, false, proto);
        state.seen.insert(src_ptr, clone_ptr);
        state.transfer_placeholders.push((src_ptr, clone_ptr));
        Ok(())
    })
}

/// 序列化成功后完成转移：对每个转移条目校验源未 detach，再移动源载荷入克隆
/// 占位（源 detach）；值图外条目同样 detach。
///
/// # 副作用
/// - 源缓冲载荷移入克隆占位（源转 detached 态）；
/// - 克隆占位的空载荷被填为源载荷。
fn finish_transfer<H: VmHost>(vm: &mut H, state: &mut CloneState) -> Result<(), JsValue> {
    for (src_ptr, clone_ptr) in &state.transfer_placeholders {
        // SAFETY: 源对象由调用方寄存器保活（值图根），载荷指针本段有效。
        let src = unsafe { &**src_ptr };
        let Some(src_payload) = crate::array_buffer::array_buffer_payload_ptr(src) else {
            return Err(data_clone_error(vm, "ArrayBuffer internal state invalid"));
        };
        // SAFETY: clone_ptr 是本克隆核新分配的占位对象，载荷指针本段有效。
        let clone_payload = unsafe {
            (**clone_ptr)
                .native_fn()
                .map(|p| p.as_ptr() as *mut crate::array_buffer::ArrayBufferPayload)
                .unwrap()
        };
        // SAFETY: 两载荷指针均经校验为存活载荷盒；移动为标量操作，不跨 JS 调用。
        let src_detached = unsafe { (*src_payload).detached };
        // 转移条目 detached 不可转移（规范报 DataCloneError）。
        if src_detached {
            return Err(data_clone_error(vm, "detached buffer in transfer list"));
        };
        let bytes = unsafe { std::mem::take(&mut (*src_payload).bytes) };
        unsafe {
            // 源转 detached 态（字节区清空、标志置位），克隆占位填为源载荷。
            (*src_payload).detached = true;
            (*clone_payload).bytes = bytes;
            (*clone_payload).detached = false;
        };
    }
    Ok(())
}

/// 递归克隆一个值：symbol 原始值拒绝，其余原始值原样拷贝，对象按类型分派。
///
/// # 步骤
/// 1. 非对象值：symbol 原始值报 DataCloneError，其余原样返回
/// 2. 对象查 `seen` 映射：命中返回既有克隆（保共享引用与循环）
/// 3. 按类型分配克隆空壳（不可克隆类型在此报 DataCloneError）
/// 4. 登记 `seen`（先于递归填充，循环引用不重入）
/// 5. 递归填充克隆内容
///
/// # 边界与前提
/// - 源对象由调用方寄存器保活；克隆经 `alloc_object` 入目标 realm 对象表即成根；
/// - 可克隆白名单：装箱对象 / 数组 / plain / Map / Set / Date / RegExp / Error /
///   缓冲区族，其余对象（function / DOM / 模块命名空间 / Promise / 迭代器 /
///   特型对象）报 DataCloneError。
fn clone_value<H: VmHost>(vm: &mut H, state: &mut CloneState, value: JsValue) -> Result<JsValue, JsValue> {
    if !value.is_object() {
        // symbol 原始值不可克隆（规范枚举的拒绝面）。
        if value.is_symbol() {
            return Err(data_clone_error(vm, "symbol values are not cloneable"));
        }
        return Ok(value);
    }
    let src_ptr = value.as_js_object_ptr();
    if let Some(&existing) = state.seen.get(&(src_ptr as *const JsObject)) {
        return Ok(JsValue::from_js_object(existing));
    }
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let src = unsafe { &*src_ptr };
    let clone_ptr = if is_boxed_cloneable(src) {
        alloc_boxed_clone(vm, src)
    } else if src.is_symbol_obj() {
        return Err(data_clone_error(vm, "symbol objects are not cloneable"));
    } else if src.is_date_obj() {
        alloc_date_clone(vm, src)
    } else if src.is_regexp_obj() {
        alloc_regexp_clone(vm, src)?
    } else if src.is_array_buffer_obj() {
        alloc_array_buffer_clone(vm, src)?
    } else if src.is_shared_array_buffer_obj() {
        alloc_shared_array_buffer_clone(vm, src)?
    } else if src.is_typed_array_obj() {
        alloc_typed_array_clone(vm, state, src)?
    } else if src.is_data_view_obj() {
        alloc_data_view_clone(vm, state, src)?
    } else if src.is_map() {
        alloc_map_clone(vm)
    } else if src.is_set() {
        alloc_set_clone(vm)
    } else if src.is_error_obj() {
        alloc_error_clone(vm, src)?
    } else if src.is_array() {
        alloc_array_clone(vm, src)
    } else if is_plain_object(src) {
        alloc_plain_clone(vm)
    } else {
        return Err(data_clone_error(vm, "value is not cloneable"));
    };
    state.seen.insert(src_ptr as *const JsObject, clone_ptr);
    fill_clone(vm, state, src, clone_ptr)?;
    Ok(JsValue::from_js_object(clone_ptr))
}

/// 填充克隆内容：Map / Set 走各自条目复制，Error 走规范字段定义，
/// 数组 / plain 统一走可枚举自有属性复制。Date / RegExp / 装箱对象 /
/// 缓冲区族在分配时已填充（时间戳 / 源与标志 / 载荷与视图状态盒），此处无操作。
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
    // 装箱对象在分配时已填充载荷（String 盒含物化的字符索引与 length 面），
    // 不再复制自有属性。
    if is_boxed_cloneable(src) {
        return Ok(());
    }
    // 缓冲区族（AB / SAB / TypedArray / DataView）在分配时已填充载荷与视图
    // 状态盒，无自有属性可复制。
    if is_buffer(src) {
        return Ok(());
    }
    // Error 臂按规范显式定义 message（非枚举）与 cause（在场时全真数据属性），
    // 不走可枚举自有属性通用复制。
    if src.is_error_obj() {
        return fill_error(vm, state, src, clone_ptr);
    }
    fill_object_props(vm, state, src, clone_ptr)
}

/// 填充 Error 克隆：按规范 message 仅取自有数据描述符槽值并 ToString（访问器
/// 不复制、不触发 getter），定义为非枚举数据属性；cause 在场时经 `read_own_value`
/// 克隆为全真数据属性（CreateDataProperty 口径），其余自有属性不复制。
///
/// # 边界与前提
/// - message / cause 缺失（无对应自有属性）时不定义；
/// - message 为访问器时不定义（getter 不触发）；
/// - message 槽值经完整 ToString（对象值经 ToPrimitive 用户代码窗口，抛错原值
///   透传）；cause 值读取经 `read_own_value`（数据属性直读槽值，访问器经 getter）。
fn fill_error<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, clone_ptr: *mut JsObject,
) -> Result<(), JsValue> {
    let src_val = JsValue::from_js_object(src as *const JsObject as *mut JsObject);
    let si_msg = vm.perm_intern("message");
    if let Some(store) = vm.get_own_property_slot(src, si_msg) {
        // message 仅数据描述符复制：访问器不触发 getter、不复制。
        let is_accessor = src.prop_meta_at(store).is_some_and(|m| m.is_accessor);
        if !is_accessor {
            let slot_val = src.get_prop_at(store);
            let msg_str = match to_string_full(slot_val, vm) {
                Ok(s) => s,
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return Err(exc);
                    }
                    return Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
                }
            };
            let msg_val = vm.new_string(&msg_str);
            define_on_clone(vm, clone_ptr, si_msg, msg_val, PropAttributes::new(true, false, true))?;
        }
    }
    let si_cause = vm.perm_intern("cause");
    if let Some(store) = vm.get_own_property_slot(src, si_cause) {
        let val = read_own_value(vm, src, src_val, si_cause, store)?;
        let cloned = clone_value(vm, state, val)?;
        define_on_clone(vm, clone_ptr, si_cause, cloned, PropAttributes::DEFAULT_DATA)?;
    }
    Ok(())
}

/// 复制可枚举自有属性（数组元素 / 命名属性 / Symbol 键）到克隆：逐键读源值、
/// 克隆，克隆侧一律按 writable / enumerable / configurable 全真数据属性定义
/// （CreateDataProperty 口径，不保留源描述符标志）。
///
/// # 边界与前提
/// - 不可枚举自有属性不复制（不读值、不触发 getter）；
/// - 访问器属性经 getter 取值（用户代码窗口），数据属性直读槽值；
/// - 数组元素键经 `define_data_property` 路由到元素区，命名 / Symbol 键路由到
///   命名属性区；数组 hole 在克隆中落为在位 undefined（稀疏数组边界，后续补齐）。
fn fill_object_props<H: VmHost>(
    vm: &mut H, state: &mut CloneState, src: &JsObject, clone_ptr: *mut JsObject,
) -> Result<(), JsValue> {
    let src_val = JsValue::from_js_object(src as *const JsObject as *mut JsObject);
    // [[OwnPropertyKeys]] 触发（源对象，deferred namespace 先求值，cyclic 抛错）。
    // plain 对象判定已排除 module namespace，此触发对实际可达路径恒 no-op。
    if let Err(msg) = vm.ensure_deferred_ns_evaluation(src, None) {
        let exc = vm
            .take_uncaught_value()
            .unwrap_or_else(|| crate::error::create_from_text(vm, &msg));
        return Err(exc);
    }
    let str_keys = walk_own_keys(vm, src);
    let sym_keys = walk_own_symbol_keys(vm, src);
    for (si, store) in str_keys {
        if !is_enumerable_own(src, store) {
            continue;
        }
        let val = read_own_value(vm, src, src_val, si, store)?;
        let cloned = clone_value(vm, state, val)?;
        define_on_clone(vm, clone_ptr, si, cloned, PropAttributes::DEFAULT_DATA)?;
    }
    for (si, store) in sym_keys {
        if !is_enumerable_own(src, store) {
            continue;
        }
        let val = read_own_value(vm, src, src_val, si, store)?;
        let cloned = clone_value(vm, state, val)?;
        define_on_clone(vm, clone_ptr, si, cloned, PropAttributes::DEFAULT_DATA)?;
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

/// 判定 store 槽的自有属性是否可枚举（无元数据槽时按默认数据属性，可枚举）。
fn is_enumerable_own(src: &JsObject, store: u32) -> bool {
    src.prop_meta_at(store).map(|m| m.attributes.enumerable()).unwrap_or(true)
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
    let proto = vm.builtin_proto(ProtoKind::ObjectProto);
    vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)))
}

/// 分配装箱对象克隆：同类型标签、同原型、同被包载荷。String 盒复用构造期
/// 物化路径（字符索引与 length 面与构造器一致），其余装箱类型直写载荷。
fn alloc_boxed_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> *mut JsObject {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, src.proto());
    obj.type_tag = src.type_tag;
    let payload = src.boxed_value();
    if src.is_string_obj() {
        oxide_runtime_api::materialize_string_box(vm, &mut obj, payload);
    } else {
        obj.set_boxed_value(payload);
    }
    vm.alloc_object(obj)
}

/// 分配数组克隆（同长度，元素区初始为在位 undefined）。
fn alloc_array_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> *mut JsObject {
    create_new_array(vm, src.logical_len() as usize)
}

/// 分配 Map 克隆（空 `MapInner`，proto = %Map.prototype%）。
fn alloc_map_clone<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let map_proto = vm.builtin_proto(ProtoKind::MapProto);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
    obj.set_map(true);
    let inner = Box::into_raw(Box::new(MapInner::new()));
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

/// 分配 Set 克隆（空 `SetInner`，proto = %Set.prototype%）。
fn alloc_set_clone<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let set_proto = vm.builtin_proto(ProtoKind::SetProto);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    obj.set_set(true);
    let inner = Box::into_raw(Box::new(SetInner::new()));
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

/// 分配 Date 克隆（同时间戳，proto = %Date.prototype%）。
fn alloc_date_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> *mut JsObject {
    let proto = vm.builtin_proto(ProtoKind::DateProto);
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
    let proto = vm.builtin_proto(ProtoKind::RegExpProto);
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
    let si_lastindex = vm.perm_intern("lastIndex");
    let _ = vm.define_data_property(&mut obj, si_lastindex, JsValue::int(0), PropAttributes::new(true, false, false));
    Ok(vm.alloc_object(obj))
}

/// 分配 Error 克隆：name 经原型链 Get 归一化为标准 Error 族名，按归一化 name
/// 选标准原型（非标准名取 %Error.prototype%），标记 Error 家族标签。
///
/// # 边界与前提
/// - name 读取是用户代码窗口（getter 抛错经 `take_uncaught_value` 原值透传）；
/// - 克隆原型与源原型解耦（源子类原型丢弃，按规范反序列化口径回落标准原型）。
fn alloc_error_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> Result<*mut JsObject, JsValue> {
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
    // 归一化：七标准名取对应标准原型，其余（非字符串或自定义名）取 %Error.prototype%。
    let proto = match vm.lookup_str(name_val).as_deref() {
        Some("EvalError") => vm.builtin_proto(ProtoKind::EvalErrorProto),
        Some("RangeError") => vm.builtin_proto(ProtoKind::RangeErrorProto),
        Some("ReferenceError") => vm.builtin_proto(ProtoKind::ReferenceErrorProto),
        Some("SyntaxError") => vm.builtin_proto(ProtoKind::SyntaxErrorProto),
        Some("TypeError") => vm.builtin_proto(ProtoKind::TypeErrorProto),
        Some("URIError") => vm.builtin_proto(ProtoKind::UriErrorProto),
        _ => vm.builtin_proto(ProtoKind::ErrorProto),
    };
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_ERROR;
    Ok(vm.alloc_object(obj))
}

/// 分配 ArrayBuffer 克隆：克隆源字节（转移条目已被 seen 映射短路返回占位，
/// 不会到达此处）；存储态上限照抄，克隆是新缓冲（immutable 恒 false，不继承
/// 源标志），detached 源报 DataCloneError。
fn alloc_array_buffer_clone<H: VmHost>(vm: &mut H, src: &JsObject) -> Result<*mut JsObject, JsValue> {
    let Some(payload_ptr) = crate::array_buffer::array_buffer_payload_ptr(src) else {
        return Err(data_clone_error(vm, "ArrayBuffer internal state invalid"));
    };
    // SAFETY: payload_ptr 经 array_buffer_payload_ptr 校验为存活载荷盒；标量
    // 拷出后借用即结束，不跨 JS 调用。
    let (bytes, max_byte_length, detached) = unsafe {
        let p = &*payload_ptr;
        (p.bytes.clone(), p.max_byte_length, p.detached)
    };
    // 值图内 detached 缓冲不可克隆（规范报 DataCloneError）。
    if detached {
        return Err(data_clone_error(vm, "detached ArrayBuffer is not cloneable"));
    }
    let proto = crate::array_buffer::default_array_buffer_proto(vm);
    // 克隆是新缓冲：immutable 恒 false（不继承源标志），存储态上限照抄。
    let clone_ptr = alloc_buffer_object(vm, bytes, max_byte_length, false, proto);
    Ok(clone_ptr)
}

/// 分配携给定载荷形态（字节区、存储态上限、immutable 标志）的 ArrayBuffer 对象。
fn alloc_buffer_object<H: VmHost>(
    vm: &mut H, bytes: Vec<u8>, max_byte_length: usize, immutable: bool, proto: JsValue,
) -> *mut JsObject {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
    let payload = crate::array_buffer::ArrayBufferPayload {
        bytes,
        detached: false,
        max_byte_length,
        immutable,
    };
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
    let clone_ptr = crate::typed_array::create_typed_array(
        vm,
        data.kind,
        cloned_buffer,
        data.byte_offset,
        data.length,
        data.auto_length,
        proto,
    );
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
    let proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::DataViewProto));
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

/// 是否可克隆的装箱对象：Boolean / Number / String 盒，或 BigInt 包装
/// （PLAIN 标签加 `boxed_value` 存 bigint 载荷）。非装箱对象的 `boxed_value`
/// 恒为 undefined，BigInt 包装判定不会误命中普通 plain 对象。
fn is_boxed_cloneable(src: &JsObject) -> bool {
    src.is_boolean_obj()
        || src.is_number_obj()
        || src.is_string_obj()
        || (src.type_tag == JsObject::OBJ_TYPE_PLAIN && src.boxed_value().is_bigint())
}

/// 构造 DataCloneError 对象（统一报错入口）：`name` 归一为自有 "DataCloneError"
/// 数据属性（非枚举、不可写、可配置），覆盖原型链继承的 "Error"。
///
/// 供同 crate 的跨 realm 值传递抽象（`message_value`）复用。
pub(crate) fn data_clone_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    let err = crate::error::create_kind_error(vm, "DataCloneError", msg);
    let si_name = vm.perm_intern("name");
    let name_val = vm.new_string("DataCloneError");
    // 新建错误对象形状为空，define 恒成功；失败忽略（不影响异常值身份）。
    let _ = vm.define_data_property(
        unsafe { &mut *err.as_js_object_ptr() },
        si_name,
        name_val,
        PropAttributes::new(false, false, true),
    );
    err
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
        let si = vm.perm_intern(name);
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
        let si_ref = vm.perm_intern("ref");
        let si_ref2 = vm.perm_intern("ref2");
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
        let si_self = vm.perm_intern("self");
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
        let p = unsafe { &*ptr };
        if p.detached {
            None
        } else {
            Some(p.bytes.clone())
        }
    }

    /// 读 TypedArray 视图状态盒（Copy）。
    fn ta_data(ta: &JsObject) -> crate::typed_array::TypedArrayData {
        let ptr = ta
            .native_fn()
            .map(|p| p.as_ptr() as *const crate::typed_array::TypedArrayData)
            .unwrap();
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
            eval("var ab = new Uint8Array([9, 8, 7]).buffer; var opts = {transfer: [ab]}; ({ab: ab, opts: opts})")
                .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ab = vm.perm_intern("ab");
        let si_opts = vm.perm_intern("opts");
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
        assert!(
            !std::ptr::eq(data.buffer.as_js_object_ptr(), src_data.buffer.as_js_object_ptr()),
            "克隆缓冲应为新对象"
        );
        assert_eq!(data.length, 3);
        assert_eq!(data.byte_offset, 0);
        let buf = unsafe { &*data.buffer.as_js_object_ptr() };
        assert_eq!(ab_bytes(buf), Some(vec![5, 6, 7]));
    }

    #[test]
    fn clone_typed_array_with_transfer() {
        let (mut vm, v) =
            eval("var ta = new Uint8Array([1, 2, 3]); var opts = {transfer: [ta.buffer]}; ({ta: ta, opts: opts})")
                .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ta = vm.perm_intern("ta");
        let si_opts = vm.perm_intern("opts");
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
        let data_ptr = cl
            .native_fn()
            .map(|p| p.as_ptr() as *const crate::data_view::DataViewData)
            .unwrap();
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
        let src_payload =
            crate::array_buffer::shared_array_buffer_payload_ptr(unsafe { &*v.as_js_object_ptr() }).unwrap();
        let cl_payload = crate::array_buffer::shared_array_buffer_payload_ptr(cl).unwrap();
        assert!(std::ptr::eq(src_payload, cl_payload), "克隆应共享同一载荷");
        // 内层字节缓冲为同一 Arc（克隆侧写入对源侧可见）。
        assert!(
            std::ptr::eq(
                Arc::as_ptr(&unsafe { &*src_payload }.buffer),
                Arc::as_ptr(&unsafe { &*cl_payload }.buffer),
            ),
            "克隆应共享同一内层字节缓冲"
        );
        // SAFETY: 载荷盒存活。
        unsafe { (*cl_payload).buffer.write_range(0, &[0xAB]).unwrap() };
        assert_eq!(unsafe { &*src_payload }.buffer.as_slice()[0], 0xAB);
    }

    #[test]
    fn transfer_non_arraybuffer_data_clone_error() {
        let (mut vm, v) =
            eval("var sab = new SharedArrayBuffer(4); var opts = {transfer: [sab]}; ({v: sab, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.perm_intern("v");
        let si_opts = vm.perm_intern("opts");
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
        let si_v = vm.perm_intern("v");
        let si_opts = vm.perm_intern("opts");
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn options_non_object_primitive_clones() {
        // 非 null 原始值 options 装箱后无 transfer，等价空转移，克隆照常成功。
        let (mut vm, v) = eval("1").unwrap();
        let c = clone_with_options(&mut vm, v, JsValue::int(42)).unwrap();
        assert_eq!(c, JsValue::int(1));
        let (mut vm2, v2) = eval("1").unwrap();
        let str_opt = vm2.new_string("x");
        let c2 = clone_with_options(&mut vm2, v2, str_opt).unwrap();
        assert_eq!(c2, JsValue::int(1));
    }

    #[test]
    fn options_null_type_error() {
        // null options 经 ToObject(null) 抛 TypeError。
        let (mut vm, v) = eval("1").unwrap();
        let e = clone_with_options(&mut vm, v, JsValue::null()).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn transfer_string_iterable_data_clone_error() {
        // 字符串经 ToList 逐字符迭代，'x' 非 ArrayBuffer → DataCloneError。
        let (mut vm, v) = eval("var opts = {transfer: 'x'}; ({v: 1, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.perm_intern("v");
        let si_opts = vm.perm_intern("opts");
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
        assert_eq!(vm.lookup_str(obj_prop(&vm, err, "name")).unwrap(), "DataCloneError");
    }

    #[test]
    fn transfer_non_iterable_type_error() {
        // 不可迭代对象经 GetIterator 抛 TypeError。
        let (mut vm, v) = eval("var opts = {transfer: {}}; ({v: 1, opts: opts})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.perm_intern("v");
        let si_opts = vm.perm_intern("opts");
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn transfer_generator_iterable() {
        // 生成器经 ToList 迭代，产出 AB 转移成功（源 detach）。
        let (mut vm, v) = eval(
            "var ab = new Uint8Array([1, 2, 3]).buffer; \
             function* gen() { yield ab; } \
             var opts = {transfer: gen()}; \
             ({ab: ab, opts: opts})",
        )
        .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ab = vm.perm_intern("ab");
        let si_opts = vm.perm_intern("opts");
        let ab = holder.get_prop_at(vm.get_own_property_slot(holder, si_ab).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let c = clone_with_options(&mut vm, ab, opts).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert_eq!(ab_bytes(cl), Some(vec![1, 2, 3]));
        // 源缓冲已 detach。
        assert!(ab_bytes(unsafe { &*ab.as_js_object_ptr() }).is_none());
    }

    #[test]
    fn clone_promise_data_clone_error() {
        let (mut vm, v) = eval("Promise.resolve(1)").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        assert!(e.is_object());
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_symbol_primitive_data_clone_error() {
        let (mut vm, v) = eval("Symbol('x')").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_symbol_box_data_clone_error() {
        let (mut vm, v) = eval("Object(Symbol('x'))").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_boolean_box() {
        let (mut vm, v) = eval("new Boolean(true)").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_boolean_obj());
        assert!(!std::ptr::eq(v.as_js_object_ptr(), c.as_js_object_ptr()), "克隆应为新对象");
        assert_eq!(cl.boxed_value(), JsValue::bool(true));
    }

    #[test]
    fn clone_number_box() {
        let (mut vm, v) = eval("new Number(42)").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_number_obj());
        assert_eq!(cl.boxed_value(), JsValue::int(42));
    }

    #[test]
    fn clone_string_box() {
        let (mut vm, v) = eval("new String('hi')").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert!(cl.is_string_obj());
        assert_eq!(vm.lookup_str(cl.boxed_value()).unwrap(), "hi");
        // 字符索引与 length 面已物化（与构造器一致；字符索引为整数键，按物理槽位读）。
        assert_eq!(obj_prop(&vm, cl, "length"), JsValue::int(2));
        assert_eq!(vm.lookup_str(cl.get_prop_at(0)).unwrap(), "h");
        assert_eq!(vm.lookup_str(cl.get_prop_at(1)).unwrap(), "i");
    }

    #[test]
    fn clone_bigint_wrapper() {
        let (mut vm, v) = eval("Object(10n)").unwrap();
        let src = unsafe { &*v.as_js_object_ptr() };
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        // BigInt 包装是 PLAIN 标签加大整数载荷，克隆保标签与载荷。
        assert_eq!(cl.type_tag, JsObject::OBJ_TYPE_PLAIN);
        assert!(cl.boxed_value().is_bigint());
        assert!(oxide_runtime_api::same_value_zero(cl.boxed_value(), src.boxed_value()));
    }

    #[test]
    fn clone_class_instance() {
        let (mut vm, v) = eval("class C { constructor() { this.x = 1; } } new C()").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        // 克隆是 plain 对象：原型链丢弃，回落 Object 原型，自有属性带过。
        assert_eq!(cl.type_tag, JsObject::OBJ_TYPE_PLAIN);
        let object_proto = vm.builtin_proto(ProtoKind::ObjectProto) as *const u8;
        assert!(std::ptr::eq(cl.proto().as_ptr(), object_proto));
        assert_eq!(obj_prop(&vm, cl, "x"), JsValue::int(1));
    }

    #[test]
    fn clone_null_proto_object() {
        let (mut vm, v) = eval("Object.create(null)").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert_eq!(cl.type_tag, JsObject::OBJ_TYPE_PLAIN);
    }

    #[test]
    fn clone_weak_map_data_clone_error() {
        let (mut vm, v) = eval("var w = new WeakMap(); var k = {}; w.set(k, 1); w").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_generator_data_clone_error() {
        let (mut vm, v) = eval("function* g() { yield 1; } g()").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_frozen_plain_object_flattens_descriptors() {
        let (mut vm, v) = eval("var o = {a: 1}; Object.freeze(o); o").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si = vm.perm_intern("a");
        let store = vm.get_own_property_slot(cl, si).unwrap();
        let meta = cl.prop_meta_at(store).unwrap();
        assert_eq!(meta.attributes, PropAttributes::DEFAULT_DATA, "克隆侧描述符应 w/e/c 全真");
        assert_eq!(cl.get_prop_at(store), JsValue::int(1));
    }

    #[test]
    fn clone_skips_non_enumerable_own_property() {
        let (mut vm, v) =
            eval("var o = {a: 1}; Object.defineProperty(o, 'b', {value: 2, enumerable: false}); o").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert_eq!(obj_prop(&vm, cl, "a"), JsValue::int(1));
        let si_b = vm.perm_intern("b");
        assert!(vm.get_own_property_slot(cl, si_b).is_none(), "不可枚举自有属性不应复制");
    }

    #[test]
    fn clone_enumerable_symbol_key() {
        let (mut vm, v) = eval("var s = Symbol('k'); var o = {}; o[s] = 7; o").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let sym_keys = crate::object::walk_own_symbol_keys(&vm, cl);
        assert_eq!(sym_keys.len(), 1);
        let (_, store) = sym_keys[0];
        assert_eq!(cl.get_prop_at(store), JsValue::int(7));
    }

    #[test]
    fn clone_error_cause_flattened_to_data_property() {
        let (mut vm, v) = eval("var e = new Error('boom'); e.cause = {x: 1}; e").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_cause = vm.perm_intern("cause");
        let store = vm.get_own_property_slot(cl, si_cause).unwrap();
        let meta = cl.prop_meta_at(store).unwrap();
        assert_eq!(meta.attributes, PropAttributes::DEFAULT_DATA, "克隆侧 cause 应全真数据属性");
    }

    #[test]
    fn clone_error_subclass_instance() {
        let (mut vm, v) = eval("class E extends Error {} var e = new E('msg'); e").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        // 引擎给 Error 子类实例打 Error 家族标签，克隆走 Error 臂；name 归一
        // "Error"，原型回落 %Error.prototype%（源子类原型丢弃），message 带过。
        assert!(cl.is_error_obj());
        let error_proto = vm.builtin_proto(ProtoKind::ErrorProto) as *const u8;
        assert!(std::ptr::eq(cl.proto().as_ptr(), error_proto), "克隆原型应回落 %Error.prototype%");
        assert_eq!(vm.lookup_str(obj_prop(&vm, cl, "message")).unwrap(), "msg");
    }

    #[test]
    fn clone_error_standard_type_keeps_proto() {
        // 标准 TypeError 克隆：name 归一 "TypeError"，原型为 %TypeError.prototype%。
        let (mut vm, v) = eval("var e = new TypeError('boom'); e").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let type_proto = vm.builtin_proto(ProtoKind::TypeErrorProto) as *const u8;
        assert!(std::ptr::eq(cl.proto().as_ptr(), type_proto), "克隆原型应为 %TypeError.prototype%");
        assert_eq!(vm.lookup_str(obj_prop(&vm, cl, "message")).unwrap(), "boom");
    }

    /// 读 ArrayBuffer 载荷存储态上限（0 定长，非 0 为请求上限加 1）。
    fn ab_max_byte_length(ab: &JsObject) -> Option<usize> {
        let ptr = crate::array_buffer::array_buffer_payload_ptr(ab)?;
        // SAFETY: ptr 经 array_buffer_payload_ptr 校验为存活载荷盒。
        Some(unsafe { &*ptr }.max_byte_length)
    }

    /// 读 ArrayBuffer 载荷 immutable 标志。
    fn ab_immutable(ab: &JsObject) -> Option<bool> {
        let ptr = crate::array_buffer::array_buffer_payload_ptr(ab)?;
        // SAFETY: ptr 经 array_buffer_payload_ptr 校验为存活载荷盒。
        Some(unsafe { &*ptr }.immutable)
    }

    #[test]
    fn transfer_failure_does_not_detach() {
        let (mut vm, v) = eval(
            "var ab = new Uint8Array([1, 2, 3]).buffer; \
                   var opts = {transfer: [ab]}; \
                   ({ab: ab, fn: function f() {}, opts: opts})",
        )
        .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ab = vm.perm_intern("ab");
        let si_opts = vm.perm_intern("opts");
        let ab = holder.get_prop_at(vm.get_own_property_slot(holder, si_ab).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        // 克隆持有者对象（含函数值，不可克隆）→ DataCloneError。
        let e = clone_with_options(&mut vm, v, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
        // 克隆失败时转移未完成，源缓冲未 detach。
        assert_eq!(ab_bytes(unsafe { &*ab.as_js_object_ptr() }), Some(vec![1, 2, 3]));
    }

    #[test]
    fn transfer_outside_value_graph_detaches() {
        let (mut vm, v) = eval(
            "var ab = new Uint8Array([1, 2, 3]).buffer; var opts = {transfer: [ab]}; \
             ({v: 42, ab: ab, opts: opts})",
        )
        .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.perm_intern("v");
        let si_ab = vm.perm_intern("ab");
        let si_opts = vm.perm_intern("opts");
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let ab = holder.get_prop_at(vm.get_own_property_slot(holder, si_ab).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let c = clone_with_options(&mut vm, val, opts).unwrap();
        assert_eq!(c, JsValue::int(42));
        // 值图外转移条目同样 detach。
        assert!(ab_bytes(unsafe { &*ab.as_js_object_ptr() }).is_none());
    }

    #[test]
    fn clone_detached_arraybuffer_data_clone_error() {
        let (mut vm, v) = eval("var ab = new ArrayBuffer(4); $262.detachArrayBuffer(ab); ab").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn transfer_detached_buffer_data_clone_error() {
        let (mut vm, v) = eval(
            "var ab = new ArrayBuffer(4); $262.detachArrayBuffer(ab); \
             var opts = {transfer: [ab]}; ({v: 1, opts: opts})",
        )
        .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_v = vm.perm_intern("v");
        let si_opts = vm.perm_intern("opts");
        let val = holder.get_prop_at(vm.get_own_property_slot(holder, si_v).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let e = clone_with_options(&mut vm, val, opts).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
    }

    #[test]
    fn clone_map_key_cycle() {
        let (mut vm, v) = eval("var m = new Map(); var k = {}; k.m = m; m.set(k, 1); m").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let inner = cl.native_data() as *const MapInner;
        let entries: Vec<(SetKey, JsValue)> = unsafe { (*inner).iter() }.collect();
        assert_eq!(entries.len(), 1);
        // 克隆键引用克隆的 map（循环保持）。
        let key = &entries[0].0;
        assert!(key.0.is_object());
        let key_obj = unsafe { &*key.0.as_js_object_ptr() };
        let si_m = vm.perm_intern("m");
        let key_m = key_obj.get_prop_at(vm.get_own_property_slot(key_obj, si_m).unwrap());
        assert!(std::ptr::eq(key_m.as_js_object_ptr(), c.as_js_object_ptr()), "克隆键应引用克隆的 map");
    }

    #[test]
    fn clone_local_map_independent_calls() {
        let (mut vm, v) = eval("var o = {a: 1}; o").unwrap();
        let c1 = clone(&mut vm, v).unwrap();
        let c2 = clone(&mut vm, v).unwrap();
        assert!(
            !std::ptr::eq(c1.as_js_object_ptr(), c2.as_js_object_ptr()),
            "两次调用应产独立克隆（seen 映射本地）"
        );
    }

    #[test]
    fn clone_deep_cycle() {
        let (mut vm, v) = eval("var a = {}; var b = {}; a.b = b; b.a = a; a").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_b = vm.perm_intern("b");
        let b_clone = cl.get_prop_at(vm.get_own_property_slot(cl, si_b).unwrap());
        let b_obj = unsafe { &*b_clone.as_js_object_ptr() };
        let si_a = vm.perm_intern("a");
        let a_back = b_obj.get_prop_at(vm.get_own_property_slot(b_obj, si_a).unwrap());
        assert!(std::ptr::eq(a_back.as_js_object_ptr(), c.as_js_object_ptr()), "深循环应指回克隆的 a");
    }

    #[test]
    fn clone_shared_reference_and_cycle_mixed() {
        let (mut vm, v) = eval("var a = {x: 1}; var b = {ref: a, ref2: a}; b.self = b; b").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_ref = vm.perm_intern("ref");
        let si_ref2 = vm.perm_intern("ref2");
        let si_self = vm.perm_intern("self");
        let ref1 = cl.get_prop_at(vm.get_own_property_slot(cl, si_ref).unwrap());
        let ref2 = cl.get_prop_at(vm.get_own_property_slot(cl, si_ref2).unwrap());
        let self_ref = cl.get_prop_at(vm.get_own_property_slot(cl, si_self).unwrap());
        assert!(std::ptr::eq(ref1.as_js_object_ptr(), ref2.as_js_object_ptr()), "共享引用应映射到同一克隆");
        assert!(std::ptr::eq(self_ref.as_js_object_ptr(), c.as_js_object_ptr()), "循环应指向克隆自身");
    }

    #[test]
    fn transfer_resizable_preserves_max_byte_length() {
        let (mut vm, v) = eval(
            "var ab = new ArrayBuffer(4, {maxByteLength: 8}); var opts = {transfer: [ab]}; \
             ({ab: ab, opts: opts})",
        )
        .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ab = vm.perm_intern("ab");
        let si_opts = vm.perm_intern("opts");
        let ab = holder.get_prop_at(vm.get_own_property_slot(holder, si_ab).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let c = clone_with_options(&mut vm, ab, opts).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        // 克隆保留存储态上限（请求上限加 1）与源字节。
        assert_eq!(ab_max_byte_length(cl), Some(9));
        assert_eq!(ab_bytes(cl), Some(vec![0, 0, 0, 0]));
        // 源缓冲已 detach。
        assert!(ab_bytes(unsafe { &*ab.as_js_object_ptr() }).is_none());
    }

    #[test]
    fn transfer_clone_does_not_inherit_immutable() {
        let (mut vm, v) = eval(
            "var ab = new ArrayBuffer(4); ab.markImmutable(); var opts = {transfer: [ab]}; \
             ({ab: ab, opts: opts})",
        )
        .unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_ab = vm.perm_intern("ab");
        let si_opts = vm.perm_intern("opts");
        let ab = holder.get_prop_at(vm.get_own_property_slot(holder, si_ab).unwrap());
        let opts = holder.get_prop_at(vm.get_own_property_slot(holder, si_opts).unwrap());
        let c = clone_with_options(&mut vm, ab, opts).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        // 转移产物是新缓冲，不继承 immutable 标志。
        assert_eq!(ab_immutable(cl), Some(false));
        assert_eq!(ab_bytes(cl), Some(vec![0, 0, 0, 0]));
        assert!(ab_bytes(unsafe { &*ab.as_js_object_ptr() }).is_none());
    }

    #[test]
    fn data_clone_error_name() {
        // DataCloneError 的 name 归一为自有 "DataCloneError"。
        let (mut vm, v) = eval("function f() {}; f").unwrap();
        let e = clone(&mut vm, v).unwrap_err();
        let err = unsafe { &*e.as_js_object_ptr() };
        assert!(err.is_error_obj());
        assert_eq!(vm.lookup_str(obj_prop(&vm, err, "name")).unwrap(), "DataCloneError");
    }

    #[test]
    fn clone_error_message_accessor_not_copied() {
        // message 为访问器时不复制（getter 不触发），克隆无 message 自有属性。
        let (mut vm, v) = eval(
            "var e = new Error('base'); \
             Object.defineProperty(e, 'message', {get: function() { return 'from-getter'; }}); \
             e",
        )
        .unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_msg = vm.perm_intern("message");
        assert!(vm.get_own_property_slot(cl, si_msg).is_none(), "访问器 message 不应复制");
    }

    #[test]
    fn clone_error_message_number_to_string() {
        // 数据描述符 message 存 42（非字符串），克隆经 ToString 落 "42"。
        let (mut vm, v) = eval("var e = new Error('base'); e.message = 42; e").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert_eq!(vm.lookup_str(obj_prop(&vm, cl, "message")).unwrap(), "42");
    }

    #[test]
    fn clone_error_no_message_own() {
        // 无 message 自有属性的 Error，克隆无 message 自有属性。
        let (mut vm, v) = eval("var e = new Error('base'); delete e.message; e").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        let si_msg = vm.perm_intern("message");
        assert!(vm.get_own_property_slot(cl, si_msg).is_none());
    }

    #[test]
    fn clone_arraybuffer_does_not_inherit_immutable() {
        // 源 markImmutable() 后克隆：克隆是新缓冲（不继承 immutable），源仍 immutable。
        let (mut vm, v) = eval("var ab = new ArrayBuffer(4); ab.markImmutable(); ab").unwrap();
        let c = clone(&mut vm, v).unwrap();
        let cl = unsafe { &*c.as_js_object_ptr() };
        assert_eq!(ab_immutable(cl), Some(false), "克隆不应继承 immutable");
        assert_eq!(ab_bytes(cl), Some(vec![0, 0, 0, 0]));
        assert_eq!(ab_immutable(unsafe { &*v.as_js_object_ptr() }), Some(true), "源应仍 immutable");
    }

    #[test]
    fn structured_clone_global_metadata() {
        // 全局绑定面：length 为 1、name 为 "structuredClone"（防绑定面回归）。
        let (vm, v) = eval("({len: structuredClone.length, name: structuredClone.name})").unwrap();
        let holder = unsafe { &*v.as_js_object_ptr() };
        let si_len = vm.perm_intern("len");
        let si_name = vm.perm_intern("name");
        let len = holder.get_prop_at(vm.get_own_property_slot(holder, si_len).unwrap());
        let name = holder.get_prop_at(vm.get_own_property_slot(holder, si_name).unwrap());
        assert_eq!(len, JsValue::int(1));
        assert_eq!(vm.lookup_str(name).unwrap(), "structuredClone");
    }
}
