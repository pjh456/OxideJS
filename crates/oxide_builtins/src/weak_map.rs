//! WeakMap 条目表：弱键 → 强值。
//!
//! 键为弱引用（GC mark 不产键边，死键由收集路径按转发表判定后丢条目），
//! 值为强引用（值边进 mark 边扫描与晋升改写）。存储盒经 `Box::into_raw` 挂
//! `JsObject.native_data`，GC 六站点（mark 边 / 移动式 sweep / 晋升克隆 /
//! 原地晋升 / drop / 字节账目）经本模块五函数族接线，口径与 Map/Set 同形。

use std::collections::HashMap;

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
pub fn weak_map_get(obj: &JsObject, key: JsValue) -> JsValue {
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
