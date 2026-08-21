//! Console 内置对象实现。
//!
//! 单例全局对象，所有方法输出到 stderr（使用 eprintln!）。
//! trace 除消息外额外打印调用栈（std::backtrace::Backtrace）。

use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::value::JsValue;

fn multi_string<H: VmHost>(vm: &mut H, args: &[u8]) -> String {
    let mut output = String::new();
    let mut first = true;
    for i in 1..args.len() as usize {
        if !first {
            output.push(' ');
        }
        first = false;
        let val = vm.reg(args[i]);
        let part = if val.is_string() {
            oxide_runtime_api::to_string(val)
        } else {
            let primitive =
                oxide_runtime_api::to_primitive(val, oxide_runtime_api::ToPrimitiveHint::String, vm)
                    .unwrap_or_else(|_| JsValue::undefined());
            oxide_runtime_api::to_string(primitive)
        };
        output.push_str(&part);
    }
    output
}

/// `console.log(...args)`：输出消息到 stderr。
pub fn log<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let msg = multi_string(vm, args);
    eprintln!("{}", msg);
    NativeResult::Ok(JsValue::undefined())
}

/// `console.warn(...args)`：输出警告到 stderr。
pub fn warn<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let msg = multi_string(vm, args);
    eprintln!("warn: {}", msg);
    NativeResult::Ok(JsValue::undefined())
}

/// `console.error(...args)`：输出错误到 stderr。
pub fn error<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let msg = multi_string(vm, args);
    eprintln!("error: {}", msg);
    NativeResult::Ok(JsValue::undefined())
}

/// `console.info(...args)`：输出信息到 stderr。
pub fn info<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let msg = multi_string(vm, args);
    eprintln!("info: {}", msg);
    NativeResult::Ok(JsValue::undefined())
}

/// `console.debug(...args)`：输出调试消息到 stderr。
pub fn debug<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let msg = multi_string(vm, args);
    eprintln!("debug: {}", msg);
    NativeResult::Ok(JsValue::undefined())
}

/// `console.trace(...args)`：输出跟踪消息及调用栈到 stderr。
pub fn trace<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let msg = multi_string(vm, args);
    let backtrace = std::backtrace::Backtrace::capture();
    eprintln!("trace: {}\n{:?}", msg, backtrace);
    NativeResult::Ok(JsValue::undefined())
}
