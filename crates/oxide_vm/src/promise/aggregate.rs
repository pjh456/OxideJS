//! Promise 静态聚合方法（all/race/allSettled/any/withResolvers）与
//! AggregateError 内建初始化。
//!
//! 元素处理器共享结算计数记录（剩余计数 / 结果数组 / 能力闭包）；
//! 计数从 1 起步，尾部哨兵计入整趟迭代。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::make_int_key;
use oxide_types::value::JsValue;

use crate::native::NativeFn;
use crate::vm::Vm;

use super::{
    AggregateKind, AGG_ALREADY_PROP, AGG_INDEX_PROP, AGG_RECORD_PROP, AGG_REJECT_PROP, AGG_REMAINING_PROP,
    AGG_RESOLVE_PROP, AGG_VALUES_PROP,
};

impl Vm {
    /// 创建聚合静态方法（all/race/allSettled/any）的共享记录对象：剩余计数
    /// 初始 1，结果数组（race 无）与能力 resolve/reject 闭包全部存为自身属性。
    fn make_agg_record(&mut self, values: JsValue, resolve: JsValue, reject: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；本次 native 调用内不搬移，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        let rem_si = self.kernel_core.perm_interner().intern(AGG_REMAINING_PROP).0;
        self.set_or_create_prop_value(obj, rem_si, JsValue::int(1));
        let val_si = self.kernel_core.perm_interner().intern(AGG_VALUES_PROP).0;
        self.set_or_create_prop_value(obj, val_si, values);
        let res_si = self.kernel_core.perm_interner().intern(AGG_RESOLVE_PROP).0;
        self.set_or_create_prop_value(obj, res_si, resolve);
        let rej_si = self.kernel_core.perm_interner().intern(AGG_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, rej_si, reject);
        JsValue::from_js_object(ptr)
    }

    /// 创建聚合静态方法的元素处理器闭包：携带共享记录与元素下标，`already` 标志
    /// 初始 false。内部状态属性非枚举且追加在 length/name 之后，保持内建函数
    /// "length 先于 name" 的属性序。
    fn make_agg_element_fn(&mut self, native_fn: NativeFn, record: JsValue, index: i32) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: native_fn 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；后续 add_fn_name_length 的 shape/属性分配不改对象地址，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        self.add_fn_name_length(obj, "", 1);
        let sh = self.kernel_core.shape_forge().as_ref();
        let attrs = PropAttributes::new(false, false, false);
        let rec_si = self.kernel_core.perm_interner().intern(AGG_RECORD_PROP).0;
        let rec_shape = sh.make_shape(obj.shape_id(), rec_si);
        obj.set_shape_id(rec_shape);
        let rec_pos = obj.push_prop(record);
        obj.set_data_meta(rec_pos, attrs);
        let idx_si = self.kernel_core.perm_interner().intern(AGG_INDEX_PROP).0;
        let idx_shape = sh.make_shape(obj.shape_id(), idx_si);
        obj.set_shape_id(idx_shape);
        let idx_pos = obj.push_prop(JsValue::int(index));
        obj.set_data_meta(idx_pos, attrs);
        let al_si = self.kernel_core.perm_interner().intern(AGG_ALREADY_PROP).0;
        let al_shape = sh.make_shape(obj.shape_id(), al_si);
        obj.set_shape_id(al_shape);
        let al_pos = obj.push_prop(JsValue::bool(false));
        obj.set_data_meta(al_pos, attrs);
        JsValue::from_js_object(ptr)
    }

    /// 构造 allSettled 的结算记录 `{status, <value_field>: value}` 普通对象。
    fn make_settled_record(&mut self, status: &str, value_field: &str, value: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；写 status/value 属性（含 new_string 分配）期间不搬移，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        let status_si = self.kernel_core.perm_interner().intern("status").0;
        let status_val = self.new_string(status);
        self.set_or_create_prop_value(obj, status_si, status_val);
        let field_si = self.kernel_core.perm_interner().intern(value_field).0;
        self.set_or_create_prop_value(obj, field_si, value);
        JsValue::from_js_object(ptr)
    }

    /// 构造 AggregateError 实例（proto = %AggregateError.prototype%）：message 非
    /// undefined 时 ToString 建自身属性，errors 存为数据属性。
    fn make_aggregate_error(&mut self, errors: JsValue, message: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.aggregate_error_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；写 message/errors 数据属性期间不搬移，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        // message/errors 为数据属性：writable、非枚举、configurable（CreateMethodProperty）。
        let attrs = PropAttributes::new(true, false, true);
        if !message.is_undefined() {
            let msg_str = oxide_runtime_api::to_string_full(message, self).unwrap_or_default();
            let msg_si = self.kernel_core.perm_interner().intern("message").0;
            let msg_val = self.new_string(&msg_str);
            let _ = self.define_data_property(obj, msg_si, msg_val, attrs);
        }
        let err_si = self.kernel_core.perm_interner().intern("errors").0;
        let _ = self.define_data_property(obj, err_si, errors, attrs);
        JsValue::from_js_object(ptr)
    }

    /// 把 errors 可迭代值收集为新数组（IterableToList）。不可迭代抛 TypeError。
    fn aggregate_errors_to_list(&mut self, errors: JsValue) -> Result<JsValue, JsValue> {
        let array_proto = JsValue::from_js_object(self.session.builtin_world().array_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, 0, self.epoch.bump()));
        let list = JsValue::from_js_object(ptr);
        let mut index = 0usize;
        oxide_builtins::iterator::iterate_elements(self, errors, |_vm, elem| {
            // SAFETY: list 是本函数新建的存活数组对象。
            let list_obj = unsafe { &mut *list.as_js_object_ptr() };
            list_obj.set_prop_at(index, elem);
            index += 1;
            Ok(())
        })?;
        Ok(list)
    }

    /// 初始化/重建 AggregateError 内建对象：`%AggregateError%` 构造器与
    /// `%AggregateError.prototype%`（proto = %Error.prototype%），绑定 global 槽。
    ///
    /// 与 `init_promise_intrinsics` 同生命周期（VM 创建 + full_reset），
    /// `Promise.any` 的拒绝路径依赖此内建。
    pub(crate) fn init_aggregate_error_intrinsics(&mut self) {
        let sf = self.kernel_core.perm_interner().as_ref();
        let sh = self.kernel_core.shape_forge().as_ref();
        let fn_proto_val = self.session.builtin_world().fn_proto_val();
        let error_proto_val =
            JsValue::from_js_object(self.session.builtin_world().error_proto.as_ptr() as *mut JsObject);

        // %AggregateError.prototype%：proto = %Error.prototype%，constructor/name/message 数据属性。
        let mut proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let ctor_si = sf.intern("constructor").0;
        let ctor_shape = sh.make_shape(proto.shape_id(), ctor_si);
        proto.set_shape_id(ctor_shape);
        proto.push_prop(JsValue::undefined());
        proto.set_data_meta(0u32, PropAttributes::new(true, false, true));
        let name_si = sf.intern("name").0;
        let name_shape = sh.make_shape(proto.shape_id(), name_si);
        proto.set_shape_id(name_shape);
        let name_pos = proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AggregateError").0)));
        proto.set_data_meta(name_pos, PropAttributes::new(true, false, true));
        let msg_si = sf.intern("message").0;
        let msg_shape = sh.make_shape(proto.shape_id(), msg_si);
        proto.set_shape_id(msg_shape);
        let msg_pos = proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("").0)));
        proto.set_data_meta(msg_pos, PropAttributes::new(true, false, true));

        // %AggregateError% 构造器：proto = %Function.prototype%，length 2。
        let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
        ctor.set_function(true);
        ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
        // SAFETY: aggregate_error_constructor 是 NativeFn 函数项。
        ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(aggregate_error_constructor as *const ()) }));
        ctor.set_native_arg_count(2);
        let length_si = sf.intern("length").0;
        let ctor_shape1 = sh.make_shape(ctor.shape_id(), length_si);
        ctor.set_shape_id(ctor_shape1);
        let lpos = ctor.push_prop(JsValue::int(2));
        ctor.set_data_meta(lpos, PropAttributes::new(false, false, true));
        let name2_si = sf.intern("name").0;
        let ctor_shape2 = sh.make_shape(ctor.shape_id(), name2_si);
        ctor.set_shape_id(ctor_shape2);
        let npos = ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AggregateError").0)));
        ctor.set_data_meta(npos, PropAttributes::new(false, false, true));
        let proto2_si = sf.intern("prototype").0;
        let ctor_shape3 = sh.make_shape(ctor.shape_id(), proto2_si);
        ctor.set_shape_id(ctor_shape3);
        ctor.push_prop(JsValue::undefined());
        ctor.set_data_meta(2u32, PropAttributes::new(false, false, false));

        // 固定地址后互相接线：proto.constructor ↔ ctor.prototype。
        Self::swap_intrinsic_proto(&mut self.aggregate_error_proto, *proto);
        Self::swap_intrinsic_proto(&mut self.aggregate_error_constructor, *ctor);
        // SAFETY: aggregate_error_proto 为堆址固定的 P<JsObject>（Arc 透明包装），本行前刚经 swap_intrinsic_proto 落地；与 ctor_mut 分属不同对象，无别名。
        let proto_mut = unsafe { &mut *self.aggregate_error_proto.as_mut_ptr() };
        proto_mut
            .set_prop_at(0u32, JsValue::from_js_object(self.aggregate_error_constructor.as_ptr() as *mut JsObject));
        // SAFETY: aggregate_error_constructor 同为堆址固定的 P<JsObject>（Arc 透明包装）；此处写 prototype 槽位（下标 2），与 proto_mut 分属不同对象，无别名。
        let ctor_mut = unsafe { &mut *self.aggregate_error_constructor.as_mut_ptr() };
        ctor_mut.set_prop_at(2u32, JsValue::from_js_object(self.aggregate_error_proto.as_ptr() as *mut JsObject));

        // 绑定 global（槽已存在则更新）。
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        // SAFETY: global 对象由 session 持有，存活整个 session；本函数内只改其 shape/属性区，期间无 reset 或对象搬移。
        let global = unsafe { &mut *global_ptr };
        let si = self.kernel_core.perm_interner().intern("AggregateError").0;
        let ctor_val = JsValue::from_js_object(self.aggregate_error_constructor.as_ptr() as *mut JsObject);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
            global.set_prop_at(pos, ctor_val);
        } else {
            let shape = self.kernel_core.shape_forge().make_shape(global.shape_id(), si);
            global.set_shape_id(shape);
            let pos = global.push_prop(ctor_val);
            global.set_data_meta(pos, PropAttributes::new(true, false, true));
            global.bump_generation();
        }
    }
}

/// `Promise.resolve(x)`：`x` 为原生 Promise 且 `x.constructor === this` 时直接返回；
/// 否则按 `this`（构造器）建能力并 PromiseResolve。
pub(super) fn promise_static_resolve(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let x = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if vm.is_promise_value(x) {
        // SAFETY: is_promise_value 已验证 x 为原生 Promise 对象，指针非空且存活；此处只读 constructor，即时消费。
        let x_obj = unsafe { &*x.as_js_object_ptr() };
        let ctor_si = vm.kernel_core.perm_interner().intern("constructor").0;
        let x_ctor = match vm.ordinary_get(x_obj, ctor_si, x) {
            Ok(c) => c,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        if oxide_runtime_api::same_value(x_ctor, ctor) {
            return NativeResult::Ok(x);
        }
    }
    let (promise, resolve, _) = match vm.new_promise_capability_with_ctor(ctor) {
        Ok(t) => t,
        Err(err) => return NativeResult::Err(err),
    };
    // PromiseResolve：调用能力 resolve（自定义构造器可能产生非原生 promise）。
    match vm.call_function_sync(resolve, JsValue::undefined(), &[x]) {
        Ok(_) => NativeResult::Ok(promise),
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// `Promise.reject(x)`：按 `this`（构造器）建能力并直接拒绝。
pub(super) fn promise_static_reject(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let x = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (promise, _, reject) = match vm.new_promise_capability_with_ctor(ctor) {
        Ok(t) => t,
        Err(err) => return NativeResult::Err(err),
    };
    match vm.call_function_sync(reject, JsValue::undefined(), &[x]) {
        Ok(_) => NativeResult::Ok(promise),
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// `Promise.withResolvers()`：按 `this` 构造器建能力（非构造器抛 TypeError），
/// 返回 `{promise, resolve, reject}` 普通对象（proto 为 `%Object.prototype%`）。
///
/// # 步骤
/// 1. 取 `this` 为构造器 C，经 `new_promise_capability_with_ctor` 建能力
/// 2. 创建普通对象，依次写入 promise / resolve / reject 三个数据属性
///
/// # 边界与前提
/// - C 非构造器（普通值 / arrow / 非构造 native）时抛 TypeError
/// - 返回值属性为默认数据描述符（writable/enumerable/configurable 均 true）
pub(super) fn promise_static_with_resolvers(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let (promise, resolve, reject) = match vm.new_promise_capability_with_ctor(ctor) {
        Ok(t) => t,
        Err(err) => return NativeResult::Err(err),
    };
    let object_proto = JsValue::from_js_object(vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
    let ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto));
    // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；写 promise/resolve/reject 属性期间不搬移，借出期间无别名。
    let obj = unsafe { &mut *ptr };
    let sf = vm.kernel_core.perm_interner().as_ref();
    let sh = vm.kernel_core.shape_forge().as_ref();
    for (name, val) in [("promise", promise), ("resolve", resolve), ("reject", reject)] {
        let si = sf.intern(name).0;
        let shape = sh.make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape);
        obj.push_prop(val);
    }
    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// `Promise.all(iterable)`：全部元素结算后以结果数组完成，任一拒绝则拒绝。
pub(super) fn promise_static_all(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::All) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.race(iterable)`：任一元素先结算即按该结果完成/拒绝。
pub(super) fn promise_static_race(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::Race) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.allSettled(iterable)`：全部元素结算后以 `{status, value|reason}` 数组完成。
pub(super) fn promise_static_all_settled(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::AllSettled) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.any(iterable)`：任一元素完成即完成；全部拒绝则以 AggregateError(errors) 拒绝。
pub(super) fn promise_static_any(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::Any) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// 聚合静态方法的共享核心：迭代可迭代输入，逐元素调 `C.resolve` 建 promise 并注册反应，
/// 按模式在全部/任一结算后交付能力 promise。
///
/// # 步骤
/// 1. 建能力（NewPromiseCapability(C) 抛错同步上抛），取 C.resolve 一次（不可调用 → 拒绝）。
/// 2. 建共享记录（剩余计数 = 1 + 元素数，结果数组，能力 resolve/reject）。
/// 3. 迭代元素：逐元素 `Call(C.resolve, C, «elem»)` 后 `Invoke(promise.then, 元素处理器)`。
/// 4. 迭代完成时哨兵计数递减；为 0 时结算（all/allSettled 完成数组，any 拒绝 AggregateError）。
///
/// # 边界与前提
/// - 迭代异常按来源区分是否 IteratorClose：next 调用与 then 注册抛错关闭迭代器，
///   迭代结果对象的 done/value 读取抛错按规范直接拒绝（不关闭）。
/// - 元素处理器同步触发时（thenable 直接调 onFulfilled），剩余计数先 +1 再注册，
///   保证中途结算仍能等齐全部元素。
fn perform_promise_combine(
    vm: &mut Vm, ctor: JsValue, iterable: JsValue, kind: AggregateKind,
) -> Result<JsValue, JsValue> {
    let (promise, resolve, reject) = vm.new_promise_capability_with_ctor(ctor)?;

    // 取 C.resolve 一次（getter 抛错或不可调用 → 拒绝能力）。
    let resolve_si = vm.kernel_core.perm_interner().intern("resolve").0;
    let promise_resolve = if ctor.is_object() {
        // SAFETY: ctor 是存活对象。
        let ctor_obj = unsafe { &*ctor.as_js_object_ptr() };
        match vm.ordinary_get(ctor_obj, resolve_si, ctor) {
            Ok(v) => v,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
                return Ok(promise);
            }
        }
    } else {
        JsValue::undefined()
    };
    if !oxide_builtins::iterator::is_callable(promise_resolve) {
        let exc = oxide_builtins::error::create_type_error(vm, "resolve is not a function");
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
        return Ok(promise);
    }

    // 结果数组（race 不需要）。
    let values = if kind == AggregateKind::Race {
        JsValue::undefined()
    } else {
        let array_proto = JsValue::from_js_object(vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject);
        let ptr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, 0, vm.epoch.bump()));
        JsValue::from_js_object(ptr)
    };
    let record = vm.make_agg_record(values, resolve, reject);

    // GetIterator 失败 → 拒绝（尚无迭代器可关闭）。
    let iterator = match oxide_builtins::iterator::make_iterator_for_value(vm, iterable) {
        Ok(it) => it,
        Err(err) => {
            let _ = vm.call_function_sync(reject, JsValue::undefined(), &[err]);
            return Ok(promise);
        }
    };

    enum IterAbrupt {
        Close(JsValue),
        NoClose(JsValue),
    }
    let mut index: i32 = 0;
    let result: Result<(), IterAbrupt> = (|| {
        let next_si = vm.kernel_core.perm_interner().intern("next").0;
        let done_si = vm.kernel_core.perm_interner().intern("done").0;
        let value_si = vm.kernel_core.perm_interner().intern("value").0;
        // SAFETY: iterator 是存活对象。
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_fn = vm
            .ordinary_get(iter_obj, next_si, iterator)
            .map_err(|e| IterAbrupt::Close(agg_engine_error(vm, &e)))?;
        loop {
            let step = vm
                .call_function_sync(next_fn, iterator, &[])
                .map_err(|e| IterAbrupt::Close(agg_engine_error(vm, &e)))?;
            if !step.is_object() {
                return Err(IterAbrupt::NoClose(oxide_builtins::error::create_type_error(
                    vm,
                    "iterator result is not an object",
                )));
            }
            // SAFETY: step 是存活对象。
            let step_obj = unsafe { &*step.as_js_object_ptr() };
            let done = vm
                .ordinary_get(step_obj, done_si, step)
                .map_err(|e| IterAbrupt::NoClose(agg_engine_error(vm, &e)))?;
            if oxide_runtime_api::to_boolean(done) {
                return Ok(());
            }
            let elem = vm
                .ordinary_get(step_obj, value_si, step)
                .map_err(|e| IterAbrupt::NoClose(agg_engine_error(vm, &e)))?;
            let next_promise = vm
                .call_function_sync(promise_resolve, ctor, &[elem])
                .map_err(|e| IterAbrupt::Close(agg_engine_error(vm, &e)))?;

            // 剩余计数先 +1 再注册：thenable 同步结算时 handler 依赖该计数已含自身。
            let cur = agg_read_remaining(vm, record);
            agg_write_remaining(vm, record, cur + 1);
            match kind {
                AggregateKind::All => {
                    let re = vm.make_agg_element_fn(promise_all_resolve_element, record, index);
                    vm.invoke_then(next_promise, &[re, reject]).map_err(IterAbrupt::Close)?;
                }
                AggregateKind::Race => {
                    // race 直接把能力 resolve/reject 作为反应处理器（规范无元素函数包装，
                    // 能力结算一次后其余调用为 no-op）。
                    vm.invoke_then(next_promise, &[resolve, reject]).map_err(IterAbrupt::Close)?;
                }
                AggregateKind::AllSettled => {
                    let re = vm.make_agg_element_fn(promise_all_settled_resolve_element, record, index);
                    let rj = vm.make_agg_element_fn(promise_all_settled_reject_element, record, index);
                    vm.invoke_then(next_promise, &[re, rj]).map_err(IterAbrupt::Close)?;
                }
                AggregateKind::Any => {
                    // any 的完成侧直接用能力 resolve；拒绝侧用带 AlreadyCalled 的 reject 元素。
                    let rj = vm.make_agg_element_fn(promise_any_reject_element, record, index);
                    vm.invoke_then(next_promise, &[resolve, rj]).map_err(IterAbrupt::Close)?;
                }
            }
            index += 1;
        }
    })();

    match result {
        Ok(()) => {
            // 迭代完成：哨兵 1 递减；为 0 时结算（race 无计数语义）。
            let remaining = agg_read_remaining(vm, record) - 1;
            agg_write_remaining(vm, record, remaining);
            match kind {
                AggregateKind::All | AggregateKind::AllSettled => {
                    if remaining == 0 {
                        let values = agg_record_val(vm, record, AGG_VALUES_PROP);
                        agg_call_resolve(vm, record, values);
                    }
                }
                AggregateKind::Any => {
                    if remaining == 0 {
                        let errors = agg_record_val(vm, record, AGG_VALUES_PROP);
                        let agg = vm.make_aggregate_error(errors, JsValue::undefined());
                        let rej = agg_record_val(vm, record, AGG_REJECT_PROP);
                        let _ = vm.call_function_sync(rej, JsValue::undefined(), &[agg]);
                    }
                }
                AggregateKind::Race => {}
            }
        }
        Err(IterAbrupt::Close(e)) => {
            close_agg_iterator(vm, iterator);
            let rej = agg_record_val(vm, record, AGG_REJECT_PROP);
            let _ = vm.call_function_sync(rej, JsValue::undefined(), &[e]);
        }
        Err(IterAbrupt::NoClose(e)) => {
            let rej = agg_record_val(vm, record, AGG_REJECT_PROP);
            let _ = vm.call_function_sync(rej, JsValue::undefined(), &[e]);
        }
    }
    Ok(promise)
}

/// 聚合元素处理器的公共 prologue：取共享记录与下标；`already` 已置位返回 None
/// （元素函数只生效一次），否则置位后返回记录。
fn agg_element_state(vm: &mut Vm) -> Option<(JsValue, i32)> {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return None;
    }
    let callee_ptr = callee.as_js_object_ptr();
    let rec_si = vm.kernel_core.perm_interner().intern(AGG_RECORD_PROP).0;
    let idx_si = vm.kernel_core.perm_interner().intern(AGG_INDEX_PROP).0;
    let al_si = vm.kernel_core.perm_interner().intern(AGG_ALREADY_PROP).0;
    // SAFETY: callee 是当前调用的存活函数对象。
    let callee_ref = unsafe { &*callee_ptr };
    if vm
        .resolve_property(callee_ref, al_si)
        .is_some_and(oxide_runtime_api::to_boolean)
    {
        return None;
    }
    let record = vm.resolve_property(callee_ref, rec_si).unwrap_or(JsValue::undefined());
    let index = vm
        .resolve_property(callee_ref, idx_si)
        .map_or(0, |v| if v.is_int() { v.as_int() } else { 0 });
    // SAFETY: 同一 callee 对象，此处仅写 already 标志。
    vm.set_or_create_prop_value(unsafe { &mut *callee_ptr }, al_si, JsValue::bool(true));
    Some((record, index))
}

/// 读取记录对象属性（非对象/缺失返回 undefined）。
fn agg_record_val(vm: &Vm, record: JsValue, prop: &str) -> JsValue {
    if !record.is_object() {
        return JsValue::undefined();
    }
    let si = vm.kernel_core.perm_interner().intern(prop).0;
    // SAFETY: record 是存活对象。
    vm.resolve_property(unsafe { &*record.as_js_object_ptr() }, si)
        .unwrap_or(JsValue::undefined())
}

/// 读取记录剩余计数。
fn agg_read_remaining(vm: &Vm, record: JsValue) -> i32 {
    let v = agg_record_val(vm, record, AGG_REMAINING_PROP);
    if v.is_int() {
        v.as_int()
    } else {
        0
    }
}

/// 写回记录剩余计数。
fn agg_write_remaining(vm: &mut Vm, record: JsValue, n: i32) {
    if !record.is_object() {
        return;
    }
    let si = vm.kernel_core.perm_interner().intern(AGG_REMAINING_PROP).0;
    // SAFETY: record 是存活对象。
    vm.set_or_create_prop_value(unsafe { &mut *record.as_js_object_ptr() }, si, JsValue::int(n));
}

/// 调用记录上的能力 resolve；抛错时改以能力 reject 拒绝（IfAbruptRejectPromise）。
fn agg_call_resolve(vm: &mut Vm, record: JsValue, arg: JsValue) {
    let resolve = agg_record_val(vm, record, AGG_RESOLVE_PROP);
    if let Err(e) = vm.call_function_sync(resolve, JsValue::undefined(), &[arg]) {
        let exc = vm
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
        let reject = agg_record_val(vm, record, AGG_REJECT_PROP);
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
    }
}

/// 把 `call_function_sync` 的错误文本恢复为原始异常值。
fn agg_engine_error(vm: &mut Vm, err: &str) -> JsValue {
    vm.last_uncaught_value
        .take()
        .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, err))
}

/// IteratorClose：调用迭代器的 `return()`（可调用时），忽略其抛错。
fn close_agg_iterator(vm: &mut Vm, iterator: JsValue) {
    if !iterator.is_object() {
        return;
    }
    let return_si = vm.kernel_core.perm_interner().intern("return").0;
    // SAFETY: iterator 是存活对象。
    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
    if let Ok(ret) = vm.ordinary_get(iter_obj, return_si, iterator) {
        if oxide_builtins::iterator::is_callable(ret) {
            // return() 的抛错被忽略，其值不得外泄进槽覆盖在途异常。
            let saved_uncaught = vm.last_uncaught_value.take();
            let _ = vm.call_function_sync(ret, iterator, &[]);
            vm.last_uncaught_value = saved_uncaught;
        }
    }
}

/// `Promise.all` resolve 元素：写 `values[index]`，剩余计数归零时完成数组。
fn promise_all_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let key_si = make_int_key(index as u32);
        // SAFETY: values 是存活数组对象；直写元素区不触发数组 setter。
        vm.set_or_create_prop_value(unsafe { &mut *values.as_js_object_ptr() }, key_si, value);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        agg_call_resolve(vm, record, values);
    }
    NativeResult::Ok(JsValue::undefined())
}

// `Promise.race` 使用能力 resolve/reject 直连，无独立元素函数。

/// `Promise.allSettled` resolve 元素：写 `{status:'fulfilled', value}` 记录。
fn promise_all_settled_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let settled = vm.make_settled_record("fulfilled", "value", value);
        let key_si = make_int_key(index as u32);
        // SAFETY: values 是存活数组对象。
        vm.set_or_create_prop_value(unsafe { &mut *values.as_js_object_ptr() }, key_si, settled);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        agg_call_resolve(vm, record, values);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.allSettled` reject 元素：写 `{status:'rejected', reason}` 记录。
fn promise_all_settled_reject_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let reason = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let settled = vm.make_settled_record("rejected", "reason", reason);
        let key_si = make_int_key(index as u32);
        // SAFETY: values 是存活数组对象。
        vm.set_or_create_prop_value(unsafe { &mut *values.as_js_object_ptr() }, key_si, settled);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        agg_call_resolve(vm, record, values);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.any` reject 元素：写 `errors[index]`，全部拒绝时以 AggregateError 拒绝。
fn promise_any_reject_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let reason = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let errors = agg_record_val(vm, record, AGG_VALUES_PROP);
    if errors.is_object() {
        let key_si = make_int_key(index as u32);
        // SAFETY: errors 是存活数组对象。
        vm.set_or_create_prop_value(unsafe { &mut *errors.as_js_object_ptr() }, key_si, reason);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        let agg = vm.make_aggregate_error(errors, JsValue::undefined());
        let reject = agg_record_val(vm, record, AGG_REJECT_PROP);
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[agg]);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `AggregateError(errors, message)` 构造器：message 非 undefined 时先 ToString 建
/// 自身属性，再把 errors 可迭代收集为 `errors` 数据属性。
fn aggregate_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let errors = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let message = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let this = if this_val.is_object() {
        this_val.as_js_object_ptr()
    } else {
        let proto_val = JsValue::from_js_object(vm.aggregate_error_proto.as_ptr() as *mut JsObject);
        vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val))
    };
    if !message.is_undefined() {
        let msg_str = match oxide_runtime_api::to_string_full(message, vm) {
            Ok(s) => s,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        let msg_si = vm.kernel_core.perm_interner().intern("message").0;
        let msg_val = vm.new_string(&msg_str);
        // SAFETY: this 是本次构造的存活对象。
        let _ = vm.define_data_property(unsafe { &mut *this }, msg_si, msg_val, PropAttributes::new(true, false, true));
    }
    let errors_list = match vm.aggregate_errors_to_list(errors) {
        Ok(list) => list,
        Err(err) => return NativeResult::Err(err),
    };
    let err_si = vm.kernel_core.perm_interner().intern("errors").0;
    // SAFETY: this 为本次构造得到的既有或新建存活对象；此处写 errors 数据属性，与上方 message 写入顺序独占同一对象，无别名。
    let _ = vm.define_data_property(unsafe { &mut *this }, err_si, errors_list, PropAttributes::new(true, false, true));
    NativeResult::Ok(JsValue::from_js_object(this))
}
