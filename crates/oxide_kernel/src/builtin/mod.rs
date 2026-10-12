//! 内置对象世界：持有并构造全部内置原型、构造器与全局单例（Math/JSON），
//! 支持按脏标记重建，并提供绑定层的 native 方法指针表。

mod bind;
mod construct;
mod methods;
mod rebuild;
mod world;
pub use bind::BindEnv;
pub use methods::{ArrayMethods, ErrorMethods, FunctionMethods, ObjectMethods, RegExpMethods, StringMethods};
pub use world::{BuiltinWorld, FnWrapperKey};

/// 批量绑定一组 native 方法，条目形如 `(名称, 函数指针, 参数个数)`。
///
/// 首参 `$env` 是 kernel 绑定环境 [`BindEnv`]：把 builtin world、shape forge、
/// string forge 三要素打包为单一首参。第二形态多一个 `$label`（绑定站点标签）：
/// 非 P 目标上同名方法槽（Generator / AsyncGenerator 原型的 next / return /
/// throw）靠它区分。
#[macro_export]
macro_rules! bind_methods {
    ($env:expr, $target:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method($env, $target, $name, _func_ptr, $nargs);
        })*
    };
    ($env:expr, $target:expr, $label:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_labeled_static($env, $target, $name, _func_ptr, $nargs, $label);
        })*
    };
}
