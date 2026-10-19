use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{encode_symbol_key, make_int_key};
use oxide_types::string_forge::PermInterner;
use oxide_types::value::JsValue;

use oxide_runtime_api::{to_object, NativeResult, ProtoKind, VmHost};

const INNER_PROP: &str = "__inner__";
const INDEX_PROP: &str = "__index__";
const MODE_PROP: &str = "__mode__";
const NEXT_CACHE_PROP: &str = "__next__";
const KIND_PROP: &str = "__kind__";
const STATE_PROP: &str = "__state__";
const COUNTER_PROP: &str = "__counter__";
const CALLBACK_PROP: &str = "__callback__";
const INNER_ITER_PROP: &str = "__inner_iter__";
const INNER_NEXT_PROP: &str = "__inner_next__";
const ZIPS_PROP: &str = "__zips__";
const OPEN_PROP: &str = "__open__";
const RESULTS_PROP: &str = "__results__";
const PADDING_PROP: &str = "__padding__";
const KEYS_PROP: &str = "__keys__";
const ITEMS_PROP: &str = "__items__";
const SEG_PROP: &str = "__seg__";
const CURRENT_PROP: &str = "__current__";
const CURRENT_NEXT_PROP: &str = "__current_next__";

/// `Iterator` 构造逻辑：不是构造函数，`new Iterator()` / `Iterator()` 均抛 TypeError；
/// subclass `super()` 路径放行并返回 undefined（原型由调用方按 newTarget 设置）。
///
/// 引擎无 native [[Construct]]/newTarget 通道，用与 `Symbol` 构造器同款的启发式
/// 判别调用形态：
///
/// # 步骤
/// 1. `this` 非对象 → TypeError（普通调用 `Iterator()`：emit 以 undefined 作 this）。
/// 2. `this.proto === %IteratorPrototype%` → TypeError（`new Iterator()`：调用方以
///    ctor.prototype 为原型建新对象传入）。
/// 3. newTarget 槽非对象 → TypeError（`Iterator.call(x)` 顶层调用，顶层 newTarget
///    初始化为 undefined）。newTarget 由 255 号寄存器承载：254/255 是 VM 为
///    `this` / new.target 保留的两个槽，调用方负责保存与恢复。
/// 4. 否则返回 undefined（subclass `super()`：调用方随后按 new.target.prototype
///    设置实例原型）。
///
/// # 注意事项
/// 类构造器内调用 `Iterator.call(x)` 时 newTarget 槽为类对象会被放行（规范外
/// 罕见场景，与 `Symbol` 构造器同款已知边界）。
pub fn iterator_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator is not a constructor"));
    }
    let ptr = this_val.as_js_object_ptr();
    if !ptr.is_null() {
        let obj = unsafe { &*ptr };
        let proto = obj.proto();
        if proto.is_object() {
            let proto_ptr = proto.as_js_object_ptr();
            if !proto_ptr.is_null() {
                let home = vm.builtin_proto(ProtoKind::IteratorProto);
                if std::ptr::eq(proto_ptr, home) {
                    return NativeResult::Err(crate::error::create_type_error(vm, "Iterator is not a constructor"));
                }
            }
        }
    }
    if !vm.reg(255).is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator is not a constructor"));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%IteratorPrototype%` 上 `constructor` 访问器的 getter：返回当前 global 上的
/// `Iterator` 构造器。
///
/// 动态查 global（shape 槽查找）而非缓存指针：global 整体重建（dirty reset）会
/// 重建内置对象族，换新的 `Iterator` 函数对象；缓存旧指针会指向已释放对象。
/// lookup 失败（极端：global 无 `Iterator`）返回 undefined，不 panic。
pub fn iterator_constructor_getter<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let (shape_id, global_ptr) = {
        let global = vm.global_object();
        (global.shape_id(), global.as_ptr() as *mut JsObject)
    };
    let si = vm.perm_intern("Iterator");
    let Some(pos) = vm.lookup_position(shape_id, si) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    NativeResult::Ok(unsafe { &*global_ptr }.get_prop_at(pos))
}

/// `%IteratorPrototype%` 上 `Symbol.toStringTag` 访问器的 getter：返回 `"Iterator"`。
pub fn iterator_to_string_tag_getter<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    // SAFETY: perm interner 随 VM 存活，本表达式内只读即时消费。
    let sf = vm.perm_interner_ptr() as *const PermInterner;
    NativeResult::Ok(JsValue::perm_string(unsafe { &*sf }.string_ptr(vm.perm_intern("Iterator"))))
}

/// 忽略原型属性的 setter 的共享实现（对应规范 SetterThatIgnoresPrototypeProperties）：
/// `%IteratorPrototype%` 的 `constructor` 与 `Symbol.toStringTag` 访问器共用同一
/// 语义，仅属性键不同。
///
/// # 步骤
/// 1. `this` 非对象 → TypeError（原始值直接抛，不建 own 属性）。
/// 2. `this` 为 `%IteratorPrototype%` 本体 → TypeError（模拟对原型上不可写数据
///    属性的严格模式赋值）。
/// 3. `this` 无 own 指定键属性 → CreateDataPropertyOrThrow（写全可写可枚举）。
/// 4. 有 own 属性 → 普通 Set（继承访问器时在此被调用的场景）。
fn iterator_setter_ignore_proto_props<H: VmHost>(vm: &mut H, args: &[u8], key: u32) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator property setter called on non-object"));
    }
    let home = vm.builtin_proto(ProtoKind::IteratorProto);
    if std::ptr::eq(this_val.as_js_object_ptr(), home) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Cannot assign to read only property of Iterator prototype",
        ));
    }
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let this_obj = unsafe { &mut *this_val.as_js_object_ptr() };
    if vm.lookup_position(this_obj.shape_id(), key).is_none() {
        match vm.define_data_property(this_obj, key, val, PropAttributes::new(true, true, true)) {
            Ok(()) => NativeResult::Ok(JsValue::undefined()),
            Err(err) => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        }
    } else {
        match vm.ordinary_set(this_obj, key, val, this_val, true) {
            Ok(()) => NativeResult::Ok(JsValue::undefined()),
            Err(err) => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        }
    }
}

/// `constructor` 访问器的 setter（键 = "constructor"）。
pub fn iterator_constructor_setter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let key = vm.perm_intern("constructor");
    iterator_setter_ignore_proto_props(vm, args, key)
}

/// `Symbol.toStringTag` 访问器的 setter（键为 `@@toStringTag`）。
pub fn iterator_to_string_tag_setter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    iterator_setter_ignore_proto_props(vm, args, encode_symbol_key(vm.realm_id(), 9))
}

/// `%IteratorPrototype%[@@dispose]`：GetMethod(this, "return")，有则调用并返回
/// undefined。
///
/// # 步骤
/// 1. `this` 非对象 → TypeError（GetMethod 的 GetV 语义要求对象）。
/// 2. 读 `return` 方法：不可调用（undefined/null）则跳过；getter 抛错透传原值。
/// 3. 调用 `return()` 成功/抛错均最终返回 undefined 或透传原异常。
pub fn iterator_dispose<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "%IteratorPrototype%[@@dispose] called on non-object",
        ));
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let return_si = vm.perm_intern("return");
    let return_fn = match vm.ordinary_get(obj, return_si, this_val) {
        Ok(f) if is_callable(f) => f,
        Ok(_) => JsValue::undefined(),
        Err(err) => return NativeResult::Err(engine_error(vm, &err)),
    };
    if !return_fn.is_undefined() {
        if let Err(err) = vm.call_function_sync(return_fn, this_val, &[]) {
            return NativeResult::Err(engine_error(vm, &err));
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

// ── 终端方法共享工具（forEach/every/some/find/reduce/toArray 共用）──

/// 终端方法的统一前置校验：`this` 非对象 → TypeError；回调不可调用 → 先关闭底层
/// 迭代器（此路径不读取 next）后抛 TypeError。
///
/// # 步骤
/// 1. `this` 非对象 → TypeError（原始值直接抛，不读 next）。
/// 2. 回调不可调用 → 以 `{Iterator: this, NextMethod: undefined}` 构造记录并
///    IteratorClose（延迟 GetMethod return 并调用），随后抛 TypeError。
/// 3. GetIteratorDirect：读 `next` 一次并缓存（getter 抛错透传原值）。
///
/// # 返回值
/// - `Ok((iterated, next))`：可进入循环；`Err` 为透传的异常值。
pub(crate) fn validate_terminal_and_get_direct<H: VmHost>(
    vm: &mut H, this_val: JsValue, callback: JsValue,
) -> Result<(JsValue, JsValue), JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "Iterator.prototype method called on non-object"));
    }
    if !is_callable(callback) {
        // 校验失败仍关底层：NextMethod 未读，IteratorClose 只经 return 方法关闭。
        let err = crate::error::create_type_error(vm, "Iterator.prototype method requires a callable callback");
        let _ = iterator_close_record(vm, this_val, Some(err));
        return Err(err);
    }
    get_iterator_direct(vm, this_val)
}

/// 读 `next` 一次并缓存，返回 `(iterated, next)` 对（对应规范 GetIteratorDirect）。
///
/// # 边界与前提
/// - `this_val` 必须是对象（调用方已校验）。
/// - `next` getter 抛错时透传原异常值。
pub(crate) fn get_iterator_direct<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<(JsValue, JsValue), JsValue> {
    let iter_obj = unsafe { &*this_val.as_js_object_ptr() };
    let next_si = vm.perm_intern("next");
    let next = vm.ordinary_get(iter_obj, next_si, this_val).map_err(|e| engine_error(vm, &e))?;
    Ok((this_val, next))
}

/// 调用缓存的 next 取一步，读结果对象的 `done`/`value`（对应规范 IteratorStepValue）。
///
/// # 返回值
/// - `Ok(Some(value))`：有元素产出；
/// - `Ok(None)`：迭代完成（done=true，不读 value getter）；
/// - `Err`：next 抛错 / 结果非对象 / done/value getter 抛错（透传原值，不关底层）。
pub(crate) fn iterator_record_step<H: VmHost>(
    vm: &mut H, next: JsValue, iterated: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let result = vm.call_function_sync(next, iterated, &[]).map_err(|e| engine_error(vm, &e))?;
    if !result.is_object() {
        return Err(crate::error::create_type_error(vm, "iterator result is not an object"));
    }
    let result_obj = unsafe { &*result.as_js_object_ptr() };
    let done_si = vm.perm_intern("done");
    let done = vm.ordinary_get(result_obj, done_si, result).map_err(|e| engine_error(vm, &e))?;
    if oxide_runtime_api::to_boolean(done) {
        return Ok(None);
    }
    let value_si = vm.perm_intern("value");
    let value = vm
        .ordinary_get(result_obj, value_si, result)
        .map_err(|e| engine_error(vm, &e))?;
    Ok(Some(value))
}

/// 调用缓存的 next 取一步，只读结果对象的 `done`（对应规范 IteratorStep，
/// 不读 value getter）。
///
/// # 返回值
/// - `Ok(true)`：迭代完成（done=true）；
/// - `Ok(false)`：未耗尽（done=false，不读 value）；
/// - `Err`：next 抛错 / 结果非对象 / done getter 抛错（透传原值，不关底层）。
pub(crate) fn iterator_step_done_only<H: VmHost>(
    vm: &mut H, next: JsValue, iterated: JsValue,
) -> Result<bool, JsValue> {
    let result = vm.call_function_sync(next, iterated, &[]).map_err(|e| engine_error(vm, &e))?;
    if !result.is_object() {
        return Err(crate::error::create_type_error(vm, "iterator result is not an object"));
    }
    let result_obj = unsafe { &*result.as_js_object_ptr() };
    let done_si = vm.perm_intern("done");
    let done = vm.ordinary_get(result_obj, done_si, result).map_err(|e| engine_error(vm, &e))?;
    Ok(oxide_runtime_api::to_boolean(done))
}

/// IteratorClose：延迟 GetMethod(iterator, "return") 并调用，按 completion 形态决定
/// 错误胜者（throw → 原错误胜出；正常 → return 相关错误胜出）。
///
/// # 参数
/// - `completion_err`：`Some(v)` 表示在途异常（completion 为 throw，v 为原异常值）；
///   `None` 表示正常完成。
///
/// # 返回值
/// - 正常完成时 `Ok(())` 表示关闭成功，`Err` 为 return 相关错误；
/// - throw completion 时恒返回 `Err(v)`（原异常胜出，return 错误被吞）。
pub(crate) fn iterator_close_record<H: VmHost>(
    vm: &mut H, iterated: JsValue, completion_err: Option<JsValue>,
) -> Result<(), JsValue> {
    let iter_obj = unsafe { &*iterated.as_js_object_ptr() };
    let return_si = vm.perm_intern("return");
    let return_fn = match vm.ordinary_get(iter_obj, return_si, iterated) {
        Ok(f) if is_callable(f) => f,
        Ok(_) => {
            // 无 return 方法（或不可调用）：GetMethod 返回 undefined，直接收尾。
            return match completion_err {
                Some(v) => Err(v),
                None => Ok(()),
            };
        }
        Err(err) => {
            let exc = engine_error(vm, &err);
            return match completion_err {
                Some(v) => Err(v),
                None => Err(exc),
            };
        }
    };
    match vm.call_function_sync(return_fn, iterated, &[]) {
        Ok(result) => match completion_err {
            // throw completion：原异常胜出，不检查 return 结果形态。
            Some(v) => Err(v),
            // 正常完成：return 结果须为对象，否则 TypeError。
            None => {
                if !result.is_object() {
                    return Err(crate::error::create_type_error(vm, "iterator return() result is not an object"));
                }
                Ok(())
            }
        },
        Err(err) => {
            let exc = engine_error(vm, &err);
            match completion_err {
                Some(v) => Err(v),
                None => Err(exc),
            }
        }
    }
}

/// 计数器转 Number 值：int 区间用 int 表示，越界回落 float（规范 `𝔽(counter)`）。
fn counter_number(counter: i64) -> JsValue {
    if counter >= i32::MIN as i64 && counter <= i32::MAX as i64 {
        JsValue::int(counter as i32)
    } else {
        JsValue::float(counter as f64)
    }
}

// ── 返回迭代器 5 方法（map/filter/take/drop/flatMap）──

/// Iterator helper 的 kind 编码：wrapper `__kind__` 槽存枚举值，
/// `%IteratorHelperPrototype%` 的单一 next/return/throw 读槽后按 kind 分发
/// （复用 Map/Set 迭代器 `__mode__` 分发模式）。判别值稳定，改动须同步 `from_i32`。
#[derive(Clone, Copy, PartialEq)]
#[repr(i32)]
pub(crate) enum IteratorHelperKind {
    Map = 0,
    Filter = 1,
    Take = 2,
    Drop = 3,
    FlatMap = 4,
    Zip = 5,
    Concat = 6,
}

impl IteratorHelperKind {
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Map,
            1 => Self::Filter,
            2 => Self::Take,
            3 => Self::Drop,
            4 => Self::FlatMap,
            5 => Self::Zip,
            6 => Self::Concat,
            _ => Self::Map,
        }
    }
}

/// 读 wrapper 整型槽；槽缺失或类型不符时返回 `fallback`。
fn read_slot_int<H: VmHost>(vm: &mut H, obj: &JsObject, si: u32, fallback: i32) -> i32 {
    match vm.ordinary_get(obj, si, JsValue::undefined()) {
        Ok(v) if v.is_int() => v.as_int(),
        Ok(v) if v.is_double() => v.as_double() as i32,
        _ => fallback,
    }
}

/// 读 wrapper 浮点槽（take/drop 的 remaining）；槽缺失时返回 `fallback`。
fn read_slot_double<H: VmHost>(vm: &mut H, obj: &JsObject, si: u32, fallback: f64) -> f64 {
    match vm.ordinary_get(obj, si, JsValue::undefined()) {
        Ok(v) if v.is_double() => v.as_double(),
        Ok(v) if v.is_int() => v.as_int() as f64,
        _ => fallback,
    }
}

/// 读 wrapper 计数器槽为 i64（map/filter/flatMap 的元素计数，可能溢出 i32）。
fn read_counter<H: VmHost>(vm: &mut H, obj: &JsObject, si: u32) -> i64 {
    match vm.ordinary_get(obj, si, JsValue::undefined()) {
        Ok(v) if v.is_int() => v.as_int() as i64,
        Ok(v) if v.is_double() => v.as_double() as i64,
        _ => 0,
    }
}

/// ToIntegerOrInfinity 近似：输入值经 ToNumber 后向零截断，保留 ±∞；NaN 判定由
/// 调用方在截断前语义等价地做（`trunc(NaN)` 仍为 NaN）。ToNumber 抛错透传原异常值。
fn to_integer_or_infinity<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let num = vm.coerce_number_bounded(value).map_err(|e| engine_error(vm, &e))?;
    Ok(num.trunc())
}

/// take/drop 的共享前置：`this` 非对象 → TypeError；limit 经 ToIntegerOrInfinity
/// 校验（NaN/负值 → RangeError，ToNumber 抛错透传），校验失败均先关闭底层迭代器
/// （此路径不读取 next）再抛错。
///
/// # 返回
/// - `Ok((iterated, next, int_limit))`：GetIteratorDirect 结果 + 截断后的 limit
///   （±∞ 用 f64 哨兵表示）。
fn validate_limit_and_get_direct<H: VmHost>(
    vm: &mut H, this_val: JsValue, limit: JsValue,
) -> Result<(JsValue, JsValue, f64), JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "Iterator.prototype method called on non-object"));
    }
    let int_limit = match to_integer_or_infinity(vm, limit) {
        Ok(n) => n,
        Err(v) => {
            let _ = iterator_close_record(vm, this_val, Some(v));
            return Err(v);
        }
    };
    if int_limit.is_nan() || int_limit < 0.0 {
        let err = crate::error::create_range_error(vm, "Iterator.prototype method requires a non-negative limit");
        let _ = iterator_close_record(vm, this_val, Some(err));
        return Err(err);
    }
    get_iterator_direct(vm, this_val).map(|(iterated, next)| (iterated, next, int_limit))
}

/// 把 flatMap 的 mapper 返回值解析为内层迭代器记录（对象 + 缓存 next）；原始值
/// 一律拒绝（对应规范 GetIteratorFlattenable）。
///
/// 不能复用 [`get_iterator`]：后者对 Array 等内建集合直接返回原值、不走
/// `@@iterator`，而 flattenable 要求数组经 `@@iterator` 产出内层迭代器。
///
/// # 步骤
/// 1. 非对象 → TypeError（原始值一律拒绝，含字符串）。
/// 2. `@@iterator` 可调用 → 调用，结果非对象 → TypeError；getter/call 抛错透传。
/// 3. `@@iterator` 为 null/undefined → 回退原对象自身作迭代器（鸭子 next）。
/// 4. `@@iterator` 为其它不可调用值 → TypeError。
///
/// # 返回
/// - `Ok((iter, next))`：内层迭代器 + GetIteratorDirect 缓存的 next。
fn get_iterator_flattenable<H: VmHost>(vm: &mut H, value: JsValue) -> Result<(JsValue, JsValue), JsValue> {
    if !value.is_object() {
        return Err(crate::error::create_type_error(vm, "iterator mapper result is not an object"));
    }
    let obj = unsafe { &*value.as_js_object_ptr() };
    let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
    let method = match vm.ordinary_get(obj, sym_iter_si, value) {
        Ok(m) => m,
        Err(err) => return Err(engine_error(vm, &err)),
    };
    let inner = if is_callable(method) {
        let it = vm.call_function_sync(method, value, &[]).map_err(|e| engine_error(vm, &e))?;
        if !it.is_object() {
            return Err(crate::error::create_type_error(
                vm,
                "iterator mapper result @@iterator returned a non-object",
            ));
        }
        it
    } else if method.is_undefined() || method.is_null() {
        value
    } else {
        return Err(crate::error::create_type_error(vm, "iterator mapper result @@iterator is not callable"));
    };
    get_iterator_direct(vm, inner)
}

/// 全 GetIterator（无鸭子 next 回退）：`GetMethod(obj, @@iterator)` 不可调用抛
/// TypeError，调用结果非对象抛 TypeError，再 GetIteratorDirect。
///
/// 不能复用 [`get_iterator`] / [`get_iterator_flattenable`]：两者均带鸭子 next
/// 回退，而 `iterables` 实参要求带可调用 `next` 但无 `@@iterator` 的对象必须
/// 抛 TypeError。
///
/// # 边界与前提
/// - `value` 必须是对象（调用方已校验）。
/// - getter 抛错 / 调用抛错透传原异常值。
///
/// # 返回
/// - `Ok((iter, next))`：迭代器 + GetIteratorDirect 缓存的 next。
fn get_iterator_strict<H: VmHost>(vm: &mut H, what: &str, value: JsValue) -> Result<(JsValue, JsValue), JsValue> {
    let obj = unsafe { &*value.as_js_object_ptr() };
    let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
    let method = match vm.ordinary_get(obj, sym_iter_si, value) {
        Ok(m) => m,
        Err(err) => return Err(engine_error(vm, &err)),
    };
    if !is_callable(method) {
        return Err(crate::error::create_type_error(vm, &format!("Iterator.zip {what} is not iterable")));
    }
    let iter = vm.call_function_sync(method, value, &[]).map_err(|e| engine_error(vm, &e))?;
    if !iter.is_object() {
        return Err(crate::error::create_type_error(
            vm,
            &format!("Iterator.zip {what} @@iterator returned a non-object"),
        ));
    }
    get_iterator_direct(vm, iter)
}

/// GetMethod 只读：`@@iterator` getter 抛错透传原值，方法为 undefined/null/
/// 其它不可调用值一律 TypeError（规范 GetMethod 步 4 同落 TypeError），返回
/// 方法闭包本身，不调用（concat 构造期只缓存方法，懒开由各段首次 next 触发）。
///
/// # 边界与前提
/// - `value` 必须是对象（调用方已校验）。
///
/// # 返回
/// - `Ok(method)`：可调用方法闭包；`Err` 为透传异常值或 TypeError。
fn get_iterator_method<H: VmHost>(vm: &mut H, value: JsValue) -> Result<JsValue, JsValue> {
    let obj = unsafe { &*value.as_js_object_ptr() };
    let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
    let method = match vm.ordinary_get(obj, sym_iter_si, value) {
        Ok(m) => m,
        Err(err) => return Err(engine_error(vm, &err)),
    };
    if !is_callable(method) {
        return Err(crate::error::create_type_error(
            vm,
            "Iterable object does not have a callable @@iterator method",
        ));
    }
    Ok(method)
}

/// GetOptionsObject：undefined 建 null 原型空对象，对象原样，其余抛 TypeError。
fn get_options_object<H: VmHost>(vm: &mut H, options: JsValue) -> Result<JsValue, JsValue> {
    if options.is_undefined() {
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        return Ok(JsValue::from_js_object(obj));
    }
    if options.is_object() {
        return Ok(options);
    }
    Err(crate::error::create_type_error(vm, "Iterator.zip options must be undefined or an object"))
}

/// 逐字比较 UTF-16 单元串与 ASCII 文本（mode 严格字符串校验不走 ToPrimitive）。
fn is_ascii_str(units: &[u16], text: &str) -> bool {
    units.len() == text.len() && units.iter().zip(text.bytes()).all(|(u, b)| *u == b as u16)
}

/// 创建 Iterator helper 结果对象：空对象挂 `%IteratorHelperPrototype%`，写
/// `__inner__`/`__next__`/`__kind__`/`__state__`/`__counter__`/`__callback__`
/// 槽；flatMap 的内层槽初始为 undefined。next/return/throw 挂在共享原型上，
/// wrapper 不设 own 方法（与集合迭代器 wrapper 同构，GC 经 shape 属性槽自动遍历）。
///
/// # 参数
/// - `counter`：map/filter/flatMap 传 `0`（int 计数），take/drop 传
///   `float(int_limit)`（f64 remaining，`+∞` 为哨兵）。
fn make_iterator_helper<H: VmHost>(
    vm: &mut H, inner: JsValue, next: JsValue, kind: IteratorHelperKind, callback: JsValue, counter: JsValue,
) -> JsValue {
    let helper_proto = vm.builtin_proto(ProtoKind::IteratorHelperProto);
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(helper_proto)));
    let wrapper_obj = unsafe { &mut *wrapper };
    let inner_si = vm.perm_intern(INNER_PROP);
    let next_si = vm.perm_intern(NEXT_CACHE_PROP);
    let kind_si = vm.perm_intern(KIND_PROP);
    let state_si = vm.perm_intern(STATE_PROP);
    let counter_si = vm.perm_intern(COUNTER_PROP);
    let callback_si = vm.perm_intern(CALLBACK_PROP);
    let inner_iter_si = vm.perm_intern(INNER_ITER_PROP);
    let inner_next_si = vm.perm_intern(INNER_NEXT_PROP);
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next);
    vm.set_or_create_prop_value(wrapper_obj, kind_si, JsValue::int(kind as i32));
    vm.set_or_create_prop_value(wrapper_obj, state_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, counter_si, counter);
    vm.set_or_create_prop_value(wrapper_obj, callback_si, callback);
    vm.set_or_create_prop_value(wrapper_obj, inner_iter_si, JsValue::undefined());
    vm.set_or_create_prop_value(wrapper_obj, inner_next_si, JsValue::undefined());
    JsValue::from_js_object(wrapper)
}

/// 校验 wrapper 对象形态并读出 kind：`this` 非对象 → TypeError；其余 kind 以
/// `__inner__` 槽为对象作判据，Zip 以 `__zips__` 槽为对象作判据（zip wrapper
/// 无 `__inner__` 槽），Concat 以 `__items__` 槽为对象作判据（concat wrapper
/// 无 `__inner__` 槽）。
///
/// # 返回
/// - `Ok((kind, inner))`：kind 与底层值（其余 kind 为 `__inner__` 槽，Zip 为
///   `__zips__` 槽，Concat 为 `__items__` 槽）；`Err` 为透传的异常值。
fn validate_helper_this<H: VmHost>(
    vm: &mut H, this_val: JsValue, method: &str,
) -> Result<(IteratorHelperKind, JsValue), JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, &format!("{method} called on non-object")));
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let kind_si = vm.perm_intern(KIND_PROP);
    let kind = IteratorHelperKind::from_i32(read_slot_int(vm, obj, kind_si, 0));
    if kind == IteratorHelperKind::Zip {
        let zips_si = vm.perm_intern(ZIPS_PROP);
        let zips = vm.ordinary_get(obj, zips_si, this_val).map_err(|e| engine_error(vm, &e))?;
        if !zips.is_object() {
            return Err(crate::error::create_type_error(vm, &format!("{method} called on non-iterator-helper")));
        }
        return Ok((kind, zips));
    }
    if kind == IteratorHelperKind::Concat {
        let items_si = vm.perm_intern(ITEMS_PROP);
        let items = vm.ordinary_get(obj, items_si, this_val).map_err(|e| engine_error(vm, &e))?;
        if !items.is_object() {
            return Err(crate::error::create_type_error(vm, &format!("{method} called on non-iterator-helper")));
        }
        return Ok((kind, items));
    }
    let inner_si = vm.perm_intern(INNER_PROP);
    let inner = vm.ordinary_get(obj, inner_si, this_val).map_err(|e| engine_error(vm, &e))?;
    if !inner.is_object() {
        return Err(crate::error::create_type_error(vm, &format!("{method} called on non-iterator-helper")));
    }
    Ok((kind, inner))
}

// ── 终端 6 方法 ──

/// `%Iterator.prototype%.forEach(procedure)`：消费全部元素，逐个调用 procedure。
///
/// # 步骤
/// 1. 校验 this/回调并取底层迭代器（共享前置）。
/// 2. 循环取元素，每个元素 `Call(procedure, undefined, «value, counter»)`。
/// 3. 回调抛错 → IteratorClose 后透传原值；耗尽返回 undefined。
pub fn iterator_for_each<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let mut counter: i64 = 0;
    loop {
        let value = match iterator_record_step(vm, next, iterated) {
            Ok(Some(v)) => v,
            Ok(None) => return NativeResult::Ok(JsValue::undefined()),
            Err(v) => return NativeResult::Err(v),
        };
        let cb_args = [value, counter_number(counter)];
        if let Err(err) = vm.call_function_sync(callback, JsValue::undefined(), &cb_args) {
            let exc = engine_error(vm, &err);
            let _ = iterator_close_record(vm, iterated, Some(exc));
            return NativeResult::Err(exc);
        }
        counter += 1;
    }
}

/// `%Iterator.prototype%.every(predicate)`：谓词全真返回 true，首个 falsy 关底层返 false。
///
/// # 步骤
/// 1. 校验 this/回调并取底层迭代器（共享前置）。
/// 2. 循环取元素，每个元素 `Call(predicate, undefined, «value, counter»)`。
/// 3. 谓词 falsy → IteratorClose（正常完成，return 错误胜出）后返回 false；耗尽返回 true。
pub fn iterator_every<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let mut counter: i64 = 0;
    loop {
        let value = match iterator_record_step(vm, next, iterated) {
            Ok(Some(v)) => v,
            Ok(None) => return NativeResult::Ok(JsValue::bool(true)),
            Err(v) => return NativeResult::Err(v),
        };
        let cb_args = [value, counter_number(counter)];
        let result = match vm.call_function_sync(callback, JsValue::undefined(), &cb_args) {
            Ok(r) => r,
            Err(err) => {
                let exc = engine_error(vm, &err);
                let _ = iterator_close_record(vm, iterated, Some(exc));
                return NativeResult::Err(exc);
            }
        };
        if !oxide_runtime_api::to_boolean(result) {
            return match iterator_close_record(vm, iterated, None) {
                Ok(()) => NativeResult::Ok(JsValue::bool(false)),
                Err(v) => NativeResult::Err(v),
            };
        }
        counter += 1;
    }
}

/// `%Iterator.prototype%.some(predicate)`：谓词首个 truthy 关底层返回 true，耗尽返 false。
///
/// # 步骤
/// 1. 校验 this/回调并取底层迭代器（共享前置）。
/// 2. 循环取元素，每个元素 `Call(predicate, undefined, «value, counter»)`。
/// 3. 谓词 truthy → IteratorClose（正常完成，return 错误胜出）后返回 true；耗尽返回 false。
pub fn iterator_some<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let mut counter: i64 = 0;
    loop {
        let value = match iterator_record_step(vm, next, iterated) {
            Ok(Some(v)) => v,
            Ok(None) => return NativeResult::Ok(JsValue::bool(false)),
            Err(v) => return NativeResult::Err(v),
        };
        let cb_args = [value, counter_number(counter)];
        let result = match vm.call_function_sync(callback, JsValue::undefined(), &cb_args) {
            Ok(r) => r,
            Err(err) => {
                let exc = engine_error(vm, &err);
                let _ = iterator_close_record(vm, iterated, Some(exc));
                return NativeResult::Err(exc);
            }
        };
        if oxide_runtime_api::to_boolean(result) {
            return match iterator_close_record(vm, iterated, None) {
                Ok(()) => NativeResult::Ok(JsValue::bool(true)),
                Err(v) => NativeResult::Err(v),
            };
        }
        counter += 1;
    }
}

/// `%Iterator.prototype%.find(predicate)`：谓词首个 truthy 关底层返回对应 value，耗尽
/// 返回 undefined。
///
/// # 步骤
/// 1. 校验 this/回调并取底层迭代器（共享前置）。
/// 2. 循环取元素，每个元素 `Call(predicate, undefined, «value, counter»)`。
/// 3. 谓词 truthy → IteratorClose（正常完成，return 错误胜出）后返回 value；耗尽 undefined。
pub fn iterator_find<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let mut counter: i64 = 0;
    loop {
        let value = match iterator_record_step(vm, next, iterated) {
            Ok(Some(v)) => v,
            Ok(None) => return NativeResult::Ok(JsValue::undefined()),
            Err(v) => return NativeResult::Err(v),
        };
        let cb_args = [value, counter_number(counter)];
        let result = match vm.call_function_sync(callback, JsValue::undefined(), &cb_args) {
            Ok(r) => r,
            Err(err) => {
                let exc = engine_error(vm, &err);
                let _ = iterator_close_record(vm, iterated, Some(exc));
                return NativeResult::Err(exc);
            }
        };
        if oxide_runtime_api::to_boolean(result) {
            return match iterator_close_record(vm, iterated, None) {
                Ok(()) => NativeResult::Ok(value),
                Err(v) => NativeResult::Err(v),
            };
        }
        counter += 1;
    }
}

/// `%Iterator.prototype%.reduce(reducer, initialValue?)`：归约全部元素。
///
/// # 步骤
/// 1. 校验 this/回调并取底层迭代器（共享前置）。
/// 2. 无 initialValue：首元素作 accumulator（首步即 done → TypeError，不关底层）；
///    有 initialValue：直接作 accumulator。
/// 3. 循环取元素，`Call(reducer, undefined, «accumulator, value, counter»)` 结果续作
///    accumulator；回调抛错 → IteratorClose 后透传原值。
/// 4. 耗尽返回 accumulator。
pub fn iterator_reduce<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let has_initial = args.len() > 2;
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    // 无初始值：首元素直接作 accumulator（首步 done 抛 TypeError，规范不关底层）。
    let (mut accumulator, mut counter): (JsValue, i64) = if has_initial {
        (vm.reg(args[2]), 0)
    } else {
        match iterator_record_step(vm, next, iterated) {
            Ok(Some(first)) => (first, 1),
            Ok(None) => {
                return NativeResult::Err(crate::error::create_type_error(
                    vm,
                    "Reduce of empty iterator with no initial value",
                ))
            }
            Err(v) => return NativeResult::Err(v),
        }
    };
    loop {
        let value = match iterator_record_step(vm, next, iterated) {
            Ok(Some(v)) => v,
            Ok(None) => return NativeResult::Ok(accumulator),
            Err(v) => return NativeResult::Err(v),
        };
        let cb_args = [accumulator, value, counter_number(counter)];
        match vm.call_function_sync(callback, JsValue::undefined(), &cb_args) {
            Ok(result) => accumulator = result,
            Err(err) => {
                let exc = engine_error(vm, &err);
                let _ = iterator_close_record(vm, iterated, Some(exc));
                return NativeResult::Err(exc);
            }
        }
        counter += 1;
    }
}

/// `%Iterator.prototype%.toArray()`：消费全部元素，返回普通数组。
///
/// # 步骤
/// 1. `this` 非对象 → TypeError；GetIteratorDirect 取底层迭代器。
/// 2. 循环收集元素到列表，耗尽后构造数组返回。
pub fn iterator_to_array<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Iterator.prototype method called on non-object",
        ));
    }
    let (iterated, next) = match get_iterator_direct(vm, this_val) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let mut items: Vec<JsValue> = Vec::new();
    loop {
        match iterator_record_step(vm, next, iterated) {
            Ok(Some(value)) => items.push(value),
            Ok(None) => return NativeResult::Ok(make_array_from_list(vm, &items)),
            Err(v) => return NativeResult::Err(v),
        }
    }
}

/// 按元素列表构造普通数组，以 Array.prototype 为原型（对应规范 CreateArrayFromList）。
fn make_array_from_list<H: VmHost>(vm: &mut H, items: &[JsValue]) -> JsValue {
    let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        items.len().min(oxide_types::object::MAX_DENSE_PROPS),
    ));
    // 逐元素写入数组元素区，最后统一 set_prop_count（new_array 预分配不足时自动扩容）。
    for (i, item) in items.iter().enumerate() {
        // SAFETY: arr 是刚经统一入口分配的数组对象，元素区已就绪。
        unsafe {
            (*arr).set_prop_at(i, *item);
        }
    }
    // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
    unsafe {
        (*arr).set_prop_count(items.len());
    }
    JsValue::from_js_object(arr)
}

// ── 返回迭代器 5 方法 + %IteratorHelperPrototype% 状态机 ──

/// `%Iterator.prototype%.map(mapper)`：逐元素 `mapper(value, counter)`，产出
/// helper wrapper。
///
/// # 步骤
/// 1. 共享前置：`this` 对象校验 + 回调可调用校验（失败关底层）+ GetIteratorDirect。
/// 2. 建 Map wrapper（`__kind__`=0，回调存 `__callback__` 槽，计数从 0 起）。
pub fn iterator_map<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    NativeResult::Ok(make_iterator_helper(
        vm,
        iterated,
        next,
        IteratorHelperKind::Map,
        callback,
        JsValue::int(0),
    ))
}

/// `%Iterator.prototype%.filter(predicate)`：谓词 truthy 的元素才产出，
/// 计数每元素 +1（含被过滤元素）。
///
/// # 步骤
/// 1. 共享前置：`this` 对象校验 + 谓词可调用校验（失败关底层）+ GetIteratorDirect。
/// 2. 建 Filter wrapper（`__kind__`=1）。
pub fn iterator_filter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    NativeResult::Ok(make_iterator_helper(
        vm,
        iterated,
        next,
        IteratorHelperKind::Filter,
        callback,
        JsValue::int(0),
    ))
}

/// `%Iterator.prototype%.take(limit)`：最多产出 limit 个元素，达标即关底层。
///
/// # 步骤
/// 1. limit 经 ToIntegerOrInfinity 校验（NaN/负值 → RangeError 且关底层）。
/// 2. 建 Take wrapper（`__counter__` 槽存 f64 remaining，+∞ 哨兵）。
/// 3. next：remaining 为 0 → 关底层返回 done；否则递减后逐元素产出。
pub fn iterator_take<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let limit = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next, int_limit) = match validate_limit_and_get_direct(vm, this_val, limit) {
        Ok(triple) => triple,
        Err(v) => return NativeResult::Err(v),
    };
    NativeResult::Ok(make_iterator_helper(
        vm,
        iterated,
        next,
        IteratorHelperKind::Take,
        JsValue::undefined(),
        JsValue::float(int_limit),
    ))
}

/// `%Iterator.prototype%.drop(limit)`：跳过 limit 个元素后透传，永不主动关底层。
///
/// # 步骤
/// 1. limit 经 ToIntegerOrInfinity 校验（NaN/负值 → RangeError 且关底层）。
/// 2. 建 Drop wrapper（`__counter__` 槽存 f64 remaining，+∞ 哨兵）。
/// 3. next：remaining>0 时循环跳过（耗尽不关底层），之后直通底层。
pub fn iterator_drop<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let limit = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next, int_limit) = match validate_limit_and_get_direct(vm, this_val, limit) {
        Ok(triple) => triple,
        Err(v) => return NativeResult::Err(v),
    };
    NativeResult::Ok(make_iterator_helper(
        vm,
        iterated,
        next,
        IteratorHelperKind::Drop,
        JsValue::undefined(),
        JsValue::float(int_limit),
    ))
}

/// `%Iterator.prototype%.flatMap(mapper)`：mapper 返回值经 GetIteratorFlattenable
/// 展开为内层迭代器，逐元素产出后再取外层下一元素。
///
/// # 步骤
/// 1. 共享前置：`this` 对象校验 + 回调可调用校验（失败关底层）+ GetIteratorDirect。
/// 2. 建 FlatMap wrapper（`__kind__`=4，内层槽初始为 undefined）。
/// 3. next：内层活跃 → 步内层产出；内层耗尽 → 清槽后步外层 → mapper →
///    GetIteratorFlattenable 建新内层。
pub fn iterator_flat_map<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let callback = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (iterated, next) = match validate_terminal_and_get_direct(vm, this_val, callback) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    NativeResult::Ok(make_iterator_helper(
        vm,
        iterated,
        next,
        IteratorHelperKind::FlatMap,
        callback,
        JsValue::int(0),
    ))
}

/// `%IteratorHelperPrototype%.next`：读 wrapper `__kind__` 分发 6 种推进循环，
/// 统一完成态短路与重入守卫。
///
/// # 步骤
/// 1. `__state__`=2（完成）→ 直接返回 `{undefined, true}`（return 不再转发）。
/// 2. `__state__`=1（重入）→ TypeError（等价 GeneratorValidate 的 executing 检查）。
/// 3. 置 1 后按 kind 推进：产出值 → 置回 0 返回 `{value, false}`；耗尽/错误 → 置 2
///    后返回 done / 透传异常（规范生成器 body 终止即 completed，后续调用短路）。
pub fn iterator_helper_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let (kind, inner) = match validate_helper_this(vm, this_val, "Iterator helper next") {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let helper = unsafe { &mut *this_val.as_js_object_ptr() };
    let state_si = vm.perm_intern(STATE_PROP);
    let state = read_slot_int(vm, helper, state_si, 2);
    if state == 2 {
        return NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true));
    }
    if state == 1 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator helper is already executing"));
    }
    vm.set_or_create_prop_value(helper, state_si, JsValue::int(1));
    let next_si = vm.perm_intern(NEXT_CACHE_PROP);
    let next = vm.ordinary_get(helper, next_si, this_val).unwrap_or(JsValue::undefined());
    let step = match kind {
        IteratorHelperKind::Map => helper_step_map(vm, this_val, helper, inner, next),
        IteratorHelperKind::Filter => helper_step_filter(vm, this_val, helper, inner, next),
        IteratorHelperKind::Take => helper_step_take(vm, helper, inner, next),
        IteratorHelperKind::Drop => helper_step_drop(vm, helper, inner, next),
        IteratorHelperKind::FlatMap => helper_step_flat_map(vm, this_val, helper, inner, next),
        IteratorHelperKind::Zip => helper_step_zip(vm, this_val, helper),
        IteratorHelperKind::Concat => helper_step_concat(vm, this_val, helper),
    };
    match step {
        Ok(Some(value)) => {
            // zip 与 concat 产出后为 suspended-yield（state 3，return/throw 关闭期
            // 重入守卫需区分 suspended-start 的完成态短路）；其余 kind 恒 0。
            let new_state = if matches!(kind, IteratorHelperKind::Zip | IteratorHelperKind::Concat) {
                3
            } else {
                0
            };
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(new_state));
            NativeResult::Ok(make_iter_result(vm, value, false))
        }
        Ok(None) => {
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
            NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true))
        }
        Err(exc) => {
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
            NativeResult::Err(exc)
        }
    }
}

/// `%IteratorHelperPrototype%.return`：关闭底层迭代器后置完成，返回
/// `{undefined, true}`。
///
/// # 步骤
/// 1. 完成态 → 直接返回 done（return 不重复转发）。
/// 2. 重入（执行中）→ TypeError。
/// 3. 置完成；flatMap 先关内层（return 语义：内层关闭错误优先于外层）再关外层；
///    关闭抛错 → 传播，此后 next/return/throw 全部短路。
pub fn iterator_helper_return<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let (kind, inner) = match validate_helper_this(vm, this_val, "Iterator helper return") {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let helper = unsafe { &mut *this_val.as_js_object_ptr() };
    let state_si = vm.perm_intern(STATE_PROP);
    let state = read_slot_int(vm, helper, state_si, 2);
    if state == 2 {
        return NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true));
    }
    if state == 1 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator helper is already executing"));
    }
    vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
    if kind == IteratorHelperKind::Zip {
        // suspended-start 关全部前已置完成（嵌套调用短路）；suspended-yield 关
        // 全部前置执行中（嵌套调用 TypeError），关闭后回落完成。
        let close_state = if state == 0 { 2 } else { 1 };
        vm.set_or_create_prop_value(helper, state_si, JsValue::int(close_state));
        let zips_si = vm.perm_intern(ZIPS_PROP);
        let zips_val = vm.ordinary_get(helper, zips_si, this_val).unwrap_or(JsValue::undefined());
        let iter_count = if zips_val.is_object() {
            unsafe { &*zips_val.as_js_object_ptr() }.prop_count()
        } else {
            0
        };
        let open_si = vm.perm_intern(OPEN_PROP);
        let open = read_slot_int(vm, helper, open_si, -1);
        let result = match close_remaining_open(vm, this_val, helper, open, iter_count, None) {
            Ok(()) => NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true)),
            Err(v) => NativeResult::Err(v),
        };
        vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
        return result;
    }
    if kind == IteratorHelperKind::Concat {
        // 以 `__current__` 槽在场判定内层是否已开：已开则置执行中（重入守卫）
        // 后关当前内层（正常完成，关闭错误传播）返 done；未开段（首次 next
        // 前）与自然耗尽后均空操作，不转发 return。
        let current_si = vm.perm_intern(CURRENT_PROP);
        let current = vm.ordinary_get(helper, current_si, this_val).unwrap_or(JsValue::undefined());
        if current.is_object() {
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(1));
            let result = match iterator_close_record(vm, current, None) {
                Ok(()) => NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true)),
                Err(v) => NativeResult::Err(v),
            };
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
            return result;
        }
        vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
        return NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true));
    }
    let kind_si = vm.perm_intern(KIND_PROP);
    if IteratorHelperKind::from_i32(read_slot_int(vm, helper, kind_si, 0)) == IteratorHelperKind::FlatMap {
        let inner_iter_si = vm.perm_intern(INNER_ITER_PROP);
        if let Ok(inner_iter) = vm.ordinary_get(helper, inner_iter_si, this_val) {
            if inner_iter.is_object() {
                // 内层关闭出错：以该错误关外层（外层 return 错误被吞），传播内层错误。
                if let Err(inner_err) = iterator_close_record(vm, inner_iter, None) {
                    let _ = iterator_close_record(vm, inner, Some(inner_err));
                    return NativeResult::Err(inner_err);
                }
            }
        }
    }
    match iterator_close_record(vm, inner, None) {
        Ok(()) => NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true)),
        Err(v) => NativeResult::Err(v),
    }
}

/// `%IteratorHelperPrototype%.throw(value)`：把 value 作为异常注入 helper。
///
/// # 步骤
/// 1. 完成态 → 直接抛 value（GeneratorResumeAbrupt 的 completed 分支）。
/// 2. 重入（执行中）→ TypeError。
/// 3. 置完成；flatMap 先内层后外层关底层（原值恒胜出），再抛 value。
pub fn iterator_helper_throw<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (kind, inner) = match validate_helper_this(vm, this_val, "Iterator helper throw") {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let helper = unsafe { &mut *this_val.as_js_object_ptr() };
    let state_si = vm.perm_intern(STATE_PROP);
    let state = read_slot_int(vm, helper, state_si, 2);
    if state == 2 {
        return NativeResult::Err(value);
    }
    if state == 1 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator helper is already executing"));
    }
    vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
    if kind == IteratorHelperKind::Zip {
        // 同 return 臂：suspended-start 置完成后关全部，suspended-yield 置执行中
        // 后关全部，原值恒胜出。
        let close_state = if state == 0 { 2 } else { 1 };
        vm.set_or_create_prop_value(helper, state_si, JsValue::int(close_state));
        let zips_si = vm.perm_intern(ZIPS_PROP);
        let zips_val = vm.ordinary_get(helper, zips_si, this_val).unwrap_or(JsValue::undefined());
        let iter_count = if zips_val.is_object() {
            unsafe { &*zips_val.as_js_object_ptr() }.prop_count()
        } else {
            0
        };
        let open_si = vm.perm_intern(OPEN_PROP);
        let open = read_slot_int(vm, helper, open_si, -1);
        let _ = close_remaining_open(vm, this_val, helper, open, iter_count, Some(value));
        vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
        return NativeResult::Err(value);
    }
    if kind == IteratorHelperKind::Concat {
        // 与 return 臂同判据：`__current__` 在场则置执行中（重入守卫）后关当前
        // 内层（throw 完成，原值恒胜出，关闭错误被吞）再抛 value；未开段直接
        // 抛 value。
        let current_si = vm.perm_intern(CURRENT_PROP);
        let current = vm.ordinary_get(helper, current_si, this_val).unwrap_or(JsValue::undefined());
        if current.is_object() {
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(1));
            let _ = iterator_close_record(vm, current, Some(value));
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
            return NativeResult::Err(value);
        }
        vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
        return NativeResult::Err(value);
    }
    let kind_si = vm.perm_intern(KIND_PROP);
    if IteratorHelperKind::from_i32(read_slot_int(vm, helper, kind_si, 0)) == IteratorHelperKind::FlatMap {
        let inner_iter_si = vm.perm_intern(INNER_ITER_PROP);
        if let Ok(inner_iter) = vm.ordinary_get(helper, inner_iter_si, this_val) {
            if inner_iter.is_object() {
                // throw 语义：原值胜出，内层/外层关闭错误均被吞。
                let _ = iterator_close_record(vm, inner_iter, Some(value));
            }
        }
    }
    let _ = iterator_close_record(vm, inner, Some(value));
    NativeResult::Err(value)
}

/// Map 推进：底层 step → `mapper(value, counter)` → 产出 mapped。mapper 抛错
/// 关底层后透传原值；底层 step 错误不关（规范 IteratorStepValue 字面）。
fn helper_step_map<H: VmHost>(
    vm: &mut H, this_val: JsValue, helper: &mut JsObject, inner: JsValue, next: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let counter_si = vm.perm_intern(COUNTER_PROP);
    let callback_si = vm.perm_intern(CALLBACK_PROP);
    let mapper = vm.ordinary_get(helper, callback_si, this_val).unwrap_or(JsValue::undefined());
    let counter = read_counter(vm, helper, counter_si);
    let value = match iterator_record_step(vm, next, inner) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        Err(v) => return Err(v),
    };
    let cb_args = [value, counter_number(counter)];
    match vm.call_function_sync(mapper, JsValue::undefined(), &cb_args) {
        Ok(mapped) => {
            vm.set_or_create_prop_value(helper, counter_si, counter_number(counter + 1));
            Ok(Some(mapped))
        }
        Err(err) => {
            let exc = engine_error(vm, &err);
            let _ = iterator_close_record(vm, inner, Some(exc));
            Err(exc)
        }
    }
}

/// Filter 推进：底层 step → `predicate(value, counter)`；truthy 产出 value，
/// falsy 继续。counter 每元素 +1（含被过滤元素）；谓词抛错关底层后透传原值。
fn helper_step_filter<H: VmHost>(
    vm: &mut H, this_val: JsValue, helper: &mut JsObject, inner: JsValue, next: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let counter_si = vm.perm_intern(COUNTER_PROP);
    let callback_si = vm.perm_intern(CALLBACK_PROP);
    let predicate = vm.ordinary_get(helper, callback_si, this_val).unwrap_or(JsValue::undefined());
    let mut counter = read_counter(vm, helper, counter_si);
    loop {
        let value = match iterator_record_step(vm, next, inner) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(None),
            Err(v) => return Err(v),
        };
        let cb_args = [value, counter_number(counter)];
        match vm.call_function_sync(predicate, JsValue::undefined(), &cb_args) {
            Ok(selected) => {
                counter += 1;
                vm.set_or_create_prop_value(helper, counter_si, counter_number(counter));
                if oxide_runtime_api::to_boolean(selected) {
                    return Ok(Some(value));
                }
            }
            Err(err) => {
                let exc = engine_error(vm, &err);
                let _ = iterator_close_record(vm, inner, Some(exc));
                return Err(exc);
            }
        }
    }
}

/// Take 推进：`__counter__` 槽存 f64 remaining；为 0 时关底层返回 done，
/// 否则递减后逐元素产出。自然耗尽或 limit 达标后的完成态短路使 return 不再转发。
fn helper_step_take<H: VmHost>(
    vm: &mut H, helper: &mut JsObject, inner: JsValue, next: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let counter_si = vm.perm_intern(COUNTER_PROP);
    let remaining = read_slot_double(vm, helper, counter_si, f64::INFINITY);
    if remaining == 0.0 {
        // remaining 归零：正常完成形态关底层（return 错误胜出）。
        return match iterator_close_record(vm, inner, None) {
            Ok(()) => Ok(None),
            Err(v) => Err(v),
        };
    }
    let remaining = if remaining == f64::INFINITY { f64::INFINITY } else { remaining - 1.0 };
    let value = match iterator_record_step(vm, next, inner) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        Err(v) => return Err(v),
    };
    vm.set_or_create_prop_value(helper, counter_si, JsValue::float(remaining));
    Ok(Some(value))
}

/// Drop 推进：先跳过 remaining 个元素（耗尽不关底层），随后直通底层产出。
fn helper_step_drop<H: VmHost>(
    vm: &mut H, helper: &mut JsObject, inner: JsValue, next: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let counter_si = vm.perm_intern(COUNTER_PROP);
    let mut remaining = read_slot_double(vm, helper, counter_si, 0.0);
    while remaining > 0.0 {
        if remaining != f64::INFINITY {
            remaining -= 1.0;
        }
        match iterator_record_step(vm, next, inner) {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(None),
            Err(v) => return Err(v),
        }
    }
    // 跳过完成：remaining 归 0，之后每次 next 直通底层。
    vm.set_or_create_prop_value(helper, counter_si, JsValue::float(0.0));
    let value = match iterator_record_step(vm, next, inner) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        Err(v) => return Err(v),
    };
    Ok(Some(value))
}

/// FlatMap 推进：内层活跃则步内层产出；内层耗尽则清槽回外层取下一映射。
/// 内层步错只关外层（规范 IfAbruptCloseIterator(innerValue, iterated)），
/// 外层步错不关；mapper/flattenable 抛错关外层后透传原值。
fn helper_step_flat_map<H: VmHost>(
    vm: &mut H, this_val: JsValue, helper: &mut JsObject, outer: JsValue, outer_next: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let counter_si = vm.perm_intern(COUNTER_PROP);
    let callback_si = vm.perm_intern(CALLBACK_PROP);
    let inner_iter_si = vm.perm_intern(INNER_ITER_PROP);
    let inner_next_si = vm.perm_intern(INNER_NEXT_PROP);
    let mapper = vm.ordinary_get(helper, callback_si, this_val).unwrap_or(JsValue::undefined());
    let mut counter = read_counter(vm, helper, counter_si);
    loop {
        let inner_iter = vm.ordinary_get(helper, inner_iter_si, this_val).unwrap_or(JsValue::undefined());
        if inner_iter.is_object() {
            let inner_next = vm.ordinary_get(helper, inner_next_si, this_val).unwrap_or(JsValue::undefined());
            match iterator_record_step(vm, inner_next, inner_iter) {
                Ok(Some(v)) => return Ok(Some(v)),
                Ok(None) => {
                    // 内层自然耗尽：不关内层（规范只置 innerAlive=false），清槽回外层。
                    vm.set_or_create_prop_value(helper, inner_iter_si, JsValue::undefined());
                    vm.set_or_create_prop_value(helper, inner_next_si, JsValue::undefined());
                    continue;
                }
                Err(v) => {
                    let _ = iterator_close_record(vm, outer, Some(v));
                    return Err(v);
                }
            }
        }
        let value = match iterator_record_step(vm, outer_next, outer) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(None),
            Err(v) => return Err(v),
        };
        let cb_args = [value, counter_number(counter)];
        let mapped = match vm.call_function_sync(mapper, JsValue::undefined(), &cb_args) {
            Ok(m) => m,
            Err(err) => {
                let exc = engine_error(vm, &err);
                let _ = iterator_close_record(vm, outer, Some(exc));
                return Err(exc);
            }
        };
        let (inner_it, inner_next_fn) = match get_iterator_flattenable(vm, mapped) {
            Ok(pair) => pair,
            Err(v) => {
                let _ = iterator_close_record(vm, outer, Some(v));
                return Err(v);
            }
        };
        counter += 1;
        vm.set_or_create_prop_value(helper, counter_si, counter_number(counter));
        vm.set_or_create_prop_value(helper, inner_iter_si, inner_it);
        vm.set_or_create_prop_value(helper, inner_next_si, inner_next_fn);
    }
}

// ── Iterator.zip（构造期 + shortest 模式迭代期）──

/// `Iterator.zip(iterables, options)`：构造期按规范顺序执行——先校验
/// `iterables` 对象、读 options（mode 严格字符串校验，longest 时读 padding），
/// 再全 GetIterator 取 `inputIter`，元素收集循环（IteratorStepValue 骤发逆序关
/// 已收集元素迭代器；元素经 GetIteratorFlattenable 骤发逆序关含 inputIter），
/// longest 且 padding 为对象时预计算 padding 数组（骤发逆序关元素迭代器、
/// padding 迭代器不关；未耗尽收口正常关），最后建 zip wrapper。
///
/// # 步骤
/// 1. `iterables` 非对象 → TypeError（options 不读）。
/// 2. GetOptionsObject；`mode` 读后严格字符串比较（undefined 落 shortest，
///    String 包装对象拒绝、不触发 toPrimitive，无效 mode 不读 padding）。
/// 3. longest 模式读 `padding`（undefined 或对象，其余 TypeError）。
/// 4. GetIterator（无鸭子回退）取 inputIter；元素收集循环骤发逆序关后透传。
/// 5. longest 且 padding 为对象：GetIterator(padding) 加升序步进至多元素数次
///    预计算 padding 数组（耗尽余位填 undefined，未耗尽正常关）。
/// 6. 建 zip wrapper（`__kind__`=5）。
pub fn iterator_zip<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let iterables = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if !iterables.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator.zip iterables must be an object"));
    }
    let options_obj = match get_options_object(vm, options) {
        Ok(o) => o,
        Err(v) => return NativeResult::Err(v),
    };
    let options_ref = unsafe { &*options_obj.as_js_object_ptr() };
    let mode_si = vm.perm_intern("mode");
    let mode_val = match vm.ordinary_get(options_ref, mode_si, options_obj) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(engine_error(vm, &err)),
    };
    let mode = if mode_val.is_undefined() {
        0
    } else if mode_val.is_string() {
        let units = vm.string_units(mode_val);
        if is_ascii_str(&units, "shortest") {
            0
        } else if is_ascii_str(&units, "longest") {
            1
        } else if is_ascii_str(&units, "strict") {
            2
        } else {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "Iterator.zip mode must be \"shortest\", \"longest\", or \"strict\"",
            ));
        }
    } else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Iterator.zip mode must be \"shortest\", \"longest\", or \"strict\"",
        ));
    };
    let padding = if mode == 1 {
        let padding_si = vm.perm_intern("padding");
        let p = match vm.ordinary_get(options_ref, padding_si, options_obj) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(engine_error(vm, &err)),
        };
        if !p.is_undefined() && !p.is_object() {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "Iterator.zip padding must be undefined or an object",
            ));
        }
        p
    } else {
        JsValue::undefined()
    };
    let (input_iter, input_next) = match get_iterator_strict(vm, "iterables", iterables) {
        Ok(pair) => pair,
        Err(v) => return NativeResult::Err(v),
    };
    let mut zips: Vec<(JsValue, JsValue)> = Vec::new();
    loop {
        match iterator_record_step(vm, input_next, input_iter) {
            Ok(None) => break,
            Ok(Some(value)) => match get_iterator_flattenable(vm, value) {
                Ok(pair) => zips.push(pair),
                Err(v) => {
                    // 元素骤发：逆序关 inputIter + 已收集（inputIter 在列表首位，最后关）。
                    close_zip_pairs(vm, &zips, Some(input_iter), Some(v));
                    return NativeResult::Err(v);
                }
            },
            Err(v) => {
                // 步进骤发：只关已收集，inputIter 不关。
                close_zip_pairs(vm, &zips, None, Some(v));
                return NativeResult::Err(v);
            }
        }
    }
    // longest 且 padding 为对象：预计算 padding 数组（长度恒为元素数，耗尽后余位
    // 填 undefined）。骤发（GetIterator 与 next/done/value getter）逆序关已收集
    // 元素迭代器而 padding 迭代器不关，透传原值；全部步进后未耗尽则正常关，
    // 关闭骤发同样逆序关元素迭代器透传。
    let padding_value = if mode == 1 && padding.is_object() {
        let (padding_iter, padding_next) = match get_iterator_strict(vm, "padding", padding) {
            Ok(pair) => pair,
            Err(v) => {
                close_zip_pairs(vm, &zips, None, Some(v));
                return NativeResult::Err(v);
            }
        };
        let mut pad_values: Vec<JsValue> = Vec::new();
        let mut using_iterator = true;
        for _ in 0..zips.len() {
            if using_iterator {
                match iterator_record_step(vm, padding_next, padding_iter) {
                    Ok(Some(value)) => pad_values.push(value),
                    Ok(None) => {
                        using_iterator = false;
                        pad_values.push(JsValue::undefined());
                    }
                    Err(v) => {
                        close_zip_pairs(vm, &zips, None, Some(v));
                        return NativeResult::Err(v);
                    }
                }
            } else {
                pad_values.push(JsValue::undefined());
            }
        }
        if using_iterator {
            if let Err(v) = iterator_close_record(vm, padding_iter, None) {
                close_zip_pairs(vm, &zips, None, Some(v));
                return NativeResult::Err(v);
            }
        }
        make_array_from_list(vm, &pad_values)
    } else {
        JsValue::undefined()
    };
    NativeResult::Ok(make_iterator_zip_helper(vm, &zips, mode, padding_value, &[]))
}

/// `Iterator.zipKeyed(iterables, options)`：`iterables` 为对象（属性键=结果键），
/// 各可枚举属性值须为可展平迭代器；返回迭代器逐键产出 null 原型对象。
///
/// # 步骤
/// 1. 校验 `iterables` 为对象（非对象 TypeError）；读 options/mode/padding
///    （与 zip 同口径，mode 非法 TypeError、longest 时 padding 非对象 TypeError）。
/// 2. 遍历 `iterables.[[OwnPropertyKeys]]`：可枚举且值非 undefined 的属性才
///    收键（`keys`）并 GetIteratorFlattenable 收迭代器（`iters`）；骤发逆序关
///    已收迭代器透传原值。
/// 3. longest 且 padding 为对象：逐键 `Get(padding, key)` 预计算 padding 数组
///    （骤发逆序关已收迭代器透传）；padding 为 undefined 时全位 undefined。
/// 4. 建 zip wrapper（带 `__keys__` 槽），步进期逐键构造 null 原型结果对象。
///
/// # 边界
/// - 静态方法，无 this 检查。
/// - 属性值为 undefined 时跳过（不收键、不收迭代器）。
pub fn iterator_zip_keyed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let iterables = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if !iterables.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator.zipKeyed iterables must be an object"));
    }
    let options_obj = match get_options_object(vm, options) {
        Ok(o) => o,
        Err(v) => return NativeResult::Err(v),
    };
    let options_ref = unsafe { &*options_obj.as_js_object_ptr() };
    let mode_si = vm.perm_intern("mode");
    let mode_val = match vm.ordinary_get(options_ref, mode_si, options_obj) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(engine_error(vm, &err)),
    };
    let mode = if mode_val.is_undefined() {
        0
    } else if mode_val.is_string() {
        let units = vm.string_units(mode_val);
        if is_ascii_str(&units, "shortest") {
            0
        } else if is_ascii_str(&units, "longest") {
            1
        } else if is_ascii_str(&units, "strict") {
            2
        } else {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "Iterator.zipKeyed mode must be \"shortest\", \"longest\", or \"strict\"",
            ));
        }
    } else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Iterator.zipKeyed mode must be \"shortest\", \"longest\", or \"strict\"",
        ));
    };
    let padding = if mode == 1 {
        let padding_si = vm.perm_intern("padding");
        let p = match vm.ordinary_get(options_ref, padding_si, options_obj) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(engine_error(vm, &err)),
        };
        if !p.is_undefined() && !p.is_object() {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "Iterator.zipKeyed padding must be undefined or an object",
            ));
        }
        p
    } else {
        JsValue::undefined()
    };
    let iterables_obj = unsafe { &*iterables.as_js_object_ptr() };
    let all_keys = match vm.own_keys(iterables_obj) {
        Ok(k) => k,
        Err(e) => return NativeResult::Err(engine_error(vm, &e)),
    };
    let mut keys: Vec<JsValue> = Vec::new();
    let mut iters: Vec<(JsValue, JsValue)> = Vec::new();
    let enumerable_si = vm.perm_intern("enumerable");
    for key in all_keys {
        let key_si = vm.property_key_si(key);
        let desc = match vm.get_own_property_desc(iterables_obj, key_si) {
            Ok(d) => d,
            Err(e) => {
                let exc = engine_error(vm, &e);
                close_zip_pairs(vm, &iters, None, Some(exc));
                return NativeResult::Err(exc);
            }
        };
        let Some(desc_val) = desc else {
            continue;
        };
        if !desc_val.is_object() {
            continue;
        }
        let desc_ref = unsafe { &*desc_val.as_js_object_ptr() };
        let enumerable = match vm.ordinary_get(desc_ref, enumerable_si, desc_val) {
            Ok(v) => v,
            Err(e) => {
                let exc = engine_error(vm, &e);
                close_zip_pairs(vm, &iters, None, Some(exc));
                return NativeResult::Err(exc);
            }
        };
        if !oxide_runtime_api::to_boolean(enumerable) {
            continue;
        }
        let value = match vm.ordinary_get(iterables_obj, key_si, iterables) {
            Ok(v) => v,
            Err(e) => {
                let exc = engine_error(vm, &e);
                close_zip_pairs(vm, &iters, None, Some(exc));
                return NativeResult::Err(exc);
            }
        };
        if value.is_undefined() {
            continue;
        }
        keys.push(key);
        match get_iterator_flattenable(vm, value) {
            Ok(pair) => iters.push(pair),
            Err(v) => {
                close_zip_pairs(vm, &iters, None, Some(v));
                return NativeResult::Err(v);
            }
        }
    }
    // longest 且 padding 为对象：逐键 Get(padding, key) 预计算 padding 数组（长度
    // 恒为元素数）；骤发逆序关已收迭代器透传原值。padding 为 undefined 时全位
    // undefined（longest 步进期越界落点同样 undefined）。
    let padding_value = if mode == 1 && padding.is_object() {
        let padding_ref = unsafe { &*padding.as_js_object_ptr() };
        let mut pad_values: Vec<JsValue> = Vec::with_capacity(iters.len());
        for key in &keys {
            let key_si = vm.property_key_si(*key);
            let v = match vm.ordinary_get(padding_ref, key_si, padding) {
                Ok(v) => v,
                Err(e) => {
                    let exc = engine_error(vm, &e);
                    close_zip_pairs(vm, &iters, None, Some(exc));
                    return NativeResult::Err(exc);
                }
            };
            pad_values.push(v);
        }
        make_array_from_list(vm, &pad_values)
    } else {
        JsValue::undefined()
    };
    NativeResult::Ok(make_iterator_zip_helper(vm, &iters, mode, padding_value, &keys))
}

/// 建 zip wrapper：空对象挂 `%IteratorHelperPrototype%`，写 `__zips__`（元素
/// 迭代器对数组，每元素为 [iter, next] 对）、`__open__`（位掩码，初始全 1）、
/// `__mode__`（0/1/2）、`__state__`（0）、`__kind__`（5）、`__padding__` 槽。
///
/// `__padding__` 存预计算 padding 数组（longest 且 padding 为对象时，长度恒为
/// 元素数）或 undefined；步进期按 done 位下标取落点值。
///
/// `keys` 非空时（zipKeyed）写 `__keys__` 槽（键数组，与 `__zips__` 同序）：
/// 步进期据此把结果构造成 null 原型对象（键=结果值）；为空时（zip）构造数组。
///
/// 独立构造器（不复用 [`make_iterator_helper`]）：槽位布局不同，zip 无
/// `__inner__`/`__next__` 槽。
fn make_iterator_zip_helper<H: VmHost>(
    vm: &mut H, zips: &[(JsValue, JsValue)], mode: i32, padding: JsValue, keys: &[JsValue],
) -> JsValue {
    let helper_proto = vm.builtin_proto(ProtoKind::IteratorHelperProto);
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(helper_proto)));
    let wrapper_obj = unsafe { &mut *wrapper };
    let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
    let zips_arr =
        vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), zips.len()));
    for (i, (iter, next)) in zips.iter().enumerate() {
        let pair = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), 2));
        // SAFETY: pair 是刚经统一入口分配的数组对象，元素区已就绪。
        unsafe {
            (*pair).set_prop_at(0, *iter);
            (*pair).set_prop_at(1, *next);
        }
        // SAFETY: 同 epoch，写入后按实际元素数设 prop_count。
        unsafe {
            (*zips_arr).set_prop_at(i, JsValue::from_js_object(pair));
        }
    }
    // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
    unsafe {
        (*zips_arr).set_prop_count(zips.len());
    }
    let zips_si = vm.perm_intern(ZIPS_PROP);
    let open_si = vm.perm_intern(OPEN_PROP);
    let mode_si = vm.perm_intern(MODE_PROP);
    let state_si = vm.perm_intern(STATE_PROP);
    let kind_si = vm.perm_intern(KIND_PROP);
    let padding_si = vm.perm_intern(PADDING_PROP);
    vm.set_or_create_prop_value(wrapper_obj, zips_si, JsValue::from_js_object(zips_arr));
    vm.set_or_create_prop_value(wrapper_obj, open_si, JsValue::int(-1));
    vm.set_or_create_prop_value(wrapper_obj, mode_si, JsValue::int(mode));
    vm.set_or_create_prop_value(wrapper_obj, state_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, kind_si, JsValue::int(IteratorHelperKind::Zip as i32));
    vm.set_or_create_prop_value(wrapper_obj, padding_si, padding);
    if !keys.is_empty() {
        let keys_arr =
            vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), keys.len()));
        for (i, key) in keys.iter().enumerate() {
            // SAFETY: keys_arr 是刚经统一入口分配的数组对象，元素区已就绪。
            unsafe {
                (*keys_arr).set_prop_at(i, *key);
            }
        }
        // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
        unsafe {
            (*keys_arr).set_prop_count(keys.len());
        }
        let keys_si = vm.perm_intern(KEYS_PROP);
        vm.set_or_create_prop_value(wrapper_obj, keys_si, JsValue::from_js_object(keys_arr));
    }
    JsValue::from_js_object(wrapper)
}

/// 位掩码第 i 位判定开放（i32 掩码 32 位以上恒视为开放，超 32 迭代器降级）。
fn zip_bit_open(mask: i32, i: u32) -> bool {
    if i < 32 {
        ((mask >> i) & 1) == 1
    } else {
        true
    }
}

/// 清位掩码第 i 位（32 位以上无位可清，原样返回）。
fn zip_bit_clear(mask: i32, i: u32) -> i32 {
    if i < 32 {
        mask & !(1 << i)
    } else {
        mask
    }
}

/// 逆序 IteratorCloseAll：按位掩码对开放迭代器逆下标逐个关闭，关闭错误链式
/// 升级 completion（throw 完成原值恒胜出）。
fn close_remaining_open<H: VmHost>(
    vm: &mut H, this_val: JsValue, helper: &mut JsObject, open_mask: i32, iter_count: u32,
    completion_err: Option<JsValue>,
) -> Result<(), JsValue> {
    let mut completion_err = completion_err;
    for i in (0..iter_count).rev() {
        if !zip_bit_open(open_mask, i) {
            continue;
        }
        // 每次用户调用窗口后重读槽位（回调可改写 wrapper 槽）。
        let zips_si = vm.perm_intern(ZIPS_PROP);
        let zips_val = vm.ordinary_get(helper, zips_si, this_val).unwrap_or(JsValue::undefined());
        if !zips_val.is_object() {
            continue;
        }
        let zips_obj = unsafe { &*zips_val.as_js_object_ptr() };
        let pair_val = zips_obj.get_prop_at(i);
        if !pair_val.is_object() {
            continue;
        }
        let pair_obj = unsafe { &*pair_val.as_js_object_ptr() };
        let iter = pair_obj.get_prop_at(0);
        if !iter.is_object() {
            continue;
        }
        if let Err(v) = iterator_close_record(vm, iter, completion_err) {
            completion_err = Some(v);
        }
    }
    match completion_err {
        Some(v) => Err(v),
        None => Ok(()),
    }
}

/// zip 元素收集期骤发关闭：逆序关 `tail`（inputIter，`Some` 时）加已收集元素
/// 迭代器（逆序），throw 完成原值恒胜出。
fn close_zip_pairs<H: VmHost>(
    vm: &mut H, zips: &[(JsValue, JsValue)], input_iter: Option<JsValue>, completion_err: Option<JsValue>,
) {
    let mut completion_err = completion_err;
    for (iter, _) in zips.iter().rev() {
        if let Err(v) = iterator_close_record(vm, *iter, completion_err) {
            completion_err = Some(v);
        }
    }
    if let Some(input) = input_iter {
        let _ = iterator_close_record(vm, input, completion_err);
    }
}

/// zip 推进：升序步进开放迭代器，收集到钉在 `__results__` 槽的新数组。三 mode
/// 臂：shortest done 位逆序关其余开放迭代器（正常完成）收口；longest done 位填
/// 预计算 `padding[i]`（越界 undefined）续行、全耗尽收口；strict done 位 i 非 0
/// 逆序关全部开放迭代器（throw 完成，新 TypeError 胜出）后抛新 TypeError，i 为 0
/// 升序余者扫描（只读 done，已关位视 done 不重调 next），首个骤发逆序关全部开放
/// 迭代器透传原值、首个未耗尽逆序关全部开放迭代器抛新 TypeError、全耗尽正常收口；
/// 步进骤发清位后逆序关其余（原值胜出）透传。
fn helper_step_zip<H: VmHost>(
    vm: &mut H, this_val: JsValue, helper: &mut JsObject,
) -> Result<Option<JsValue>, JsValue> {
    let zips_si = vm.perm_intern(ZIPS_PROP);
    let zips_val = vm.ordinary_get(helper, zips_si, this_val).unwrap_or(JsValue::undefined());
    if !zips_val.is_object() {
        return Ok(None);
    }
    let zips_obj = unsafe { &*zips_val.as_js_object_ptr() };
    let iter_count = zips_obj.prop_count();
    if iter_count == 0 {
        return Ok(None);
    }
    let open_si = vm.perm_intern(OPEN_PROP);
    let mut open = read_slot_int(vm, helper, open_si, -1);
    if !zip_bit_open(open, 0) && (0..iter_count).all(|i| !zip_bit_open(open, i)) {
        return Ok(None);
    }
    let mode_si = vm.perm_intern(MODE_PROP);
    let mode = read_slot_int(vm, helper, mode_si, 0);
    // longest 臂落点值来自预计算 padding 数组（用户调用窗口前读取）。
    let padding_si = vm.perm_intern(PADDING_PROP);
    let padding_val = vm.ordinary_get(helper, padding_si, this_val).unwrap_or(JsValue::undefined());
    // 结果形态：`__keys__` 槽在场（zipKeyed）时构造 null 原型对象（键=结果值），
    // 否则（zip）构造数组。键数组与 `__zips__` 同序，步进期按下标取键。
    let keys_si = vm.perm_intern(KEYS_PROP);
    let keys_val = vm.ordinary_get(helper, keys_si, this_val).unwrap_or(JsValue::undefined());
    let keys_obj = if keys_val.is_object() {
        Some(unsafe { &*keys_val.as_js_object_ptr() })
    } else {
        None
    };
    let results_si = vm.perm_intern(RESULTS_PROP);
    let mut values: Vec<JsValue> = Vec::with_capacity(iter_count as usize);
    for i in 0..iter_count {
        if !zip_bit_open(open, i) {
            // 已关迭代器：longest 臂填 padding 落点续行（结果数组每个输入恒有
            // 一槽）；shortest/strict 首 done 即返回，本臂不会到达已关位。
            if mode == 1 {
                let pad = if padding_val.is_object() {
                    let pad_obj = unsafe { &*padding_val.as_js_object_ptr() };
                    pad_obj.get_prop_at(i)
                } else {
                    JsValue::undefined()
                };
                values.push(pad);
            }
            continue;
        }
        // 每次用户调用窗口后重读槽位（回调可改写 wrapper 槽）。
        let pair_val = zips_obj.get_prop_at(i);
        if !pair_val.is_object() {
            continue;
        }
        let pair_obj = unsafe { &*pair_val.as_js_object_ptr() };
        let iter = pair_obj.get_prop_at(0);
        let next_fn = pair_obj.get_prop_at(1);
        match iterator_record_step(vm, next_fn, iter) {
            Ok(Some(value)) => {
                values.push(value);
            }
            Ok(None) => {
                // 直接重赋值外层掩码：本臂不返回时后续迭代器的清位须在此基础上
                // 链式叠加，遮蔽新绑定会丢失先前清位。
                open = zip_bit_clear(open, i);
                vm.set_or_create_prop_value(helper, open_si, JsValue::int(open));
                vm.set_or_create_prop_value(helper, results_si, JsValue::undefined());
                if mode == 0 {
                    // shortest：done 迭代器不关，逆序关其余开放迭代器（正常完成）。
                    return match close_remaining_open(vm, this_val, helper, open, iter_count, None) {
                        Ok(()) => Ok(None),
                        Err(v) => Err(v),
                    };
                }
                if mode == 2 {
                    // strict：i 非 0 done → 逆序关全部开放迭代器（throw 完成，新
                    // TypeError 胜出）后抛新 TypeError。
                    if i != 0 {
                        let type_err = crate::error::create_type_error(
                            vm,
                            "Iterator.zip strict mode: iterators are not all the same length",
                        );
                        let _ = close_remaining_open(vm, this_val, helper, open, iter_count, Some(type_err));
                        return Err(type_err);
                    }
                    // i 为 0：升序余者扫描（只读 done，已关位视 done 不重调 next）。
                    for k in 1..iter_count {
                        if !zip_bit_open(open, k) {
                            continue;
                        }
                        let pair_val = zips_obj.get_prop_at(k);
                        if !pair_val.is_object() {
                            continue;
                        }
                        let pair_obj = unsafe { &*pair_val.as_js_object_ptr() };
                        let iter_k = pair_obj.get_prop_at(0);
                        let next_k = pair_obj.get_prop_at(1);
                        match iterator_step_done_only(vm, next_k, iter_k) {
                            Err(v) => {
                                open = zip_bit_clear(open, k);
                                vm.set_or_create_prop_value(helper, open_si, JsValue::int(open));
                                let _ = close_remaining_open(vm, this_val, helper, open, iter_count, Some(v));
                                return Err(v);
                            }
                            Ok(true) => {
                                // done 位移出开放集（不重调 next，不参与关全部）。
                                open = zip_bit_clear(open, k);
                                vm.set_or_create_prop_value(helper, open_si, JsValue::int(open));
                            }
                            Ok(false) => {
                                let type_err = crate::error::create_type_error(
                                    vm,
                                    "Iterator.zip strict mode: iterators are not all the same length",
                                );
                                let _ = close_remaining_open(vm, this_val, helper, open, iter_count, Some(type_err));
                                return Err(type_err);
                            }
                        }
                    }
                    // 全耗尽：正常收口。
                    return Ok(None);
                }
                // longest：done 位填 padding[i]（越界 undefined）续行。
                if !zip_bit_open(open, 0) && (0..iter_count).all(|k| !zip_bit_open(open, k)) {
                    return Ok(None);
                }
                let pad = if padding_val.is_object() {
                    let pad_obj = unsafe { &*padding_val.as_js_object_ptr() };
                    pad_obj.get_prop_at(i)
                } else {
                    JsValue::undefined()
                };
                values.push(pad);
            }
            Err(v) => {
                open = zip_bit_clear(open, i);
                vm.set_or_create_prop_value(helper, open_si, JsValue::int(open));
                vm.set_or_create_prop_value(helper, results_si, JsValue::undefined());
                let _ = close_remaining_open(vm, this_val, helper, open, iter_count, Some(v));
                return Err(v);
            }
        }
    }
    vm.set_or_create_prop_value(helper, results_si, JsValue::undefined());
    // 收口：按 `__keys__` 形态构造结果对象。zipKeyed 建 null 原型对象逐键写
    // 结果值（CreateDataPropertyOrThrow 语义，键已按自身键序收集）；zip 建数组
    // 按下标写。
    let result_val = if let Some(keys_arr) = keys_obj {
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let obj_ptr = obj;
        for (i, value) in values.iter().enumerate() {
            let key = keys_arr.get_prop_at(i);
            // 键经 ToPropertyKey 归一为 si（字符串/符号统一口径），逐键写结果值。
            let key_si = vm.property_key_si(key);
            vm.set_or_create_prop_value(unsafe { &mut *obj_ptr }, key_si, *value);
        }
        JsValue::from_js_object(obj)
    } else {
        let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
        let arr =
            vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), values.len()));
        for (i, value) in values.iter().enumerate() {
            // SAFETY: arr 是刚经统一入口分配的数组对象，元素区已就绪。
            unsafe {
                (*arr).set_prop_at(i, *value);
            }
        }
        // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
        unsafe {
            (*arr).set_prop_count(values.len());
        }
        JsValue::from_js_object(arr)
    };
    Ok(Some(result_val))
}

/// `Iterator.concat(...items)`：返回按序依次产出各实参元素的迭代器。
///
/// # 步骤
/// 1. 逐实参按序：对象检查（非对象 TypeError）加 GetMethod 只读缓存 `@@iterator`
///    方法（getter 构造期调用一次、方法不调用）；任一实参抛错即终止，后续
///    实参不读。
/// 2. 建 wrapper（内层迭代器懒开：各段首次 next 才调用方法）。
///
/// # 边界
/// - 静态方法，无 this 检查、无 options 参数。
/// - 方法为 undefined/null/其它不可调用值 → TypeError（构造期不触发方法调用）。
pub fn iterator_concat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let mut items: Vec<(JsValue, JsValue)> = Vec::new();
    for &reg in args.iter().skip(1) {
        let item = vm.reg(reg);
        if !item.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "Iterator.concat argument is not an object"));
        }
        let method = match get_iterator_method(vm, item) {
            Ok(m) => m,
            Err(v) => return NativeResult::Err(v),
        };
        items.push((method, item));
    }
    NativeResult::Ok(make_iterator_concat_helper(vm, &items))
}

/// 建 concat wrapper：空对象挂 `%IteratorHelperPrototype%`，写 `__items__`
/// （[method, iterable] 对数组）、`__seg__`（段下标，初始 0）、`__current__`
/// （当前内层，初始 undefined）、`__current_next__`（缓存 next，初始
/// undefined）、`__state__`（0）、`__kind__`（6）槽。
///
/// 独立构造器（不复用 [`make_iterator_helper`]）：槽位布局不同，concat 无
/// `__inner__`/`__next__` 槽。
fn make_iterator_concat_helper<H: VmHost>(vm: &mut H, items: &[(JsValue, JsValue)]) -> JsValue {
    let helper_proto = vm.builtin_proto(ProtoKind::IteratorHelperProto);
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(helper_proto)));
    let wrapper_obj = unsafe { &mut *wrapper };
    let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
    let items_arr =
        vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), items.len()));
    for (i, (method, iterable)) in items.iter().enumerate() {
        let pair = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), 2));
        // SAFETY: pair 是刚经统一入口分配的数组对象，元素区已就绪。
        unsafe {
            (*pair).set_prop_at(0, *method);
            (*pair).set_prop_at(1, *iterable);
        }
        // SAFETY: 同 epoch，写入后按实际元素数设 prop_count。
        unsafe {
            (*items_arr).set_prop_at(i, JsValue::from_js_object(pair));
        }
    }
    // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
    unsafe {
        (*items_arr).set_prop_count(items.len());
    }
    let items_si = vm.perm_intern(ITEMS_PROP);
    let seg_si = vm.perm_intern(SEG_PROP);
    let current_si = vm.perm_intern(CURRENT_PROP);
    let current_next_si = vm.perm_intern(CURRENT_NEXT_PROP);
    let state_si = vm.perm_intern(STATE_PROP);
    let kind_si = vm.perm_intern(KIND_PROP);
    vm.set_or_create_prop_value(wrapper_obj, items_si, JsValue::from_js_object(items_arr));
    vm.set_or_create_prop_value(wrapper_obj, seg_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, current_si, JsValue::undefined());
    vm.set_or_create_prop_value(wrapper_obj, current_next_si, JsValue::undefined());
    vm.set_or_create_prop_value(wrapper_obj, state_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, kind_si, JsValue::int(IteratorHelperKind::Concat as i32));
    JsValue::from_js_object(wrapper)
}

/// concat 推进：自 `__seg__` 起逐段——未开段（`__current__` 非对象）先懒开
/// （`Call(method, iterable)`，结果非对象抛 TypeError，加 GetIteratorDirect
/// 写槽）；内层步进：产出返回；done 清槽不关、切下一段；骤发只关当前内层
/// （throw 完成，原值恒胜出）透传；全段耗尽收口。
///
/// # 注意事项
/// 每次用户调用窗口后重读 `__items__`/`__seg__`/`__current__` 槽（回调可改写
/// wrapper 槽），不消费求值前快照。
fn helper_step_concat<H: VmHost>(
    vm: &mut H, this_val: JsValue, helper: &mut JsObject,
) -> Result<Option<JsValue>, JsValue> {
    let items_si = vm.perm_intern(ITEMS_PROP);
    let seg_si = vm.perm_intern(SEG_PROP);
    let current_si = vm.perm_intern(CURRENT_PROP);
    let current_next_si = vm.perm_intern(CURRENT_NEXT_PROP);
    loop {
        // 每次用户调用窗口后重读槽位（回调可改写 wrapper 槽）。
        let items_val = vm.ordinary_get(helper, items_si, this_val).unwrap_or(JsValue::undefined());
        if !items_val.is_object() {
            return Ok(None);
        }
        let items_obj = unsafe { &*items_val.as_js_object_ptr() };
        let item_count = items_obj.prop_count();
        let seg = read_slot_int(vm, helper, seg_si, 0) as u32;
        if seg >= item_count {
            return Ok(None);
        }
        let pair_val = items_obj.get_prop_at(seg);
        let (method, iterable) = if pair_val.is_object() {
            let pair_obj = unsafe { &*pair_val.as_js_object_ptr() };
            (pair_obj.get_prop_at(0), pair_obj.get_prop_at(1))
        } else {
            return Ok(None);
        };
        let current = vm.ordinary_get(helper, current_si, this_val).unwrap_or(JsValue::undefined());
        if !current.is_object() {
            // 懒开：本段首次 next 才调用方法（构造期只缓存不调用）。
            let inner = vm.call_function_sync(method, iterable, &[]).map_err(|e| engine_error(vm, &e))?;
            if !inner.is_object() {
                return Err(crate::error::create_type_error(
                    vm,
                    "Iterator.concat iterable @@iterator returned a non-object",
                ));
            }
            let (inner, next) = get_iterator_direct(vm, inner)?;
            vm.set_or_create_prop_value(helper, current_si, inner);
            vm.set_or_create_prop_value(helper, current_next_si, next);
            continue;
        }
        let next = vm
            .ordinary_get(helper, current_next_si, this_val)
            .unwrap_or(JsValue::undefined());
        match iterator_record_step(vm, next, current) {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {
                // 自然耗尽：清槽不关内层，切下一段。
                vm.set_or_create_prop_value(helper, current_si, JsValue::undefined());
                vm.set_or_create_prop_value(helper, current_next_si, JsValue::undefined());
                vm.set_or_create_prop_value(helper, seg_si, JsValue::int((seg + 1) as i32));
            }
            Err(v) => {
                // 骤发：只关当前内层（throw 完成，原值恒胜出），透传原值。
                let _ = iterator_close_record(vm, current, Some(v));
                return Err(v);
            }
        }
    }
}

/// `%IteratorPrototype%[@@iterator]`：返回 this（迭代器对象自迭代）。
///
/// 挂在 `%IteratorPrototype%` 上，所有集合迭代器经原型链继承，保证
/// `it[Symbol.iterator]() === it` 恒等成立；this 为任意值（含原始值）时原样返回。
pub fn iterator_symbol_iterator<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = if args.is_empty() { JsValue::undefined() } else { vm.reg(args[0]) };
    NativeResult::Ok(this_val)
}

/// `Iterator.from(iterable)`：为任意可迭代值包装一个迭代器对象。
/// 包装器带 `next` 与 `return`（用于 for-of 提前退出时的 IteratorClose 清理）。
pub fn iterator_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // GetIteratorFlattenable 语义：非字符串原始值非对象 → TypeError（不装箱，
    // 区别于 Array.from 的 GetIterator 装箱路径）。字符串保留原始值，由 String
    // 臂以原始值 receiver 读 @@iterator（typeof this === 'string'）。
    if !iterable.is_object() && !iterable.is_string() {
        return NativeResult::Err(crate::error::create_type_error(vm, "value is not an object"));
    }
    match make_iterator_for_value(vm, iterable) {
        Ok(iterator) => NativeResult::Ok(iterator),
        Err(err) => NativeResult::Err(err),
    }
}

/// 为任意值创建统一迭代器包装对象：String/Array/Map/Set 直接支持索引遍历，
/// 其它对象则要求提供可调用的 `next`。不可迭代时返回 TypeError。
pub fn make_iterator_for_value<H: VmHost>(vm: &mut H, value: JsValue) -> Result<JsValue, JsValue> {
    match try_make_iterator_inner(vm, value, true) {
        Ok(Some(iterator)) => Ok(iterator),
        Ok(None) => Err(crate::error::create_type_error(vm, "value is not iterable")),
        Err(err) => Err(err),
    }
}

/// 同 [`make_iterator_for_value`]，但包装器不绑定 `return` 方法（`yield*` 委托专用）：
/// 委托转发对内层迭代器延迟 GetMethod，避免创建包装器时访问内层 return getter。
pub fn make_iterator_for_value_without_return<H: VmHost>(vm: &mut H, value: JsValue) -> Result<JsValue, JsValue> {
    match try_make_iterator_inner(vm, value, false) {
        Ok(Some(iterator)) => Ok(iterator),
        Ok(None) => Err(crate::error::create_type_error(vm, "value is not iterable")),
        Err(err) => Err(err),
    }
}

/// 尝试创建迭代器包装对象，把"不可迭代"与"真异常"区分返回。
///
/// # 步骤
/// 1. 经迭代协议取内层迭代器（String/Array/Map/Set 直接作为内层，其余对象调用
///    `@@iterator` 或回退可调用的 `next`；鸭子回退路径把读到的 `next` 闭包
///    缓存进包装器 `__next__` 槽，消费期不重触发 getter）。
/// 2. 包装成统一迭代器对象（带 `next` 与可选 `return`），供调用方逐个取元素。
///
/// # 返回值
/// - `Ok(Some(iterator))`：可迭代，返回包装器；
/// - `Ok(None)`：不可迭代（调用方回退 array-like 路径）；
/// - `Err`：`@@iterator` getter/call 抛错，透传原异常值。
/// - `bind_return` 控制是否暴露 `return` 方法（for-of/解构的 IteratorClose 需要，
///   `yield*` 委托不需要且须避免创建时访问内层 return getter）。
pub(crate) fn try_make_iterator_inner<H: VmHost>(
    vm: &mut H, value: JsValue, bind_return: bool,
) -> Result<Option<JsValue>, JsValue> {
    try_make_iterator_inner_proto(vm, value, bind_return, None)
}

/// 同 [`try_make_iterator_inner`]，但允许调用方指定包装器原型
/// （String 迭代器挂 %StringIteratorPrototype%，其余默认 %IteratorPrototype%）。
pub(crate) fn try_make_iterator_inner_proto<H: VmHost>(
    vm: &mut H, value: JsValue, bind_return: bool, wrapper_proto: Option<*mut JsObject>,
) -> Result<Option<JsValue>, JsValue> {
    let (inner, cached_next) = match get_iterator(vm, value) {
        Ok(Some(pair)) => pair,
        Ok(None) => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(Some(build_iterator_wrapper(vm, inner, cached_next, bind_return, wrapper_proto)))
}

/// 以给定内层迭代器构造统一迭代器包装对象。
///
/// # 步骤
/// 1. 选定包装器原型：调用方指定时优先（如 String 迭代器的
///    %StringIteratorPrototype%），否则默认 %IteratorPrototype%（经原型链获得
///    `@@iterator` 返回自身）。
/// 2. 写入 `__inner__`/`__index__` 槽；鸭子回退路径把创建时读到的 `next` 闭包
///    缓存进 `__next__` 槽，消费期直读槽不重触发 getter。
/// 3. 挂 `next` 方法；`bind_return` 且内层是对象时恒挂 `return` 方法
///    （内层 return 在关闭期惰性读取，构造期不触碰其 getter）。
///
/// # 边界
/// - `bind_return` 控制是否暴露 `return` 方法（for-of/解构的 IteratorClose 需要，
///   `yield*` 委托不需要）；内层 return 一律关闭期惰性 GetMethod，构造期不触碰
///   其 getter，getter 抛错与不可调用判定在关闭期发生。
/// - 本函数不解析 `@@iterator`：`get_iterator` 派生内层，或默认迭代器函数直接以
///   已 ToString 的串为内层构造（其语义只由 `this` 决定，与属性表状态解耦）。
pub(crate) fn build_iterator_wrapper<H: VmHost>(
    vm: &mut H, inner: JsValue, cached_next: Option<JsValue>, bind_return: bool, wrapper_proto: Option<*mut JsObject>,
) -> JsValue {
    let iterator_proto = match wrapper_proto {
        Some(proto) => proto,
        None => vm.builtin_proto(ProtoKind::IteratorProto),
    };
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(iterator_proto)));

    let inner_si = vm.perm_intern(INNER_PROP);
    let index_si = vm.perm_intern(INDEX_PROP);
    let next_si = vm.perm_intern("next");
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));

    // 鸭子回退包装器：from() 时读到的 next 闭包缓存进 __next__ 槽（与 helper 包装器
    // "只读一次"同模式），消费期直读槽不重触发 getter；非鸭子路径无槽，wrapper next
    // 回退重读内层 next（既有行为）。
    if let Some(next_fn) = cached_next {
        let next_cache_si = vm.perm_intern(NEXT_CACHE_PROP);
        vm.set_or_create_prop_value(wrapper_obj, next_cache_si, next_fn);
    }

    let wrapper_next = make_native_function(vm, "next", iterator_wrapper_next::<H> as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, wrapper_next);

    // for-of/解构的 IteratorClose 需要 return 方法：bind_return 且内层是对象时恒挂，
    // 内层 return 由包装器 return 方法在关闭期惰性 GetMethod（getter 抛错传播、
    // 不可调用抛 TypeError、undefined/null 无操作）。
    // `yield*` 委托（bind_return=false）不绑定，转发时对内层延迟 GetMethod。
    let return_si = vm.perm_intern("return");
    if bind_return && inner.is_object() {
        let wrapper_return = make_native_function(vm, "return", iterator_wrapper_return::<H> as *const (), 0);
        vm.set_or_create_prop_value(wrapper_obj, return_si, wrapper_return);
    }

    JsValue::from_js_object(wrapper)
}

/// 迭代器包装器的 `return` 方法：关闭期惰性读取内层 `return`（完整 GetMethod
/// 步序）并转发调用，默认零参。
///
/// # 步骤
/// 1. 读 `__inner__` 槽；this 非对象或槽缺失/非对象时返回 undefined（无操作）。
/// 2. GetMethod(inner, "return")：getter 抛错透传原异常；undefined/null 返回空
///    对象（无操作，使调用方步 7 的对象校验通过）；已定义不可调用值抛 TypeError。
/// 3. 调用方显式传参时转发该实参（Iterator.prototype.return 的 return(v) 语义），
///    否则零参调用（IteratorClose 步 4c）；结果原样返回，步 7 由调用方执行。
fn iterator_wrapper_return<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let wrapper = unsafe { &*this_val.as_js_object_ptr() };
    let inner_si = vm.perm_intern(INNER_PROP);
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if inner.is_object() => inner,
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    // 延迟 GetMethod：内层 return 在关闭期读取，getter 抛错透传原异常。
    let inner_obj = unsafe { &*inner.as_js_object_ptr() };
    let return_si = vm.perm_intern("return");
    let return_fn = match vm.ordinary_get(inner_obj, return_si, inner) {
        Ok(f) if is_callable(f) => f,
        // undefined/null：GetMethod 返回 unused，无操作，返空对象使调用方步 7 通过。
        Ok(f) if f.is_undefined() || f.is_null() => {
            let object_proto = vm.builtin_proto(ProtoKind::ObjectProto);
            let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
            return NativeResult::Ok(JsValue::from_js_object(obj));
        }
        // 已定义不可调用值：GetMethod 抛 TypeError。
        Ok(_) => return NativeResult::Err(crate::error::create_type_error(vm, "iterator return is not a function")),
        Err(err) => return NativeResult::Err(engine_error(vm, &err)),
    };
    // 默认零参（IteratorClose 步 4c）；调用方显式传参时转发（Iterator.prototype.return
    // 的 return(v) 语义）。结果原样返回，步 7 由调用方 IteratorClose 执行。
    let result = if args.len() > 1 {
        vm.call_function_sync(return_fn, inner, &[vm.reg(args[1])])
    } else {
        vm.call_function_sync(return_fn, inner, &[])
    };
    match result {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => match vm.take_uncaught_value() {
            Some(original) => NativeResult::Err(original),
            None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        },
    }
}

/// 迭代器包装器的 `next` 方法：对 Array/String/Map/Set 直接按索引取值，
/// 其它对象优先用 `__next__` 槽（鸭子回退包装器创建时只读一次缓存的闭包），
/// 槽缺失时委托内层自身 `next`；底层抛出异常时透传原始值（不做二次包装）。
pub fn iterator_wrapper_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator wrapper next called on non-object"));
    }

    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.perm_intern(INNER_PROP);
    let index_si = vm.perm_intern(INDEX_PROP);
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "Iterator wrapper has no inner iterator")),
    };

    match next_array_like(vm, wrapper, inner, index_si) {
        Ok(Some(result)) => return NativeResult::Ok(result),
        Ok(None) => {}
        Err(original) => return NativeResult::Err(original),
    }

    if inner.is_object() {
        // 鸭子回退包装器优先读 `__next__` 槽（from() 时只读一次的缓存闭包，
        // 消费期不重触发 getter）；槽缺失或不可调用时回退重读内层 next
        // （非鸭子路径既有行为）。
        let next_cache_si = vm.perm_intern(NEXT_CACHE_PROP);
        let cached = match vm.ordinary_get(wrapper, next_cache_si, this_val) {
            Ok(c) => c,
            Err(err) => {
                // `__next__` 槽读取抛错：透传原异常，不重新包装成 TypeError。
                return match vm.take_uncaught_value() {
                    Some(original) => NativeResult::Err(original),
                    None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
                };
            }
        };
        let next = if is_callable(cached) {
            cached
        } else {
            let inner_obj = unsafe { &*inner.as_js_object_ptr() };
            let next_si = vm.perm_intern("next");
            match vm.ordinary_get(inner_obj, next_si, inner) {
                Ok(n) => n,
                Err(err) => {
                    // GetMethod 的 next getter 抛错：透传原异常，不重新包装成 TypeError。
                    return match vm.take_uncaught_value() {
                        Some(original) => NativeResult::Err(original),
                        None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
                    };
                }
            }
        };
        // 默认零参（IteratorNext 步 4 零参调用）；调用方显式传参时转发（Iterator.
        // prototype.next 的 next(v) 语义）。
        let result = if args.len() > 1 {
            vm.call_function_sync(next, inner, &[vm.reg(args[1])])
        } else {
            vm.call_function_sync(next, inner, &[])
        };
        return match result {
            Ok(result) => NativeResult::Ok(result),
            // 透传原始抛出的值（任意类型）而非重新包装成 TypeError，
            // 使外围 try/catch 能看到真正的错误。
            Err(err) => match vm.take_uncaught_value() {
                Some(original) => NativeResult::Err(original),
                None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
            },
        };
    }

    NativeResult::Err(crate::error::create_type_error(vm, "value is not iterable"))
}

/// 只读取 value 的 `@@iterator` 方法而不调用，按 GetMethod-可选入口的三态语义
/// 判定可迭代性（Array.from / TypedArray 构造器 / TypedArray.from 共用）。
///
/// 统一经原型链解析 `@@iterator`：解析到可调用返 true（调用方走迭代臂）；
/// 解析为 null/undefined 返 false（调用方落 array-like 索引读臂），此时再按引擎
/// 扩展回退到自身可调用的 `next`；解析为非空不可调用值时抛 TypeError
/// （GetMethod 步 4），不落入 array-like 臂。
///
/// # 边界
/// 字符串原始值按自身 wrapper 的原型链读 `@@iterator`（读起点 String.prototype、
/// receiver = 原始值），不套用 duck-next 回退（原始串无自身 `next`）；非对象原始值
/// （number/boolean 等）经 ToObject 得查找起点，receiver 保持原始值（GetV 语义）。
/// `@@iterator` getter 抛错时透传 `Err`（不落入鸭子回退）。
///
/// # 注意事项
/// 三态是 GetMethod-可选入口的规范语义：非空不可调用（含 [[IsHTMLDDA]]
/// 宿主值等不可调用对象）必须抛错而非静默 array-like。
pub(crate) fn peek_iterator_method<H: VmHost>(vm: &mut H, value: JsValue) -> Result<bool, JsValue> {
    // 字符串原始值：读 String.prototype 上的 @@iterator，receiver = 原始值
    // （与 get_iterator 的 String 臂同口径）。默认迭代器被删除/置 null 时须落
    // array-like，而非按"String 恒可迭代"放行迭代路径。
    if value.is_string() {
        let proto_ptr = vm.builtin_proto(ProtoKind::StringProto);
        // SAFETY: string_proto 是 BuiltinWorld 长驻原型对象，进程内有效且不被 GC 搬移。
        let proto_obj = unsafe { &*proto_ptr };
        let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
        let method = match vm.ordinary_get(proto_obj, sym_iter_si, value) {
            Ok(m) => m,
            Err(err) => {
                // GetMethod 取 @@iterator 时 getter 抛出：透传原值，不落入 array-like。
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
                return Err(exc);
            }
        };
        if is_callable(method) {
            return Ok(true);
        }
        // GetMethod 步 4：非空不可调用方法是 TypeError，不落入 array-like 臂。
        if !method.is_null() && !method.is_undefined() {
            return Err(crate::error::create_type_error(vm, "value is not iterable"));
        }
        // null/undefined：原始串的临时 wrapper 无自身可调用 next，落 array-like。
        return Ok(false);
    }
    // 原始值（number/bigint/boolean/symbol）经 ToObject 得 @@iterator 查找起点，
    // receiver 保持原始值本身（getter 观测原始类型，如 `Array.from(5)` 经
    // Number.prototype[Symbol.iterator]）；null/undefined 装箱失败按不可迭代处理。
    let recv = if value.is_object() {
        value
    } else {
        match to_object(value, vm) {
            Ok(o) => o,
            Err(_) => return Ok(false),
        }
    };
    if recv.is_object() {
        let obj = unsafe { &*recv.as_js_object_ptr() };
        let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
        let method = match vm.ordinary_get(obj, sym_iter_si, value) {
            Ok(m) => m,
            Err(err) => {
                // GetMethod 取 @@iterator 时 getter 抛出：透传原值，不落入鸭子回退。
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
                return Err(exc);
            }
        };
        if is_callable(method) {
            return Ok(true);
        }
        // GetMethod 步 4：非空不可调用方法是 TypeError（不落入鸭子回退与
        // array-like 臂）；null/undefined 才放行到回退判定。
        if !method.is_null() && !method.is_undefined() {
            return Err(crate::error::create_type_error(vm, "value is not iterable"));
        }
        // 鸭子回退：@@iterator 解析为 null/undefined 且对象自身有可调用 next。
        // 装箱串不套用：get_iterator 的 String 臂无 duck-next 回退，规范也只看
        // @@iterator（Array.from/%TypedArray%.from 应落 array-like）。
        if !obj.is_string_obj() {
            let next_si = vm.perm_intern("next");
            if let Ok(next) = vm.ordinary_get(obj, next_si, value) {
                if is_callable(next) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// 判断已读到的 `@@iterator` 结果是否仍是内建集合的默认迭代器函数
/// （内建原型别名槽上的同一函数对象）。
///
/// # 边界
/// value 须为内建集合（Array/TypedArray/Map/Set），锚点槽按种类选取
/// （Array/TypedArray/Set 为 `values`、Map 为 `entries`）。
///
/// # 注意
/// 锚点槽读取失败（如用户把别名改写为抛出 getter）一律返回 false，
/// 落入通用协议按 `method` 继续处理，不在此处吞错。
fn builtin_iterator_default_intact<H: VmHost>(vm: &mut H, value: JsValue, method: JsValue) -> bool {
    if !method.is_object() {
        return false;
    }
    let (anchor_ptr, anchor_name) = if is_array_value(value) {
        (vm.builtin_proto(ProtoKind::ArrayProto), "values")
    } else if is_typed_array_value(value) {
        (vm.builtin_proto(ProtoKind::TypedArrayProto), "values")
    } else if is_map_value(value) {
        (vm.builtin_proto(ProtoKind::MapProto), "entries")
    } else {
        (vm.builtin_proto(ProtoKind::SetProto), "values")
    };
    let anchor_obj = unsafe { &*anchor_ptr };
    let anchor_si = vm.perm_intern(anchor_name);
    let Ok(anchor) = vm.ordinary_get(anchor_obj, anchor_si, JsValue::from_js_object(anchor_ptr)) else {
        return false;
    };
    anchor.is_object() && std::ptr::eq(method.as_js_object_ptr(), anchor.as_js_object_ptr())
}

/// 经迭代协议取内层迭代器：内建集合（原型链 `@@iterator` 未被覆盖时）直接
/// 作为内层（无缓存 next），用户覆盖的 `@@iterator` 或 duck-next 回退路径
/// 的调用结果作为内层（无缓存 next），鸭子回退路径同时回传读到的 `next`
/// 闭包供调用方缓存进包装器 `__next__` 槽（消费期不重触发 getter）。
///
/// # 返回值
/// - `Ok(Some((inner, next)))`：`inner` 为内层迭代器；鸭子回退时 `next` 为
///   读到的闭包，其余路径为 `None`；
/// - `Ok(None)`：不可迭代；
/// - `Err`：`@@iterator` 或 `next` getter 抛错，透传原异常值。
fn get_iterator<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<(JsValue, Option<JsValue>)>, JsValue> {
    // 字符串面：原始串与装箱串（is_string_obj）同走本臂，GetMethod 语义。
    // receiver = value：原始串 typeof this 'string'，装箱串 'object'。读 @@iterator
    // 从值的自身对象起（原始串无自身对象，落 String.prototype；装箱串自身 → 原型链），
    // 覆盖默认快速路径的码元步进，并避免调默认迭代器再递归回本臂二次触发 getter。
    let value_is_string_box = if value.is_string() {
        false
    } else if value.is_object() {
        unsafe { &*value.as_js_object_ptr() }.is_string_obj()
    } else {
        false
    };
    if value.is_string() || value_is_string_box {
        // 读 @@iterator 的对象起点：装箱串读自身（可命中自身 @@iterator），
        // 原始串读 String.prototype（原始串无自身属性）。
        let read_ptr = if value_is_string_box {
            value.as_js_object_ptr()
        } else {
            vm.builtin_proto(ProtoKind::StringProto)
        };
        let read_obj = unsafe { &*read_ptr };
        let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
        let method = match vm.ordinary_get(read_obj, sym_iter_si, value) {
            Ok(m) => m,
            Err(err) => {
                // GetMethod 取 @@iterator 时 getter 抛出：透传原值，不落入码元快速路径。
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
                return Err(exc);
            }
        };
        // 码元快速路径的 inner：原始串取 value 本身；装箱串取其 boxed_value
        // 载荷——wrapper 码元步进按原始串产出，装箱串不直接作 inner。
        let inner = if value_is_string_box {
            let raw = unsafe { (&*value.as_js_object_ptr()).boxed_value() };
            if raw.is_string() {
                raw
            } else {
                value
            }
        } else {
            value
        };
        // 默认迭代器（自别名：即 @@iterator 槽本身）走码元快速路径、不调方法——
        // 调默认函数会递归回本 String 臂。指针未捕获（绑定前瞬时）亦回退快速路径。
        let default_ptr = vm.string_default_iterator();
        if !default_ptr.is_null()
            && is_callable(method)
            && std::ptr::eq(method.as_js_object_ptr() as *const JsObject, default_ptr)
        {
            return Ok(Some((inner, None)));
        }
        if is_callable(method) {
            // 用户覆盖：调用，receiver = value（原始串/装箱串）。
            let iterator = match vm.call_function_sync(method, value, &[]) {
                Ok(it) => it,
                Err(err) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
                    return Err(exc);
                }
            };
            if !iterator.is_object() {
                return Err(crate::error::create_type_error(
                    vm,
                    "Result of the Symbol.iterator method is not an object",
                ));
            }
            return Ok(Some((iterator, None)));
        }
        if !method.is_null() && !method.is_undefined() {
            // GetMethod 步 4：非空不可调用方法 → TypeError。
            return Err(crate::error::create_type_error(vm, "value is not iterable"));
        }
        // @@iterator 解析为 null/undefined：String 不再恒可迭代。返回不可迭代由
        // make_iterator_for_value 统一折成 TypeError；Array.from/%TypedArray%.from
        // 已由 peek 门控提前落 array-like，不会到达本臂。
        return Ok(None);
    }
    // 内建集合标记：仅当 @@iterator 经原型链读到的仍是内建默认迭代器函数
    // 时才走快速路径；用户覆盖（自身属性或原型链改写/删除）走通用协议
    // 调用用户函数。
    let builtin_value =
        is_array_value(value) || is_typed_array_value(value) || is_map_value(value) || is_set_value(value);

    // 非字符串 primitive（boolean/number/symbol/bigint）：GetIterator 以 ToObject
    // 得 @@iterator 查找起点，receiver 与被调方法的 this 保持原始值（GetV /
    // GetIteratorFromMethod 语义，如 `yield* true` 委托 Boolean.prototype[Symbol.iterator]）。
    // null/undefined 的 ToObject 失败按不可迭代处理。
    let obj_value = if value.is_object() {
        value
    } else {
        match to_object(value, vm) {
            Ok(obj) => obj,
            Err(_) => return Ok(None),
        }
    };
    let obj = unsafe { &*obj_value.as_js_object_ptr() };
    // 迭代协议：GetIterator 先取 value[Symbol.iterator] 并调用；装箱对象仅作查找
    // 起点与鸭子回退 inner，receiver/this 用原始 value（GetV / GetIteratorFromMethod）。
    let sym_iter_si = encode_symbol_key(vm.realm_id(), 0);
    let pc_before = vm.pc();
    let method = match vm.ordinary_get(obj, sym_iter_si, value) {
        Ok(m) => m,
        Err(err) => {
            // GetMethod 取 @@iterator 时 getter 抛出：透传原值，不落入鸭子回退。
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
            return Err(exc);
        }
    };
    // 取 @@iterator 期间源 getter 抛出：深度 0 的 raise_call_error 已就地 unwind
    //（跳转 catch），pc 必已变化——须立即停止，不得落入鸭子回退二次触源 getter。
    if vm.pc() != pc_before {
        return Ok(None);
    }
    if builtin_value && builtin_iterator_default_intact(vm, value, method) {
        return Ok(Some((value, None)));
    }
    if is_callable(method) {
        let iterator = match vm.call_function_sync(method, value, &[]) {
            Ok(it) => it,
            Err(err) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
                return Err(exc);
            }
        };
        if !iterator.is_object() {
            return Err(crate::error::create_type_error(
                vm,
                "Result of the Symbol.iterator method is not an object",
            ));
        }
        return Ok(Some((iterator, None)));
    }
    // 鸭子回退：对象自身有可调用 next（Map/Set 迭代器包装等既有用法）。
    // getter 抛错透传原值（不落入"不可迭代"），读到的闭包随 inner 回传供缓存。
    let next_si = vm.perm_intern("next");
    let next = match vm.ordinary_get(obj, next_si, value) {
        Ok(n) => n,
        Err(err) => {
            // GetMethod 取 next 时 getter 抛出：透传原值，不降级为 "not iterable"。
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &err));
            return Err(exc);
        }
    };
    if is_callable(next) {
        return Ok(Some((obj_value, Some(next))));
    }

    Ok(None)
}

/// 字符串迭代的一个产出元素：单个码元，或整个合法代理对（超平面字符，
/// 按 Unicode 标量产出单元素）。
#[derive(Clone, Copy)]
enum StrIterElem {
    One(u16),
    Pair(u16, u16),
}

/// 单元序列的字符串迭代步进（Unicode 标量口径）：合法代理对整体产出一个
/// 2 单元元素（游标推进 2），否则产出单单元元素（推进 1）；越界产出 None。
fn str_iter_step(units: &[u16], at: usize) -> (usize, Option<StrIterElem>) {
    let Some(&u) = units.get(at) else {
        return (0, None);
    };
    if (0xD800..=0xDBFF).contains(&u) {
        if let Some(lo) = units.get(at + 1) {
            if (0xDC00..=0xDFFF).contains(lo) {
                return (at + 2, Some(StrIterElem::Pair(u, *lo)));
            }
        }
    }
    (at + 1, Some(StrIterElem::One(u)))
}

/// 内置包装器的内层值种类：for-of 内置包装器（Array/String/TA/Map/Set）的
/// 内层类型五判据（与 `next_array_like` 臂分派同判据）。快路径按 kind 直步，
/// kind 失配时回落慢路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinIterKind {
    Array,
    String,
    TypedArray,
    MapEntries,
    SetValues,
}

/// 按五判据分类内层值（臂序与 `next_array_like` 一致），非内置类型返回 None。
fn classify_inner(inner: JsValue) -> Option<BuiltinIterKind> {
    if is_array_value(inner) {
        Some(BuiltinIterKind::Array)
    } else if inner.is_string() {
        Some(BuiltinIterKind::String)
    } else if is_typed_array_value(inner) {
        Some(BuiltinIterKind::TypedArray)
    } else if is_map_value(inner) {
        Some(BuiltinIterKind::MapEntries)
    } else if is_set_value(inner) {
        Some(BuiltinIterKind::SetValues)
    } else {
        None
    }
}

/// 检测内置包装器种类：读 `__inner__` 槽后按五判据分类，连带返回 inner 值。
///
/// # 返回值
/// - `Some((kind, inner))`：内置包装器，inner 为 `__inner__` 槽值快照；
/// - `None`：包装器非对象 / `__inner__` 缺失（undefined）/ 非内置类型（调用方走慢路径）。
///
/// # 注意事项
/// - `__inner__` 是用户可观察槽，随时可能被改写；返回值是 init 期快照，
///   快路径按快照直步不再每步重读复判（见 `builtin_iter_fast_step`）。
pub fn builtin_iter_kind<H: VmHost>(vm: &mut H, wrapper: JsValue) -> Option<(BuiltinIterKind, JsValue)> {
    if !wrapper.is_object() {
        return None;
    }
    // SAFETY: wrapper 刚判定为对象。
    let wrapper_obj = unsafe { &*wrapper.as_js_object_ptr() };
    let inner_si = vm.perm_intern(INNER_PROP);
    let inner = match vm.ordinary_get(wrapper_obj, inner_si, wrapper) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return None,
    };
    classify_inner(inner).map(|kind| (kind, inner))
}

/// 内置包装器步进核心：按内层类型产出元素值、done 标志与新游标。
///
/// 游标由调用方传入并回传（快路径存条目、慢路径写回 `__index__` 槽），
/// 本核心不触碰包装器属性槽。
///
/// # 返回值
/// - `Ok(None)`：内层非内置类型（调用方落慢路径）；
/// - `Ok(Some((value, done, next_cursor)))`：本步元素值、done 标志与新游标；
/// - `Err`：原始异常值（元素 getter 抛出、TA detach/越界等）。
fn step_builtin_inner<H: VmHost>(
    vm: &mut H, inner: JsValue, cursor: usize,
) -> Result<Option<(JsValue, bool, usize)>, JsValue> {
    if is_array_value(inner) {
        let index = cursor;
        let arr = unsafe { &*inner.as_js_object_ptr() };
        if index < arr.prop_count() as usize {
            // 数组元素读取走 GetValue：普通数据属性返回槽值，访问器属性
            // （defineProperty getter）触发 getter 并透传异常。整数键免 intern。
            let key_si = make_int_key(index as u32);
            let value = match vm.ordinary_get(arr, key_si, inner) {
                Ok(v) => v,
                Err(err) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_from_text(vm, &err));
                    return Err(exc);
                }
            };
            return Ok(Some((value, false, index + 1)));
        }
        return Ok(Some((JsValue::undefined(), true, cursor)));
    }

    if inner.is_string() {
        // 游标按载荷形态钉死：单元形态（FlatU16/Cons）存单元偏移；Flat 形态存
        // 字节偏移×2（与单元口径对齐，每字符步长按其 UTF-16 单元数）。
        //
        // 产出按 Unicode 标量（规格口径）：合法代理对整体产出一个 2 单元元素，
        // 孤立 surrogate 各产出 1 单元元素。
        // 源串裸指针借用压缩到单个表达式：产出元素为 Copy 值，不携带借用，
        // 之后对 VM 状态的可变访问不再与源串借用共存。
        let (next_cursor, elem): (usize, Option<StrIterElem>) = unsafe {
            let sp = &*inner.as_string_ptr();
            if sp.is_flat() {
                let byteoff = cursor / 2;
                match sp.as_str().get(byteoff..).unwrap_or("").chars().next() {
                    // 超平面字符：出整个代理对（单元素），游标跳越字符。
                    Some(c) if c.len_utf8() == 4 => {
                        let v = c as u32;
                        let hi = (0xD800 + (((v - 0x10000) >> 10) & 0x3FF)) as u16;
                        let lo = (0xDC00 + ((v - 0x10000) & 0x3FF)) as u16;
                        ((byteoff + 4) * 2, Some(StrIterElem::Pair(hi, lo)))
                    }
                    Some(c) => ((byteoff + c.len_utf8()) * 2, Some(StrIterElem::One(c as u16))),
                    None => (0, None),
                }
            } else if let Some(units) = sp.units_borrowed() {
                str_iter_step(units, cursor)
            } else {
                let units = sp.units();
                str_iter_step(&units, cursor)
            }
        };
        if let Some(elem) = elem {
            let value = match elem {
                // ASCII 单元走单字符缓存零分配，其余（含孤立 surrogate）落串创建。
                StrIterElem::One(u) => match vm.single_unit(u) {
                    Some(v) => v,
                    None => vm.new_string_units(&[u]),
                },
                StrIterElem::Pair(hi, lo) => vm.new_string_units(&[hi, lo]),
            };
            return Ok(Some((value, false, next_cursor)));
        }
        return Ok(Some((JsValue::undefined(), true, cursor)));
    }

    if is_typed_array_value(inner) {
        let index = cursor;
        let obj = unsafe { &*inner.as_js_object_ptr() };
        // 每次 next 入口校验：detach/越界抛 TypeError，auto 收缩按 live 长度停。
        let view = crate::typed_array::get_typed_array_data(vm, inner)?;
        let view = crate::typed_array::ta_validate(vm, view, false)?;
        let len = crate::typed_array::ta_live_length(view);
        if index < len {
            let value = match crate::typed_array::typed_array_element_get(vm, obj, index as u32) {
                Ok(v) => v,
                Err(e) => return Err(crate::error::create_type_error(vm, &e)),
            };
            return Ok(Some((value, false, index + 1)));
        }
        return Ok(Some((JsValue::undefined(), true, cursor)));
    }

    // for-of 循环默认迭代：Map 产出 [key, value] 对，Set 产出值。
    if is_map_value(inner) {
        let (value, done, next) = map_set_step_core(vm, inner, cursor, MapSetMode::MapEntries);
        return Ok(Some((value, done, next)));
    }
    if is_set_value(inner) {
        let (value, done, next) = map_set_step_core(vm, inner, cursor, MapSetMode::SetValues);
        return Ok(Some((value, done, next)));
    }

    Ok(None)
}

/// 内置包装器步进一步（for-of 快路径入口）：按 init 期快照的 inner 与游标直步，
/// 不重读 `__inner__` 槽、不读写 `__index__` 槽。
///
/// # 返回值
/// - `Ok((value, done, next_cursor))`：本步元素值、done 标志与新游标；
/// - `Err`：原始异常值（元素 getter 抛出、TA detach/越界等）。
pub fn builtin_iter_fast_step<H: VmHost>(
    vm: &mut H, inner: JsValue, cursor: usize,
) -> Result<(JsValue, bool, usize), JsValue> {
    match step_builtin_inner(vm, inner, cursor)? {
        Some(step) => Ok(step),
        // 内层非内置类型：理论上不可达（kind 在 init 期已判且 inner 为快照），
        // 防御性按 done 收口。
        None => Ok((JsValue::undefined(), true, cursor)),
    }
}

/// 内置包装器的 `next` 步进（慢路径）：从 `__index__` 槽读游标、步进核心产出
/// 元素、新游标写回槽后包成 `{value, done}` 结果对象；慢路径行为与抽提前逐位一致。
fn next_array_like<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32,
) -> Result<Option<JsValue>, JsValue> {
    let cursor = current_index(vm, wrapper, index_si);
    match step_builtin_inner(vm, inner, cursor)? {
        Some((value, done, next_cursor)) => {
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(next_cursor as i32));
            Ok(Some(make_iter_result(vm, value, done)))
        }
        None => Ok(None),
    }
}

fn current_index<H: VmHost>(vm: &mut H, wrapper: &JsObject, index_si: u32) -> usize {
    match vm.ordinary_get(wrapper, index_si, JsValue::undefined()) {
        Ok(value) if value.is_int() => value.as_int().max(0) as usize,
        Ok(value) if value.is_double() => value.as_double().max(0.0) as usize,
        _ => 0,
    }
}

/// 构造迭代器结果对象 `{value, done}`（生成器 next/return 结果复用）。
pub fn make_iter_result<H: VmHost>(vm: &mut H, value: JsValue, done: bool) -> JsValue {
    let object_proto = vm.builtin_proto(ProtoKind::ObjectProto);
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let value_si = vm.perm_intern("value");
    let done_si = vm.perm_intern("done");
    let obj_ref = unsafe { &mut *obj };
    vm.set_or_create_prop_value(obj_ref, value_si, value);
    vm.set_or_create_prop_value(obj_ref, done_si, JsValue::bool(done));
    JsValue::from_js_object(obj)
}

/// 构造 native 函数对象并返回：设置函数标记、native 函数项、参数个数与 `name`
/// 属性。供迭代器包装器把自己的 next/return 方法装到包装对象上，不经绑定层注册。
pub(crate) fn make_native_function<H: VmHost>(vm: &mut H, name: &str, native_fn: *const (), arg_count: u8) -> JsValue {
    let function_proto = vm.builtin_proto(ProtoKind::FunctionProto);
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
    func.set_function(true);
    // SAFETY: native_fn 来自 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(native_fn) }));
    func.set_native_arg_count(arg_count);
    let func = vm.alloc_object(func);
    let name_si = vm.perm_intern("name");
    let value = vm.new_string(name);
    let func_ref = unsafe { &mut *func };
    vm.set_or_create_prop_value(func_ref, name_si, value);
    JsValue::from_js_object(func)
}

fn is_array_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_array()
}

fn is_typed_array_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_typed_array_obj()
}

/// 判断值是否为可调用对象（native 或字节码函数）。
pub fn is_callable(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_function()
}

/// 把 `call_function_sync` 返回的 `Err` 文本恢复为原始异常值；
/// 无保留的 uncaught 值时回退为普通 TypeError。
pub(crate) fn engine_error<H: VmHost>(vm: &mut H, err: &str) -> JsValue {
    vm.take_uncaught_value()
        .unwrap_or_else(|| crate::error::create_from_text(vm, err))
}

/// IteratorClose：异常退出时调用迭代器包装器的 `return()`（转发给内层迭代器），
/// 丢弃 return 自身抛出的错误，保留在途异常。
pub(crate) fn close_iterator<H: VmHost>(vm: &mut H, iterator: JsValue) {
    if !iterator.is_object() {
        return;
    }
    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
    let return_si = vm.perm_intern("return");
    if let Ok(ret) = vm.ordinary_get(iter_obj, return_si, iterator) {
        if is_callable(ret) {
            // return() 的抛错被忽略，其值不得外泄进槽覆盖在途异常。
            let saved_uncaught = vm.take_uncaught_value();
            let _ = vm.call_function_sync(ret, iterator, &[]);
            vm.restore_uncaught_value(saved_uncaught);
        }
    }
}

/// 遍历可迭代值，把每个元素交给 `on_elem`。
///
/// # 步骤
/// 1. 经迭代协议取迭代器包装器，逐次 `next` 读取 `{done, value}`。
/// 2. 每个元素调用 `on_elem`；元素读取或回调抛错时先 IteratorClose 再透传原异常。
///
/// # 返回值
/// - `Ok(())`：迭代完成；
/// - `Err`：迭代或回调抛出的原异常值（任意类型）。
pub fn iterate_elements<H: VmHost, F>(vm: &mut H, iterable: JsValue, mut on_elem: F) -> Result<(), JsValue>
where
    F: FnMut(&mut H, JsValue) -> Result<(), JsValue>,
{
    let iterator = make_iterator_for_value(vm, iterable)?;
    iterate_iterator(vm, iterator, &mut on_elem)
}

/// 遍历一个已取得的迭代器对象（带 `next`），把每个元素交给 `on_elem`。
/// 与 [`iterate_elements`] 的差异：入参是迭代器本身而非可迭代值，
/// 用于 Set 方法从 SetRecord 的 `keys` 方法返回值继续取元素。
///
/// # 副作用
/// 迭代或回调抛错时调用迭代器的 `return()`（IteratorClose）后透传原异常。
pub fn iterate_iterator<H: VmHost, F>(vm: &mut H, iterator: JsValue, mut on_elem: F) -> Result<(), JsValue>
where
    F: FnMut(&mut H, JsValue) -> Result<(), JsValue>,
{
    let next_si = vm.perm_intern("next");
    let done_si = vm.perm_intern("done");
    let value_si = vm.perm_intern("value");
    let run: Result<(), JsValue> = (|| {
        loop {
            let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
            let next_fn = vm.ordinary_get(iter_obj, next_si, iterator).map_err(|e| engine_error(vm, &e))?;
            let result = vm
                .call_function_sync(next_fn, iterator, &[])
                .map_err(|e| engine_error(vm, &e))?;
            if !result.is_object() {
                return Err(crate::error::create_type_error(vm, "iterator result is not an object"));
            }
            let result_obj = unsafe { &*result.as_js_object_ptr() };
            let done = vm.ordinary_get(result_obj, done_si, result).map_err(|e| engine_error(vm, &e))?;
            if oxide_runtime_api::to_boolean(done) {
                break;
            }
            let elem = vm
                .ordinary_get(result_obj, value_si, result)
                .map_err(|e| engine_error(vm, &e))?;
            on_elem(vm, elem)?;
        }
        Ok(())
    })();
    if run.is_err() {
        close_iterator(vm, iterator);
    }
    run
}

fn is_map_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_map()
}

fn is_set_value(value: JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    let ptr = value.as_js_object_ptr();
    !ptr.is_null() && unsafe { &*ptr }.is_set()
}

fn make_map_set_pair<H: VmHost>(vm: &mut H, a: JsValue, b: JsValue) -> JsValue {
    let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
    let pair = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), 2));
    // SAFETY: pair 是刚经统一入口分配的数组 JsObject。
    unsafe {
        (*pair).set_prop_at(0, a);
        (*pair).set_prop_at(1, b);
        (*pair).set_prop_count(2);
    }
    JsValue::from_js_object(pair)
}

/// Map/Set 迭代器模式编码：创建时存 wrapper `__mode__` 槽，%MapIteratorPrototype%/
/// %SetIteratorPrototype% 的单一 next 读槽后按模式推进（值含家族区分，
/// 分发不依赖具体原型）。判别值稳定，改动须同步 `from_i32`。
#[derive(Clone, Copy)]
#[repr(i32)]
pub(crate) enum MapSetMode {
    MapEntries = 0,
    MapValues = 1,
    MapKeys = 2,
    SetValues = 3,
    SetEntries = 4,
}

impl MapSetMode {
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::MapEntries,
            1 => Self::MapValues,
            2 => Self::MapKeys,
            3 => Self::SetValues,
            4 => Self::SetEntries,
            _ => Self::MapEntries,
        }
    }
}

/// Map/Set 迭代步进核心：按模式产出元素值、done 标志与新下标。
///
/// 条目存放在集合 native-data 槽中（Map 为槽表：空槽跳过、下标口径为"下一待检槽"；
/// Set 为插入序表）；(a, b) 对在任意分配之前拷出，使对 native 集合的借用不会
/// 跨越 `vm` 调用保持。下标由调用方传入并回传（快路径存条目、慢路径写回
/// `__index__` 槽），本核心不触碰包装器属性槽。
fn map_set_step_core<H: VmHost>(vm: &mut H, inner: JsValue, cursor: usize, mode: MapSetMode) -> (JsValue, bool, usize) {
    let index = cursor;
    let is_map = matches!(mode, MapSetMode::MapEntries | MapSetMode::MapValues | MapSetMode::MapKeys);
    let (entry, next_index): (Option<(JsValue, JsValue)>, usize) = unsafe {
        let obj_ptr = inner.as_js_object_ptr();
        if obj_ptr.is_null() {
            (None, 0)
        } else if is_map {
            let p = (*obj_ptr).native_data() as *const crate::map::MapInner;
            if p.is_null() {
                (None, 0)
            } else {
                // 从当前下标起扫首个活槽；命中的写回值为"下一待检槽"，
                // 耗尽（含 done 哨兵后的下标）恒 done。
                let mut found = None;
                let mut i = index;
                while i < (*p).slot_count() {
                    match (*p).slot(i) {
                        Some((key, value)) => {
                            found = Some((key.0, value));
                            break;
                        }
                        None => i += 1,
                    }
                }
                match found {
                    Some(e) => (Some(e), i + 1),
                    None => (None, 0),
                }
            }
        } else {
            let p = (*obj_ptr).native_data() as *const crate::set::SetInner;
            if p.is_null() {
                (None, 0)
            } else {
                match (*p).get_index(index) {
                    Some(elem) => (Some((elem.0, elem.0)), index + 1),
                    None => (None, 0),
                }
            }
        }
    };

    match entry {
        Some((a, b)) => {
            let value = match mode {
                MapSetMode::MapEntries | MapSetMode::SetEntries => make_map_set_pair(vm, a, b),
                MapSetMode::MapValues => b,
                MapSetMode::MapKeys | MapSetMode::SetValues => a,
            };
            (value, false, next_index)
        }
        None => {
            // 迭代器已耗尽：把下标推进到永不匹配的哨兵值，使后续 next() 恒返回 done，
            // 即使集合之后又新增元素也不会"复活"。
            (JsValue::undefined(), true, i32::MAX as usize)
        }
    }
}

/// 把 Map/Set 迭代器包装器推进一步，按指定模式产出 `{value, done}` 结果：
/// 从 `__index__` 槽读游标、步进核心产出元素、新游标写回槽。
fn map_set_step<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32, mode: MapSetMode,
) -> JsValue {
    let cursor = current_index(vm, wrapper, index_si);
    let (value, done, next_cursor) = map_set_step_core(vm, inner, cursor, mode);
    vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(next_cursor as i32));
    make_iter_result(vm, value, done)
}

/// Map/Set 迭代器 `next`：模式从 wrapper `__mode__` 槽读取后按模式推进。
///
/// %MapIteratorPrototype%/%SetIteratorPrototype% 绑定同一实现——家族与模式
/// 在创建时编码进 `__mode__` 槽，`next` 挂在原型上（wrapper 不设 own next）。
pub fn map_set_iterator_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "iterator next called on non-object"));
    }
    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.perm_intern(INNER_PROP);
    let index_si = vm.perm_intern(INDEX_PROP);
    let mode_si = vm.perm_intern(MODE_PROP);
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "iterator has no inner collection")),
    };
    let mode = vm
        .ordinary_get(wrapper, mode_si, this_val)
        .ok()
        .and_then(|v| if v.is_int() { Some(MapSetMode::from_i32(v.as_int())) } else { None })
        .unwrap_or(MapSetMode::MapEntries);
    NativeResult::Ok(map_set_step(vm, wrapper, inner, index_si, mode))
}

/// 构造 Map/Set 迭代器包装器：`__inner__`/`__index__`/`__mode__` 三槽记录状态。
///
/// `next` 不挂实例 own——由 %MapIteratorPrototype%/%SetIteratorPrototype% 上的
/// 单一 next（读 `__mode__` 分发）经原型链提供，符合规范原型形状。
pub(crate) fn make_collection_iterator<H: VmHost>(
    vm: &mut H, inner: JsValue, proto_val: JsValue, mode: MapSetMode,
) -> JsValue {
    let proto_ptr = if proto_val.is_object() {
        proto_val.as_js_object_ptr()
    } else {
        std::ptr::null_mut()
    };
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
    let inner_si = vm.perm_intern(INNER_PROP);
    let index_si = vm.perm_intern(INDEX_PROP);
    let mode_si = vm.perm_intern(MODE_PROP);
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, mode_si, JsValue::int(mode as i32));
    JsValue::from_js_object(wrapper)
}
