use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::private_key::{make_int_key, make_well_known_symbol_key};
use oxide_types::value::JsValue;

use oxide_runtime_api::{to_object, NativeResult, VmHost};

const INNER_PROP: &str = "__inner__";
const INDEX_PROP: &str = "__index__";
/// 字符串迭代的字节游标（`next` 增量推进的当前位置），避免每步整串复制 + 从头重扫。
const BYTEOFF_PROP: &str = "__byteoff__";

/// 占位构造函数：`Iterator` 不是构造函数，任何调用都抛 TypeError。
pub fn iterator_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Iterator is not a constructor"))
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
    let inner = match get_iterator(vm, value) {
        Ok(Some(inner)) => inner,
        Ok(None) => return Ok(None),
        Err(err) => return Err(err),
    };
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));

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
        let index = current_index(vm, wrapper, index_si);
        let byteoff_si = vm.kernel_core().perm_interner().intern(BYTEOFF_PROP).0;
        let byteoff = current_index(vm, wrapper, byteoff_si);
        // 源串裸指针借用压缩到单个表达式：ch 是 Copy 的 char，不携带借用，
        // 之后对 VM 状态的可变访问不再与源串借用共存。
        let ch = unsafe { &*inner.as_string_ptr() }.as_str()[byteoff..].chars().next();
        if let Some(ch) = ch {
            vm.set_or_create_prop_value(wrapper, byteoff_si, JsValue::int((byteoff + ch.len_utf8()) as i32));
            vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
            let value = vm.new_string(&ch.to_string());
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
            let _ = vm.call_function_sync(ret, iterator, &[]);
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

#[derive(Clone, Copy)]
enum MapSetMode {
    MapEntries,
    MapValues,
    MapKeys,
    SetEntries,
    SetValues,
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

fn map_set_next_dispatch<H: VmHost>(vm: &mut H, args: &[u8], mode: MapSetMode) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "iterator next called on non-object"));
    }
    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "iterator has no inner collection")),
    };
    NativeResult::Ok(map_set_step(vm, wrapper, inner, index_si, mode))
}

pub(crate) fn map_entries_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::MapEntries)
}

pub(crate) fn map_values_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::MapValues)
}

pub(crate) fn map_keys_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::MapKeys)
}

pub(crate) fn set_values_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::SetValues)
}

pub(crate) fn set_entries_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    map_set_next_dispatch::<H>(vm, args, MapSetMode::SetEntries)
}

#[derive(Clone, Copy)]
enum TypedArrayMode {
    Values,
    Keys,
    Entries,
}

/// 按模式让 TypedArray 迭代器包装器推进一步：values 产出元素，keys 产出索引，
/// entries 产出 `[index, element]` 对；耗尽后把下标推进到哨兵值防止"复活"。
fn typed_array_step<H: VmHost>(
    vm: &mut H, wrapper: &mut JsObject, inner: JsValue, index_si: u32, mode: TypedArrayMode,
) -> Result<JsValue, JsValue> {
    let index = current_index(vm, wrapper, index_si);
    let view = crate::typed_array::get_typed_array_data(vm, inner)?;
    if index >= view.length {
        vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(i32::MAX));
        return Ok(make_iter_result(vm, JsValue::undefined(), true));
    }
    let elem = crate::typed_array::typed_array_element_get(vm, unsafe { &*inner.as_js_object_ptr() }, index as u32)
        .map_err(|e| crate::error::create_type_error(vm, &e))?;
    vm.set_or_create_prop_value(wrapper, index_si, JsValue::int((index + 1) as i32));
    let value = match mode {
        TypedArrayMode::Values => elem,
        TypedArrayMode::Keys => JsValue::int(index as i32),
        TypedArrayMode::Entries => make_map_set_pair(vm, JsValue::int(index as i32), elem),
    };
    Ok(make_iter_result(vm, value, false))
}

/// TypedArray 模式迭代器 `next` 的分发：校验包装器后按模式推进。
fn typed_array_next_dispatch<H: VmHost>(vm: &mut H, args: &[u8], mode: TypedArrayMode) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "iterator next called on non-object"));
    }
    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let inner = match vm.ordinary_get(wrapper, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => inner,
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "iterator has no inner typed array")),
    };
    match typed_array_step(vm, wrapper, inner, index_si, mode) {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => NativeResult::Err(err),
    }
}

pub(crate) fn typed_array_values_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    typed_array_next_dispatch::<H>(vm, args, TypedArrayMode::Values)
}

pub(crate) fn typed_array_keys_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    typed_array_next_dispatch::<H>(vm, args, TypedArrayMode::Keys)
}

pub(crate) fn typed_array_entries_iter_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    typed_array_next_dispatch::<H>(vm, args, TypedArrayMode::Entries)
}

/// 构造迭代器包装器，其 `next` 委托给调用方指定的按模式分发的 native 函数。
/// 与 `make_iterator_for_value` 一致，但允许 Map/Set 原型方法选择
/// values/keys/entries 变体。
pub(crate) fn make_mode_iterator<H: VmHost>(vm: &mut H, inner: JsValue, next_fn: *const ()) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let inner_si = vm.kernel_core().perm_interner().intern(INNER_PROP).0;
    let index_si = vm.kernel_core().perm_interner().intern(INDEX_PROP).0;
    let next_si = vm.kernel_core().perm_interner().intern("next").0;
    let wrapper_obj = unsafe { &mut *wrapper };
    vm.set_or_create_prop_value(wrapper_obj, inner_si, inner);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    let next = make_native_function(vm, "next", next_fn, 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next);
    JsValue::from_js_object(wrapper)
}
