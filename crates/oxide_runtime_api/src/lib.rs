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

#[cfg(test)]
use oxide_types::value::JsValue;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(d: f64) -> String {
        js_number_to_string(d)
    }

    #[test]
    fn number_to_string_boundaries() {
        assert_eq!(fmt(1e21), "1e+21");
        assert_eq!(fmt(1e22), "1e+22");
        assert_eq!(fmt(1e20), "100000000000000000000");
        assert_eq!(fmt(123.45), "123.45");
        assert_eq!(fmt(0.000001), "0.000001");
        assert_eq!(fmt(0.0000001), "1e-7");
        assert_eq!(fmt(1e15), "1000000000000000");
        assert_eq!(fmt(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(fmt(0.0), "0");
        assert_eq!(fmt(-0.0), "0");
        assert_eq!(fmt(-123.45), "-123.45");
        assert_eq!(fmt(-1e21), "-1e+21");
        assert_eq!(fmt(1e16), "10000000000000000");
        assert_eq!(fmt(f64::MAX), "1.7976931348623157e+308");
        assert_eq!(fmt(5e-324), "5e-324");
        assert_eq!(fmt(1.5e20), "150000000000000000000");
        assert_eq!(fmt(f64::NAN), "NaN");
        assert_eq!(fmt(f64::INFINITY), "Infinity");
        assert_eq!(fmt(f64::NEG_INFINITY), "-Infinity");
        // 整数 double 快路径：2^53 内直写（与最短表示一致）。
        assert_eq!(fmt(42.0), "42");
        assert_eq!(fmt(-42.0), "-42");
        assert_eq!(fmt(123.0), "123");
        assert_eq!(fmt(2.0), "2");
        assert_eq!(fmt(1000000.0), "1000000");
        assert_eq!(fmt(-1000000.0), "-1000000");
        // 2^53 边界：等于 2^53 落 ryu 路径，输出仍为定点。
        assert_eq!(fmt(9_007_199_254_740_992.0), "9007199254740992");
        // 2^63：精确值 9223372036854775808 的最短表示为 17 位舍入
        // "9223372036854776000"（规范输出，非精确值）。
        assert_eq!(fmt(2f64.powi(63)), "9223372036854776000");
    }

    #[test]
    fn push_to_string_matches_to_string() {
        let samples: Vec<f64> = vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            42.0,
            -42.0,
            std::f64::consts::PI,
            1e15,
            1e16,
            1e20,
            1e21,
            1e22,
            9_007_199_254_740_992.0,
            2f64.powi(63),
            f64::MAX,
            5e-324,
            0.1 + 0.2,
            123.45,
            -123.45,
            1e-7,
            0.000001,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for &d in &samples {
            let mut buf = String::new();
            push_to_string(JsValue::float(d), &mut buf);
            assert_eq!(buf, to_string(JsValue::float(d)), "double {d} 的 push/to_string 输出不一致");
            assert_eq!(buf, js_number_to_string(d), "double {d} 的 push/js_number_to_string 输出不一致");
        }
    }

    #[test]
    fn push_to_string_negative_double_appends_to_nonempty_buffer() {
        // 追加语义回归：非空前缀下负数 double 的负号必须紧跟当前数值，不能插到
        // 整个缓冲最前。覆盖整数快路径、ryu 定点（≥2^53）、ryu 科学计数与边界值。
        let samples: Vec<f64> = vec![
            -42.0,                    // 整数快路径
            -1.5,                     // ryu 定点
            -9_007_199_254_740_994.0, // 2^53+2：≥2^53 可精确表示整数，落 ryu 路径
            -2f64.powi(63),           // ryu 科学计数（规范最短表示）
            -1e21,                    // ryu 科学计数
            -1e-7,                    // ryu 科学计数（小指数）
            -f64::MAX,
            -5e-324,
        ];
        for &d in &samples {
            let mut buf = String::from("pre");
            push_to_string(JsValue::float(d), &mut buf);
            let expected = format!("pre{}", js_number_to_string(d));
            assert_eq!(buf, expected, "非空前缀 + double {d} 的追加语义破坏，实际 {buf}");
        }
    }
}
