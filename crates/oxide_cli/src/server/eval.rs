//! 执行请求路径：parse → compile → spawn → run → format → drop。
//!
//! `handle_eval` 是纯函数，参数全注入、无全局状态；每请求用单一
//! `catch_unwind` 包裹整条路径，panic 载荷转成错误帧，不 panic、不退出。
//!
//! # release 边界
//! 根 `Cargo.toml` 的 `[profile.release]` 为 `panic=abort`：release 下
//! `catch_unwind` 是空操作、panic 直接终止进程，watchdog 是 release 崩溃后
//! 恢复的唯一自动手段；debug 下本路径完整生效，是日常验证环境。

use std::sync::Arc;

use oxide_compiler::compiler::{compiled_module_hash, Compiler};
use oxide_kernel::kernel::KernelCore;
use oxide_parser::Allocator;
use oxide_vm::vm_pool::VmPool;

use super::protocol::ServerResponse;

/// 执行请求处理：解析 → 编译 → 取 VM → 执行 → 渲染完成值 → 归还池的六段路径。
///
/// # 步骤
/// 1. parse：局部分配器加 `oxide_parser::parse`，失败时各条错误换行拼接成错误帧。
/// 2. compile：`Compiler` 加 `CodeForge` 结构哈希缓存，失败以原文成错误帧。
/// 3. spawn：从池取 `VmGuard`（闭包内创建，panic 时随展开被 drop）。
/// 4. run：设每请求步数覆盖（含 None）后 `run`，`Err` 即错误帧。
/// 5. format：成功值经 `crate::format_js_value` 渲染成完成值文本，`eval_ok` 封装。
/// 6. drop：guard 在闭包出口 drop，干净 VM 经 `full_reset` 回池复用、被污染 VM 丢弃并新建替补。
///
/// # 边界与前提
/// - guard 在 `catch_unwind` 闭包内创建：panic 时 guard 随展开在闭包边界被 drop，
///   `VmGuard::drop` 的 `thread::panicking()` 检查此刻为真，VM 按 dirty 丢弃。
///
/// # 副作用
/// - 无（纯函数，参数全注入）。
///
/// # 注意事项
/// - release 下 `panic=abort` 使 `catch_unwind` 空操作，watchdog 是崩溃后恢复的唯一自动手段。
pub fn handle_eval(code: &str, max_steps: Option<u64>, kernel: &Arc<KernelCore>, pool: &Arc<VmPool>) -> ServerResponse {
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // parse：局部分配器，解析失败各条换行拼接成错误帧。
        let allocator = Allocator::default();
        let program = match oxide_parser::parse(&allocator, code) {
            Ok(program) => program,
            Err(errors) => {
                let message = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n");
                return ServerResponse::eval_err(message);
            }
        };

        // compile：结构哈希缓存，失败以原文成错误帧。
        let compiler = Compiler::new();
        let hash = compiled_module_hash(&program);
        let module = match kernel.code_forge().get_or_insert_with(hash, || compiler.compile(&program)) {
            Ok(module) => module,
            Err(message) => return ServerResponse::eval_err(message),
        };

        // spawn：从池取 VM；guard 在闭包内创建，panic 时随展开被 drop。
        let mut guard = pool.spawn();

        // run：设每请求步数覆盖（含 None）后执行。
        guard.vm_mut().set_max_steps(max_steps);
        let result = guard.vm_mut().run(&module);

        // format：渲染完成值，`Err` 即错误帧。
        match result {
            Ok(value) => {
                let text = crate::format_js_value(
                    guard.vm(),
                    kernel.perm_interner().as_ref(),
                    kernel.shape_forge().as_ref(),
                    value,
                );
                ServerResponse::eval_ok(text)
            }
            Err(message) => ServerResponse::eval_err(message),
        }
        // drop：guard 在闭包出口 drop，干净 VM 经 full_reset 回池复用、dirty 丢弃。
    }));

    match response {
        Ok(resp) => resp,
        Err(payload) => ServerResponse::eval_err(format!("internal panic: {}", panic_payload_str(&payload))),
    }
}

/// 从 `catch_unwind` 的 panic payload 提取可读文本：`&str` 与 `String` 两种
/// 常见形态，其余返回占位文本。
fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "non-string payload".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_compiler::DefaultCompilerService;
    use oxide_kernel::kernel::KernelConfig;

    /// 测试环境：minimal 内核加单 VM 池（与池既有测试同口径）。
    fn test_env() -> (Arc<KernelCore>, Arc<VmPool>) {
        let kernel = KernelCore::new(KernelConfig::minimal());
        let pool = VmPool::new(Arc::clone(&kernel), Arc::new(DefaultCompilerService), 1, Some(2));
        (kernel, pool)
    }

    /// 成功渲染："1 + 1" 得完成值 "2"，无错误。
    #[test]
    fn eval_success_renders_value() {
        let (kernel, pool) = test_env();
        let response = handle_eval("1 + 1", None, &kernel, &pool);
        match response {
            ServerResponse::EvalResult { ref value, ref error } => {
                assert_eq!(value.as_deref(), Some("2"), "完成值应为 2：{response:?}");
                assert!(error.is_none(), "不应有错误：{response:?}");
            }
            other => panic!("应得 EvalResult 帧，实得 {other:?}"),
        }
    }

    /// 解析错误帧："function(" 得非空错误，无完成值。
    #[test]
    fn eval_parse_error_frame() {
        let (kernel, pool) = test_env();
        let response = handle_eval("function(", None, &kernel, &pool);
        match response {
            ServerResponse::EvalResult { ref value, ref error } => {
                assert!(value.is_none(), "不应有完成值：{response:?}");
                assert!(error.as_deref().map(|e| !e.is_empty()).unwrap_or(false), "错误应非空：{response:?}");
            }
            other => panic!("应得 EvalResult 帧，实得 {other:?}"),
        }
    }

    /// 运行时错误帧：throw 得含 uncaught 与 boom 的错误。
    #[test]
    fn eval_runtime_error_frame() {
        let (kernel, pool) = test_env();
        let response = handle_eval("throw new Error('boom')", None, &kernel, &pool);
        match response {
            ServerResponse::EvalResult { ref value, ref error } => {
                assert!(value.is_none(), "不应有完成值：{response:?}");
                let err = error.as_deref().expect("应有错误");
                assert!(err.contains("uncaught"), "错误应含 uncaught：{err}");
                assert!(err.contains("boom"), "错误应含 boom：{err}");
            }
            other => panic!("应得 EvalResult 帧，实得 {other:?}"),
        }
    }

    /// 步数超限帧：for(;;){} 加 Some(1000) 得含 step limit 的错误。
    #[test]
    fn eval_step_limit_frame() {
        let (kernel, pool) = test_env();
        let response = handle_eval("for(;;){}", Some(1000), &kernel, &pool);
        match response {
            ServerResponse::EvalResult { ref value, ref error } => {
                assert!(value.is_none(), "不应有完成值：{response:?}");
                let err = error.as_deref().expect("应有错误");
                assert!(err.contains("step limit"), "错误应含 step limit：{err}");
            }
            other => panic!("应得 EvalResult 帧，实得 {other:?}"),
        }
    }

    /// 池复用：连续两次成功 eval 后池可用数不变。
    #[test]
    fn eval_pool_reuse() {
        let (kernel, pool) = test_env();
        let before = pool.available_count();
        handle_eval("1 + 1", None, &kernel, &pool);
        handle_eval("2 + 2", None, &kernel, &pool);
        let after = pool.available_count();
        assert_eq!(before, after, "池可用数应不变：before={before} after={after}");
    }
}
