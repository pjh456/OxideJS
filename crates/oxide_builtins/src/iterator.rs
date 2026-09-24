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
/// 动态查 global（shape 槽查找）而非缓存指针：global 整体重建（dirty reset）会
/// 重建内置对象族，换新的 `Iterator` 函数对象；缓存旧指针会指向已释放对象。
/// lookup 失败（极端：global 无 `Iterator`）返回 undefined，不 panic。
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
        match vm.ordinary_set(this_obj, key, val, this_val, true) {
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

/// `Symbol.toStringTag` 访问器的 setter（键为 `@@toStringTag`）。
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
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
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
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(helper_proto)));
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

/// 按元素列表构造普通数组，以 Array.prototype 为原型（对应规范 CreateArrayFromList）。
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
/// 3. 挂 `next` 方法；`bind_return` 且内层有可调用 `return` 时条件暴露 `return`。
///
/// # 边界
/// - `bind_return` 控制是否暴露 `return` 方法（for-of/解构的 IteratorClose 需要，
///   `yield*` 委托不需要且须避免创建时访问内层 return getter）。
/// - 本函数不解析 `@@iterator`：`get_iterator` 派生内层，或默认迭代器函数直接以
///   已 ToString 的串为内层构造（其语义只由 `this` 决定，与属性表状态解耦）。
pub(crate) fn build_iterator_wrapper<H: VmHost>(
    vm: &mut H, inner: JsValue, cached_next: Option<JsValue>, bind_return: bool, wrapper_proto: Option<*mut JsObject>,
) -> JsValue {
    let iterator_proto = match wrapper_proto {
        Some(proto) => proto,
        None => vm.session().builtin_world().iterator_proto.as_ptr() as *mut JsObject,
    };
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(iterator_proto)));

    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));

    // 鸭子回退包装器：from() 时读到的 next 闭包缓存进 __next__ 槽（与 helper 包装器
    // "只读一次"同模式），消费期直读槽不重触发 getter；非鸭子路径无槽，wrapper next
    // 回退重读内层 next（既有行为）。
    if let Some(next_fn) = cached_next {
        let next_cache_si = vm.kernel_core().perm_interner().intern(NEXT_CACHE_PROP).0;
        vm.set_or_create_prop_value(wrapper_obj, next_cache_si, next_fn);
    }

    let wrapper_next = make_native_function(vm, "next", iterator_wrapper_next::<H> as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, wrapper_next);

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

    JsValue::from_js_object(wrapper)
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
/// 其它对象优先用 `__next__` 槽（鸭子回退包装器创建时只读一次缓存的闭包），
/// 槽缺失时委托内层自身 `next`；底层抛出异常时透传原始值（不做二次包装）。
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
        // 鸭子回退包装器优先读 `__next__` 槽（from() 时只读一次的缓存闭包，
        // 消费期不重触发 getter）；槽缺失或不可调用时回退重读内层 next
        // （非鸭子路径既有行为）。
        let next_cache_si = vm.kernel_core().perm_interner().intern(NEXT_CACHE_PROP).0;
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
            let next_si = vm.kernel_core().perm_interner().intern("next").0;
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
        let proto_ptr = vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject;
        // SAFETY: string_proto 是 BuiltinWorld 长驻原型对象，进程内有效且不被 GC 搬移。
        let proto_obj = unsafe { &*proto_ptr };
        let sym_iter_si = make_well_known_symbol_key(0);
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
        // GetMethod 步 4：非空不可调用方法是 TypeError（不落入鸭子回退与
        // array-like 臂）；null/undefined 才放行到回退判定。
        if !method.is_null() && !method.is_undefined() {
            return Err(crate::error::create_type_error(vm, "value is not iterable"));
        }
        // 鸭子回退：@@iterator 解析为 null/undefined 且对象自身有可调用 next。
        // 装箱串不套用：get_iterator 的 String 臂无 duck-next 回退，规范也只看
        // @@iterator（Array.from/%TypedArray%.from 应落 array-like）。
        if !obj.is_string_obj() {
            let next_si = vm.kernel_core().perm_interner().intern("next").0;
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
    let world = vm.session().builtin_world();
    let (anchor_ptr, anchor_name) = if is_array_value(value) {
        (world.array_proto.as_ptr() as *mut JsObject, "values")
    } else if is_typed_array_value(value) {
        (world.typed_array_proto.as_ptr() as *mut JsObject, "values")
    } else if is_map_value(value) {
        (world.map_proto.as_ptr() as *mut JsObject, "entries")
    } else {
        (world.set_proto.as_ptr() as *mut JsObject, "values")
    };
    let anchor_obj = unsafe { &*anchor_ptr };
    let anchor_si = vm.kernel_core().perm_interner().intern(anchor_name).0;
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
            vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject
        };
        let read_obj = unsafe { &*read_ptr };
        let sym_iter_si = make_well_known_symbol_key(0);
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
        let default_ptr = vm.session().builtin_world().string_default_iterator.get();
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
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
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
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_from_text(vm, &err));
                    return Err(exc);
                }
            };
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            return Ok(Some(make_iter_result(vm, value, false)));
        }
        return Ok(Some(make_iter_result(vm, JsValue::undefined(), true)));
    }

    if inner.is_string() {
        // index 槽在字符串分支存游标（按载荷形态钉死）：单元形态（FlatU16/Cons）
        // 存单元偏移；Flat 形态存 字节偏移×2（与单元口径对齐，每字符步长按其
        // UTF-16 单元数）。游标只被 next 内部读写、无外部消费者，包装器创建后
        // 类型固定不跨类型复用。
        //
        // 产出按 Unicode 标量（规格口径）：合法代理对整体产出一个 2 单元元素，
        // 孤立 surrogate 各产出 1 单元元素。
        let cursor = current_index(vm, wrapper, index_si);
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
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(next_cursor as i32));
            let value = match elem {
                // ASCII 单元走单字符缓存零分配，其余（含孤立 surrogate）落串创建。
                StrIterElem::One(u) => match vm.single_unit(u) {
                    Some(v) => v,
                    None => vm.new_string_units(&[u]),
                },
                StrIterElem::Pair(hi, lo) => vm.new_string_units(&[hi, lo]),
            };
            return Ok(Some(make_iter_result(vm, value, false)));
        }
        return Ok(Some(make_iter_result(vm, JsValue::undefined(), true)));
    }

    if is_typed_array_value(inner) {
        let index = current_index(vm, wrapper, index_si);
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
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let obj_ref = unsafe { &mut *obj };
    vm.set_or_create_prop_value(obj_ref, value_si, value);
    vm.set_or_create_prop_value(obj_ref, done_si, JsValue::bool(done));
    JsValue::from_js_object(obj)
}

/// 构造 native 函数对象并返回：设置函数标记、native 函数项、参数个数与 `name`
/// 属性。供迭代器包装器把自己的 next/return 方法装到包装对象上，不经绑定层注册。
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
        .unwrap_or_else(|| crate::error::create_from_text(vm, err))
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
/// 条目存放在集合 native-data 槽中（Map 为槽表：空槽跳过、下标口径为"下一待检槽"；
/// Set 为插入序表）；(a, b) 对在任意分配之前拷出，使对 native 集合的借用不会
/// 跨越 `vm` 调用保持。
fn map_set_step<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32, mode: MapSetMode,
) -> JsValue {
    let index = current_index(vm, wrapper, index_si);
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
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(next_index as i32));
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
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let mode_si = vm.kernel_core().perm_interner().intern(MODE_PROP).0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, mode_si, JsValue::int(mode as i32));
    JsValue::from_js_object(wrapper)
}
