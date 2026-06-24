mod annex_b;
mod uri;

use crate::coercion;
use crate::vm::Vm;

pub use annex_b::{js_escape, js_unescape};
pub use uri::{decode_uri, decode_uri_component, encode_uri, encode_uri_component};

fn string_arg(vm: &mut Vm, args: &[u8]) -> String {
    if args.len() > 1 {
        coercion::to_string(vm.reg(args[1]))
    } else {
        "undefined".to_string()
    }
}

fn parse_hex_u8(slice: &[u8]) -> Option<u8> {
    std::str::from_utf8(slice).ok().and_then(|s| u8::from_str_radix(s, 16).ok())
}
