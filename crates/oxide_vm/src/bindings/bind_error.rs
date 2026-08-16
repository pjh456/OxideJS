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
    let function_proto_ptr = session.builtin_world().function_proto.as_ptr() as *mut JsObject;

    let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto_ptr)));
    ctor.set_function(true);
    // Error 子类型须标记可构造：new TypeError(...) 走 NEW_EXPRESSION 校验。
    ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
    // SAFETY: ctor_fn 是调用方转成 *const () 的 NativeFn 函数项指针。
    ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(ctor_fn) }));
    ctor.set_native_arg_count(arg_count);

    let si_prototype = sf.intern("prototype").0;
    let si_name = sf.intern("name").0;
    let si_length = sf.intern("length").0;
    let name_si = sf.intern(name).0;

    let ctor_shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = sh.make_shape(ctor_shape1, si_name);
    let ctor_shape3 = sh.make_shape(ctor_shape2, si_length);
    ctor.set_shape_id(ctor_shape3);
    ctor.ensure_hash_props().push(JsValue::from_js_object(proto_ptr));
    ctor.ensure_hash_props().push(JsValue::perm_string(sf.string_ptr(name_si)));
    ctor.ensure_hash_props().push(JsValue::int(arg_count as i32));
    // 构造器 length 按规范为不可写不可枚举（Function.length 属性描述符约定）。
    ctor.set_data_meta(2u32, PropAttributes::new(false, false, true));

    let ctor_ptr = Box::into_raw(ctor);

    let proto = unsafe { &mut *proto_ptr };
    let proto_ctor_shape = sh.make_shape(proto.shape_id(), sf.intern("constructor").0);
    proto.set_shape_id(proto_ctor_shape);
    proto.ensure_hash_props().push(JsValue::from_js_object(ctor_ptr));
    // 原型上的 constructor 按规范为非枚举数据属性（与 Error.prototype.constructor 一致）。
    let ctor_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    proto.set_data_meta(ctor_pos, PropAttributes::new(true, false, true));

    let global_shape = sh.make_shape(global.shape_id(), name_si);
    global.set_shape_id(global_shape);
    global.ensure_hash_props().push(JsValue::from_js_object(ctor_ptr));
    // 全局子类型构造器槽位非枚举（规范全局构造器描述符约定）。
    let global_pos = global.prop_vec_len().saturating_sub(1) as u32;
    global.set_data_meta(global_pos, PropAttributes::new(true, false, true));
    global.bump_generation();
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
        stack: oxide_builtins::error::error_stack_getter::<crate::vm::Vm> as *const (),
    };
    session.builtin_world().bind_error_methods(
        &error_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    let si_err = core.perm_interner().intern("Error").0;
    let err_shape = core.shape_forge().make_shape(global.shape_id(), si_err);
    let err_val = JsValue::from_js_object(session.builtin_world().error_constructor.as_ptr() as *mut JsObject);
    global.set_shape_id(err_shape);
    global.ensure_hash_props().push(err_val);
    // 全局 Error 构造器槽位非枚举（规范 { writable:true, enumerable:false, configurable:true }）。
    let err_pos = global.prop_vec_len().saturating_sub(1) as u32;
    global.set_data_meta(err_pos, PropAttributes::new(true, false, true));
    global.bump_generation();

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
