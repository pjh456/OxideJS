//! Promise 静态聚合方法（all/race/allSettled/any/allKeyed 族/withResolvers）与
//! AggregateError 内建初始化。
//!
//! 元素处理器共享结算计数记录（剩余计数 / 结果数组 / 能力闭包）；
//! 计数从 1 起步，尾部哨兵计入整趟迭代。
//! keyed 族（allKeyed / allSettledKeyed）与 all 族共用能力 / 元素函数 /
//! 结算路径，差异仅在元素来源（own keys 枚举替代迭代器）与结果承载
//! （null 原型键控对象替代数组）。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::{
    is_symbol_key, make_int_key, make_symbol_key, make_well_known_symbol_key, symbol_index_from_key,
    well_known_symbol_id_from_key, WELL_KNOWN_SYMBOL_COUNT,
};
use oxide_types::value::JsValue;

use crate::native::NativeFn;
use crate::vm::Vm;

use super::{
    AggregateKind, AGG_ALREADY_PROP, AGG_INDEX_PROP, AGG_KEYS_PROP, AGG_RECORD_PROP, AGG_REJECT_PROP,
    AGG_REMAINING_PROP, AGG_RESOLVE_PROP, AGG_VALUES_PROP, TRY_ARGS_PROP, TRY_EXECUTOR_PROP,
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

    /// 创建 keyed 聚合静态方法的共享记录对象：剩余计数（初始 1）、结果平行数组、
    /// 键平行数组与能力 resolve/reject 闭包存为自身属性。
    fn make_agg_record_keyed(&mut self, values: JsValue, keys: JsValue, resolve: JsValue, reject: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；本次 native 调用内不搬移，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        let rem_si = self.kernel_core.perm_interner().intern(AGG_REMAINING_PROP).0;
        self.set_or_create_prop_value(obj, rem_si, JsValue::int(1));
        let val_si = self.kernel_core.perm_interner().intern(AGG_VALUES_PROP).0;
        self.set_or_create_prop_value(obj, val_si, values);
        let key_si = self.kernel_core.perm_interner().intern(AGG_KEYS_PROP).0;
        self.set_or_create_prop_value(obj, key_si, keys);
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
        // Error 家族标签：[[ErrorData]] 谓词（stack 访问器 / Error.isError）据此判定。
        obj.type_tag = JsObject::OBJ_TYPE_ERROR;
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

    /// 建普通数组对象并依次写入元素（Promise.try 转发实参的承载）。
    fn make_plain_array(&mut self, elements: Vec<JsValue>) -> JsValue {
        let array_proto = JsValue::from_js_object(self.session.builtin_world().array_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, 0, self.epoch.bump()));
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；元素写入不搬移对象。
        let obj = unsafe { &mut *ptr };
        for (i, elem) in elements.into_iter().enumerate() {
            obj.set_prop_at(i as u32, elem);
        }
        JsValue::from_js_object(ptr)
    }

    /// 建 Promise.try 的包装闭包 W：native 函数对象，executor 与转发实参数组挂
    /// 自身属性（GC 边走属性区，免 native_data 接线）；W 被构造器以
    /// (resolve, reject) 实参调用。
    fn make_try_wrapper(&mut self, executor: JsValue, call_args: Vec<JsValue>) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: promise_try_wrapper 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(promise_try_wrapper as *const ()) }));
        func.set_native_arg_count(2);
        let ptr = self.alloc_object(func);
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；后续属性/shape 分配不改对象地址，无别名。
        let obj = unsafe { &mut *ptr };
        let exec_si = self.kernel_core.perm_interner().intern(TRY_EXECUTOR_PROP).0;
        self.set_or_create_prop_value(obj, exec_si, executor);
        let args_array = self.make_plain_array(call_args);
        let args_si = self.kernel_core.perm_interner().intern(TRY_ARGS_PROP).0;
        self.set_or_create_prop_value(obj, args_si, args_array);
        self.add_fn_name_length(obj, "", 2);
        JsValue::from_js_object(ptr)
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

/// `Promise.allKeyed(object)`：输入自身可枚举键的元素全部结算后以 null 原型
/// 键控结果对象完成，任一元素拒绝则拒绝。
pub(super) fn promise_static_all_keyed(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let promises = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine_keyed(vm, ctor, promises, KeyedVariant::All) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.allSettledKeyed(object)`：输入自身可枚举键的元素全部结算后以
/// null 原型键控结果对象完成，每元素记 `{status, value|reason}` 记录。
pub(super) fn promise_static_all_settled_keyed(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let promises = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine_keyed(vm, ctor, promises, KeyedVariant::AllSettled) {
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

/// keyed 聚合族的语义模式：决定元素处理器形态（all 拒绝侧直拒 /
/// allSettled 双侧记录）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyedVariant {
    All,
    AllSettled,
}

/// keyed 聚合静态方法（allKeyed / allSettledKeyed）共享核心：按 own keys 序
/// 枚举输入对象的可枚举自身键，逐元素包装 promise 并注册反应，全部结算后
/// 以 null 原型键控结果对象交付能力 promise。
///
/// # 步骤
/// 1. 建能力（NewPromiseCapability(C) 抛错同步上抛）；取 C.resolve
///    （读错 / 不可调用 → 拒绝）。
/// 2. 非对象输入 → 异步以新 TypeError 拒绝（键枚举之前）。
/// 3. 按 OwnKeys 序枚举（整数键升序、字符串键与符号键插入序）：可枚举键
///    经完整 Get 取值，`Call(C.resolve, C, «value»)` 包装，注册元素处理器
///    （剩余计数先 +1 再注册）。
/// 4. 循环尾部哨兵计数递减，归 0 时建结果对象并结算。
///
/// # 边界与前提
/// - Get / resolve 调用 / then 注册抛错均异步拒绝能力 promise
///   （IfAbruptRejectPromise），不同步抛出；仅 NewPromiseCapability 同步传播。
/// - ownKeys 枚举在非 Proxy 对象上无抛点（本引擎无 Proxy）；描述符读取与
///   Get 同走现件路径。
fn perform_promise_combine_keyed(
    vm: &mut Vm, ctor: JsValue, promises: JsValue, variant: KeyedVariant,
) -> Result<JsValue, JsValue> {
    // NewPromiseCapability(C)：本函数唯一同步传播臂。
    let (promise, resolve, reject) = vm.new_promise_capability_with_ctor(ctor)?;

    // GetPromiseResolve(C)：读取抛错或不可调用 → 以原抛出值拒绝。
    let resolve_si = vm.kernel_core.perm_interner().intern("resolve").0;
    let promise_resolve = if ctor.is_object() {
        // SAFETY: ctor 是存活对象。
        let ctor_obj = unsafe { &*ctor.as_js_object_ptr() };
        match vm.ordinary_get(ctor_obj, resolve_si, ctor) {
            Ok(v) => v,
            Err(e) => {
                let exc = agg_engine_error(vm, &e);
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

    // 非对象输入：能力已建、键枚举之前，异步以新 TypeError 拒绝。
    if !promises.is_object() {
        let exc = oxide_builtins::error::create_type_error(vm, "promises argument is not an object");
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
        return Ok(promise);
    }

    // 平行承载：values 结果数组与 keys 键值数组同下标对齐，记录对象持有两者。
    // SAFETY: promises 是存活对象。
    let promises_obj = unsafe { &*promises.as_js_object_ptr() };
    let values = vm.make_plain_array(Vec::new());
    let keys = vm.make_plain_array(Vec::new());
    let record = vm.make_agg_record_keyed(values, keys, resolve, reject);

    // OwnKeys 序：整数键升序、字符串键插入序；符号键按插入序追加（shape 位置
    // 换算绝对存储下标，与 walk_own_keys 口径一致）。
    let mut all_keys = oxide_builtins::object::walk_own_keys(vm, promises_obj);
    for sym in oxide_builtins::object::own_symbol_key_values(vm, promises_obj) {
        let idx = sym.as_symbol_index();
        let si = if idx < WELL_KNOWN_SYMBOL_COUNT {
            make_well_known_symbol_key(idx)
        } else {
            make_symbol_key(idx)
        };
        if let Some(pos) = vm.kernel_core.shape_forge().lookup_position(promises_obj.shape_id(), si) {
            let store = if promises_obj.is_array() { promises_obj.array_prop_count + pos } else { pos };
            all_keys.push((si, store));
        }
    }

    let mut index: i32 = 0;
    for (si, store) in all_keys {
        // 可枚举性：TA 元素键（哨兵存储下标）恒在场且可枚举；其余查元数据区
        // （无元数据 = 默认数据描述符，可枚举）。
        let enumerable = if store == u32::MAX {
            true
        } else {
            promises_obj
                .prop_meta_at(store)
                .map_or(true, |m| !m.is_hole() && m.attributes.enumerable())
        };
        if !enumerable {
            continue;
        }

        // Get：完整读（走原型链，访问器触发）；抛错异步拒绝。
        let value = match vm.ordinary_get(promises_obj, si, promises) {
            Ok(v) => v,
            Err(e) => {
                let exc = agg_engine_error(vm, &e);
                let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
                return Ok(promise);
            }
        };

        // 键值物化进 keys 数组（与 values 下标平行）。
        let key_value = key_si_to_key_value(vm, si);
        // SAFETY: keys 是本函数新建的存活数组对象；元素区直写不触发数组 setter。
        vm.set_or_create_prop_value(unsafe { &mut *keys.as_js_object_ptr() }, make_int_key(index as u32), key_value);

        // nextPromise = Call(C.resolve, C, «value»）；抛错异步拒绝。
        let next_promise = match vm.call_function_sync(promise_resolve, ctor, &[value]) {
            Ok(p) => p,
            Err(e) => {
                let exc = agg_engine_error(vm, &e);
                let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
                return Ok(promise);
            }
        };

        // 剩余计数先 +1 再注册：thenable 同步结算时 handler 依赖计数已含自身。
        let cur = agg_read_remaining(vm, record);
        agg_write_remaining(vm, record, cur + 1);

        // 元素处理器按模式注册：all 变体拒绝侧直挂能力 reject，
        // allSettled 变体双侧用记录元素函数。
        let outcome = match variant {
            KeyedVariant::All => {
                let re = vm.make_agg_element_fn(promise_keyed_resolve_element, record, index);
                vm.invoke_then(next_promise, &[re, reject])
            }
            KeyedVariant::AllSettled => {
                let re = vm.make_agg_element_fn(promise_keyed_settled_resolve_element, record, index);
                let rj = vm.make_agg_element_fn(promise_keyed_settled_reject_element, record, index);
                vm.invoke_then(next_promise, &[re, rj])
            }
        };
        if let Err(exc) = outcome {
            let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
            return Ok(promise);
        }
        index += 1;
    }

    // 尾部哨兵递减；归 0 时结算（键非空时 thenable 同步结算亦可发生）。
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        settle_keyed_aggregate(vm, record);
    }
    Ok(promise)
}

/// 键 si 物化为 JS 可见键值：字符串 / 数字串经文本还原，符号键还原为符号值。
fn key_si_to_key_value(vm: &mut Vm, si: u32) -> JsValue {
    if is_symbol_key(si) {
        if let Some(id) = well_known_symbol_id_from_key(si) {
            return JsValue::symbol(id);
        }
        return JsValue::symbol(symbol_index_from_key(si));
    }
    oxide_builtins::object::key_si_to_js_value(vm, si)
}

/// 结算 keyed 聚合：按 keys/values 平行数组建 null 原型结果对象（键序 =
/// 输入 OwnKeys 序），经能力 resolve 交付；结算抛错降级为拒绝。
fn settle_keyed_aggregate(vm: &mut Vm, record: JsValue) {
    let keys = agg_record_val(vm, record, AGG_KEYS_PROP);
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    let ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；结算循环内属性写入不搬移对象，借出期间无别名。
    let obj = unsafe { &mut *ptr };
    if keys.is_object() && values.is_object() {
        // SAFETY: keys/values 是本次调用新建的存活平行数组；元素区读写不搬移对象。
        let keys_obj = unsafe { &*keys.as_js_object_ptr() };
        let values_obj = unsafe { &*values.as_js_object_ptr() };
        for i in 0..keys_obj.prop_count() {
            let key_val = keys_obj.get_prop_at(i);
            let si = match vm.property_key_si(key_val) {
                Ok(si) => si,
                Err(_) => continue,
            };
            vm.set_or_create_prop_value(obj, si, values_obj.get_prop_at(i));
        }
    }
    agg_call_resolve(vm, record, JsValue::from_js_object(ptr));
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

/// `Promise.allKeyed` resolve 元素：写 `values[index]`，剩余计数归零时建
/// null 原型结果对象并完成。
fn promise_keyed_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
        settle_keyed_aggregate(vm, record);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.allSettledKeyed` resolve 元素：写 `{status:'fulfilled', value}`
/// 记录，剩余计数归零时建结果对象并完成。
fn promise_keyed_settled_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
        settle_keyed_aggregate(vm, record);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.allSettledKeyed` reject 元素：写 `{status:'rejected', reason}`
/// 记录，剩余计数归零时建结果对象并完成。
fn promise_keyed_settled_reject_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
        settle_keyed_aggregate(vm, record);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `AggregateError(errors, message)` 构造器：message 非 undefined 时先 ToString 建
/// 自身属性，再把 errors 可迭代收集为 `errors` 数据属性。
fn aggregate_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let errors = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let message = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let new_target = vm.reg(255);
    let this = if this_val.is_object() && new_target.is_object() {
        this_val.as_js_object_ptr()
    } else {
        let proto_val = JsValue::from_js_object(vm.aggregate_error_proto.as_ptr() as *mut JsObject);
        vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val))
    };
    // SAFETY: this 来自构造路径预分配或 alloc_object 新建，均存活且本段无别名。
    unsafe {
        (*this).type_tag = JsObject::OBJ_TYPE_ERROR;
    }
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

/// `Promise.try(executor, ...args)` 静态方法：以包装闭包 W 为唯一构造器实参
/// `Construct(this, «W»)`；W 被构造器以 (resolve, reject) 调用时执行 executor
/// （接收者 undefined、转发实参），正常完成经 resolve 结算、异常完成以原抛出值
/// 经 reject 结算。
///
/// # 边界与前提
/// - this 非对象 → TypeError；非构造器 this 由 `construct_ctor` 的 IsConstructor
///   校验拒绝（同样 TypeError）；
/// - 构造器自身抛错原样向调用方同步传播（不转拒绝）。
pub(super) fn promise_static_try(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !ctor.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Promise.try called on non-object"));
    }
    let executor = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let call_args: Vec<JsValue> = match args.get(2..) {
        Some(regs) => regs.iter().map(|&r| vm.reg(r)).collect(),
        None => Vec::new(),
    };
    let wrapper = vm.make_try_wrapper(executor, call_args);
    match vm.construct_ctor(ctor, &[wrapper]) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(exc) => NativeResult::Err(exc),
    }
}

/// Promise.try 的包装闭包 W：以 (resolve, reject) 实参被构造器调用，执行用户
/// executor 并按完成形态结算；两态均返回 undefined（不同步向 try 调用方抛错）。
fn promise_try_wrapper(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let resolve = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let reject = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    // SAFETY: is_object 保证指针非空且指向存活对象；只读 TRY_* 属性即时消费，不跨 GC/reset。
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let exec_si = vm.kernel_core.perm_interner().intern(TRY_EXECUTOR_PROP).0;
    let executor = vm.resolve_property(callee_obj, exec_si).unwrap_or(JsValue::undefined());
    // 转发实参自 W 的 args 数组逐位读出（元素区）。
    let mut call_args: Vec<JsValue> = Vec::new();
    let args_si = vm.kernel_core.perm_interner().intern(TRY_ARGS_PROP).0;
    if let Some(args_val) = vm.resolve_property(callee_obj, args_si) {
        if args_val.is_object() {
            // SAFETY: args_val 是存活数组对象；元素区读取不搬移，即时消费。
            let args_obj = unsafe { &*args_val.as_js_object_ptr() };
            for i in 0..args_obj.logical_len() {
                call_args.push(args_obj.get_prop_at(i));
            }
        }
    }
    match vm.call_function_sync(executor, JsValue::undefined(), &call_args) {
        Ok(value) => {
            let _ = vm.call_function_sync(resolve, JsValue::undefined(), &[value]);
        }
        Err(e) => {
            // executor 抛错：以原抛出值拒绝（原值经 last_uncaught_value 保留）。
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
        }
    }
    NativeResult::Ok(JsValue::undefined())
}
