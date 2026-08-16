//! eval 全局函数：非字符串原样返回；字符串按脚本/表达式动态编译执行。
//!
//! # 契约
//! - 非字符串实参**不** ToString（`eval(new String("1+1"))` 返回对象本身）。
//! - 字符串经动态编译以 global 为 this 同步调用；编译失败 → SyntaxError；
//!   运行异常 → 优先重抛原始异常值（`take_uncaught_value`，`eval("throw 1")` 捕获到 `1`）。
//! - 档 1 函数模式：eval 内 var 不落全局；档 2 切 `create_dynamic_script` 后 var 落全局。

use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::error;

/// `eval(x)`：非字符串实参原样返回；字符串按脚本编译执行（档 1 走函数模式）。
///
/// # 步骤
/// 1. 实参非字符串 → 原样返回（规范不 ToString）。
/// 2. 字符串 → 动态编译为匿名函数（`create_dynamic_function`），以 global 为 this 同步调用。
/// 3. 编译失败 → SyntaxError；运行异常 → 优先重抛原始异常值（`take_uncaught_value`）。
///
/// # 边界与前提
/// - 档 1 函数模式：eval 内 var 不落全局；档 2 切 `create_dynamic_script` 后 var 落全局。
/// - 无实参等价于 `eval(undefined)` → undefined。
pub fn eval<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let arg = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !arg.is_string() {
        return NativeResult::Ok(arg);
    }
    let code = vm.string_ref(arg).to_string();
    // 档 2 起把此分支换成 `vm.create_dynamic_script(&code)`。
    match vm.create_dynamic_function(&[], &code) {
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
