use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::value::JsValue;
mod annex_b;
mod uri;

/// 全局函数模块：聚合 Annex B 的 `escape`/`unescape` 与 URI 处理函数
/// （`encodeURI`/`decodeURI`/`encodeURIComponent`/`decodeURIComponent`）。
pub use annex_b::{js_escape, js_unescape};
/// URI 编解码全局函数（`encodeURI`/`decodeURI` 等）。
pub use uri::{decode_uri, decode_uri_component, encode_uri, encode_uri_component};

/// `isNaN(x)`：ToNumber 后检查是否为 NaN（隐式类型转换）。
///
/// ToNumber 可抛：Symbol 抛 TypeError，对象 ToPrimitive 触发 valueOf/toString
/// 抛出的原生异常原样传播。
pub fn global_is_nan<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() < 2 {
        f64::NAN
    } else {
        match oxide_runtime_api::to_number_full(vm.reg(args[1]), vm) {
            Ok(n) => n,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        }
    };
    NativeResult::Ok(JsValue::bool(n.is_nan()))
}

/// `isFinite(x)`：ToNumber 后检查是否为有限数（隐式类型转换）。
///
/// ToNumber 可抛：Symbol 抛 TypeError，对象 ToPrimitive 触发 valueOf/toString
/// 抛出的原生异常原样传播。
pub fn global_is_finite<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() < 2 {
        f64::NAN
    } else {
        match oxide_runtime_api::to_number_full(vm.reg(args[1]), vm) {
            Ok(n) => n,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        }
    };
    NativeResult::Ok(JsValue::bool(n.is_finite()))
}

fn string_arg<H: VmHost>(vm: &mut H, args: &[u8]) -> String {
    if args.len() > 1 {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        "undefined".to_string()
    }
}

fn parse_hex_u8(slice: &[u8]) -> Option<u8> {
    std::str::from_utf8(slice).ok().and_then(|s| u8::from_str_radix(s, 16).ok())
}
