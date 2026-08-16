use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{make_int_key, make_well_known_symbol_key};
use oxide_types::value::JsValue;

use oxide_runtime_api::{to_object, NativeResult, VmHost};

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
/// 3. newTarget 槽（regs[255]）非对象 → TypeError（`Iterator.call(x)` 顶层调用，
///    顶层 newTarget 初始化为 undefined）。
/// 4. 否则返回 undefined（subclass `super()`：调用方随后按 new.target.prototype
///    设置实例原型）。
///
/// # 注意事项
/// 类构造器内调用 `Iterator.call(x)` 时 regs[255] 为类对象会被放行（规范外罕见
/// 场景，与 `Symbol` 构造器同款已知边界）。
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
                let home = vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject;
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
/// 动态查 global（shape 槽查找）而非缓存指针：dirty reset 重建 global 时会换新的
/// `Iterator` 函数对象，缓存旧指针会指向已释放对象。lookup 失败（极端：global 无
/// `Iterator`）返回 undefined，不 panic。
pub fn iterator_constructor_getter<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let global = vm.session().global_object();
    let si = vm.kernel_core().perm_interner().intern("Iterator").0;
    let Some(pos) = vm.kernel_core().shape_forge().lookup_position(global.shape_id(), si) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    NativeResult::Ok(global.get_prop_at(pos))
}

/// `%IteratorPrototype%` 上 `Symbol.toStringTag` 访问器的 getter：返回 `"Iterator"`。
pub fn iterator_to_string_tag_getter<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let sf = vm.kernel_core().perm_interner().as_ref();
    NativeResult::Ok(JsValue::perm_string(sf.string_ptr(sf.intern("Iterator").0)))
}

/// SetterThatIgnoresPrototypeProperties 的共享实现：`%IteratorPrototype%` 的
/// `constructor` 与 `Symbol.toStringTag` 访问器共用同一语义，仅属性键不同。
///
/// # 步骤
/// 1. `this` 非对象 → TypeError（原始值直接抛，不建 own 属性）。
/// 2. `this` 为 `%IteratorPrototype%`（home 对象）→ TypeError（模拟对 home 不可写
///    数据属性的严格模式赋值）。
/// 3. `this` 无 own 指定键属性 → CreateDataPropertyOrThrow（写全可写可枚举）。
/// 4. 有 own 属性 → 普通 Set（继承访问器时在此被调用的场景）。
fn iterator_setter_ignore_proto_props<H: VmHost>(vm: &mut H, args: &[u8], key: u32) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator property setter called on non-object"));
    }
    let home = vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject;
    if std::ptr::eq(this_val.as_js_object_ptr(), home) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Cannot assign to read only property of Iterator prototype",
        ));
    }
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let this_obj = unsafe { &mut *this_val.as_js_object_ptr() };
    if vm
        .kernel_core()
        .shape_forge()
        .lookup_position(this_obj.shape_id(), key)
        .is_none()
    {
        match vm.define_data_property(this_obj, key, val, PropAttributes::new(true, true, true)) {
            Ok(()) => NativeResult::Ok(JsValue::undefined()),
            Err(err) => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        }
    } else {
        match vm.ordinary_set(this_obj, key, val, this_val) {
            Ok(()) => NativeResult::Ok(JsValue::undefined()),
            Err(err) => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        }
    }
}

/// `constructor` 访问器的 setter（键 = "constructor"）。
pub fn iterator_constructor_setter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let key = vm.kernel_core().perm_interner().intern("constructor").0;
    iterator_setter_ignore_proto_props(vm, args, key)
}

/// `Symbol.toStringTag` 访问器的 setter（键 = well-known symbol 9）。
pub fn iterator_to_string_tag_setter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    iterator_setter_ignore_proto_props(vm, args, make_well_known_symbol_key(9))
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
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
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

/// 终端方法的统一前置校验：`this` 非对象 → TypeError；回调不可调用 → 关底层后抛
/// TypeError（2024 规范更新：参数校验失败也执行 IteratorClose，且此时不读 next）。
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

/// GetIteratorDirect：读 `next` 一次并缓存，返回 `(iterated, next)` 对。
///
/// # 边界与前提
/// - `this_val` 必须是对象（调用方已校验）。
/// - `next` getter 抛错时透传原异常值。
pub(crate) fn get_iterator_direct<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<(JsValue, JsValue), JsValue> {
    let iter_obj = unsafe { &*this_val.as_js_object_ptr() };
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let next = vm.ordinary_get(iter_obj, next_si, this_val).map_err(|e| engine_error(vm, &e))?;
    Ok((this_val, next))
}

/// IteratorStepValue：调缓存 next 取一步，读结果对象 `done`/`value`。
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
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let done = vm.ordinary_get(result_obj, done_si, result).map_err(|e| engine_error(vm, &e))?;
    if oxide_runtime_api::to_boolean(done) {
        return Ok(None);
    }
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let value = vm
        .ordinary_get(result_obj, value_si, result)
        .map_err(|e| engine_error(vm, &e))?;
    Ok(Some(value))
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
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
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
}

impl IteratorHelperKind {
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Map,
            1 => Self::Filter,
            2 => Self::Take,
            3 => Self::Drop,
            4 => Self::FlatMap,
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

/// ToIntegerOrInfinity 近似：ToNumber 后向零截断，保留 ±∞；NaN 判定由调用方
/// 在截断前语义等价地做（`trunc(NaN)` 仍为 NaN）。ToNumber 抛错透传原异常值。
fn to_integer_or_infinity<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let num = vm.coerce_number_bounded(value).map_err(|e| engine_error(vm, &e))?;
    Ok(num.trunc())
}

/// take/drop 的共享前置：`this` 非对象 → TypeError；limit 经 ToIntegerOrInfinity
/// 校验（NaN/负值 → RangeError，ToNumber 抛错透传），校验失败均先关底层
/// （2024 规范更新：参数校验失败也执行 IteratorClose，且不读 next）。
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

/// GetIteratorFlattenable（reject-primitives）：把 flatMap 的 mapper 返回值解析为
/// 内层迭代器记录（对象 + 缓存 next）。
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
    let sym_iter_si = make_well_known_symbol_key(0);
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
    let helper_proto = vm.session().builtin_world().iterator_helper_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(helper_proto)));
    let wrapper_obj = unsafe { &mut *wrapper };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let next_si = vm.kernel_core().perm_interner().intern(NEXT_CACHE_PROP).0;
    let kind_si = vm.kernel_core().perm_interner().intern(KIND_PROP).0;
    let state_si = vm.kernel_core().perm_interner().intern(STATE_PROP).0;
    let counter_si = vm.kernel_core().perm_interner().intern(COUNTER_PROP).0;
    let callback_si = vm.kernel_core().perm_interner().intern(CALLBACK_PROP).0;
    let inner_iter_si = vm.kernel_core().perm_interner().intern(INNER_ITER_PROP).0;
    let inner_next_si = vm.kernel_core().perm_interner().intern(INNER_NEXT_PROP).0;
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

/// 校验 wrapper 对象形态：`this` 非对象或非 helper wrapper（无 `__inner__` 槽）
/// → TypeError，等价规范的 RequireInternalSlot 检查。
///
/// # 返回
/// - `Ok(inner)`：底层迭代器值（wrapper `__inner__` 槽）。
fn validate_helper_this<H: VmHost>(vm: &mut H, this_val: JsValue, method: &str) -> Result<JsValue, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, &format!("{method} called on non-object")));
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let inner = vm.ordinary_get(obj, inner_si, this_val).map_err(|e| engine_error(vm, &e))?;
    if !inner.is_object() {
        return Err(crate::error::create_type_error(vm, &format!("{method} called on non-iterator-helper")));
    }
    Ok(inner)
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

/// CreateArrayFromList：按元素列表构造普通数组（Array.prototype 为原型）。
fn make_array_from_list<H: VmHost>(vm: &mut H, items: &[JsValue]) -> JsValue {
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        items.len().min(oxide_types::object::MAX_DENSE_PROPS),
        vm.epoch().bump(),
    ));
    // 逐元素写入数组元素区，最后统一 set_prop_count（new_array 预分配不足时自动扩容）。
    for (i, item) in items.iter().enumerate() {
        // SAFETY: arr 是当前 epoch 新分配数组对象，元素区已就绪。
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

/// `%IteratorHelperPrototype%.next`：读 wrapper `__kind__` 分发 5 种推进循环，
/// 统一完成态短路与重入守卫。
///
/// # 步骤
/// 1. `__state__`=2（完成）→ 直接返回 `{undefined, true}`（return 不再转发）。
/// 2. `__state__`=1（重入）→ TypeError（等价 GeneratorValidate 的 executing 检查）。
/// 3. 置 1 后按 kind 推进：产出值 → 置回 0 返回 `{value, false}`；耗尽/错误 → 置 2
///    后返回 done / 透传异常（规范生成器 body 终止即 completed，后续调用短路）。
pub fn iterator_helper_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = match validate_helper_this(vm, this_val, "Iterator helper next") {
        Ok(v) => v,
        Err(v) => return NativeResult::Err(v),
    };
    let helper = unsafe { &mut *this_val.as_js_object_ptr() };
    let state_si = vm.kernel_core().perm_interner().intern(STATE_PROP).0;
    let state = read_slot_int(vm, helper, state_si, 2);
    if state == 2 {
        return NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true));
    }
    if state == 1 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator helper is already executing"));
    }
    let kind_si = vm.kernel_core().perm_interner().intern(KIND_PROP).0;
    let kind = IteratorHelperKind::from_i32(read_slot_int(vm, helper, kind_si, 0));
    vm.set_or_create_prop_value(helper, state_si, JsValue::int(1));
    let next_si = vm.kernel_core().perm_interner().intern(NEXT_CACHE_PROP).0;
    let next = vm.ordinary_get(helper, next_si, this_val).unwrap_or(JsValue::undefined());
    let step = match kind {
        IteratorHelperKind::Map => helper_step_map(vm, this_val, helper, inner, next),
        IteratorHelperKind::Filter => helper_step_filter(vm, this_val, helper, inner, next),
        IteratorHelperKind::Take => helper_step_take(vm, helper, inner, next),
        IteratorHelperKind::Drop => helper_step_drop(vm, helper, inner, next),
        IteratorHelperKind::FlatMap => helper_step_flat_map(vm, this_val, helper, inner, next),
    };
    match step {
        Ok(Some(value)) => {
            vm.set_or_create_prop_value(helper, state_si, JsValue::int(0));
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
    let inner = match validate_helper_this(vm, this_val, "Iterator helper return") {
        Ok(v) => v,
        Err(v) => return NativeResult::Err(v),
    };
    let helper = unsafe { &mut *this_val.as_js_object_ptr() };
    let state_si = vm.kernel_core().perm_interner().intern(STATE_PROP).0;
    let state = read_slot_int(vm, helper, state_si, 2);
    if state == 2 {
        return NativeResult::Ok(make_iter_result(vm, JsValue::undefined(), true));
    }
    if state == 1 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator helper is already executing"));
    }
    vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
    let kind_si = vm.kernel_core().perm_interner().intern(KIND_PROP).0;
    if IteratorHelperKind::from_i32(read_slot_int(vm, helper, kind_si, 0)) == IteratorHelperKind::FlatMap {
        let inner_iter_si = vm.kernel_core().perm_interner().intern(INNER_ITER_PROP).0;
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
    let inner = match validate_helper_this(vm, this_val, "Iterator helper throw") {
        Ok(v) => v,
        Err(v) => return NativeResult::Err(v),
    };
    let helper = unsafe { &mut *this_val.as_js_object_ptr() };
    let state_si = vm.kernel_core().perm_interner().intern(STATE_PROP).0;
    let state = read_slot_int(vm, helper, state_si, 2);
    if state == 2 {
        return NativeResult::Err(value);
    }
    if state == 1 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator helper is already executing"));
    }
    vm.set_or_create_prop_value(helper, state_si, JsValue::int(2));
    let kind_si = vm.kernel_core().perm_interner().intern(KIND_PROP).0;
    if IteratorHelperKind::from_i32(read_slot_int(vm, helper, kind_si, 0)) == IteratorHelperKind::FlatMap {
        let inner_iter_si = vm.kernel_core().perm_interner().intern(INNER_ITER_PROP).0;
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
    let counter_si = vm.kernel_core().perm_interner().intern(COUNTER_PROP).0;
    let callback_si = vm.kernel_core().perm_interner().intern(CALLBACK_PROP).0;
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
    let counter_si = vm.kernel_core().perm_interner().intern(COUNTER_PROP).0;
    let callback_si = vm.kernel_core().perm_interner().intern(CALLBACK_PROP).0;
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
    let counter_si = vm.kernel_core().perm_interner().intern(COUNTER_PROP).0;
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
    let counter_si = vm.kernel_core().perm_interner().intern(COUNTER_PROP).0;
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
    let counter_si = vm.kernel_core().perm_interner().intern(COUNTER_PROP).0;
    let callback_si = vm.kernel_core().perm_interner().intern(CALLBACK_PROP).0;
    let inner_iter_si = vm.kernel_core().perm_interner().intern(INNER_ITER_PROP).0;
    let inner_next_si = vm.kernel_core().perm_interner().intern(INNER_NEXT_PROP).0;
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

/// 同 [`make_iterator_for_value`]，但包装器原型指向调用方指定的原型
/// （`String.prototype[@@iterator]` 用 %StringIteratorPrototype%）。
pub(crate) fn make_iterator_for_value_with_proto<H: VmHost>(
    vm: &mut H, value: JsValue, wrapper_proto: *mut JsObject,
) -> Result<JsValue, JsValue> {
    match try_make_iterator_inner_proto(vm, value, true, Some(wrapper_proto)) {
        Ok(Some(iterator)) => Ok(iterator),
        Ok(None) => Err(crate::error::create_type_error(vm, "value is not iterable")),
        Err(err) => Err(err),
    }
}

/// 尝试创建迭代器包装对象，把"不可迭代"与"真异常"区分返回。
///
/// # 步骤
/// 1. 经迭代协议取内层迭代器（String/Array/Map/Set 直接作为内层，其余对象调用
///    `@@iterator` 或回退可调用的 `next`）。
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
    let inner = match get_iterator(vm, value) {
        Ok(Some(inner)) => inner,
        Ok(None) => return Ok(None),
        Err(err) => return Err(err),
    };
    // 通用包装器默认挂 %IteratorPrototype%（经原型链获得 @@iterator 返回自身）；
    // 调用方指定原型时优先（如 String 迭代器的 %StringIteratorPrototype%）。
    let iterator_proto = match wrapper_proto {
        Some(proto) => proto,
        None => vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject,
    };
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(iterator_proto)));

    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));

    let next_fn = make_native_function(vm, "next", iterator_wrapper_next::<H> as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next_fn);

    // for-of/解构的 IteratorClose 需要 return 方法：条件暴露（内层有可调用 return 时）。
    // `yield*` 委托（bind_return=false）不绑定，转发时对内层延迟 GetMethod。
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
    if bind_return && inner.is_object() {
        let inner_obj = unsafe { &*inner.as_js_object_ptr() };
        if let Ok(return_fn) = vm.ordinary_get(inner_obj, return_si, inner) {
            if is_callable(return_fn) {
                let wrapper_return = make_native_function(vm, "return", iterator_wrapper_return::<H> as *const (), 0);
                vm.set_or_create_prop_value(wrapper_obj, return_si, wrapper_return);
            }
        }
    }

    Ok(Some(JsValue::from_js_object(wrapper)))
}

fn iterator_wrapper_return<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let wrapper = unsafe { &*this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if inner.is_object() => inner,
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    // 延迟 GetMethod：内层无 return 方法时返回 undefined（IteratorClose 跳过）。
    let inner_obj = unsafe { &*inner.as_js_object_ptr() };
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
    let return_fn = match vm.ordinary_get(inner_obj, return_si, inner) {
        Ok(f) if is_callable(f) => f,
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    // 转发调用实参（`yield*` 委托的 return(v) 语义），缺省为 undefined。
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.call_function_sync(return_fn, inner, &[arg]) {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => match vm.take_uncaught_value() {
            Some(original) => NativeResult::Err(original),
            None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
        },
    }
}

/// 迭代器包装器的 `next` 方法：对 Array/String/Map/Set 直接按索引取值，
/// 其它对象委托其自身 `next`；底层抛出异常时透传原始值（不做二次包装）。
pub fn iterator_wrapper_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Iterator wrapper next called on non-object"));
    }

    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
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
        let inner_obj = unsafe { &*inner.as_js_object_ptr() };
        let next_si = vm.kernel_core().perm_interner().intern("next").0;
        let next = match vm.ordinary_get(inner_obj, next_si, inner) {
            Ok(next) => next,
            Err(err) => {
                // GetMethod 的 next getter 抛错：透传原异常，不重新包装成 TypeError。
                return match vm.take_uncaught_value() {
                    Some(original) => NativeResult::Err(original),
                    None => NativeResult::Err(crate::error::create_type_error(vm, &err)),
                };
            }
        };
        // 转发调用实参（`yield*` 委托的 next(v) 语义），缺省为 undefined。
        let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
        return match vm.call_function_sync(next, inner, &[arg]) {
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

/// 判断 value 是否可迭代，只读取 `@@iterator` 方法而不调用它（GetMethod 语义）。
///
/// 与 [`get_iterator`] 的判定一致：内建集合（String/Array/TypedArray/Map/Set）恒可迭代；
/// 其它对象读取 `@@iterator`，可调用即视为可迭代，否则回退到自身可调用的 `next`。
/// `@@iterator` getter 抛错时透传 `Err`。
pub(crate) fn peek_iterator_method<H: VmHost>(vm: &mut H, value: JsValue) -> Result<bool, JsValue> {
    if value.is_string()
        || is_array_value(value)
        || is_typed_array_value(value)
        || is_map_value(value)
        || is_set_value(value)
    {
        return Ok(true);
    }
    if value.is_object() {
        let obj = unsafe { &*value.as_js_object_ptr() };
        let sym_iter_si = make_well_known_symbol_key(0);
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
        // 鸭子回退：对象自身有可调用 next。
        let next_si = vm.kernel_core().perm_interner().intern("next").0;
        if let Ok(next) = vm.ordinary_get(obj, next_si, value) {
            if is_callable(next) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn get_iterator<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<JsValue>, JsValue> {
    if value.is_string()
        || is_array_value(value)
        || is_typed_array_value(value)
        || is_map_value(value)
        || is_set_value(value)
    {
        return Ok(Some(value));
    }

    // 非字符串 primitive（boolean/number/symbol/bigint）：GetIterator 先 ToObject，
    // 再走迭代协议（如 `yield* true` 委托 Boolean.prototype[Symbol.iterator]）。
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
    // 迭代协议：GetIterator 先取 value[Symbol.iterator] 并调用。
    let sym_iter_si = make_well_known_symbol_key(0);
    let method = match vm.ordinary_get(obj, sym_iter_si, obj_value) {
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
        let iterator = match vm.call_function_sync(method, obj_value, &[]) {
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
        return Ok(Some(iterator));
    }
    // 鸭子回退：对象自身有可调用 next（Map/Set 迭代器包装等既有用法）。
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    if let Ok(next) = vm.ordinary_get(obj, next_si, obj_value) {
        if is_callable(next) {
            return Ok(Some(obj_value));
        }
    }

    Ok(None)
}

fn next_array_like<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32,
) -> Result<Option<JsValue>, JsValue> {
    if is_array_value(inner) {
        let index = current_index(vm, wrapper, index_si);
        let arr = unsafe { &*inner.as_js_object_ptr() };
        if index < arr.prop_count() as usize {
            // 数组元素读取走 GetValue：普通数据属性返回槽值，访问器属性
            // （defineProperty getter）触发 getter 并透传异常。整数键免 intern。
            let key_si = make_int_key(index as u32);
            let value = match vm.ordinary_get(arr, key_si, inner) {
                Ok(v) => v,
                Err(err) => {
                    let exc = vm.take_uncaught_value().unwrap_or_else(|| crate::error::create_error(vm, &err));
                    return Err(exc);
                }
            };
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            return Ok(Some(make_iter_result(vm, value, false)));
        }
        return Ok(Some(make_iter_result(vm, JsValue::undefined(), true)));
    }

    if inner.is_string() {
        // index 槽在字符串分支存字节游标（每步推进一个 char 的 UTF-8 宽度）而非
        // 元素序号：该槽只被 next 内部读写、无外部消费者，包装器创建后类型固定
        // 不跨类型复用，故可安全借用语义（字符串=字节偏移）。
        let byteoff = current_index(vm, wrapper, index_si);
        // 源串裸指针借用压缩到单个表达式：ch 是 Copy 的 char，不携带借用，
        // 之后对 VM 状态的可变访问不再与源串借用共存。
        let ch = unsafe { &*inner.as_string_ptr() }.as_str()[byteoff..].chars().next();
        if let Some(ch) = ch {
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((byteoff + ch.len_utf8()) as i32));
            let value = match vm.single_char(ch) {
                Some(v) => v,
                None => vm.new_string(&ch.to_string()),
            };
            return Ok(Some(make_iter_result(vm, value, false)));
        }
        return Ok(Some(make_iter_result(vm, JsValue::undefined(), true)));
    }

    if is_typed_array_value(inner) {
        let index = current_index(vm, wrapper, index_si);
        let length_si = vm.kernel_core().perm_interner().intern("length").0;
        let obj = unsafe { &*inner.as_js_object_ptr() };
        let len = vm
            .ordinary_get(obj, length_si, inner)
            .map(|v| if v.is_int() { v.as_int().max(0) as usize } else { 0 })
            .unwrap_or(0);
        if index < len {
            let value = match crate::typed_array::typed_array_element_get(vm, obj, index as u32) {
                Ok(v) => v,
                Err(e) => return Err(crate::error::create_type_error(vm, &e)),
            };
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            return Ok(Some(make_iter_result(vm, value, false)));
        }
        return Ok(Some(make_iter_result(vm, JsValue::undefined(), true)));
    }

    // for-of 循环默认迭代：Map 产出 [key, value] 对，Set 产出值。
    if is_map_value(inner) {
        return Ok(Some(map_set_step(vm, wrapper, inner, index_si, MapSetMode::MapEntries)));
    }
    if is_set_value(inner) {
        return Ok(Some(map_set_step(vm, wrapper, inner, index_si, MapSetMode::SetValues)));
    }

    Ok(None)
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
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let obj = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let obj_ref = unsafe { &mut *obj };
    vm.set_or_create_prop_value(obj_ref, value_si, value);
    vm.set_or_create_prop_value(obj_ref, done_si, JsValue::bool(done));
    JsValue::from_js_object(obj)
}

pub(crate) fn make_native_function<H: VmHost>(vm: &mut H, name: &str, native_fn: *const (), arg_count: u8) -> JsValue {
    let function_proto = vm.session().builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
    func.set_function(true);
    // SAFETY: native_fn 来自 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(native_fn) }));
    func.set_native_arg_count(arg_count);
    let func = vm.alloc_object(func);
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
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
        .unwrap_or_else(|| crate::error::create_type_error(vm, err))
}

/// IteratorClose：异常退出时调用迭代器包装器的 `return()`（转发给内层迭代器），
/// 丢弃 return 自身抛出的错误，保留在途异常。
pub(crate) fn close_iterator<H: VmHost>(vm: &mut H, iterator: JsValue) {
    if !iterator.is_object() {
        return;
    }
    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
    let return_si = vm.kernel_core().perm_interner().intern("return").0;
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
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
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
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let pair = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        2,
        vm.epoch().bump(),
    ));
    // SAFETY: pair 是当前 epoch 内新分配的数组 JsObject。
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

/// 把 Map/Set 迭代器包装器推进一步，按指定模式产出 `{value, done}` 结果。
/// 条目存放在集合 native-data 槽的 indexmap 中；(a, b) 对在任意分配之前拷出，
/// 使对 native 集合的借用不会跨越 `vm` 调用保持。
fn map_set_step<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32, mode: MapSetMode,
) -> JsValue {
    let index = current_index(vm, wrapper, index_si);
    let is_map = matches!(mode, MapSetMode::MapEntries | MapSetMode::MapValues | MapSetMode::MapKeys);
    let entry: Option<(JsValue, JsValue)> = unsafe {
        let obj_ptr = inner.as_js_object_ptr();
        if obj_ptr.is_null() {
            None
        } else if is_map {
            let p = (*obj_ptr).native_data() as *const crate::map::MapInner;
            if p.is_null() {
                None
            } else {
                (*p).get_index(index).map(|(key, value)| (key.0, *value))
            }
        } else {
            let p = (*obj_ptr).native_data() as *const crate::set::SetInner;
            if p.is_null() {
                None
            } else {
                (*p).get_index(index).map(|elem| (elem.0, elem.0))
            }
        }
    };

    match entry {
        Some((a, b)) => {
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            let value = match mode {
                MapSetMode::MapEntries | MapSetMode::SetEntries => make_map_set_pair(vm, a, b),
                MapSetMode::MapValues => b,
                MapSetMode::MapKeys | MapSetMode::SetValues => a,
            };
            make_iter_result(vm, value, false)
        }
        None => {
            // 迭代器已耗尽：把下标推进到永不匹配的哨兵值，使后续 next() 恒返回 done，
            // 即使集合之后又新增元素也不会"复活"。
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(i32::MAX));
            make_iter_result(vm, JsValue::undefined(), true)
        }
    }
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
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let mode_si = vm.kernel_core().perm_interner().intern(MODE_PROP).0;
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
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let mode_si = vm.kernel_core().perm_interner().intern(MODE_PROP).0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, mode_si, JsValue::int(mode as i32));
    JsValue::from_js_object(wrapper)
}
