pub mod bind_array;
pub mod bind_boolean;
pub mod bind_date;
pub mod bind_error;
pub mod bind_function;
pub mod bind_json;
pub mod bind_map;
pub mod bind_math;
pub mod bind_number;
pub mod bind_object;
pub mod bind_set;
pub mod bind_string;

use std::sync::Arc;

use oxide_kernel::kernel::OxideKernel;

#[macro_export]
macro_rules! bind_constructor {
    ($kernel:expr, $global:expr, $name:literal, $ctor_ptr:expr, $ctor_fn:path, $nargs:literal) => {{
        let si = $kernel.string_forge().intern($name).0;
        let shape = $kernel.shape_forge().make_shape($global.shape_id(), si);
        let val = $crate::JsValue::from_js_object($ctor_ptr);
        $global.set_shape_id(shape);
        $global.push_prop(val);
        let ctor = unsafe { &mut *$ctor_ptr };
        ctor.set_native_fn(Some($ctor_fn as *const ()));
        ctor.set_native_arg_count($nargs);
    }};
}

#[macro_export]
macro_rules! bind_constructor_hash {
    ($kernel:expr, $global:expr, $name:literal, $ctor_ptr:expr, $ctor_fn:path, $nargs:literal) => {{
        let si = $kernel.string_forge().intern($name).0;
        let shape = $kernel.shape_forge().make_shape($global.shape_id(), si);
        let val = $crate::JsValue::from_js_object($ctor_ptr);
        $global.set_shape_id(shape);
        $global.ensure_hash_props().push(Box::new(val));
        $global.bump_generation();
        let ctor = unsafe { &mut *$ctor_ptr };
        ctor.set_native_fn(Some($ctor_fn as *const ()));
        ctor.set_native_arg_count($nargs);
    }};
}

pub fn init_kernel_builtins(kernel: &Arc<OxideKernel>) {
    let global_ptr = kernel.global_object().as_ptr() as *mut oxide_types::object::JsObject;
    let global = unsafe { &mut *global_ptr };

    bind_object::bind_object(kernel, global);
    bind_array::bind_array(kernel, global);
    bind_error::bind_error(kernel, global);
    bind_string::bind_string(kernel, global);
    bind_number::bind_number(kernel, global);
    bind_math::bind_math(kernel, global);
    bind_json::bind_json(kernel, global);
    bind_date::bind_date(kernel, global);
    bind_set::bind_set(kernel, global);
    bind_map::bind_map(kernel, global);
    bind_boolean::bind_boolean(kernel, global);
    bind_function::bind_function(kernel);
}
