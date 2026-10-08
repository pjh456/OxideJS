//! Atomics 全局纯对象的 12 个原子方法（load/store/exchange/add/sub/and/or/
//! xor/compareExchange/isLockFree/wait/notify）。单线程引擎无锁层级：算术族
//! 退化为读-改-写；wait/notify 退化为"调用时即定"的同步比较/恒 0（无真
//! 阻塞、无 waiter 面）；宽度截断与元素写路径（`write_element`）结构同构。

use num_bigint::BigInt;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, TypedArrayKind};
use oxide_types::value::JsValue;

use crate::array_buffer::{buffer_store_mut_ptr, BufferStoreMut};
use crate::typed_array::{
    get_typed_array_data, read_element, ta_element_value, ta_to_integer_or_infinity, ta_validate, write_element,
    write_element_shared, TypedArrayData,
};
use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn type_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_type_error(vm, msg)
}

fn range_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_range_error(vm, msg)
}

/// 按寄存器下标取实参；越界（实参缺失）回落 undefined（JS 缺省参语义）。
fn arg_at<H: VmHost>(vm: &mut H, args: &[u8], idx: usize) -> JsValue {
    if args.len() > idx {
        vm.reg(args[idx])
    } else {
        JsValue::undefined()
    }
}

/// 原子操作可接受的整数宽度：八整数种类（含 BigInt64/BigUint64）；
/// 浮点（Float32/Float64）与 Uint8Clamped 视图非整数宽度，抛 TypeError。
fn is_integer_kind(kind: TypedArrayKind) -> bool {
    matches!(
        kind,
        TypedArrayKind::Int8
            | TypedArrayKind::Uint8
            | TypedArrayKind::Int16
            | TypedArrayKind::Uint16
            | TypedArrayKind::Int32
            | TypedArrayKind::Uint32
            | TypedArrayKind::BigInt64
            | TypedArrayKind::BigUint64
    )
}

fn is_bigint_kind(kind: TypedArrayKind) -> bool {
    matches!(kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64)
}

/// 原子访问共享核：品牌 → 整数宽度守卫 → 索引 ToIntegerOrInfinity →
/// 相对字节偏移越界 RangeError →（写族）ValidateTypedArray 写守卫。
///
/// # 步骤
/// 1. `get_typed_array_data` 品牌校验（非 TA 对象抛 TypeError）。
/// 2. 整数宽度守卫（浮点/clamped 视图抛 TypeError）。
/// 3. 写族加查写守卫（immutable/detach 抛 TypeError），先于索引/值 coercion。
/// 4. 索引经 ToIntegerOrInfinity（valueOf/toString 抛错原值上抛，先于越界判）。
/// 5. 相对字节偏移 = view.byte_offset + index·bpe（f64 运算）；越界
///    （`< 0` 或 `>= buffer 字节长`）抛 RangeError，detach 视同越界。
///
/// # 边界与前提
/// - 返回的载荷指针、视图与偏移在 session 独占期内有效，调用方同一语句内消费。
/// - 偏移已验证落在 `[0, buffer 字节长)`，`as usize` 安全。
///
/// # 副作用
/// 无（纯读；写守卫只读 immutable 位）。
fn resolve_atomic_access<'a, H: VmHost>(
    vm: &mut H, this_val: JsValue, writable: bool, index_val: JsValue,
) -> Result<(BufferStoreMut<'a>, TypedArrayData, usize), JsValue> {
    let view = get_typed_array_data(vm, this_val)?;
    if !is_integer_kind(view.kind) {
        return Err(type_error(vm, "Atomics operations require an integer TypedArray type"));
    }
    // 写守卫（不可变缓冲 TypeError）先于索引/值 coercion 求值：守卫抛错时
    // index/value 的 valueOf 不得被调用（语料 calls 序列口径）。
    if writable {
        ta_validate(vm, view, true)?;
    }
    let index = ta_to_integer_or_infinity(vm, index_val)?;

    // 缓冲字节长：AB/SAB 双认，detached 视同 0（越界）。
    let buffer_ptr = view.buffer.as_js_object_ptr();
    if buffer_ptr.is_null() {
        return Err(type_error(vm, "TypedArray buffer internal state invalid"));
    }
    // SAFETY: buffer 对象与视图同生命周期，此处只读写载荷。
    let obj: &'a JsObject = unsafe { &*buffer_ptr };
    let buffer_len = match buffer_store_mut_ptr(obj) {
        Some(BufferStoreMut::Ab(bytes)) => bytes.len(),
        Some(BufferStoreMut::Sab(buffer)) => buffer.len(),
        None => 0,
    };

    let offset = view.byte_offset as f64 + index * (view.kind.bytes_per_element() as f64);
    if offset < 0.0 || offset >= buffer_len as f64 {
        return Err(range_error(vm, "invalid indexed access to Atomics"));
    }
    // 界判通过，载荷必在场（detached 已越界排除）。
    let store = buffer_store_mut_ptr(obj)
        .ok_or_else(|| range_error(vm, "invalid indexed access to Atomics"))?;
    Ok((store, view, offset as usize))
}

/// 读指定偏移元素并转为 JS 值（detach 已由 resolve 的越界判收口，载荷恒在场）。
fn atomic_read<H: VmHost>(
    vm: &mut H, store: &BufferStoreMut, kind: TypedArrayKind, offset: usize,
) -> Option<JsValue> {
    // SAFETY: store 经 resolve_atomic_access 校验为存活载荷，只读字节切片。
    let bytes: &[u8] = match store {
        BufferStoreMut::Ab(b) => b,
        BufferStoreMut::Sab(buffer) => buffer.as_slice(),
    };
    Some(read_element(vm, kind, bytes, offset))
}

/// 把已转换值按位模式写入指定偏移（detach 已由 resolve 的越界判收口）。
fn atomic_write<H: VmHost>(
    vm: &mut H, store: &mut BufferStoreMut, kind: TypedArrayKind, offset: usize, value: JsValue,
) {
    // SAFETY: store 经 resolve_atomic_access 校验为存活载荷，只写字节切片。
    match store {
        BufferStoreMut::Ab(bytes) => {
            write_element(vm, kind, bytes, offset, value);
        }
        BufferStoreMut::Sab(buffer) => {
            // SAB 臂无 detach 路径，写经共享缓冲（越界静默不写）。
            write_element_shared(vm, kind, buffer, offset, value);
        }
    }
}

/// 读-改-写运算结果：整数宽度按算术/位运算在 i32 域（语料值 < 2^31，f64 精确），
/// BigInt 宽度在 BigInt 域；宽度截断由 [`atomic_write`] 收口，返回待写值。
fn arithmetic_write_value<H: VmHost>(
    vm: &mut H, kind: TypedArrayKind, op: &str, old: JsValue, value: JsValue,
) -> JsValue {
    if is_bigint_kind(kind) {
        let old_bi = vm.bigint_value(old).clone();
        let val_bi = vm.bigint_value(value).clone();
        let r: BigInt = match op {
            "add" => &old_bi + &val_bi,
            "sub" => &old_bi - &val_bi,
            "and" => &old_bi & &val_bi,
            "or" => &old_bi | &val_bi,
            "xor" => &old_bi ^ &val_bi,
            _ => unreachable!("未知原子运算"),
        };
        return vm.new_bigint(r);
    }
    let old_i = oxide_runtime_api::to_number(old) as i32;
    let val_i = oxide_runtime_api::to_number(value) as i32;
    let r = match op {
        "add" => old_i.wrapping_add(val_i),
        "sub" => old_i.wrapping_sub(val_i),
        "and" => old_i & val_i,
        "or" => old_i | val_i,
        "xor" => old_i ^ val_i,
        _ => unreachable!("未知原子运算"),
    };
    JsValue::int(r)
}

/// wait/notify 共享验证核：品牌 → {Int32, BigInt64} 宽度守卫 → 越界/detach
/// TypeError → 缓冲字节长读取（先于 index 强转，"长度读取"语料钉）→
/// index ToIntegerOrInfinity → 相对字节偏移越界 RangeError。
///
/// # 步骤
/// 1. `get_typed_array_data` 品牌校验（非 TA 对象抛 TypeError）。
/// 2. 宽度守卫（仅 Int32/BigInt64；其余整数/浮点/clamped 视图抛 TypeError，
///    与算术族的八整数宽度集不同）。
/// 3. 越界/detach 守卫（只读面，不查 immutable——notify 对不可变缓冲返 0 不抛）。
/// 4. `require_sab` 臂：非 SAB 抛 TypeError（先于 index 强转）。
/// 5. 缓冲字节长与载荷指针在 index 强转前读取：index 的 valueOf 内若发生
///    缓冲 grow/resize，越界判定仍用强转前读到的长度（语料钉）。
/// 6. 索引越界（负值，或元素字节区间超缓冲）抛 RangeError。
///
/// # 边界与前提
/// - 返回的载荷指针在 session 独占期内有效，调用方同一语句内消费。
/// - `require_sab` 臂（wait）在 index 强转前即抛非 SAB TypeError（毒序钉：
///   毒 index 不得被评估）；`is_sab` 标志供 notify 臂在 count 强转后选早返 0。
///
/// # 副作用
/// 无（纯读）。
fn resolve_atomic_wait_access<'a, H: VmHost>(
    vm: &mut H, this_val: JsValue, index_val: JsValue, require_sab: bool,
) -> Result<(BufferStoreMut<'a>, TypedArrayData, usize, bool), JsValue> {
    let view = get_typed_array_data(vm, this_val)?;
    if !matches!(view.kind, TypedArrayKind::Int32 | TypedArrayKind::BigInt64) {
        return Err(type_error(vm, "not an int32 or BigInt64 typed array"));
    }
    ta_validate(vm, view, false)?;

    let buffer_ptr = view.buffer.as_js_object_ptr();
    if buffer_ptr.is_null() {
        return Err(type_error(vm, "TypedArray buffer internal state invalid"));
    }
    // SAFETY: buffer 对象与视图同生命周期，此处只读写载荷。
    let obj: &'a JsObject = unsafe { &*buffer_ptr };
    let is_sab = obj.is_shared_array_buffer_obj();
    if require_sab && !is_sab {
        return Err(type_error(vm, "Atomics.wait cannot be used on a non-shared buffer"));
    }
    // 缓冲字节长：AB/SAB 双认，detached 视同 0（越界）。
    let buffer_len = match buffer_store_mut_ptr(obj) {
        Some(BufferStoreMut::Ab(bytes)) => bytes.len(),
        Some(BufferStoreMut::Sab(buffer)) => buffer.len(),
        None => 0,
    };

    let index = ta_to_integer_or_infinity(vm, index_val)?;
    let esize = view.kind.bytes_per_element() as f64;
    let offset = view.byte_offset as f64 + index * esize;
    if offset < 0.0 || offset + esize > buffer_len as f64 {
        return Err(range_error(vm, "invalid indexed access to Atomics"));
    }
    // 界判通过，载荷必在场（detached 已越界排除）。
    let store = buffer_store_mut_ptr(obj)
        .ok_or_else(|| range_error(vm, "invalid indexed access to Atomics"))?;
    Ok((store, view, offset as usize, is_sab))
}

/// `Atomics.wait(typedArray, index, value, timeout)` 单线程退化面：值/超时
/// 强转（毒传播）后读载荷比较——等值返 "timed-out"、不等返 "not-equal"；
/// 无真阻塞（单线程无他 agent 可改值，结果调用时即定）。
pub fn atomics_wait<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let value_val = arg_at(vm, args, 3);
    let timeout_val = arg_at(vm, args, 4);
    let (store, view, offset, _) = native_try!(resolve_atomic_wait_access(vm, ta_val, index_val, true));
    let value = native_try!(ta_element_value(vm, view.kind, value_val));
    // 超时 ToNumber 归一（NaN → +∞、负值 → 0）：单线程下无观察差，只承载毒传播。
    native_try!(oxide_runtime_api::to_number_full(timeout_val, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
    let old = atomic_read(vm, &store, view.kind, offset).unwrap_or(JsValue::undefined());
    let equal = if is_bigint_kind(view.kind) {
        let old_bi = vm.bigint_value(old).clone();
        let value_bi = vm.bigint_value(value).clone();
        old_bi.cmp(&value_bi) == std::cmp::Ordering::Equal
    } else {
        oxide_runtime_api::to_number(old) == oxide_runtime_api::to_number(value)
    };
    NativeResult::Ok(vm.new_string(if equal { "timed-out" } else { "not-equal" }))
}

/// `Atomics.notify(typedArray, index, count)`：值验证后 count 经
/// ToIntegerOrInfinity（毒传播，NaN → +∞、有限负值 → 0）；非 SAB 提前返 0
/// （先于本早返的越界/类型异常照常抛出）；SAB 臂唤醒 (缓冲, 偏移) 处登记的
/// waitAsync waiter，返唤醒数（无登记恒 0）。
pub fn atomics_notify<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let count_val = arg_at(vm, args, 3);
    let (_store, view, offset, is_sab) = native_try!(resolve_atomic_wait_access(vm, ta_val, index_val, false));
    // count 实参缺省 → +∞（全唤醒）；显式传入才走强转（毒传播语料钉：
    // 非 SAB 亦先评估 count 再返 0）。
    let count = if args.len() > 3 {
        native_try!(ta_to_integer_or_infinity(vm, count_val))
    } else {
        f64::INFINITY
    };
    if !is_sab {
        return NativeResult::Ok(JsValue::int(0));
    }
    // 与登记侧同一读径取缓冲现指针，同 run 无 GC 时恒匹配。
    let buffer_ptr = view.buffer.as_js_object_ptr();
    NativeResult::Ok(JsValue::int(vm.atomics_wake_waiters(buffer_ptr, offset, count) as i32))
}

/// `Atomics.load(typedArray, index)`：读指定索引元素。
pub fn atomics_load<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let (store, view, offset) = native_try!(resolve_atomic_access(vm, ta_val, false, index_val));
    NativeResult::Ok(atomic_read(vm, &store, view.kind, offset).unwrap_or(JsValue::undefined()))
}

/// `Atomics.store(typedArray, index, value)`：写值后读回返回（宽度归一，-0 → +0）。
pub fn atomics_store<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let value_val = arg_at(vm, args, 3);
    let (mut store, view, offset) = native_try!(resolve_atomic_access(vm, ta_val, true, index_val));
    let value = native_try!(ta_element_value(vm, view.kind, value_val));
    atomic_write(vm, &mut store, view.kind, offset, value);
    NativeResult::Ok(atomic_read(vm, &store, view.kind, offset).unwrap_or(value))
}

/// `Atomics.exchange(typedArray, index, value)`：读旧值、写新值、返回旧值。
pub fn atomics_exchange<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let value_val = arg_at(vm, args, 3);
    let (mut store, view, offset) = native_try!(resolve_atomic_access(vm, ta_val, true, index_val));
    let old = atomic_read(vm, &store, view.kind, offset).unwrap_or(JsValue::undefined());
    let value = native_try!(ta_element_value(vm, view.kind, value_val));
    atomic_write(vm, &mut store, view.kind, offset, value);
    NativeResult::Ok(old)
}

/// `Atomics.add(typedArray, index, value)`：读旧值、加 value 写回、返回旧值。
pub fn atomics_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    atomic_arithmetic(vm, args, "add")
}

/// `Atomics.sub(typedArray, index, value)`：读旧值、减 value 写回、返回旧值。
pub fn atomics_sub<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    atomic_arithmetic(vm, args, "sub")
}

/// `Atomics.and(typedArray, index, value)`：读旧值、与 value 按位与写回、返回旧值。
pub fn atomics_and<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    atomic_arithmetic(vm, args, "and")
}

/// `Atomics.or(typedArray, index, value)`：读旧值、与 value 按位或写回、返回旧值。
pub fn atomics_or<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    atomic_arithmetic(vm, args, "or")
}

/// `Atomics.xor(typedArray, index, value)`：读旧值、与 value 按位异或写回、返回旧值。
pub fn atomics_xor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    atomic_arithmetic(vm, args, "xor")
}

/// add/sub/and/or/xor 共享核：读旧值 → 运算值转换 → 读-改-写 → 返回旧值。
fn atomic_arithmetic<H: VmHost>(vm: &mut H, args: &[u8], op: &str) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let value_val = arg_at(vm, args, 3);
    let (mut store, view, offset) = native_try!(resolve_atomic_access(vm, ta_val, true, index_val));
    let old = atomic_read(vm, &store, view.kind, offset).unwrap_or(JsValue::undefined());
    let value = native_try!(ta_element_value(vm, view.kind, value_val));
    let result = arithmetic_write_value(vm, view.kind, op, old, value);
    atomic_write(vm, &mut store, view.kind, offset, result);
    NativeResult::Ok(old)
}

/// `Atomics.compareExchange(typedArray, index, expected, replacement)`：读旧值；
/// 相等（数值宽度 `==`、BigInt 宽度 BigInt 相等）则写 replacement；返回旧值。
pub fn atomics_compare_exchange<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let expected_val = arg_at(vm, args, 3);
    let replacement_val = arg_at(vm, args, 4);
    let (mut store, view, offset) = native_try!(resolve_atomic_access(vm, ta_val, true, index_val));
    let old = atomic_read(vm, &store, view.kind, offset).unwrap_or(JsValue::undefined());
    let expected = native_try!(ta_element_value(vm, view.kind, expected_val));
    let matched = if is_bigint_kind(view.kind) {
        let old_bi = vm.bigint_value(old).clone();
        let expected_bi = vm.bigint_value(expected).clone();
        old_bi.cmp(&expected_bi) == std::cmp::Ordering::Equal
    } else {
        oxide_runtime_api::to_number(old) == oxide_runtime_api::to_number(expected)
    };
    if matched {
        let replacement = native_try!(ta_element_value(vm, view.kind, replacement_val));
        atomic_write(vm, &mut store, view.kind, offset, replacement);
    }
    NativeResult::Ok(old)
}

/// `Atomics.isLockFree(size)`：单线程引擎无锁层级，`size ∈ {1,2,4,8}` 恒真，余假。
pub fn atomics_is_lock_free<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let size_val = arg_at(vm, args, 1);
    let n = native_try!(ta_to_integer_or_infinity(vm, size_val));
    NativeResult::Ok(JsValue::bool(matches!(n, 1.0 | 2.0 | 4.0 | 8.0)))
}

/// waitAsync 结果对象：普通对象（proto = Object.prototype），own 属性
/// async/value 双 data 属性（普通创建路径默认形）。
fn wait_async_result_object<H: VmHost>(vm: &mut H, async_arm: bool, value: JsValue) -> JsValue {
    let object_proto = vm.builtin_proto(ProtoKind::ObjectProto);
    let obj_ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    // SAFETY: alloc_object 返回存活 arena 指针，借出期间无别名。
    let obj = unsafe { &mut *obj_ptr };
    let async_si = vm.string_key_si("async");
    let value_si = vm.string_key_si("value");
    vm.set_or_create_prop_value(obj, async_si, JsValue::bool(async_arm));
    vm.set_or_create_prop_value(obj, value_si, value);
    JsValue::from_js_object(obj_ptr)
}

/// `Atomics.waitAsync(typedArray, index, value, timeout)`：值强转（毒传播）
/// 后读载荷比较——不等值返 {async:false, value:"not-equal"}；等值再归一 timeout
/// （NaN → +∞、<0 → 0），≤0 返 {async:false, value:"timed-out"}；等值且 >0
/// 建 pending Promise 登记 waiter 表（notify 唤醒结算 "ok"），返
/// {async:true, value:Promise}。timeout 强转在不等值早返之后（spec 步序）。
pub fn atomics_wait_async<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ta_val = arg_at(vm, args, 1);
    let index_val = arg_at(vm, args, 2);
    let value_val = arg_at(vm, args, 3);
    let timeout_val = arg_at(vm, args, 4);
    let (store, view, offset, _) = native_try!(resolve_atomic_wait_access(vm, ta_val, index_val, true));
    let value = native_try!(ta_element_value(vm, view.kind, value_val));
    let old = atomic_read(vm, &store, view.kind, offset).unwrap_or(JsValue::undefined());
    let equal = if is_bigint_kind(view.kind) {
        let old_bi = vm.bigint_value(old).clone();
        let value_bi = vm.bigint_value(value).clone();
        old_bi.cmp(&value_bi) == std::cmp::Ordering::Equal
    } else {
        oxide_runtime_api::to_number(old) == oxide_runtime_api::to_number(value)
    };
    if !equal {
        let str = vm.new_string("not-equal");
        return NativeResult::Ok(wait_async_result_object(vm, false, str));
    }
    // 超时 ToNumber 归一：毒传播 + NaN → +∞、负值 → 0。
    let timeout = native_try!(
        oxide_runtime_api::to_number_full(timeout_val, vm).map_err(|e| crate::iterator::engine_error(vm, &e))
    );
    let normalized = if timeout.is_nan() {
        f64::INFINITY
    } else if timeout < 0.0 {
        0.0
    } else {
        timeout
    };
    if normalized <= 0.0 {
        let str = vm.new_string("timed-out");
        return NativeResult::Ok(wait_async_result_object(vm, false, str));
    }
    // 登记期取视图 buffer 现指针为键：同 run 无 GC 时 notify 侧同读径恒匹配。
    let promise = vm.atomics_new_waiter_promise();
    vm.atomics_register_waiter(view.buffer.as_js_object_ptr(), offset, promise);
    NativeResult::Ok(wait_async_result_object(vm, true, promise))
}

/// `Atomics.pause(hint)`：hint 为 undefined 或整数 Number 时直返 undefined；
/// 其余值（布尔/串/BigInt/对象/非整数 Number 等）抛 TypeError。
pub fn atomics_pause<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let hint = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if hint.is_undefined() {
        return NativeResult::Ok(JsValue::undefined());
    }
    // hint 须为整数 Number：int 表示恒整数；double 表示须有限且无小数部分。
    let is_integral_number = if hint.is_int() {
        true
    } else if hint.is_double() {
        let v = hint.as_double();
        v.is_finite() && v.fract() == 0.0
    } else {
        false
    };
    if !is_integral_number {
        return NativeResult::Err(crate::error::create_type_error(vm, "iterationNumber must be an integer"));
    }
    NativeResult::Ok(JsValue::undefined())
}
