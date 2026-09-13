use std::hash::{Hash, Hasher};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::{JsType, JsValue};

use oxide_runtime_api::{bigint_data, same_value_zero, NativeResult, VmHost};

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

/// Set/Map 的键包装：相等与哈希都按 SameValueZero（ECMA-262 §7.2.11）值语义：
/// - Number：数值相等——NaN 与自身相同、+0/-0 同键、int/double 表示合并；
/// - String/BigInt：内容相等——运行期构造与常量池的同内容值互为同键；
/// - Object/Function：引用相等；Symbol 按下标相同；Bool/null/undefined 按值。
#[derive(Clone, Copy, Debug)]
pub struct SetKey(pub JsValue);

impl PartialEq for SetKey {
    fn eq(&self, other: &Self) -> bool {
        same_value_zero(self.0, other.0)
    }
}

impl Eq for SetKey {}

impl Hash for SetKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let v = self.0;
        // 先写类标签再写载荷：SameValueZero 相等的值哈希必须一致，
        // 标签使跨类碰撞（合法但浪费探测）尽量不发生。
        match v.js_type() {
            // Number：规范位——NaN 经 `JsValue::float` 归一到统一位模式，
            // ±0 归 +0，int/double 混合表示经 f64 转换后一致。
            JsType::Int | JsType::Double => {
                0u8.hash(state);
                let d = if v.is_int() { v.as_int() as f64 } else { v.as_double() };
                let bits = if d == 0.0 { 0.0f64.to_bits() } else { JsValue::float(d).to_bits() };
                bits.hash(state);
            }
            // String：单元序列（rope 经扁平化缓存，幂等；单元口径与
            // same_value_zero 的内容比较同域，同值同哈希）。
            JsType::String => {
                1u8.hash(state);
                // SAFETY: 串指针在 native 调用期间存活（session 串不在调用中途释放），
                // 借出在本哈希调用内即时消费。
                unsafe { (*v.as_string_ptr()).units().as_ref().hash(state) };
            }
            // BigInt：`to_bytes_le` 规范形（零号统一 NoZero，同值同字节）。
            JsType::BigInt => {
                2u8.hash(state);
                // SAFETY: bigint box 由 VM 登记，存活至 full_reset。
                let (sign, mag) = unsafe { bigint_data(v) }.to_bytes_le();
                matches!(sign, num_bigint::Sign::Minus).hash(state);
                mag.hash(state);
            }
            // Object：引用同一性。
            JsType::Object => {
                3u8.hash(state);
                v.to_bits().hash(state);
            }
            // Symbol：符号表下标。
            JsType::Symbol => {
                4u8.hash(state);
                v.as_symbol_index().hash(state);
            }
            // Bool：值本身。
            JsType::Bool => {
                5u8.hash(state);
                v.as_bool().hash(state);
            }
            JsType::Null => 6u8.hash(state),
            JsType::Undefined => 7u8.hash(state),
        }
    }
}

pub(crate) type SetInner = indexmap::IndexSet<SetKey>;

/// 取出 Set 对象 native-data 槽中存储的 `IndexSet` 指针。
///
/// # 调用方维护的安全性契约
///
/// 指针在 Set `JsObject` 存活期间有效：`JsObject` 分配于当前 `Epoch` arena，
/// native builtin 执行期间不会调用 `Epoch::reset()`。持有分配的
/// `Box<IndexSet>` 由 `new_set_inner()` 创建，Set 生命周期内不释放。
/// native 调用为单线程，同一 Set 对象同时至多存在一个活 `*mut` 别名。
fn get_set_inner<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<*mut SetInner, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "called on non-Set object"));
    }
    let set_ptr = this_val.as_js_object_ptr();
    if set_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Set internal state invalid"));
    }
    // SAFETY: set_ptr 是当前 Epoch bump 分配的 JsObject 的非空、对齐指针，
    // 本调用期间有效。
    let set_obj = unsafe { &*set_ptr };
    if !set_obj.is_set() {
        return Err(crate::error::create_type_error(vm, "Set.prototype.add called on incompatible receiver"));
    }
    // SAFETY: native_data 持有 `alloc_set` 写入的裸指针，即有效的
    // 堆分配 `Box<IndexSet<SetKey>>`；IndexSet 至多要求 8 字节对齐，
    // 全局分配器满足该要求。
    let inner_ptr = set_obj.native_data() as *mut SetInner;
    if inner_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Set internal state invalid"));
    }
    Ok(inner_ptr)
}

fn new_set_inner() -> *mut SetInner {
    Box::into_raw(Box::new(SetInner::new()))
}

fn alloc_set<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let set_proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    obj.set_set(true);
    let inner = new_set_inner();
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

/// 收集 Set 内全部元素边（GC mark 边）：对象/字符串/BigInt 均产出，
/// 消费侧按值类型分发到对象栈与存活集。
pub fn set_native_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_set() {
        return Vec::new();
    }
    let inner = obj.native_data() as *const SetInner;
    if inner.is_null() {
        return Vec::new();
    }
    unsafe { (*inner).iter().map(|key| key.0).collect() }
}

/// 克隆 Set 的 native 数据到新对象，用 `rewrite` 改写其中的对象引用。
pub fn clone_set_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_set() {
        return;
    }
    let inner = src.native_data() as *const SetInner;
    if inner.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    let mut cloned = SetInner::new();
    unsafe {
        for key in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { *key };
            cloned.insert(new_key);
        }
    }
    dst.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 原地重写 Set 的 native 数据，用 `rewrite` 改写其中的对象引用。
pub fn rewrite_set_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !obj.is_set() {
        return;
    }
    let inner = obj.native_data() as *mut SetInner;
    if inner.is_null() {
        return;
    }
    unsafe {
        let mut rewritten = SetInner::with_capacity((*inner).len());
        for key in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { *key };
            rewritten.insert(new_key);
        }
        *inner = rewritten;
    }
}

/// 只读核算 Set 的 native 数据字节（不释放）。
/// 与 `drop_set_native` 释放口径一致（capacity），供 GC 账目核算。
pub fn set_native_size(obj: &JsObject) -> u64 {
    if !obj.is_set() {
        return 0;
    }
    let inner = obj.native_data() as *mut SetInner;
    if inner.is_null() {
        return 0;
    }
    unsafe { (std::mem::size_of::<SetInner>() + (*inner).capacity() * std::mem::size_of::<SetKey>()) as u64 }
}

/// 释放 Set 的 native 数据（IndexSet），返回释放的字节数供泄漏统计。
pub fn drop_set_native(obj: &mut JsObject) -> u64 {
    let bytes = set_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let inner = obj.native_data() as *mut SetInner;
    // SAFETY: inner 非空（set_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe {
        drop(Box::from_raw(inner));
    }
    obj.set_native_data(std::ptr::null_mut());
    bytes
}

/// `Set` 构造函数：创建带空 IndexSet native 数据的 Set 对象，若提供可迭代实参
/// 则按插入序逐个 add。
///
/// # 步骤
/// 1. 校验 NewTarget：`this` 的原型须是 Set.prototype（普通调用 `Set()` 抛 TypeError）。
/// 2. 创建空 Set。
/// 3. 取 adder = Get(set, "add")，要求可调用（否则 TypeError）。
/// 4. 对可迭代实参逐元素调用 adder（异常时先 IteratorClose 再透传）。
///
/// # 边界与前提
/// - 无实参或实参为 null/undefined 时返回空 Set，不触碰 adder。
pub fn set_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_new_call = this_val.is_object() && {
        let set_proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
        // 沿原型链查找 Set.prototype：`new Set()` 直接命中，子类 `super()` 经
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
                if std::ptr::eq(proto_ptr, set_proto) {
                    found = true;
                    break;
                }
                proto = unsafe { &*proto_ptr }.proto();
            }
            found
        }
    };
    if !is_new_call {
        return NativeResult::Err(crate::error::create_type_error(vm, "Set must be called with new"));
    }

    let set_obj = alloc_set(vm);
    let set_val = JsValue::from_js_object(set_obj);

    if args.len() > 1 {
        let iterable = vm.reg(args[1]);
        if !iterable.is_undefined() && !iterable.is_null() {
            let set_ref = unsafe { &*set_obj };
            let add_si = vm.kernel_core().perm_interner().intern("add").0;
            let adder = match vm.ordinary_get(set_ref, add_si, set_val) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            };
            if !crate::iterator::is_callable(adder) {
                return NativeResult::Err(crate::error::create_type_error(vm, "Set.add is not callable"));
            }
            if let Err(err) = crate::iterator::iterate_elements(vm, iterable, |vm, elem| {
                match vm.call_function_sync(adder, set_val, &[elem]) {
                    Ok(_) => Ok(()),
                    Err(err) => Err(crate::iterator::engine_error(vm, &err)),
                }
            }) {
                return NativeResult::Err(err);
            }
        }
    }

    NativeResult::Ok(set_val)
}

/// `Set.prototype.add(value)`：插入元素，返回 this。
pub fn set_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(val));
    }
    NativeResult::Ok(this_val)
}

/// `Set.prototype.has(value)`：元素是否存在。
pub fn set_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains(&SetKey(val)) };
    NativeResult::Ok(JsValue::bool(found))
}

/// `Set.prototype.delete(value)`：删除元素并返回是否删除成功。
pub fn set_delete<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(val)) };
    NativeResult::Ok(JsValue::bool(removed))
}

/// `Set.prototype.clear()`：清空全部元素，返回 undefined。
pub fn set_clear<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    unsafe {
        (*inner).clear();
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Set.prototype.forEach(callbackfn, thisArg)`：按插入序对每个值调用回调，
/// 回调参数为 `(value, value, set)`。迭代期间新增的元素也会被访问。
pub fn set_for_each<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let callback = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    if !crate::iterator::is_callable(callback) {
        return NativeResult::Err(crate::error::create_type_error(vm, "callback is not a function"));
    }
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 按下标迭代：每次回调后重新读当前下标（支持迭代期间插入）。
    let mut index = 0usize;
    loop {
        let value = unsafe { (*inner).get_index(index).map(|elem| elem.0) };
        let Some(value) = value else { break };
        index += 1;
        if let Err(err) = vm.call_function_sync(callback, this_arg, &[value, value, this_val]) {
            return NativeResult::Err(crate::iterator::engine_error(vm, &err));
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Set.prototype.size` getter：返回元素数量。
pub fn set_size<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    NativeResult::Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}

/// `Set.prototype.entries()`：返回按插入序迭代 `[value, value]` 对的迭代器。
pub fn set_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let _inner = native_try!(get_set_inner(vm, this_val));
    NativeResult::Ok(crate::iterator::make_collection_iterator(
        vm,
        this_val,
        JsValue::from_js_object(vm.session().builtin_world().set_iterator_proto.as_ptr() as *mut JsObject),
        crate::iterator::MapSetMode::SetEntries,
    ))
}

/// `Set.prototype.values()`：返回按插入序迭代值的迭代器。
pub fn set_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let _inner = native_try!(get_set_inner(vm, this_val));
    NativeResult::Ok(crate::iterator::make_collection_iterator(
        vm,
        this_val,
        JsValue::from_js_object(vm.session().builtin_world().set_iterator_proto.as_ptr() as *mut JsObject),
        crate::iterator::MapSetMode::SetValues,
    ))
}

/// `Set.prototype.keys()`：别名 `values()`（Set 无独立键），返回同样的迭代器。
pub fn set_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let _inner = native_try!(get_set_inner(vm, this_val));
    // Set 的 keys() 是 values() 的别名——返回同样的逐元素迭代器。
    NativeResult::Ok(crate::iterator::make_collection_iterator(
        vm,
        this_val,
        JsValue::from_js_object(vm.session().builtin_world().set_iterator_proto.as_ptr() as *mut JsObject),
        crate::iterator::MapSetMode::SetValues,
    ))
}

/// SetRecord 协议：读取参数对象的 `size`/`has`/`keys` 并校验。
///
/// # 步骤
/// 1. 参数非对象抛 TypeError。
/// 2. `size` 经 ToNumber 转换，为 NaN 抛 TypeError。
/// 3. `has`/`keys` 须可调用，否则抛 TypeError。
///
/// # 返回值
/// `(对象, size, has, keys)`。
fn get_set_record<H: VmHost>(vm: &mut H, other: JsValue) -> Result<(JsValue, f64, JsValue, JsValue), JsValue> {
    if !other.is_object() {
        return Err(crate::error::create_type_error(vm, "argument is not an object"));
    }
    let obj = unsafe { &*other.as_js_object_ptr() };
    let size_si = vm.kernel_core().perm_interner().intern("size").0;
    let raw_size = vm
        .ordinary_get(obj, size_si, other)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let num_size = match oxide_runtime_api::to_number_full(raw_size, vm) {
        Ok(n) => n,
        Err(err) => return Err(crate::error::create_type_error(vm, &err)),
    };
    if num_size.is_nan() {
        return Err(crate::error::create_type_error(vm, "size must be a number"));
    }
    let has_si = vm.kernel_core().perm_interner().intern("has").0;
    let has = vm
        .ordinary_get(obj, has_si, other)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if !crate::iterator::is_callable(has) {
        return Err(crate::error::create_type_error(vm, "has must be callable"));
    }
    let keys_si = vm.kernel_core().perm_interner().intern("keys").0;
    let keys = vm
        .ordinary_get(obj, keys_si, other)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if !crate::iterator::is_callable(keys) {
        return Err(crate::error::create_type_error(vm, "keys must be callable"));
    }
    Ok((other, num_size, has, keys))
}

/// 把 -0.0 归一化为 +0.0（集合运算把 `-0𝔽` 当 `+0𝔽` 处理）。
fn normalize_neg_zero(value: JsValue) -> JsValue {
    if value.is_double() {
        let d = value.as_double();
        if d == 0.0 && d.is_sign_negative() {
            return JsValue::float(0.0);
        }
    }
    value
}

/// 用一个已填充的 `SetInner` 创建普通 Set 对象（原型为 Set.prototype）。
fn alloc_set_with_inner<H: VmHost>(vm: &mut H, inner: SetInner) -> JsValue {
    let set_proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    obj.set_set(true);
    obj.set_native_data(Box::into_raw(Box::new(inner)) as *mut u8);
    JsValue::from_js_object(vm.alloc_object(obj))
}

/// 回调控制流：`Continue` 继续迭代，`Stop` 提前终止并 IteratorClose。
enum KeysFlow {
    Continue,
    Stop,
}

/// 调用 SetRecord 的 `keys` 方法取得迭代器并逐元素回调。
///
/// # 步骤
/// 1. `Call(keys, set)` 得迭代器对象（非对象抛 TypeError）。
/// 2. 逐次 `next` 读取 `{done, value}` 交给 `f`。
/// 3. 迭代抛错或 `f` 返回 `Stop` 时先 IteratorClose 再返回。
///
/// # 返回值
/// `Ok(Continue)` 迭代耗尽；`Ok(Stop)` 回调提前终止；`Err` 透传异常。
fn iterate_record_keys<H: VmHost, F>(
    vm: &mut H, set: JsValue, keys_method: JsValue, mut f: F,
) -> Result<KeysFlow, JsValue>
where
    F: FnMut(&mut H, JsValue) -> Result<KeysFlow, JsValue>,
{
    let iter = match vm.call_function_sync(keys_method, set, &[]) {
        Ok(v) => v,
        Err(err) => return Err(crate::iterator::engine_error(vm, &err)),
    };
    if !iter.is_object() {
        return Err(crate::error::create_type_error(vm, "keys() result is not an object"));
    }
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let mut flow = KeysFlow::Continue;
    let run: Result<(), JsValue> = (|| {
        loop {
            let iter_obj = unsafe { &*iter.as_js_object_ptr() };
            let next_fn = vm
                .ordinary_get(iter_obj, next_si, iter)
                .map_err(|e| crate::iterator::engine_error(vm, &e))?;
            let result = vm
                .call_function_sync(next_fn, iter, &[])
                .map_err(|e| crate::iterator::engine_error(vm, &e))?;
            if !result.is_object() {
                return Err(crate::error::create_type_error(vm, "iterator result is not an object"));
            }
            let result_obj = unsafe { &*result.as_js_object_ptr() };
            let done = vm
                .ordinary_get(result_obj, done_si, result)
                .map_err(|e| crate::iterator::engine_error(vm, &e))?;
            if oxide_runtime_api::to_boolean(done) {
                break;
            }
            let value = vm
                .ordinary_get(result_obj, value_si, result)
                .map_err(|e| crate::iterator::engine_error(vm, &e))?;
            flow = f(vm, value)?;
            if matches!(flow, KeysFlow::Stop) {
                break;
            }
        }
        Ok(())
    })();
    if run.is_err() || matches!(flow, KeysFlow::Stop) {
        crate::iterator::close_iterator(vm, iter);
    }
    run.map(|()| flow)
}

/// `Set.prototype.union(other)`：返回 this 与 other 的并集（新 Set，顺序为 this 在前）。
///
/// # 步骤
/// 1. 取 SetRecord，复制 this 的 [[SetData]]。
/// 2. 经 other 的 `keys()` 迭代追加缺失元素，`-0𝔽` 归一化为 `+0𝔽`。
///
/// # 注意事项
/// 只调用 other 的 `keys`，不调用 `has`。
pub fn set_union<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, _size, _has, rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    let mut result: SetInner = unsafe { (*inner).clone() };
    let flow = match iterate_record_keys(vm, rec_set, rec_keys, |_vm, value| {
        result.insert(SetKey(normalize_neg_zero(value)));
        Ok(KeysFlow::Continue)
    }) {
        Ok(f) => f,
        Err(err) => return NativeResult::Err(err),
    };
    debug_assert!(matches!(flow, KeysFlow::Continue));
    NativeResult::Ok(alloc_set_with_inner(vm, result))
}

/// `Set.prototype.intersection(other)`：返回 this 与 other 的交集（新 Set）。
///
/// # 步骤
/// 1. 取 SetRecord。
/// 2. this 元素数 ≤ other 时：遍历 this，保留 other 的 `has(e)` 为真的元素（顺序为 this）。
/// 3. 否则遍历 other 的 `keys()`，保留 this 中含有的元素（顺序为 other）。
pub fn set_intersection<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, rec_size, rec_has, rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    let this_size = unsafe { (*inner).len() } as f64;
    let mut result = SetInner::new();
    if this_size <= rec_size {
        let entries: Vec<JsValue> = unsafe { (*inner).iter().map(|k| k.0).collect() };
        for e in entries {
            let in_other = match vm.call_function_sync(rec_has, rec_set, &[e]) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            };
            if oxide_runtime_api::to_boolean(in_other) {
                result.insert(SetKey(e));
            }
        }
    } else {
        match iterate_record_keys(vm, rec_set, rec_keys, |_vm, value| {
            let value = normalize_neg_zero(value);
            if unsafe { (*inner).contains(&SetKey(value)) } {
                result.insert(SetKey(value));
            }
            Ok(KeysFlow::Continue)
        }) {
            Ok(_) => {}
            Err(err) => return NativeResult::Err(err),
        }
    }
    NativeResult::Ok(alloc_set_with_inner(vm, result))
}

/// `Set.prototype.difference(other)`：返回 this 中不在 other 的元素（新 Set）。
///
/// # 步骤
/// 1. 取 SetRecord。
/// 2. this 元素数 ≤ other 时：遍历 this，保留 other 的 `has(e)` 为假的元素。
/// 3. 否则复制 this，经 other 的 `keys()` 逐个移除存在的元素。
pub fn set_difference<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, rec_size, rec_has, rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    let this_size = unsafe { (*inner).len() } as f64;
    let mut result = SetInner::new();
    if this_size <= rec_size {
        let entries: Vec<JsValue> = unsafe { (*inner).iter().map(|k| k.0).collect() };
        for e in entries {
            let in_other = match vm.call_function_sync(rec_has, rec_set, &[e]) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            };
            if !oxide_runtime_api::to_boolean(in_other) {
                result.insert(SetKey(e));
            }
        }
    } else {
        unsafe {
            for k in (*inner).iter() {
                result.insert(*k);
            }
        }
        match iterate_record_keys(vm, rec_set, rec_keys, |_vm, value| {
            result.shift_remove(&SetKey(normalize_neg_zero(value)));
            Ok(KeysFlow::Continue)
        }) {
            Ok(_) => {}
            Err(err) => return NativeResult::Err(err),
        }
    }
    NativeResult::Ok(alloc_set_with_inner(vm, result))
}

/// `Set.prototype.symmetricDifference(other)`：返回对称差集（新 Set）。
///
/// # 步骤
/// 1. 取 SetRecord，复制 this 的 [[SetData]]。
/// 2. 经 other 的 `keys()`：元素已在结果中则移除，否则追加。
pub fn set_symmetric_difference<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, _size, _has, rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    let mut result: SetInner = unsafe { (*inner).clone() };
    match iterate_record_keys(vm, rec_set, rec_keys, |_vm, value| {
        let key = SetKey(normalize_neg_zero(value));
        if result.shift_remove(&key) {
            // 已在结果中（交集部分）→ 移除。
        } else {
            result.insert(key);
        }
        Ok(KeysFlow::Continue)
    }) {
        Ok(_) => {}
        Err(err) => return NativeResult::Err(err),
    }
    NativeResult::Ok(alloc_set_with_inner(vm, result))
}

/// `Set.prototype.isSubsetOf(other)`：this 的每个元素都在 other 中。
/// this 元素数 > other 的 size 时直接返回 false；否则遍历 this 调 other 的 `has`。
pub fn set_is_subset_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, rec_size, rec_has, _rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    if (unsafe { (*inner).len() }) as f64 > rec_size {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let entries: Vec<JsValue> = unsafe { (*inner).iter().map(|k| k.0).collect() };
    for e in entries {
        let in_other = match vm.call_function_sync(rec_has, rec_set, &[e]) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
        };
        if !oxide_runtime_api::to_boolean(in_other) {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `Set.prototype.isSupersetOf(other)`：other 的每个元素都在 this 中。
/// this 元素数 < other 的 size 时直接返回 false；否则经 other 的 `keys()` 逐元素
/// 检查，提前发现不在时终止并 IteratorClose。
pub fn set_is_superset_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, rec_size, _has, rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    if ((unsafe { (*inner).len() }) as f64) < rec_size {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let flow = match iterate_record_keys(vm, rec_set, rec_keys, |_vm, value| {
        if !unsafe { (*inner).contains(&SetKey(normalize_neg_zero(value))) } {
            Ok(KeysFlow::Stop)
        } else {
            Ok(KeysFlow::Continue)
        }
    }) {
        Ok(f) => f,
        Err(err) => return NativeResult::Err(err),
    };
    NativeResult::Ok(JsValue::bool(matches!(flow, KeysFlow::Continue)))
}

/// `Set.prototype.isDisjointFrom(other)`：this 与 other 无公共元素。
/// this 元素数 ≤ other 时遍历 this 调 other 的 `has`；否则遍历 other 的 `keys()`。
pub fn set_is_disjoint_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let other = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let (rec_set, rec_size, rec_has, rec_keys) = match get_set_record(vm, other) {
        Ok(r) => r,
        Err(err) => return NativeResult::Err(err),
    };
    let this_size = unsafe { (*inner).len() } as f64;
    let disjoint = if this_size <= rec_size {
        let entries: Vec<JsValue> = unsafe { (*inner).iter().map(|k| k.0).collect() };
        let mut disjoint = true;
        for e in entries {
            let in_other = match vm.call_function_sync(rec_has, rec_set, &[e]) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            };
            if oxide_runtime_api::to_boolean(in_other) {
                disjoint = false;
                break;
            }
        }
        disjoint
    } else {
        match iterate_record_keys(vm, rec_set, rec_keys, |_vm, value| {
            if unsafe { (*inner).contains(&SetKey(normalize_neg_zero(value))) } {
                Ok(KeysFlow::Stop)
            } else {
                Ok(KeysFlow::Continue)
            }
        }) {
            Ok(KeysFlow::Continue) => true,
            Ok(KeysFlow::Stop) => false,
            Err(err) => return NativeResult::Err(err),
        }
    };
    NativeResult::Ok(JsValue::bool(disjoint))
}

#[cfg(test)]
mod set_key_tests {
    use super::SetKey;
    use oxide_types::object::JsString;
    use oxide_types::value::JsValue;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash_of(key: &SetKey) -> u64 {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        h.finish()
    }

    /// 构造两个独立分配的串键；返回键与持有 Box（键指针在测试内须保持有效）。
    fn string_keys(a: &str, b: &str) -> (SetKey, SetKey, Box<JsString>, Box<JsString>) {
        let ba = Box::new(JsString::new(a.to_string()));
        let bb = Box::new(JsString::new(b.to_string()));
        (SetKey(JsValue::string(&*ba)), SetKey(JsValue::string(&*bb)), ba, bb)
    }

    #[test]
    fn string_key_content_equality_and_hash() {
        // 同内容不同分配的串键：值相等且哈希一致；不同内容不等。
        let (ka, kb, _ba, _bb) = string_keys("hello", "hello");
        assert_eq!(ka, kb);
        assert_eq!(hash_of(&ka), hash_of(&kb));
        let (_kc, kd, _bc, _bd) = string_keys("hello", "world");
        assert_ne!(ka, kd);
    }

    #[test]
    fn number_key_same_value_zero_canonical() {
        // NaN 同自身；±0 同键（含 int 0 与 double -0.0 混表示）；
        // NaN 与最小正非规格化数（位模式 1）不得同键。
        let nan = SetKey(JsValue::float(f64::NAN));
        assert_eq!(nan, nan);
        assert_eq!(hash_of(&nan), hash_of(&nan));
        let zero_int = SetKey(JsValue::int(0));
        let neg_zero = SetKey(JsValue::float(-0.0));
        assert_eq!(zero_int, neg_zero);
        assert_eq!(hash_of(&zero_int), hash_of(&neg_zero));
        assert_eq!(hash_of(&SetKey(JsValue::int(42))), hash_of(&SetKey(JsValue::float(42.0))));
        let subnormal = SetKey(JsValue::float(f64::from_bits(1)));
        assert_ne!(nan, subnormal);
        assert_ne!(zero_int, subnormal);
        assert_ne!(hash_of(&nan), hash_of(&subnormal));
    }

    #[test]
    fn bigint_key_value_equality_and_hash() {
        // 同值不同盒相等且哈希一致；异值不等。
        let ba = Box::new(num_bigint::BigInt::from(123));
        let bb = Box::new(num_bigint::BigInt::from(123));
        let bc = Box::new(num_bigint::BigInt::from(-123));
        let ka = SetKey(JsValue::bigint(&*ba));
        let kb = SetKey(JsValue::bigint(&*bb));
        let kc = SetKey(JsValue::bigint(&*bc));
        assert_eq!(ka, kb);
        assert_eq!(hash_of(&ka), hash_of(&kb));
        assert_ne!(ka, kc);
    }
}
