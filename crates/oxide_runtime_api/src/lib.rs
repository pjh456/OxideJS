//! `oxide_runtime_api` —— builtins 与 VM 之间的抽象接口。
//!
//! Builtins 以泛型方式针对 [`VmHost`] trait 编写
//! （`fn xxx<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult`）；`Vm`
//! 实现 `VmHost`。这打破了 builtins crate 与 `oxide_vm` 之间本会形成的
//! 循环依赖：
//!
//! `oxide_types ← oxide_kernel ← oxide_runtime_api ← oxide_builtins ← oxide_vm`
//!
//! trait 面向泛型而非对象安全：单态化 `H = Vm` 使每个 `host.*()` 调用内联，
//! 相对 builtins 直接位于 `oxide_vm` 内部没有运行时开销。

/// ECMAScript 强转函数族。
mod coercion;
/// builtins 依赖的 `VmHost` 能力面。
mod host;
/// builtin native 函数的三态返回值。
mod native_result;
mod runtime_api_log;

pub use coercion::{
    abstract_eq, bigint_data, bigint_to_f64, bigint_to_string, format_error_message, js_number_to_string,
    push_to_string, push_units_to, relational_compare, same_value, same_value_zero, strict_equality, string_concat,
    string_value_eq, to_bigint_full, to_boolean, to_int32, to_integer_or_infinity, to_length, to_number,
    to_number_full, to_object, to_primitive, to_string, to_string_for_string_constructor, to_string_full,
    to_string_value_full, to_uint32, to_units_full, well_known_symbol_id, well_known_symbol_name, write_number_into,
    ToPrimitiveHint,
};
pub use host::VmHost;
pub use native_result::NativeResult;
