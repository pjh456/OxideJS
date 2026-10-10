//! EventTarget 验收测试：addEventListener / removeEventListener / dispatchEvent
//! 三方法、原型链（Worker / MessagePort / BroadcastChannel 经 EventTarget.prototype）、
//! GC 剪枝。
//!
//! 集成测试走公共 API（`eval` 返回脚本完成值字符串），不访问私有字段。
//! 目标对象一律用 MessagePort（EventTarget 实例）——普通对象原型链上没有
//! EventTarget.prototype，DOM 语义下 addEventListener 也只存在于 EventTarget 实例。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

/// 编译并运行脚本，返回完成值字符串（解析 / 编译 / 运行失败时返回错误描述）。
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
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    match vm.run(&Arc::new(module)) {
        // 字符串结果经 lookup_str 提取内容（JsValue Display 只打印 {string}）。
        Ok(result) => vm.lookup_str(result).unwrap_or_else(|| format!("{result}")),
        Err(e) => format!("vm error: {e}"),
    }
}

/// 建一个注入编译服务的 VM 并运行首段脚本（供 GC 测试中途收集）。
fn vm_with_script(source: &str) -> Vm {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse 应成功");
    let module = Compiler::new().compile(&program).expect("compile 应成功");
    let mut vm = Vm::new();
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    vm.run(&Arc::new(module)).expect("run 应成功");
    vm
}

/// 在既有 VM 上运行一段脚本，返回完成值字符串。
fn vm_eval(vm: &mut Vm, source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse 应成功");
    let module = Compiler::new().compile(&program).expect("compile 应成功");
    match vm.run(&Arc::new(module)) {
        // 字符串结果经 lookup_str 提取内容（JsValue Display 只打印 {string}）。
        Ok(result) => vm.lookup_str(result).unwrap_or_else(|| format!("{result}")),
        Err(e) => format!("vm error: {e}"),
    }
}

/// 建一个 MessagePort 目标的前缀（每个测试目标独立，避免跨测试串扰）。
const TARGET: &str = "var mc = new MessageChannel(); var t = mc.port1;";

/// dispatchEvent(new Event('x')) 返回 true（无 preventDefault）。
#[test]
fn dispatch_event_returns_true() {
    assert_eq!(
        eval(&format!("{TARGET} t.dispatchEvent(new Event('x'));")),
        "true"
    );
}

/// preventDefault() 后 dispatchEvent 返回 false（cancelable 事件）。
#[test]
fn dispatch_event_returns_false_after_prevent_default() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} t.addEventListener('x', function(e) {{ e.preventDefault(); }}); \
                 t.dispatchEvent(new Event('x', {{ cancelable: true }}));"
            )
        ),
        "false"
    );
}

/// 非 cancelable 事件 preventDefault 无效，dispatchEvent 仍返回 true。
#[test]
fn dispatch_event_non_cancelable_stays_true() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} t.addEventListener('x', function(e) {{ e.preventDefault(); }}); \
                 t.dispatchEvent(new Event('x'));"
            )
        ),
        "true"
    );
}

/// removeEventListener 生效：移除后派发不再调用监听器。
#[test]
fn remove_event_listener_works() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} var calls = 0; \
                 var fn = function(e) {{ calls++; }}; \
                 t.addEventListener('x', fn); \
                 t.removeEventListener('x', fn); \
                 t.dispatchEvent(new Event('x')); \
                 calls;"
            )
        ),
        "0"
    );
}

/// 重复登记（同 type + callback + capture）为 no-op：派发只调用一次。
#[test]
fn duplicate_add_event_listener_is_noop() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} var calls = 0; \
                 var fn = function(e) {{ calls++; }}; \
                 t.addEventListener('x', fn); \
                 t.addEventListener('x', fn); \
                 t.dispatchEvent(new Event('x')); \
                 calls;"
            )
        ),
        "1"
    );
}

/// once 选项：监听器调用一次后自动移除。
#[test]
fn once_listener_removed_after_call() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} var calls = 0; \
                 var fn = function(e) {{ calls++; }}; \
                 t.addEventListener('x', fn, {{ once: true }}); \
                 t.dispatchEvent(new Event('x')); \
                 t.dispatchEvent(new Event('x')); \
                 calls;"
            )
        ),
        "1"
    );
}

/// stopImmediatePropagation 截断后续监听器。
#[test]
fn stop_immediate_propagation_truncates() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} var calls = 0; \
                 t.addEventListener('x', function(e) {{ calls++; e.stopImmediatePropagation(); }}); \
                 t.addEventListener('x', function(e) {{ calls++; }}); \
                 t.dispatchEvent(new Event('x')); \
                 calls;"
            )
        ),
        "1"
    );
}

/// 监听器回调的 this 是目标对象、实参是事件（e.type 为类型串）。
#[test]
fn listener_receives_target_this_and_event() {
    assert_eq!(
        eval(
            &format!(
                "{TARGET} var gotThis = null; var gotType = null; \
                 t.addEventListener('x', function(e) {{ gotThis = (this === t); gotType = e.type; }}); \
                 t.dispatchEvent(new Event('x')); \
                 gotThis + ':' + gotType;"
            )
        ),
        "true:x"
    );
}

/// MessagePort 原型链经 EventTarget.prototype：port.addEventListener 生效。
#[test]
fn port_add_event_listener_works() {
    assert_eq!(
        eval(
            "var mc = new MessageChannel(); var calls = 0; \
             mc.port1.addEventListener('message', function(e) { calls++; }); \
             mc.port1.dispatchEvent(new Event('message')); \
             calls;"
        ),
        "1"
    );
}

/// BroadcastChannel 原型链经 EventTarget.prototype：channel.addEventListener 生效。
#[test]
fn channel_add_event_listener_works() {
    assert_eq!(
        eval(
            "var ch = new BroadcastChannel('t'); var calls = 0; \
             ch.addEventListener('message', function(e) { calls++; }); \
             ch.dispatchEvent(new Event('message')); \
             calls;"
        ),
        "1"
    );
}

/// Worker.prototype 原型链经 EventTarget.prototype：worker.addEventListener 生效。
#[test]
fn worker_add_event_listener_works() {
    let path = std::env::temp_dir().join(format!("oxide_et_{}.js", std::process::id()));
    let _ = std::fs::write(&path, "1");
    let script = format!(
        "var w = new Worker('{}'); var calls = 0; \
         w.addEventListener('message', function(e) {{ calls++; }}); \
         w.dispatchEvent(new Event('message')); \
         calls;",
        path.display()
    );
    assert_eq!(eval(&script), "1");
    let _ = std::fs::remove_file(&path);
}

/// 监听器对象经 GC 后仍有效（目标存活时回调随目标存活，无悬垂）。
#[test]
fn listener_survives_gc_while_target_live() {
    let mut vm = vm_with_script(
        "var mc = new MessageChannel(); globalThis.t = mc.port1; \
         globalThis.t.addEventListener('x', function(e) { globalThis.calls = (globalThis.calls || 0) + 1; });",
    );
    // 强制完整收集：目标存活（globalThis.t），监听器回调应保活。
    vm.collect_session_gc();
    // 收集后派发：监听器仍应被调用（无悬垂）。
    assert_eq!(
        vm_eval(&mut vm, "globalThis.t.dispatchEvent(new Event('x')); globalThis.calls;"),
        "1"
    );
}

/// 目标对象被回收后注册表剪枝（GC 后无悬垂指针）。
#[test]
fn registry_pruned_after_target_collected() {
    // t 是局部变量，脚本结束后无引用 → 目标死亡。
    let mut vm = vm_with_script(
        "var mc = new MessageChannel(); var t = mc.port1; t.addEventListener('x', function() {});",
    );
    // 强制完整收集：注册表应按 mark 位剪枝死目标条目。
    vm.collect_session_gc();
    // 剪枝后无悬垂：新建同型目标派发不触发旧监听器（旧条目已剪），无崩溃。
    assert_eq!(
        vm_eval(
            &mut vm,
            "var mc2 = new MessageChannel(); mc2.port1.dispatchEvent(new Event('x')); 'ok';"
        ),
        "ok"
    );
}
