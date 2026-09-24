use std::collections::HashMap;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::set::SetKey;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

/// Map 内部槽表：`slots` 按插入序存放槽（`None` = 空槽，键已删除且未重加），
/// `index` 是活键到槽下标的辅助索引（只索引活槽）。
///
/// 语义对齐规范 `[[MapData]]` List + `~empty~` 空槽：删除原位留洞、后续槽
/// 不左移，重加/新增键追加 List 末尾。活扫（forEach、迭代器）跳空槽且每轮
/// 重读总槽数，故"删当前键后重加"在末尾再访问、"删未来键"跳过、"新增键"
/// 在末尾被访问。
///
/// 不变式：`index` 与 `slots` 活槽一一对应，remove/clear 同步除名。
pub(crate) struct MapInner {
    slots: Vec<Option<(SetKey, JsValue)>>,
    index: HashMap<SetKey, usize>,
}

impl MapInner {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            index: HashMap::with_capacity(capacity),
        }
    }

    /// 活表条目数（规范 SetDataSize 口径，空槽不计）。
    pub(crate) fn len(&self) -> usize {
        self.index.len()
    }

    /// 总槽数（含空槽）：活扫的重读口径，每轮重读以捕获迭代期追加。
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// 读槽：`None` 为空槽，`Some` 为活条目。
    pub(crate) fn slot(&self, i: usize) -> Option<(SetKey, JsValue)> {
        self.slots.get(i).copied().flatten()
    }

    /// 活条目按插入序迭代（空槽不产边、不入枚举）。
    pub(crate) fn iter(&self) -> impl Iterator<Item = (SetKey, JsValue)> + '_ {
        self.slots.iter().filter_map(|slot| *slot)
    }

    pub(crate) fn get(&self, key: &SetKey) -> Option<JsValue> {
        self.index
            .get(key)
            .map(|&i| self.slots[i].as_ref().expect("辅助索引指向活槽").1)
    }

    pub(crate) fn get_mut(&mut self, key: &SetKey) -> Option<&mut JsValue> {
        let i = *self.index.get(key)?;
        self.slots[i].as_mut().map(|entry| &mut entry.1)
    }

    /// 活槽原位更值（保留原键位，如 -0/+0 区分不丢）；缺失末尾追加。
    pub(crate) fn insert(&mut self, key: SetKey, value: JsValue) {
        match self.index.get(&key) {
            Some(&i) => self.slots[i].as_mut().expect("辅助索引指向活槽").1 = value,
            None => {
                self.index.insert(key, self.slots.len());
                self.slots.push(Some((key, value)));
            }
        }
    }

    /// 删除 = 置洞：槽原位留空槽（不左移），辅助索引同步除名。
    pub(crate) fn remove(&mut self, key: &SetKey) -> bool {
        let Some(i) = self.index.remove(key) else {
            return false;
        };
        self.slots[i] = None;
        true
    }

    pub(crate) fn contains(&self, key: &SetKey) -> bool {
        self.index.contains_key(key)
    }

    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.index.clear();
    }
}

/// 取出 Map 对象 native-data 槽中存储的 `MapInner` 指针。
///
/// # 调用方维护的安全性契约
///
/// 指针在 Map `JsObject` 存活期间有效：`JsObject` 分配于当前 `Epoch` arena，
/// native builtin 执行期间不会调用 `Epoch::reset()`。持有分配的
/// `Box<MapInner>` 由 `new_map_inner()` 创建，进程退出前不释放（生命周期与
/// epoch 绑定，属有意为之）。native 调用为单线程，同一 Map 对象同时至多
/// 存在一个活 `*mut` 别名。
fn get_map_inner<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<*mut MapInner, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "called on non-Map object"));
    }
    let map_ptr = this_val.as_js_object_ptr();
    if map_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Map internal state invalid"));
    }
    // SAFETY: map_ptr 是当前 Epoch bump 分配的 JsObject 的非空、对齐指针；
    // native 执行期间 epoch 不重置，指针在本调用内有效。
    let map_obj = unsafe { &*map_ptr };
    if !map_obj.is_map() {
        return Err(crate::error::create_type_error(
            vm,
            "Map.prototype method called on incompatible receiver",
        ));
    }
    // SAFETY: native_data 持有 `alloc_map` 写入的裸指针，即有效的
    // 堆分配 `Box<MapInner>`；MapInner 至多要求 8 字节对齐，
    // 全局分配器满足该要求。
    let inner_ptr = map_obj.native_data() as *mut MapInner;
    if inner_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Map internal state invalid"));
    }
    Ok(inner_ptr)
}

fn new_map_inner() -> *mut MapInner {
    Box::into_raw(Box::new(MapInner::new()))
}

fn alloc_map<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let map_proto = vm.session().builtin_world().map_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
    obj.set_map(true);
    let inner = new_map_inner();
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

/// 收集 Map 内全部键值边（GC mark 边）：对象/字符串/BigInt 均产出，
/// 消费侧按值类型分发到对象栈与存活集。
pub fn map_native_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_map() {
        return Vec::new();
    }
    let inner = obj.native_data() as *const MapInner;
    if inner.is_null() {
        return Vec::new();
    }
    unsafe { (*inner).iter().flat_map(|(key, value)| [key.0, value]).collect() }
}

/// 克隆 Map 的 native 数据到新对象，用 `rewrite` 改写其中的对象引用
/// （供跨 epoch 的对象重写/克隆流程使用）。
pub fn clone_map_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_map() {
        return;
    }
    let inner = src.native_data() as *const MapInner;
    if inner.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    let mut cloned = MapInner::new();
    unsafe {
        for (key, value) in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { key };
            let new_value = if value.is_object() { rewrite(value) } else { value };
            cloned.insert(new_key, new_value);
        }
    }
    dst.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 原地重写 Map 的 native 数据，用 `rewrite` 改写其中的对象引用。
pub fn rewrite_map_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !obj.is_map() {
        return;
    }
    let inner = obj.native_data() as *mut MapInner;
    if inner.is_null() {
        return;
    }
    unsafe {
        let mut rewritten = MapInner::with_capacity((*inner).len());
        for (key, value) in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { key };
            let new_value = if value.is_object() { rewrite(value) } else { value };
            rewritten.insert(new_key, new_value);
        }
        *inner = rewritten;
    }
}

/// 只读核算 Map 的 native 数据字节（不释放）。
/// 槽表与辅助索引均按 capacity 核算，与 `drop_map_native` 释放口径一致，
/// 供 GC 账目核算。
pub fn map_native_size(obj: &JsObject) -> u64 {
    if !obj.is_map() {
        return 0;
    }
    let inner = obj.native_data() as *mut MapInner;
    if inner.is_null() {
        return 0;
    }
    unsafe {
        let inner = &*inner;
        (std::mem::size_of::<MapInner>()
            + inner.slots.capacity() * std::mem::size_of::<Option<(SetKey, JsValue)>>()
            + inner.index.capacity() * std::mem::size_of::<(SetKey, usize)>()) as u64
    }
}

/// 释放 Map 的 native 数据（IndexMap），返回释放的字节数供泄漏统计。
pub fn drop_map_native(obj: &mut JsObject) -> u64 {
    let bytes = map_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let inner = obj.native_data() as *mut MapInner;
    // SAFETY: inner 非空（map_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe {
        drop(Box::from_raw(inner));
    }
    obj.set_native_data(std::ptr::null_mut());
    bytes
}

/// `Map` 构造函数：创建带空 IndexMap native 数据的 Map 对象，若提供可迭代实参
/// 则逐元素（须为对象）取 `[0]`/`[1]` 作为键值调用 set。
///
/// # 步骤
/// 1. 校验 NewTarget：`this` 的原型须是 Map.prototype（普通调用 `Map()` 抛 TypeError）。
/// 2. 创建空 Map。
/// 3. 取 adder = Get(map, "set")，要求可调用（否则 TypeError）。
/// 4. 对可迭代实参逐元素：元素须为对象（否则 TypeError），读 `0`/`1` 属性后调用 adder。
///
/// # 边界与前提
/// - 无实参或实参为 null/undefined 时返回空 Map，不触碰 adder。
/// - 任一环节抛错先 IteratorClose 再透传。
pub fn map_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_new_call = this_val.is_object() && {
        let map_proto = vm.session().builtin_world().map_proto.as_ptr() as *mut JsObject;
        // 沿原型链查找 Map.prototype：`new Map()` 直接命中，子类 `super()` 经
        // 子类 prototype 链命中；普通调用（global/undefined）不命中。
        let this_ptr = this_val.as_js_object_ptr();
        if this_ptr.is_null() {
            false
        } else {
            let mut proto = unsafe { &*this_ptr }.proto();
            let mut found = false;
            for _ in 0..16 {
                if !proto.is_object() {
                    break;
                }
                let proto_ptr = proto.as_js_object_ptr();
                if proto_ptr.is_null() {
                    break;
                }
                if std::ptr::eq(proto_ptr, map_proto) {
                    found = true;
                    break;
                }
                proto = unsafe { &*proto_ptr }.proto();
            }
            found
        }
    };
    if !is_new_call {
        return NativeResult::Err(crate::error::create_type_error(vm, "Map must be called with new"));
    }

    let map_obj = alloc_map(vm);
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
                return NativeResult::Err(crate::error::create_type_error(vm, "Map.set is not callable"));
            }
            // entry 读取键按 ToPropertyKey 规范化：数组 entry 的元素区与对象 entry
            // 的 shape 链都走整数键，保证 `[k,v]` 与 `{0:k,1:v}` 两种形态都命中。
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

/// `Map.prototype.set(key, value)`：插入/更新键值对，返回 this。
pub fn map_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let val = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(key), val);
    }
    NativeResult::Ok(this_val)
}

/// `Map.prototype.get(key)`：返回 key 对应的值；不存在返回 undefined。
pub fn map_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).get(&SetKey(key)) };
    NativeResult::Ok(found.unwrap_or(JsValue::undefined()))
}

/// `Map.prototype.getOrInsert(key, value)`：键命中返回已有存储值（不插入、
/// size 不变）；键缺失时末尾追加新条目并返回实参 value。
///
/// # 边界与前提
/// - 命中判定不区分存储值 undefined：命中即返回现值，不得视为缺失。
/// - 键按 SameValueZero 语义归一（±0 同键、NaN 自相等），与 set/get 同域。
pub fn map_get_or_insert<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let val = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    let found = unsafe { (*inner).get(&SetKey(key)) };
    if let Some(existing) = found {
        return NativeResult::Ok(existing);
    }
    unsafe {
        (*inner).insert(SetKey(key), val);
    }
    NativeResult::Ok(val)
}

/// `Map.prototype.getOrInsertComputed(key, callback)`：键命中返回存储值（callback
/// 不求值）；缺失时调用 callback（this 恒为 undefined，唯一实参为规范化后的键），
/// 以返回值作条目值原位覆盖（callback 期间同键已入表）或末尾追加。
///
/// # 步骤
/// 1. 校验 receiver 持 Map 内部槽（先于 callback 检查）。
/// 2. callback 须可调用，否则 TypeError。
/// 3. 键规范化：Double 零值统一为 +0（±0 唯一规范化点，存储、扫描、callback
///    实参三处同一值；NaN 不归一）。
/// 4. 首扫：命中即返回存储值，callback 不求值。
/// 5. 调用 callback；抛错时原异常值上抛，零条目落表。
/// 6. 二扫：callback 期间同键已入表则原位写值返回，不重新追加。
/// 7. 否则末尾追加新条目并返回 callback 返回值。
///
/// # 边界与前提
/// - 存储值 undefined 同样计命中：直接返回现值，不得视为缺失。
/// - callback 内可重入同 Map 方法（native 重入），键值表指针跨重入稳定。
///
/// # 注意事项
/// - 二扫覆盖保序位：命中键值更新不移动插入序，条目不重新追加末尾。
/// - 无 thisArg 形参：callback 的 this 恒为 undefined，恰一个实参（规范化键）。
/// - -0 键规范化为 +0 后才进入存储/扫描/callback 实参。
pub fn map_get_or_insert_computed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let callback = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    if !crate::iterator::is_callable(callback) {
        return NativeResult::Err(crate::error::create_type_error(vm, "callback is not a function"));
    }
    let key = if key.is_double() && key.as_double() == 0.0 {
        JsValue::float(0.0)
    } else {
        key
    };
    let found = unsafe { (*inner).get(&SetKey(key)) };
    if let Some(existing) = found {
        return NativeResult::Ok(existing);
    }
    let value = match vm.call_function_sync(callback, JsValue::undefined(), &[key]) {
        Ok(v) => v,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    if let Some(slot) = unsafe { (*inner).get_mut(&SetKey(key)) } {
        *slot = value;
        return NativeResult::Ok(value);
    }
    unsafe {
        (*inner).insert(SetKey(key), value);
    }
    NativeResult::Ok(value)
}

/// `Map.prototype.has(key)`：key 是否存在。
pub fn map_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains(&SetKey(key)) };
    NativeResult::Ok(JsValue::bool(found))
}

/// `Map.prototype.delete(key)`：删除键并返回是否删除成功。
pub fn map_delete<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).remove(&SetKey(key)) };
    NativeResult::Ok(JsValue::bool(removed))
}

/// `Map.prototype.clear()`：清空全部键值对，返回 undefined。
pub fn map_clear<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    unsafe {
        (*inner).clear();
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Map.prototype.forEach(callbackfn, thisArg)`：按插入序对每个键值对调用回调，
/// 回调参数为 `(value, key, map)`。活表扫描：空槽跳过，迭代期间新增键值对被访问，
/// 访问后删除且完成前重加的键再访问。
pub fn map_for_each<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let callback = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    if !crate::iterator::is_callable(callback) {
        return NativeResult::Err(crate::error::create_type_error(vm, "callback is not a function"));
    }
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 活扫槽：每轮重读总槽数（捕获迭代期追加），空槽不回调直接推进；
    // 下标口径恒为"下一待检槽"，重加条目落在末尾槽由后续步进访问。
    let mut index = 0usize;
    loop {
        if index >= unsafe { (*inner).slot_count() } {
            break;
        }
        let entry = unsafe { (*inner).slot(index) };
        index += 1;
        let Some((key, value)) = entry else { continue };
        if let Err(err) = vm.call_function_sync(callback, this_arg, &[value, key.0, this_val]) {
            return NativeResult::Err(crate::iterator::engine_error(vm, &err));
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Map.prototype.size` getter：返回键值对数量。
pub fn map_size<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    NativeResult::Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}

/// `Map.prototype.entries()`：返回按插入序迭代 `[key, value]` 对的迭代器。
pub fn map_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let _inner = native_try!(get_map_inner(vm, this_val));
    NativeResult::Ok(crate::iterator::make_collection_iterator(
        vm,
        this_val,
        JsValue::from_js_object(vm.session().builtin_world().map_iterator_proto.as_ptr() as *mut JsObject),
        crate::iterator::MapSetMode::MapEntries,
    ))
}

/// `Map.prototype.values()`：返回按插入序迭代值的迭代器。
pub fn map_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let _inner = native_try!(get_map_inner(vm, this_val));
    NativeResult::Ok(crate::iterator::make_collection_iterator(
        vm,
        this_val,
        JsValue::from_js_object(vm.session().builtin_world().map_iterator_proto.as_ptr() as *mut JsObject),
        crate::iterator::MapSetMode::MapValues,
    ))
}

/// `Map.prototype.keys()`：返回按插入序迭代键的迭代器。
pub fn map_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let _inner = native_try!(get_map_inner(vm, this_val));
    NativeResult::Ok(crate::iterator::make_collection_iterator(
        vm,
        this_val,
        JsValue::from_js_object(vm.session().builtin_world().map_iterator_proto.as_ptr() as *mut JsObject),
        crate::iterator::MapSetMode::MapKeys,
    ))
}

/// `Map.groupBy(items, callbackFn)`：按回调返回的键分组迭代元素到新 Map。
/// 回调以 `(element, key)` 调用（对数组 items，key 为下标），返回值作为分组键
/// （SameValueZero 语义，±0 归 +0 由 Map.set 键比较处理）。
pub fn map_group_by<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("Map.groupBy called with {} args", args.len());
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Map.groupBy requires 2 arguments"));
    }
    let callback_val = vm.reg(args[2]);
    if !crate::iterator::is_callable(callback_val) {
        return NativeResult::Err(crate::error::create_type_error(vm, "Map.groupBy: callbackFn is not callable"));
    }
    let items_val = vm.reg(args[1]);
    // 创建新 Map。
    let map_obj = alloc_map(vm);
    let map_val = JsValue::from_js_object(map_obj);
    // 取 @@iterator 方法（items 经 ToObject 装箱，null/undefined 抛 TypeError）。
    let items_obj = match oxide_runtime_api::to_object(items_val, vm) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let items_ptr = items_obj.as_js_object_ptr();
    let sym_iter_si = oxide_types::private_key::make_well_known_symbol_key(0);
    let iter_method = match unsafe { vm.ordinary_get(&*items_ptr, sym_iter_si, items_val) } {
        Ok(m) => m,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    if !crate::iterator::is_callable(iter_method) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Map.groupBy: items[Symbol.iterator] is not callable",
        ));
    }
    let iter_val = match vm.call_function_sync(iter_method, items_val, &[]) {
        Ok(v) => v,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    if !iter_val.is_object() || iter_val.as_js_object_ptr().is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Map.groupBy: iterator is not an object"));
    }
    let iter_obj = unsafe { &*iter_val.as_js_object_ptr() };
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let next_fn = match vm.ordinary_get(iter_obj, next_si, iter_val) {
        Ok(f) => f,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    // 读取 Map.set / Map.get 方法。
    let map_ref = unsafe { &*map_obj };
    let set_si = vm.kernel_core().perm_interner().intern("set").0;
    let adder = match vm.ordinary_get(map_ref, set_si, map_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    let get_si = vm.kernel_core().perm_interner().intern("get").0;
    let getter = match vm.ordinary_get(map_ref, get_si, map_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    };
    if !crate::iterator::is_callable(adder) || !crate::iterator::is_callable(getter) {
        return NativeResult::Err(crate::error::create_type_error(vm, "Map.groupBy: Map.set/get is not callable"));
    }
    let mut counter: i32 = 0;
    loop {
        let next_result = match vm.call_function_sync(next_fn, iter_val, &[]) {
            Ok(v) => v,
            Err(e) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        if !next_result.is_object() || next_result.as_js_object_ptr().is_null() {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "Map.groupBy: iterator next() returned non-object",
            ));
        }
        let nr = unsafe { &*next_result.as_js_object_ptr() };
        let done_si = vm.kernel_core().perm_interner().intern("done").0;
        let done = match vm.ordinary_get(nr, done_si, next_result) {
            Ok(v) => oxide_runtime_api::to_boolean(v),
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        if done {
            break;
        }
        let value_si = vm.kernel_core().perm_interner().intern("value").0;
        let element = match vm.ordinary_get(nr, value_si, next_result) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        // 调用 callbackFn(element, counter)：counter 对数组 items 即元素下标 k。
        let group_key =
            match vm.call_function_sync(callback_val, JsValue::undefined(), &[element, JsValue::int(counter)]) {
                Ok(v) => v,
                Err(e) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                    return NativeResult::Err(exc);
                }
            };
        // 取该键已有分组数组；无则新建空数组并 set 到 Map，再 push 元素。
        let existing = match vm.call_function_sync(getter, map_val, &[group_key]) {
            Ok(v) => v,
            Err(e) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        let arr_val = if existing.is_object()
            && !existing.as_js_object_ptr().is_null()
            && unsafe { &*existing.as_js_object_ptr() }.is_array()
        {
            existing
        } else {
            let arr_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
            let arr = vm.alloc_object(JsObject::new_array(
                EMPTY_SHAPE_ID,
                JsValue::from_js_object(arr_proto),
                0,
                vm.epoch().bump(),
            ));
            let new_arr_val = JsValue::from_js_object(arr);
            if let Err(e) = vm.call_function_sync(adder, map_val, &[group_key, new_arr_val]) {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
                return NativeResult::Err(exc);
            }
            new_arr_val
        };
        // push element 到分组数组（写入前 promote，与 Array.prototype.push 同款）。
        let arr_obj = unsafe { &mut *arr_val.as_js_object_ptr() };
        let idx = arr_obj.prop_count();
        let promoted_elem = vm.promote_if_needed_for_write_ptr(arr_val.as_js_object_ptr(), element);
        arr_obj.set_prop_at(idx, promoted_elem);
        counter += 1;
    }
    NativeResult::Ok(map_val)
}
