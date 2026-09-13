//! eval 全局函数：非字符串原样返回；字符串按脚本模式动态编译执行。
//!
//! # 契约
//! - 非字符串实参**不** ToString（`eval(new String("1+1"))` 返回对象本身）。
//! - 字符串经动态编译以 global 为 this 同步调用；编译失败 → SyntaxError；
//!   运行异常 → 优先重抛原始异常值（`take_uncaught_value`，`eval("throw 1")` 捕获到 `1`）。
//! - 脚本模式：完成值保留（`eval("1+2")` → 3）；var/函数声明落全局对象；let/const 词法隔离。

use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::error;

/// `eval(x)`：非字符串实参原样返回；字符串按脚本模式编译执行。
///
/// # 步骤
/// 1. 实参非字符串 → 原样返回（规范不 ToString）。
/// 2. 字符串 → 动态编译为脚本模块（`create_dynamic_script`），以 global 为 this 同步调用。
/// 3. 编译失败 → SyntaxError；运行异常 → 优先重抛原始异常值（`take_uncaught_value`）。
///
/// # 边界与前提
/// - 脚本模式：var/函数声明落全局对象，完成值保留，let/const 词法隔离。
/// - 无实参等价于 `eval(undefined)` → undefined。
pub fn eval<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let arg = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !arg.is_string() {
        return NativeResult::Ok(arg);
    }
    let code = vm.lookup_str(arg).unwrap_or_default();
    match vm.create_dynamic_script(&code) {
        Ok(func) => {
            let global = JsValue::from_js_object(vm.session().global_object().as_ptr() as *mut JsObject);
            match vm.call_function_sync(func, global, &[]) {
                Ok(val) => NativeResult::Ok(val),
                Err(e) => {
                    NativeResult::Err(vm.take_uncaught_value().unwrap_or_else(|| error::create_from_text(vm, &e)))
                }
            }
        }
        Err(e) => NativeResult::Err(error::create_syntax_error(vm, &e)),
    }
}
