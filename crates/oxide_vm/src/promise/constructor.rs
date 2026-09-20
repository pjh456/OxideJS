//! Promise 构造与内建安装：`Construct(C, args)` 通用路径（native /
//! bytecode 构造器）与 %Promise% 内建对象初始化。
//!
//! `init_promise_intrinsics` 在 VM 创建与 `full_reset` 后调用：原型继承
//! session 的 Object/Function 原型，session 重建后须重挂并重绑 global 槽。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::vm::{FrameArgs, FrameContinuation, Vm};

use super::aggregate::{
    promise_static_all, promise_static_all_settled, promise_static_any, promise_static_race, promise_static_reject,
    promise_static_resolve, promise_static_with_resolvers,
};
use super::reactions::{promise_catch, promise_finally, promise_then};
use super::{is_constructor_value, PromiseState, PromiseStateKind};

impl Vm {
    /// 创建空 Promise 对象（proto = `%Promise.prototype%`），状态盒为 Pending。
    pub(super) fn create_promise_object(&mut self) -> JsValue {
        let proto_val = JsValue::from_js_object(self.promise_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        // SAFETY: `alloc_object` 返回非空 arena 指针；此处写 type_tag 与 native_data
        // （Box<PromiseState> 由 Box::into_raw 分配，随对象释放），无别名。
        let obj = unsafe { &mut *ptr };
        obj.type_tag = JsObject::OBJ_TYPE_PROMISE;
        let state = Box::new(PromiseState {
            state: PromiseStateKind::Pending,
            result: JsValue::undefined(),
            reactions: Vec::new(),
            resolve_fn: JsValue::undefined(),
            reject_fn: JsValue::undefined(),
            already_resolved: false,
            promoted_clone: std::ptr::null_mut(),
        });
        obj.set_native_data(Box::into_raw(state) as *mut u8);
        JsValue::from_js_object(ptr)
    }

    /// 构造调用（Construct(C, args)）：native 构造器值传递调用，bytecode 构造器
    /// 压构造帧执行（含 derived 构造器 super() 语义），返回值非对象时回退到新对象。
    ///
    /// # 边界与前提
    /// - `ctor` 非可构造值（箭头 / 非构造 native / 普通值）返回 TypeError 的 `Err`；
    ///   消费 `last_uncaught_value`，调用方不得再取槽。
    pub(crate) fn construct_ctor(&mut self, ctor: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
        // IsConstructor 校验：arrow / 非构造 native / 普通值拒绝。
        if !is_constructor_value(ctor) {
            return Err(oxide_builtins::error::create_type_error(self, "constructor is not a constructor"));
        }
        // SAFETY: `is_constructor_value` 已含对象校验，指针非空且指向存活对象；
        // 此处只读 native_fn() 判定分支，不跨 GC/reset。
        let ctor_obj = unsafe { &*ctor.as_js_object_ptr() };
        // native 构造器：receiver 为新对象，值传递调用（%Promise% 主路径）。
        if ctor_obj.native_fn().is_some() {
            let this_ptr = self.alloc_ctor_this(ctor_obj)?;
            let this_val = JsValue::from_js_object(this_ptr);
            return match self.call_function_sync(ctor, this_val, args) {
                Ok(ret) if ret.is_object() => Ok(ret),
                Ok(_) => Ok(this_val),
                Err(e) => Err(self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e))),
            };
        }
        self.call_constructor_bytecode_inline(ctor, ctor_obj, args)
    }

    /// 分配构造 this：proto = ctor.prototype（缺省 Object.prototype）。
    fn alloc_ctor_this(&mut self, ctor_obj: &JsObject) -> Result<*mut JsObject, JsValue> {
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        let proto_val = match self.resolve_property(ctor_obj, proto_si) {
            Some(p) if p.is_object() => p,
            _ => JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject),
        };
        Ok(self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val)))
    }

    /// bytecode 构造器构造调用：压构造帧内嵌 dispatch 执行（derived 构造器
    /// super() 前 this = undefined，new.target = ctor），结果经 `do_return` 交付
    /// regs[0]（非对象回退构造 this）。
    ///
    /// # 副作用
    /// - 经 `save_inline_state` / `restore_inline_state` 保存恢复调用方执行状态。
    /// - `construct_dispatch` 标志令构造帧弹出时交付结果而非继续执行。
    fn call_constructor_bytecode_inline(
        &mut self, ctor: JsValue, ctor_obj: &JsObject, args: &[JsValue],
    ) -> Result<JsValue, JsValue> {
        // 按构造器自身记录的表代际解析：代际表缺失（已回收）或下标越界与
        // 原生函数哨兵同口径报 TypeError。
        let callee_module = match self.callee_module(ctor_obj) {
            Some(m) => m,
            None => {
                return Err(oxide_builtins::error::create_type_error(self, "constructor is not a constructor"));
            }
        };
        if callee_module.is_generator || callee_module.is_async {
            return Err(oxide_builtins::error::create_type_error(self, "constructor is not a constructor"));
        }
        // 先拷出寄存器数，释放对代际表的借用后再进入可变借用区。
        let callee_reg_count = callee_module.n_registers;
        let new_obj_ptr = self.alloc_ctor_this(ctor_obj)?;
        let new_obj_val = JsValue::from_js_object(new_obj_ptr);
        // derived 构造器 super() 前 this 为 undefined，基类 this = 新对象。
        let this_value = if ctor_obj.is_derived_constructor() {
            JsValue::undefined()
        } else {
            new_obj_val
        };
        // 入口已解析代际表：读 n_registers 定保存窗口。
        let window = self.active_reg_limit.max(callee_reg_count).max(1) as usize;
        let saved = self.save_inline_state(window);
        let prev_construct = self.construct_dispatch;
        self.construct_dispatch = true;
        let result = self
            .push_bytecode_frame(
                ctor,
                this_value,
                FrameArgs::Slice(args),
                Some(0),
                Some(new_obj_val),
                ctor,
                FrameContinuation::None,
                0,
            )
            .and_then(|_| self.dispatch());
        self.construct_dispatch = prev_construct;
        self.restore_inline_state(saved);
        match result {
            Ok(v) => Ok(v),
            Err(e) => Err(self
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e))),
        }
    }

    /// 初始化/重建 Promise 内建对象：`%Promise%` 构造器与 `%Promise.prototype%`，
    /// 绑定到 global 的 `Promise` 槽。
    ///
    /// 在 VM 创建与 `full_reset` 后调用——原型继承自 session 的 Object/Function
    /// 原型，session 重建后须重挂并重绑 global 槽。
    pub(crate) fn init_promise_intrinsics(&mut self) {
        let sf = self.kernel_core.perm_interner().as_ref();
        let sh = self.kernel_core.shape_forge().as_ref();
        let fn_proto_val = self.session.builtin_world().fn_proto_val();
        let world = self.session.builtin_world();
        let object_proto_val =
            JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);

        // %Promise.prototype%：proto = Object.prototype，方法 then/catch/finally + @@toStringTag。
        let mut proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto_val));
        let ctor_si = sf.intern("constructor").0;
        let ctor_shape = sh.make_shape(proto.shape_id(), ctor_si);
        proto.set_shape_id(ctor_shape);
        proto.push_prop(JsValue::undefined());
        proto.set_data_meta(0u32, PropAttributes::new(true, false, true));
        oxide_kernel::bind_methods_static!(
            &mut proto,
            sf,
            sh,
            world,
            ("then", promise_then as *const (), 2),
            ("catch", promise_catch as *const (), 1),
            ("finally", promise_finally as *const (), 1),
        );
        let tag_si = sf.intern("@@toStringTag").0;
        let tag_shape = sh.make_shape(proto.shape_id(), tag_si);
        proto.set_shape_id(tag_shape);
        let tag_pos = proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("Promise").0)));
        proto.set_data_meta(tag_pos, PropAttributes::new(false, false, true));

        // %Promise% 构造器：proto = Function.prototype，静态方法 resolve/reject。
        // 自身属性顺序：length、name、prototype（CreateBuiltinFunction 顺序）。
        let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
        ctor.set_function(true);
        ctor.set_native_arg_count(1);
        // SAFETY: promise_constructor 是 NativeFn 函数项。
        ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(promise_constructor as *const ()) }));
        ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
        let length_si = sf.intern("length").0;
        let ctor_shape1 = sh.make_shape(ctor.shape_id(), length_si);
        ctor.set_shape_id(ctor_shape1);
        let lpos = ctor.push_prop(JsValue::int(1));
        ctor.set_data_meta(lpos, PropAttributes::new(false, false, true));
        let name_si = sf.intern("name").0;
        let ctor_shape2 = sh.make_shape(ctor.shape_id(), name_si);
        ctor.set_shape_id(ctor_shape2);
        let npos = ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("Promise").0)));
        ctor.set_data_meta(npos, PropAttributes::new(false, false, true));
        let proto2_si = sf.intern("prototype").0;
        let ctor_shape3 = sh.make_shape(ctor.shape_id(), proto2_si);
        ctor.set_shape_id(ctor_shape3);
        ctor.push_prop(JsValue::undefined());
        ctor.set_data_meta(2u32, PropAttributes::new(false, false, false));
        oxide_kernel::bind_methods_static!(
            &mut ctor,
            sf,
            sh,
            world,
            ("resolve", promise_static_resolve as *const (), 1),
            ("reject", promise_static_reject as *const (), 1),
            ("all", promise_static_all as *const (), 1),
            ("race", promise_static_race as *const (), 1),
            ("allSettled", promise_static_all_settled as *const (), 1),
            ("any", promise_static_any as *const (), 1),
            ("withResolvers", promise_static_with_resolvers as *const (), 0),
        );

        // 固定地址后互相接线：proto.constructor ↔ ctor.prototype。
        Self::swap_intrinsic_proto(&mut self.promise_proto, *proto);
        Self::swap_intrinsic_proto(&mut self.promise_constructor, *ctor);
        // SAFETY: `promise_proto` 为 `P<JsObject>`（Arc 透明包装），堆址固定且本行前刚经
        // swap_intrinsic_proto 落地；与下一处 ctor_mut 指向不同对象，无别名。
        let proto_mut = unsafe { &mut *self.promise_proto.as_mut_ptr() };
        proto_mut.set_prop_at(0u32, JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject));
        // SAFETY: `promise_constructor` 同为堆址固定的存活 `P<JsObject>`（Arc 透明包装）；
        // 此处写 prototype 槽位（下标 2），与 proto_mut 分属不同对象，无别名。
        let ctor_mut = unsafe { &mut *self.promise_constructor.as_mut_ptr() };
        // prototype 槽位在 length/name 之后（下标 2）。
        ctor_mut.set_prop_at(2u32, JsValue::from_js_object(self.promise_proto.as_ptr() as *mut JsObject));

        // 绑定 global：槽已存在则更新（full_reset 未重建 global 时旧槽指向已弃 ctor）。
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        // SAFETY: global 对象由 session 持有，存活整个 session；本函数内只改其
        // shape/属性区，期间无 reset 或对象搬移。
        let global = unsafe { &mut *global_ptr };
        let si = self.kernel_core.perm_interner().intern("Promise").0;
        let ctor_val = JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
            global.set_prop_at(pos, ctor_val);
        } else {
            let shape = self.kernel_core.shape_forge().make_shape(global.shape_id(), si);
            global.set_shape_id(shape);
            let pos = global.push_prop(ctor_val);
            // global 数据属性：writable:true、enumerable:false、configurable:true。
            global.set_data_meta(pos, PropAttributes::new(true, false, true));
            global.bump_generation();
        }

        // Promise.any 的拒绝路径依赖 AggregateError 内建。
        self.init_aggregate_error_intrinsics();
    }
}

/// `Promise` 构造器：设置状态盒并同步调用 executor(resolve, reject)。
fn promise_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    // `new` 调用时 this 是 proto 链含 %Promise.prototype% 的新对象；裸调用拒绝。
    if !vm.has_promise_proto(this_val) {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Promise must be called with new"));
    }
    // SAFETY: `has_promise_proto` 已校验 this 为存活对象且原型链含 %Promise.prototype%；
    // 此处写 type_tag 后即新建状态盒，无别名。
    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_PROMISE;
    let resolve = vm.make_resolve_reject_fn(this_val, false, false);
    let reject = vm.make_resolve_reject_fn(this_val, true, false);
    let state = Box::new(PromiseState {
        state: PromiseStateKind::Pending,
        result: JsValue::undefined(),
        reactions: Vec::new(),
        resolve_fn: resolve,
        reject_fn: reject,
        already_resolved: false,
        promoted_clone: std::ptr::null_mut(),
    });
    obj.set_native_data(Box::into_raw(state) as *mut u8);
    let executor = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !oxide_builtins::iterator::is_callable(executor) {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Promise executor is not a function"));
    }
    match vm.call_function_sync(executor, JsValue::undefined(), &[resolve, reject]) {
        Ok(_) => NativeResult::Ok(this_val),
        Err(e) => {
            // executor 抛错：以抛出的值为拒绝原因（原值经 last_uncaught_value 保留）。
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            let _ = vm.reject_promise(this_val, exc);
            NativeResult::Ok(this_val)
        }
    }
}
