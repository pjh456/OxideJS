//! test262 宿主对象 `$262` 的最小实现。
//!
//! `$262` 是 test262 套件约定的宿主注入对象（非 ECMAScript 标准全局）：
//! 测试经它访问当前 realm 全局、执行脚本、触发 GC 等。最小绑定使引用
//! `$262.*` 的测试真实执行而非一律抛 `ReferenceError` 被 runner 吞为 skip；
//! 未实现的方法抛 `not supported` 错误，runner 按能力缺失归类。

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_units_full, NativeResult};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bindings::{apply_binding_table, bind_global_value};
use crate::vm::Vm;

/// `$262` 未实现方法统一抛错：消息带 `not supported`，runner 据此按能力缺失跳过。
fn not_supported(vm: &mut Vm, feature: &str) -> NativeResult {
    NativeResult::Err(oxide_builtins::error::create_type_error(vm, &format!("{feature} is not supported")))
}

/// `$262.evalScript(code)`：在当前 realm 编译执行一段脚本并返回其完成值。
///
/// # 步骤
/// 1. 实参 ToString 取脚本源码。
/// 2. 经动态函数构造编译执行（`create_dynamic_function` + 同步调用）。
/// 3. 编译失败转 SyntaxError；运行异常原样透传异常值。
///
/// # 边界与前提
/// - 最小实现：函数作用域语义——var/函数声明不泄漏到全局，完成值仅限显式
///   `return`（全局作用域 eval 落地前为已知上限）。
/// - 无实参或非字符串实参按 ToString 规范处理。
pub fn eval_script(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let units = match to_units_full(vm.reg(args[1]), vm) {
        Ok(u) => u,
        Err(e) => return NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
    };
    // 动态路径源契约：源码域转义形态传源（见 `create_dynamic_function`）。
    let code = oxide_kernel::string_forge::source_escape(&units);
    match vm.create_dynamic_function(&[], &code) {
        Ok(func) => {
            let global = JsValue::from_js_object(vm.session().global_object().as_ptr() as *mut JsObject);
            match vm.call_function_sync(func, global, &[]) {
                Ok(val) => NativeResult::Ok(val),
                Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
            }
        }
        Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &format!("SyntaxError: {e}"))),
    }
}

/// `$262.detachArrayBuffer(buf)`：ArrayBuffer detach 未实现，抛能力缺失错误。
pub fn detach_array_buffer(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    not_supported(vm, "detachArrayBuffer")
}

/// `$262.createRealm()`：跨 realm 未支持，抛能力缺失错误。
pub fn create_realm(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    not_supported(vm, "createRealm")
}

/// `$262.gc()`：请求一次 session 全量回收（不计返回值）。
///
/// # 副作用
/// - 执行 mark-sweep 搬移存活对象并释放不可达对象，会重写 session 对象指针。
pub fn gc(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let mut session_gc = std::mem::take(&mut vm.gc_state.session_gc);
    session_gc.sweep(vm);
    vm.gc_state.session_gc = session_gc;
    NativeResult::Ok(JsValue::undefined())
}

/// `$262.agent`：agent 线程接口依赖 SharedArrayBuffer 族，未支持，抛能力缺失错误。
pub fn agent(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    not_supported(vm, "agent")
}

/// 把 `$262` 宿主对象绑定到 global：对象本体 + 标准方法 + `global` 数据属性。
///
/// # 步骤
/// 1. 建 `$262` 普通对象（Object.prototype）。
/// 2. 绑定 evalScript/detachArrayBuffer/createRealm/gc/agent 方法。
/// 3. `global` 数据属性指向当前 session global；`$262` 本体挂到 global。
///
/// # 注意事项
/// - 宿主对象登记进 world 释放表，session 收尾时统一释放（与 Reflect/Iterator
///   等全局对象同一约定）；每次调用新建对象，同一 session 内重复绑定会泄漏旧对象
///   （全量初始化与 full_reset 频率低，可接受）。
pub fn bind_test262_host(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let object_proto = session.builtin_world().object_proto.as_ptr() as *mut JsObject;
    let mut host = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    apply_binding_table(
        session.builtin_world(),
        &mut host,
        core,
        &[
            ("evalScript", eval_script as *const (), 1),
            ("detachArrayBuffer", detach_array_buffer as *const (), 1),
            ("createRealm", create_realm as *const (), 0),
            ("gc", gc as *const (), 0),
            ("agent", agent as *const (), 0),
        ],
    );
    let global_this = JsValue::from_js_object(global as *mut JsObject);
    bind_global_value(core, &mut host, "global", global_this);
    let host_ptr = Box::into_raw(Box::new(host));
    session.builtin_world().track_leaked_object(host_ptr);
    bind_global_value(core, global, "$262", JsValue::from_js_object(host_ptr));
}
