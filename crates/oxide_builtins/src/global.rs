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
pub fn global_is_nan<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() < 2 {
        f64::NAN
    } else {
        oxide_runtime_api::to_number(vm.reg(args[1]))
    };
    NativeResult::Ok(JsValue::bool(n.is_nan()))
}

/// `isFinite(x)`：ToNumber 后检查是否为有限数（隐式类型转换）。
pub fn global_is_finite<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() < 2 {
        f64::NAN
    } else {
        oxide_runtime_api::to_number(vm.reg(args[1]))
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
