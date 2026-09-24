//! WeakMap 条目表：弱键 → 强值。
//!
//! 键为弱引用（GC mark 不产键边，死键由收集路径按转发表判定后丢条目），
//! 值为强引用（值边进 mark 边扫描与晋升改写）。存储盒经 `Box::into_raw` 挂
//! `JsObject.native_data`，GC 六站点（mark 边 / 移动式 sweep / 晋升克隆 /
//! 原地晋升 / drop / 字节账目）经本模块五函数族接线，口径与 Map/Set 同形。
//! 构造体（iterable 协议）与 set/get/has/delete 四方法的品牌三分枝守卫
//! （set/delete 对非 WeakMap this 抛 TypeError，get/has 静默返 undefined/false）
//! 也在本模块。

use std::collections::HashMap;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// WeakMap 弱键：对象臂按指针恒等（SameValue），symbol 臂按 si 值恒等
/// （interner 保证同符号同 si）。两臂互斥，不可弱持的值不建键。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WeakKey {
    /// 对象键：裸指针仅作哈希键与成员查，生命周期由 GC 转发表判定。
    Obj(*const JsObject),
    /// symbol 键：interned si 值。
    Symbol(u32),
}

impl std::hash::Hash for WeakKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Obj(ptr) => ((*ptr) as usize).hash(state),
            Self::Symbol(si) => si.hash(state),
        }
    }
}

/// WeakMap 内部槽 [[WeakMapData]]：`HashMap` 免迭代序需求（247 域无迭代器
/// 协议），无墓碑槽复杂度。
pub(crate) struct WeakMapInner {
    entries: HashMap<WeakKey, JsValue>,
}

impl WeakMapInner {
    pub(crate) fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// 按活条目数构造：原地重写整表重建时不缩容量口径。
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn get(&self, key: &WeakKey) -> Option<JsValue> {
        self.entries.get(key).copied()
    }

    pub(crate) fn insert(&mut self, key: WeakKey, value: JsValue) {
        self.entries.insert(key, value);
    }

    /// 删除条目：命中返回 true，缺失静默（规范 delete 面返 false）。
    pub(crate) fn remove(&mut self, key: &WeakKey) -> bool {
        self.entries.remove(key).is_some()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (WeakKey, JsValue)> + '_ {
        self.entries.iter().map(|(k, v)| (*k, *v))
    }
}

/// 由可弱持值建弱键；不可弱持（非对象非 symbol、HTML DDA）返回 `None`。
///
/// # 边界与前提
/// - 对象臂按指针恒等；symbol 臂按 si 值恒等。
/// - DDA 对象按规范 `CanBeHeldWeakly` 排除。
pub fn weak_key_of(key: JsValue) -> Option<WeakKey> {
    if key.is_object() {
        let ptr = key.as_js_object_ptr();
        if ptr.is_null() {
            return None;
        }
        // SAFETY: 对象值由执行核心产出，指针在 session 生命周期内有效。
        if unsafe { (&*ptr).is_html_dda_obj() } {
            return None;
        }
        Some(WeakKey::Obj(ptr as *const JsObject))
    } else if key.is_symbol() {
        Some(WeakKey::Symbol(key.as_symbol_index()))
    } else {
        None
    }
}

/// 读出 WeakMap 对象的条目表指针；非 WeakMap 或盒缺失返回空指针。
/// 品牌三分枝（非对象 / 无内部槽 / 盒缺失）的守卫入口在方法族，此处只读。
pub(crate) fn weak_map_inner_of(obj: &JsObject) -> *const WeakMapInner {
    if !obj.is_weak_map_obj() {
        return std::ptr::null();
    }
    obj.native_data() as *const WeakMapInner
}

/// 收集 WeakMap 值边（GC mark 强边）：键为弱边不产边。
/// 消费侧按值类型分发到对象栈与串/BigInt 存活集。
pub fn weak_map_native_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_weak_map_obj() {
        return Vec::new();
    }
    let inner = weak_map_inner_of(obj);
    if inner.is_null() {
        return Vec::new();
    }
    // SAFETY: mark 期对象存活，条目表指针指向有效盒。
    unsafe { (*inner).entries.values().copied().collect() }
}

/// 克隆 WeakMap 条目表到新对象：键原样搬运（生死判定交晋升后的弱键定夺
/// 路径），值经 `rewrite` 改写（强边）。源盒由释放路径单独释放，互不共享。
pub fn clone_weak_map_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_weak_map_obj() {
        return;
    }
    // SAFETY: 源盒指针由构造路径写入，克隆完成 native_data 重指前读取有效。
    let inner = weak_map_inner_of(src);
    if inner.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    let mut cloned = WeakMapInner::new();
    unsafe {
        for (key, value) in (*inner).iter() {
            let new_value = if value.is_object() { rewrite(value) } else { value };
            cloned.entries.insert(key, new_value);
        }
    }
    dst.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 原地重写 WeakMap 条目表的键与值：键经 `key_resolve` 判定（返回 `None`
/// 即死键，条目丢弃；`Some` 改指新键），值经 `value_rewrite` 改写（强边）。
/// 按原表整表重建，免条目级原位换键的容量抖动。
pub fn rewrite_weak_map_native<K, V>(obj: &mut JsObject, mut key_resolve: K, mut value_rewrite: V)
where
    K: FnMut(WeakKey) -> Option<WeakKey>,
    V: FnMut(JsValue) -> JsValue,
{
    if !obj.is_weak_map_obj() {
        return;
    }
    // native_data 为构造路径写入的有效 Box 指针，原地重写期间独占。
    let inner = obj.native_data() as *mut WeakMapInner;
    if inner.is_null() {
        return;
    }
    unsafe {
        let mut rewritten = WeakMapInner::with_capacity((*inner).len());
        for (key, value) in (*inner).iter() {
            let Some(new_key) = key_resolve(key) else {
                continue;
            };
            let new_value = if value.is_object() { value_rewrite(value) } else { value };
            rewritten.entries.insert(new_key, new_value);
        }
        *inner = rewritten;
    }
}

/// 原地重写 WeakMap 条目表的值边（强边）：键不动，值经 `rewrite` 改写。
/// 供晋升主改写路径使用——键的生死须待转发表收敛后由弱键定夺路径判定，
/// 本路径提前改写键会引入晋升序依赖。
pub fn rewrite_weak_map_native_values<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !obj.is_weak_map_obj() {
        return;
    }
    // native_data 同 rewrite_weak_map_native：构造路径写入的有效 Box 指针。
    let inner = obj.native_data() as *mut WeakMapInner;
    if inner.is_null() {
        return;
    }
    unsafe {
        let mut rewritten = WeakMapInner::with_capacity((*inner).len());
        for (key, value) in (*inner).iter() {
            let new_value = if value.is_object() { rewrite(value) } else { value };
            rewritten.entries.insert(key, new_value);
        }
        *inner = rewritten;
    }
}

/// 只读核算 WeakMap 条目表字节（不释放），供 GC 账目核算；与
/// `drop_weak_map_native` 同口径（盒本体 + 桶容量按条目尺寸计）。
pub fn weak_map_native_size(obj: &JsObject) -> u64 {
    if !obj.is_weak_map_obj() {
        return 0;
    }
    let inner = weak_map_inner_of(obj);
    if inner.is_null() {
        return 0;
    }
    unsafe {
        let inner = &*inner;
        (std::mem::size_of::<WeakMapInner>() + inner.entries.capacity() * std::mem::size_of::<(WeakKey, JsValue)>())
            as u64
    }
}

/// 释放 WeakMap 条目表，返回释放字节数供泄漏统计；释放后置空保证对象侧
/// 幂等（收尾统一释放按表枚举不会再见）。
pub fn drop_weak_map_native(obj: &mut JsObject) -> u64 {
    let bytes = weak_map_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let inner = obj.native_data() as *mut WeakMapInner;
    // SAFETY: inner 非空（weak_map_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe {
        drop(Box::from_raw(inner));
    }
    obj.set_native_data(std::ptr::null_mut());
    bytes
}

/// 读条目数（测试探针）：非 WeakMap 或盒缺失返回 0。
pub fn weak_map_entry_count(obj: &JsObject) -> usize {
    if !obj.is_weak_map_obj() {
        return 0;
    }
    let inner = weak_map_inner_of(obj);
    if inner.is_null() {
        return 0;
    }
    unsafe { (*inner).len() }
}

/// 读给定弱键对应的值（测试探针）：非 WeakMap / 盒缺失 / 键缺失返回 undefined。
pub fn weak_map_probe_get(obj: &JsObject, key: JsValue) -> JsValue {
    if !obj.is_weak_map_obj() {
        return JsValue::undefined();
    }
    let Some(key) = weak_key_of(key) else {
        return JsValue::undefined();
    };
    let inner = weak_map_inner_of(obj);
    if inner.is_null() {
        return JsValue::undefined();
    }
    unsafe { (*inner).get(&key).unwrap_or(JsValue::undefined()) }
}

/// 建空条目盒（测试探针）：供单测在构造体落地前先行制造带盒的 WeakMap 形对象。
pub fn weak_map_alloc_box() -> *mut u8 {
    Box::into_raw(Box::new(WeakMapInner::new())) as *mut u8
}

/// 向 WeakMap 形对象插一条（测试探针）：键不可弱持则静默忽略。
pub fn weak_map_insert(obj: &mut JsObject, key: JsValue, value: JsValue) {
    if !obj.is_weak_map_obj() {
        return;
    }
    let Some(key) = weak_key_of(key) else {
        return;
    };
    let inner = obj.native_data() as *mut WeakMapInner;
    if inner.is_null() {
        return;
    }
    // SAFETY: 单测在盒写入前对象独占，指针指向有效盒。
    unsafe { (*inner).insert(key, value) }
}

/// 读出 %WeakMap.prototype% 原型对象指针：经全局 `WeakMap` 构造器的
/// `prototype` 槽现值读取（弱族不占 BuiltinWorld 槽，重绑原位更新全局槽，
/// 经全局路径读恒为当前值）。
fn weak_map_proto_ptr<H: VmHost>(vm: &mut H) -> Result<*const JsObject, JsValue> {
    let global_ptr = vm.session().global_object().as_ptr();
    // SAFETY: 全局对象为 session 级根，本调用内有效。
    let global = unsafe { &*global_ptr };
    let global_val = JsValue::from_js_object(global_ptr as *mut JsObject);
    let si_weakmap = vm.kernel_core().perm_interner().intern("WeakMap").0;
    let ctor_val = vm
        .ordinary_get(global, si_weakmap, global_val)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let ctor_ptr = ctor_val.as_js_object_ptr();
    if ctor_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "WeakMap constructor unavailable"));
    }
    let si_prototype = vm.kernel_core().perm_interner().intern("prototype").0;
    let proto_val = vm
        .ordinary_get(unsafe { &*ctor_ptr }, si_prototype, ctor_val)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let proto_ptr = proto_val.as_js_object_ptr();
    if proto_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "WeakMap prototype unavailable"));
    }
    Ok(proto_ptr as *const JsObject)
}

/// NewTarget 判定：`this` 的原型链（深度上限 16）命中 %WeakMap.prototype%——
/// `new WeakMap()` 直接命中，子类 `super()` 经子类 prototype 链命中；普通
/// 调用（global/undefined）不命中。
fn is_new_target_this(this_val: JsValue, proto_ptr: *const JsObject) -> bool {
    if !this_val.is_object() {
        return false;
    }
    let this_ptr = this_val.as_js_object_ptr();
    if this_ptr.is_null() {
        return false;
    }
    // SAFETY: this 为执行核心产出的对象值，指针在本调用内有效。
    let mut proto = unsafe { &*this_ptr }.proto();
    for _ in 0..16 {
        if !proto.is_object() {
            return false;
        }
        let proto_obj_ptr = proto.as_js_object_ptr();
        if proto_obj_ptr.is_null() {
            return false;
        }
        if std::ptr::eq(proto_obj_ptr, proto_ptr) {
            return true;
        }
        // SAFETY: 原型链为执行核心产出的有效对象链。
        proto = unsafe { &*proto_obj_ptr }.proto();
    }
    false
}

/// `WeakMap` 构造器：NewTarget 校验（`this` 原型链命中 %WeakMap.prototype%）
/// 后建带空条目表的对象；提供可迭代实参时逐元素取 `[0]`/`[1]` 作键值，
/// 经原型链读出的 `set` 方法逐项调用（set 抛面即品牌三分枝抛面）。
///
/// # 步骤
/// 1. 经全局构造器 `prototype` 槽读 %WeakMap.prototype%。
/// 2. 校验 NewTarget：`this` 原型链须命中该原型（普通调用 `WeakMap()` 抛
///    TypeError）。
/// 3. 创建空 WeakMap（proto = %WeakMap.prototype%，带空条目盒）。
/// 4. 取 adder = Get(map, "set")，要求可调用（否则 TypeError）；对可迭代实参
///    逐元素：元素须为对象（否则 TypeError），读 `0`/`1` 后调用 adder。
///
/// # 边界与前提
/// - 无实参或实参为 null/undefined 时返回空 WeakMap，不触碰 adder。
/// - 空可迭代不调用 adder（原型 set 覆写探针口径）。
/// - 任一环节抛错先 IteratorClose 再透传原异常。
pub fn weak_map_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let proto_ptr = match weak_map_proto_ptr(vm) {
        Ok(ptr) => ptr,
        Err(exc) => return NativeResult::Err(exc),
    };

    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !is_new_target_this(this_val, proto_ptr) {
        return NativeResult::Err(crate::error::create_type_error(vm, "WeakMap constructor requires 'new'"));
    }

    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr as *mut JsObject));
    obj.type_tag = JsObject::OBJ_TYPE_WEAK_MAP;
    obj.set_native_data(Box::into_raw(Box::new(WeakMapInner::new())) as *mut u8);
    let map_obj = vm.alloc_object(obj);
    let map_val = JsValue::from_js_object(map_obj);

    if args.len() > 1 {
        let iterable = vm.reg(args[1]);
        if !iterable.is_undefined() && !iterable.is_null() {
            let map_ref = unsafe { &*map_obj };
            let set_si = vm.kernel_core().perm_interner().intern("set").0;
            let adder = match vm.ordinary_get(map_ref, set_si, map_val) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            };
            if !crate::iterator::is_callable(adder) {
                return NativeResult::Err(crate::error::create_type_error(vm, "WeakMap.set is not callable"));
            }
            // entry 读取键按 ToPropertyKey 规范化：数组 entry 的元素区与对象
            // entry 的 shape 链都走整数键，`[k,v]` 与 `{0:k,1:v}` 两形态都命中。
            let key_si = vm.property_key_si(JsValue::int(0));
            let value_si = vm.property_key_si(JsValue::int(1));
            if let Err(err) = crate::iterator::iterate_elements(vm, iterable, |vm, item| {
                if !item.is_object() {
                    return Err(crate::error::create_type_error(vm, "iterator value is not an entry object"));
                }
                let item_obj = unsafe { &*item.as_js_object_ptr() };
                let k = match vm.ordinary_get(item_obj, key_si, item) {
                    Ok(v) => v,
                    Err(err) => return Err(crate::iterator::engine_error(vm, &err)),
                };
                let v = match vm.ordinary_get(item_obj, value_si, item) {
                    Ok(v) => v,
                    Err(err) => return Err(crate::iterator::engine_error(vm, &err)),
                };
                match vm.call_function_sync(adder, map_val, &[k, v]) {
                    Ok(_) => Ok(()),
                    Err(err) => Err(crate::iterator::engine_error(vm, &err)),
                }
            }) {
                return NativeResult::Err(err);
            }
        }
    }

    NativeResult::Ok(map_val)
}

/// 品牌守卫只读核：receiver 为带条目盒的 WeakMap 时返回盒指针，其余形态
/// （非对象 / 无 [[WeakMapData]] 内部槽 / 盒缺失）返回 `None`，四方法一律
/// 按 `RequireInternalSlot` 抛 TypeError（仅键类型检查存在抛静差异）。
fn weak_map_brand_inner(this_val: JsValue) -> Option<*mut WeakMapInner> {
    if !this_val.is_object() {
        return None;
    }
    let ptr = this_val.as_js_object_ptr();
    if ptr.is_null() {
        return None;
    }
    // SAFETY: receiver 为调用方传入的值，native 执行期间有效。
    let obj = unsafe { &*ptr };
    if !obj.is_weak_map_obj() {
        return None;
    }
    let inner = obj.native_data() as *mut WeakMapInner;
    if inner.is_null() {
        return None;
    }
    Some(inner)
}

/// `WeakMap.prototype.set(key, value)`：键不可弱持（非对象非 symbol、HTML
/// DDA）抛 TypeError，receiver 非 WeakMap 抛 TypeError；成功后返回 this。
pub fn weak_map_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = weak_map_brand_inner(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "WeakMap.prototype.set called on non-WeakMap object",
        ));
    };
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let value = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    let Some(key) = weak_key_of(key) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "WeakMap key must be an object"));
    };
    // SAFETY: 盒指针由构造路径写入，本调用内独占。
    unsafe { (*inner).insert(key, value) };
    NativeResult::Ok(this_val)
}

/// `WeakMap.prototype.get(key)`：receiver 非 WeakMap（非对象 / 无内部槽）抛
/// TypeError；键不可弱持静默返 undefined；命中返存储值，未命中返 undefined。
pub fn weak_map_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = weak_map_brand_inner(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "WeakMap.prototype.get called on non-WeakMap object",
        ));
    };
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let Some(key) = weak_key_of(key) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    // SAFETY: 盒指针由构造路径写入，本调用内独占。
    unsafe { NativeResult::Ok((*inner).get(&key).unwrap_or(JsValue::undefined())) }
}

/// `WeakMap.prototype.has(key)`：receiver 非 WeakMap（非对象 / 无内部槽）抛
/// TypeError；键不可弱持静默返 false；命中返 true。
pub fn weak_map_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = weak_map_brand_inner(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "WeakMap.prototype.has called on non-WeakMap object",
        ));
    };
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let Some(key) = weak_key_of(key) else {
        return NativeResult::Ok(JsValue::bool(false));
    };
    // SAFETY: 盒指针由构造路径写入，本调用内独占。
    unsafe { NativeResult::Ok(JsValue::bool((*inner).get(&key).is_some())) }
}

/// `WeakMap.prototype.delete(key)`：receiver 非 WeakMap 抛 TypeError；键不可
/// 弱持静默返 false；命中删除返 true。
pub fn weak_map_delete<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = weak_map_brand_inner(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "WeakMap.prototype.delete called on non-WeakMap object",
        ));
    };
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let Some(key) = weak_key_of(key) else {
        return NativeResult::Ok(JsValue::bool(false));
    };
    // SAFETY: 盒指针由构造路径写入，本调用内独占。
    unsafe { NativeResult::Ok(JsValue::bool((*inner).remove(&key))) }
}
