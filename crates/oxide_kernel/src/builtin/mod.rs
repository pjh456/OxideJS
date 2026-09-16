//! 内置对象世界：持有并构造全部内置原型、构造器与全局单例（Math/JSON），
//! 支持按脏标记重建，并提供绑定层的 native 方法指针表。

mod bind;
mod construct;
mod methods;
mod rebuild;
mod world;
pub use methods::{ArrayMethods, ErrorMethods, FunctionMethods, ObjectMethods, RegExpMethods, StringMethods};
pub use world::{BuiltinWorld, FnWrapperKey};

#[macro_export]
macro_rules! bind_method {
    ($world:expr, $target:expr, $sf:expr, $sh:expr, $name:literal, $func:expr, $nargs:expr) => {{
        let _raw: *const () = $func as *const ();
        // SAFETY: $func 是 NativeFn 函数项；函数项强转为 *const () 始终有效。
        let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
        let _ = $world.bind_method($target, $sh, $sf, $name, _func_ptr, $nargs);
    }};
}

#[macro_export]
macro_rules! bind_methods {
    ($world:expr, $target:expr, $sf:expr, $sh:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $( $crate::bind_method!($world, $target, $sf, $sh, $name, $func, $nargs); )*
    };
}

#[macro_export]
macro_rules! bind_methods_static {
    ($target:expr, $sf:expr, $sh:expr, $world:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_static(
                $target, $sh, $sf, $name, _func_ptr, $nargs, $world,
            );
        })*
    };
    ($target:expr, $sf:expr, $sh:expr, $world:expr, $label:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_labeled_static(
                $target, $sh, $sf, $name, _func_ptr, $nargs, $world, $label,
            );
        })*
    };
}
