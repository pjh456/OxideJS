//! 内置对象世界：持有并构造全部内置原型、构造器与全局单例（Math/JSON），
//! 支持按脏标记重建，并提供绑定层的 native 方法指针表。

mod bind;
mod construct;
mod methods;
mod rebuild;
mod world;
pub use methods::{ArrayMethods, ErrorMethods, FunctionMethods, ObjectMethods, RegExpMethods, StringMethods};
pub use world::{BuiltinWorld, FnWrapperKey};

/// 把一个 native 方法绑定到目标对象：构造（或复用）wrapper 函数对象，以
/// `$name` 为属性名写入目标对象的命名属性区。
///
/// `$world` 是 wrapper 归属的 `BuiltinWorld`（wrapper 原型取自它的
/// Function.prototype，本体登记进它的释放登记表，选择性重建时按键复用）；
/// `$sf` / `$sh` 为字符串 / 形状 interner；`$nargs` 是 native 函数形式参数
/// 个数，写入 wrapper 的 `length` 属性。
#[macro_export]
macro_rules! bind_method {
    ($world:expr, $target:expr, $sf:expr, $sh:expr, $name:literal, $func:expr, $nargs:expr) => {{
        let _raw: *const () = $func as *const ();
        // SAFETY: $func 是 NativeFn 函数项；函数项强转为 *const () 始终有效。
        let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
        let _ = $world.bind_method($target, $sh, $sf, $name, _func_ptr, $nargs);
    }};
}

/// 批量绑定一组 native 方法，条目形如 `(名称, 函数指针, 参数个数)`；
/// 逐条展开为 `bind_method!`。
#[macro_export]
macro_rules! bind_methods {
    ($world:expr, $target:expr, $sf:expr, $sh:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $( $crate::bind_method!($world, $target, $sf, $sh, $name, $func, $nargs); )*
    };
}

/// `bind_methods!` 的静态版本：初始化阶段调用，显式传入 `$world`（宏展开到
/// `BuiltinWorld::bind_method_static` / `bind_method_labeled_static`）。
/// 第二形态多一个 `$label`（绑定站点标签）：非 P 目标上同名方法槽
/// （Generator / AsyncGenerator 原型的 next / return / throw）靠它区分。
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
