use std::sync::Arc;

use oxide_kernel::builtin::ErrorMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

fn bind_error_subtype_constructor(
    core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, name: &str, proto_ptr: *mut JsObject,
    ctor_fn: *const (), arg_count: u8,
) {
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let world = session.builtin_world();
    let name_si = sf.intern(name).0;
    // SAFETY: ctor_fn 是调用方转成 *const () 的 NativeFn 函数项指针。
    let ctor_fn_ptr = unsafe { NativeFnPtr::from_raw(ctor_fn) };
    // 选择性重建复用：非 P 目标（家族 0）按键查找前轮旧构造器迁移，避免每轮
    // 新建导致登记表无界累积；槽键取 global 槽名，该命名空间内全局唯一。
    let reuse_key = oxide_kernel::builtin::FnWrapperKey::new(0, 0, name_si, name_si);
    let si_prototype = sf.intern("prototype").0;
    let ctor_ptr = match world.find_fn_wrapper(reuse_key, ctor_fn_ptr, arg_count) {
        Some(ptr) => {
            // 迁移内引用：prototype 槽改指新子类型原型（旧原型已被重建替换释放）。
            let ctor = unsafe { &mut *ptr };
            if let Some(pos) = sh.lookup_position(ctor.shape_id(), si_prototype) {
                ctor.set_prop_at(pos, JsValue::from_js_object(proto_ptr));
            }
            ptr
        }
        None => {
            // 子类型构造器 [[Prototype]] 指向 Error 构造器（规范：NativeError
            // 构造器继承 Error 构造器，而非直接继承 Function.prototype；instanceof
            // 走 @@hasInstance 不受影响）。
            let error_ctor_ptr = world.error_constructor.as_ptr() as *mut JsObject;
            let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(error_ctor_ptr)));
            ctor.set_function(true);
            // Error 子类型须标记可构造：new TypeError(...) 走 NEW_EXPRESSION 校验。
            ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
            ctor.set_native_fn(Some(ctor_fn_ptr));
            ctor.set_native_arg_count(arg_count);

            let si_name = sf.intern("name").0;
            let si_length = sf.intern("length").0;

            let ctor_shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
            let ctor_shape2 = sh.make_shape(ctor_shape1, si_name);
            let ctor_shape3 = sh.make_shape(ctor_shape2, si_length);
            ctor.set_shape_id(ctor_shape3);
            ctor.ensure_hash_props().push(JsValue::from_js_object(proto_ptr));
            ctor.ensure_hash_props().push(JsValue::perm_string(sf.string_ptr(name_si)));
            ctor.ensure_hash_props().push(JsValue::int(arg_count as i32));
            // 构造器 prototype 槽按规范 { writable:false, enumerable:false, configurable:false }。
            ctor.set_data_meta(0u32, PropAttributes::new(false, false, false));
            // 构造器 name 槽按规范 { writable:false, enumerable:false, configurable:true }。
            ctor.set_data_meta(1u32, PropAttributes::new(false, false, true));
            // 构造器 length 按规范为不可写不可枚举（Function.length 属性描述符约定）。
            ctor.set_data_meta(2u32, PropAttributes::new(false, false, true));

            let ctor_ptr = Box::into_raw(ctor);
            // 登记进 world 释放表（带复用键）：session 收尾统一释放构造器本体与属性区。
            world.track_fn_wrapper(ctor_ptr, reuse_key);
            ctor_ptr
        }
    };
    let ctor_val = JsValue::from_js_object(ctor_ptr);

    // 原型上的 constructor 既有槽原位更新，无槽时开新槽（重建原型无既有槽）；
    // 非枚举数据属性（与 Error.prototype.constructor 一致）。
    let proto = unsafe { &mut *proto_ptr };
    let si_constructor = sf.intern("constructor").0;
    if let Some(pos) = sh.lookup_position(proto.shape_id(), si_constructor) {
        proto.set_prop_at(pos, ctor_val);
    } else {
        let proto_ctor_shape = sh.make_shape(proto.shape_id(), si_constructor);
        proto.set_shape_id(proto_ctor_shape);
        proto.ensure_hash_props().push(ctor_val);
        let ctor_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        proto.set_data_meta(ctor_pos, PropAttributes::new(true, false, true));
    }

    // 全局子类型构造器槽位既有槽原位更新（旧家族构造器指针不得滞留在属性 vec），
    // 无槽时开新槽；描述符非枚举（规范全局构造器描述符约定）。
    if let Some(pos) = sh.lookup_position(global.shape_id(), name_si) {
        global.set_prop_at(pos, ctor_val);
    } else {
        let global_shape = sh.make_shape(global.shape_id(), name_si);
        global.set_shape_id(global_shape);
        global.ensure_hash_props().push(ctor_val);
        let global_pos = global.prop_vec_len().saturating_sub(1) as u32;
        global.set_data_meta(global_pos, PropAttributes::new(true, false, true));
        global.bump_generation();
    }
}

/// 把 Error 及各子类型（TypeError/ReferenceError/...）构造器与原型方法绑定到 global。
pub fn bind_error(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let error_methods = ErrorMethods {
        error: oxide_builtins::error::error_constructor::<crate::vm::Vm> as *const (),
        type_error: oxide_builtins::error::type_error_constructor::<crate::vm::Vm> as *const (),
        reference_error: oxide_builtins::error::reference_error_constructor::<crate::vm::Vm> as *const (),
        range_error: oxide_builtins::error::range_error_constructor::<crate::vm::Vm> as *const (),
        syntax_error: oxide_builtins::error::syntax_error_constructor::<crate::vm::Vm> as *const (),
        uri_error: oxide_builtins::error::uri_error_constructor::<crate::vm::Vm> as *const (),
        eval_error: oxide_builtins::error::eval_error_constructor::<crate::vm::Vm> as *const (),
        suppressed_error: oxide_builtins::error::suppressed_error_constructor::<crate::vm::Vm> as *const (),
        to_string: oxide_builtins::error::error_to_string::<crate::vm::Vm> as *const (),
        is_error: oxide_builtins::error::error_is_error::<crate::vm::Vm> as *const (),
        to_json: oxide_builtins::error::error_to_json::<crate::vm::Vm> as *const (),
    };
    session.builtin_world().bind_error_methods(
        &error_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    // stack 为访问器成对安装（getter 现算栈串 / setter 走
    // SetterThatIgnoresPrototypeProperties），经 getset 绑定器登记复用键，
    // 选择性重建时同槽迁移旧 wrapper。
    {
        let proto_ptr = session.builtin_world().error_proto.as_ptr() as *mut JsObject;
        // SAFETY: error_proto 由 session 持有，存活整个 session；本块内只改其 shape/属性区，无 reset。
        let proto = unsafe { &mut *proto_ptr };
        let si_stack = core.perm_interner().intern("stack").0;
        super::bind_accessor_getset(
            core,
            session,
            proto,
            si_stack,
            "get stack",
            "set stack",
            oxide_builtins::error::error_stack_getter::<crate::vm::Vm> as *const (),
            oxide_builtins::error::error_stack_setter::<crate::vm::Vm> as *const (),
        );
    }

    // 全局 Error 槽位既有槽原位更新（旧家族构造器指针不得滞留在属性 vec），
    // 无槽时开新槽；描述符非枚举（规范 { writable:true, enumerable:false, configurable:true }）。
    let si_err = core.perm_interner().intern("Error").0;
    let err_val = JsValue::from_js_object(session.builtin_world().error_constructor.as_ptr() as *mut JsObject);
    if let Some(pos) = core.shape_forge().lookup_position(global.shape_id(), si_err) {
        global.set_prop_at(pos, err_val);
    } else {
        let err_shape = core.shape_forge().make_shape(global.shape_id(), si_err);
        global.set_shape_id(err_shape);
        global.ensure_hash_props().push(err_val);
        let err_pos = global.prop_vec_len().saturating_sub(1) as u32;
        global.set_data_meta(err_pos, PropAttributes::new(true, false, true));
        global.bump_generation();
    }

    bind_error_subtype_constructor(
        core,
        session,
        global,
        "TypeError",
        session.builtin_world().type_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::type_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_constructor(
        core,
        session,
        global,
        "ReferenceError",
        session.builtin_world().reference_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::reference_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_constructor(
        core,
        session,
        global,
        "RangeError",
        session.builtin_world().range_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::range_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_constructor(
        core,
        session,
        global,
        "SyntaxError",
        session.builtin_world().syntax_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::syntax_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_constructor(
        core,
        session,
        global,
        "URIError",
        session.builtin_world().uri_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::uri_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_constructor(
        core,
        session,
        global,
        "EvalError",
        session.builtin_world().eval_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::eval_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );

    // SuppressedError 为三参构造器（error, suppressed, message），length=3。
    bind_error_subtype_constructor(
        core,
        session,
        global,
        "SuppressedError",
        session.builtin_world().suppressed_error_proto.as_ptr() as *mut JsObject,
        oxide_builtins::error::suppressed_error_constructor::<crate::vm::Vm> as *const (),
        3,
    );

    {
        let err_ctor_ptr = session.builtin_world().error_constructor.as_ptr() as *mut JsObject;
        let err_ctor = unsafe { &mut *err_ctor_ptr };
        err_ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
        // SAFETY: error_constructor 是 NativeFn 函数项。
        err_ctor.set_native_fn(Some(unsafe {
            NativeFnPtr::from_raw(oxide_builtins::error::error_constructor::<crate::vm::Vm> as *const ())
        }));
        // 主 Error 构造器补 length 槽（值 1，与子类型构造器一致）。
        let sf = core.perm_interner().as_ref();
        let sh = core.shape_forge().as_ref();
        let si_length = sf.intern("length").0;
        let length_shape = sh.make_shape(err_ctor.shape_id(), si_length);
        err_ctor.set_shape_id(length_shape);
        err_ctor.ensure_hash_props().push(JsValue::int(1));
        let length_pos = err_ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        err_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    }
}
