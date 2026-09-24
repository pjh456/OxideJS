//! test262 宿主对象 `$262` 的最小绑定验证：对象存在、`global` 指向当前 realm
//! 全局、evalScript 可执行、未实现方法抛能力缺失错误。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

/// 单测辅助：源串 → 结果串（字符串值解包实际内容，运行错误以 `vm error: ...` 呈现）。
fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => vm.lookup_str(result).unwrap_or_else(|| format!("{result}")),
        Err(e) => format!("vm error: {e}"),
    }
}

/// 单测辅助：在调用方提供的 VM 上执行源串，返回原始结果值（低阈值配置钉用）。
fn eval_on(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse error: {}", e[0].message))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("compile error: {e}"))?;
    vm.run(&Arc::new(module))
}

/// `$262` 作为编译期已知全局绑定：typeof 为 object，`global` 即 globalThis。
#[test]
fn host_object_exists_and_exposes_global() {
    assert_eq!(eval("typeof $262"), "object");
    assert_eq!(eval("$262.global === globalThis"), "true");
}

/// `$262` 方法面：evalScript/gc 为可调用函数。
#[test]
fn host_methods_are_callable() {
    assert_eq!(eval("typeof $262.evalScript"), "function");
    assert_eq!(eval("typeof $262.gc"), "function");
    assert_eq!(eval("typeof $262.detachArrayBuffer"), "function");
    assert_eq!(eval("typeof $262.createRealm"), "function");
}

/// evalScript 以普通脚本执行并返回完成值：末条语句的正常完成值；顶层
/// `return` 在 Script goal 下是编译期 SyntaxError。
#[test]
fn eval_script_runs_code_and_returns_value() {
    // 普通脚本语义：顶层语句的正常完成值即脚本完成值（var 声明语句本身完成值为
    // undefined，末条表达式语句 `y` 完成值为 1）。
    assert_eq!(eval("$262.evalScript('var y = 1; y')"), "1");
    // Script goal 顶层 return 非法：编译期错误转 JS 层可捕获的 SyntaxError。
    let r = eval("try { $262.evalScript('return 40 + 2'); 'no-throw' } catch (e) { e.name }");
    assert_eq!(r, "SyntaxError");
}

/// evalScript 编译失败转 SyntaxError（可由 JS catch 捕获）。
#[test]
fn eval_script_syntax_error_is_catchable() {
    let r = eval("try { $262.evalScript('if (') ; 'no-throw' } catch (e) { e.name }");
    assert_eq!(r, "SyntaxError");
}

/// 未实现方法抛能力缺失错误（消息带 `not supported`），不抛 ReferenceError。
#[test]
fn unsupported_methods_throw_not_supported() {
    let r = eval("try { $262.createRealm(); 'no-throw' } catch (e) { e.name + ':' + e.message }");
    assert!(r.contains("TypeError"), "期望 TypeError，实际 {r}");
    assert!(r.contains("not supported"), "期望能力缺失消息，实际 {r}");

    let r = eval("try { $262.agent(); 'no-throw' } catch (e) { e.name + ':' + e.message }");
    assert!(r.contains("TypeError"), "期望 TypeError，实际 {r}");
}

/// `detachArrayBuffer` 真实现钉：detach 后 `detached` 访问器读 true、
/// 非 ArrayBuffer 实参抛 TypeError。
#[test]
fn detach_array_buffer_detaches() {
    assert_eq!(eval("var ab = new ArrayBuffer(8); $262.detachArrayBuffer(ab); ab.detached"), "true");
    let r = eval("try { $262.detachArrayBuffer(1); 'no-throw' } catch (e) { e.name }");
    assert_eq!(r, "TypeError");
}

/// `$262.gc()` 触发一次 session 回收且不抛错。
#[test]
fn gc_runs_without_error() {
    assert_eq!(eval("$262.gc(); 'ok'"), "ok");
}

/// `$262.gc()` 存活钉（低阈值形态）：小阈值长循环反复触发执行期收集，
/// 全局可达的 session 对象经强制收集后身份与属性读回保持完整——
/// 收集前的 mark 必须覆盖全部可达对象，漏标即误回收、读回悬垂。
#[test]
fn gc_keeps_reachable_session_objects_alive() {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(4096);
    let mut vm = Vm::with_kernel_core(KernelCore::new(config));
    let result = eval_on(
        &mut vm,
        "(function () { \
          globalThis.o = { x: 41, s: 'keep' }; \
          globalThis.o.x = 42; \
          for (var i = 0; i < 500000; i++) { var t = 'str' + i; } \
          $262.gc(); \
          return globalThis.o.x + globalThis.o.s.length === 46 \
             && $262.global.o === globalThis.o; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}
