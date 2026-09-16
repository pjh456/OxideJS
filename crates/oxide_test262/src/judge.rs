//! test262 单测试判定与失败分类：`TestOutcome`/`TestResult` 判定模型、compile/vm 错误三态判定、未绑定
//! 标识符解析与缺失全局白名单、异步 `$DONE` 判定、panic 载荷文本化、失败类别分类。5 个跨模块被调
//! 判定/分类函数与判定模型放宽 `pub(crate)`；`classify_vm_error`/`parse_undefined_ident`/`KNOWN_MISSING_GLOBALS` 保持私有。

use crate::meta::{Negative, TestMeta};
use crate::report::extract_not_callable_subkey;
use crate::runner::read_async_output;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;
use std::path::{Path, PathBuf};

/// 单个测试的判定结果：通过 / 失败 / 跳过（各带说明消息）。
#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub(crate) enum TestOutcome {
    Pass(String),
    Fail(String),
    Skip(String),
}

/// 单个测试的运行结果：路径、判定与耗时（毫秒）。
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct TestResult {
    pub(crate) path: PathBuf,
    pub(crate) outcome: TestOutcome,
    pub(crate) duration_ms: u64,
}

impl TestResult {
    /// 构造一个通过结果。
    pub(crate) fn pass(path: PathBuf, dur: u64, msg: impl Into<String>) -> Self {
        Self {
            path,
            outcome: TestOutcome::Pass(msg.into()),
            duration_ms: dur,
        }
    }

    /// 构造一个失败结果。
    pub(crate) fn fail(path: PathBuf, dur: u64, msg: impl Into<String>) -> Self {
        Self {
            path,
            outcome: TestOutcome::Fail(msg.into()),
            duration_ms: dur,
        }
    }

    /// 构造一个跳过结果（不计耗时）。
    pub(crate) fn skip(path: PathBuf, msg: String) -> Self {
        Self {
            path,
            outcome: TestOutcome::Skip(msg),
            duration_ms: 0,
        }
    }
}

/// 从 catch_unwind 的 panic payload 提取可读文本：`&str` 与 `String` 两种
/// 常见形态，其余返回占位文本。
pub(crate) fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return format!("engine panic: {s}");
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return format!("engine panic: {s}");
    }
    "engine panic (non-string payload)".into()
}

/// 异步测试判定：`$DONE` 把结果写入捕获串，run() 结束时微任务队列已 drain。
///
/// 失败 marker 优先（`Test262:AsyncTestFailure:<name>: <msg>`）；其次成功 marker
/// （`Test262:AsyncTestComplete`）；两者都无则测试未在 run() 内同步完成
/// （需 setTimeout 等异步 runner 的测试在此失败），回退到 run 结果判定。
pub(crate) fn judge_async_result(
    path: &Path, run_result: Result<JsValue, String>, vm: &mut Vm, meta: &TestMeta, dur: u64, no_skip: bool,
) -> TestResult {
    let output = read_async_output(vm);
    if let Some(pos) = output.find("Test262:AsyncTestFailure:") {
        let detail = output[pos..].trim();
        if let Some(neg) = meta.negative.as_ref() {
            let name = detail
                .strip_prefix("Test262:AsyncTestFailure:")
                .and_then(|rest| rest.split(':').next())
                .unwrap_or("");
            if name == neg.error_type {
                return TestResult::pass(path.to_path_buf(), dur, format!("expected: {detail}"));
            }
            return TestResult::fail(
                path.to_path_buf(),
                dur,
                format!("expected {} error, got: {detail}", neg.error_type),
            );
        }
        return TestResult::fail(path.to_path_buf(), dur, detail);
    }

    if output.contains("Test262:AsyncTestComplete") {
        if let Some(neg) = meta.negative.as_ref() {
            return TestResult::fail(
                path.to_path_buf(),
                dur,
                format!("expected runtime error ({}), got: async complete", neg.error_type),
            );
        }
        return TestResult::pass(path.to_path_buf(), dur, "async ok");
    }

    // $DONE 未被调用：run() 结果决定成败（顶层抛错走普通判定，未抛错视为未完成）。
    match run_result {
        Ok(_) => TestResult::fail(path.to_path_buf(), dur, "async test did not call $DONE synchronously"),
        Err(e) => judge_vm_error(path, &e, meta, dur, no_skip),
    }
}

/// 运行期错误的判定（negative 匹配优先 + 未实现特性跳过分类）。
pub(crate) fn judge_vm_error(path: &Path, e: &str, meta: &TestMeta, dur: u64, no_skip: bool) -> TestResult {
    match classify_vm_error(e, meta.negative.as_ref(), no_skip) {
        TestOutcome::Pass(msg) => TestResult::pass(path.to_path_buf(), dur, msg),
        TestOutcome::Fail(msg) => TestResult::fail(path.to_path_buf(), dur, msg),
        TestOutcome::Skip(msg) => TestResult::skip(path.to_path_buf(), msg),
    }
}

/// 从 `is not defined` 错误消息中解析未绑定标识符名。
///
/// # 边界与前提
/// - 运行期形态：`uncaught ReferenceError: {name} is not defined`
///   （可带 `vm error: ` 前缀，即 runner Fail 消息的生产形态）
/// - 编译期形态：`Identifier '{name}' is not defined`（可带 `compile error: ` 前缀）
/// - 两种形态都不匹配返回 `None`（不属未绑定标识符错误）。
fn parse_undefined_ident(e: &str) -> Option<&str> {
    let e = e.strip_prefix("vm error: ").unwrap_or(e);
    let e = e.strip_prefix("compile error: ").unwrap_or(e);
    let e = e.strip_prefix("uncaught ").unwrap_or(e);
    if let Some(rest) = e.strip_prefix("ReferenceError: ") {
        if let Some(ident) = rest.strip_suffix(" is not defined") {
            let ident = ident.trim();
            return (!ident.is_empty()).then_some(ident);
        }
    }
    if let Some(rest) = e.strip_prefix("Identifier '") {
        if let Some(ident) = rest.strip_suffix("' is not defined") {
            let ident = ident.trim();
            return (!ident.is_empty()).then_some(ident);
        }
    }
    None
}

/// 已知缺失的宿主/标准全局白名单：这些标识符未绑定是能力缺失（skip），
/// 其它 `is not defined` 是引擎回归或语义缺口（fail）。
const KNOWN_MISSING_GLOBALS: &[&str] = &["$262", "structuredClone", "queueMicrotask"];

/// 判定编译期错误结果：compile 错误无条件放行 negative；能力未实现形态
/// （含白名单内的未绑定标识符）→ Skip（`--no-skip` 下为 Fail）；其余一律 Fail。
pub(crate) fn classify_compile_error(e: &str, has_negative: bool, no_skip: bool) -> TestOutcome {
    if has_negative {
        return TestOutcome::Pass(format!("compile error: {e}"));
    }
    let unimplemented = e.contains("not yet implemented")
        || e.contains("not yet supported")
        || e.contains("not supported")
        || e.contains("unsupported")
        || e.contains("SpreadElement")
        || e.contains("already been declared")
        || e.contains("parser panicked")
        || e.contains("too many registers");
    // `is not defined` 仅当标识符在缺失全局白名单内才 skip；其余（引擎该有
    // 而未提供的标识符、回归）按真实失败计入，保证修复可见性。
    let whitelisted_undefined =
        e.contains("is not defined") && parse_undefined_ident(e).is_some_and(|i| KNOWN_MISSING_GLOBALS.contains(&i));
    if unimplemented || whitelisted_undefined {
        if no_skip {
            return TestOutcome::Fail(format!("compile error: {e}"));
        }
        return TestOutcome::Skip(format!("compile error: {e}"));
    }
    TestOutcome::Fail(format!("compile error: {e}"))
}

/// 判定运行期错误结果：negative 期望匹配 → Pass；能力未实现形态 → Skip
/// （`--no-skip` 下为 Fail）；其余一律 Fail。
///
/// # 边界与前提
/// - receiver 校验、栈溢出、不可调用、ToPrimitive 缺口等错误是引擎语义与测试
///   期望不符的真实失败，不再被 skip 子串吞没（引擎做对了反而计 skip 属误判）。
/// - `is not defined` 是否 skip 由标识符白名单判定（见 [`parse_undefined_ident`]）。
fn classify_vm_error(e: &str, neg: Option<&Negative>, no_skip: bool) -> TestOutcome {
    if let Some(neg) = neg {
        if e.contains("TypeError") && neg.error_type == "TypeError" {
            return TestOutcome::Pass(format!("expected: {e}"));
        }
        if e.contains("ReferenceError") && neg.error_type == "ReferenceError" {
            return TestOutcome::Pass(format!("expected: {e}"));
        }
        if e.contains("SyntaxError") && neg.error_type == "SyntaxError" {
            return TestOutcome::Pass(format!("expected: {e}"));
        }
        if e.contains(&neg.error_type) {
            return TestOutcome::Pass(format!("expected: {e}"));
        }
        return TestOutcome::Fail(format!("expected {} error, got: {e}", neg.error_type));
    }
    let unimplemented = e.contains("not yet implemented")
        || e.contains("not yet supported")
        || e.contains("not supported")
        || e.contains("unsupported")
        || e.contains("step limit")
        || e.contains("memory limit")
        || e.contains("NEW_EXPRESSION")
        || e.contains("GET_PROP_DYNAMIC on non-object")
        || e.contains("SET_PROP_DYNAMIC on non-object")
        || e.contains("private field brand check")
        || e.contains("CALL_NATIVE target")
        || e.contains("is not implemented")
        || e.contains("unexpected tail call")
        || e.contains("__proto__ must be an object");
    // `is not defined` 不再一票吞 skip：仅在标识符属缺失全局白名单时按能力
    // 缺失跳过；否则是真实 ReferenceError（含修复后应转 PASS 的回归）。
    let whitelisted_undefined =
        e.contains("is not defined") && parse_undefined_ident(e).is_some_and(|i| KNOWN_MISSING_GLOBALS.contains(&i));
    if unimplemented || whitelisted_undefined {
        if no_skip {
            return TestOutcome::Fail(format!("vm error: {e}"));
        }
        return TestOutcome::Skip(format!("vm: {e}"));
    }
    TestOutcome::Fail(format!("vm error: {e}"))
}

/// 把失败消息归类为可聚合的失败类别与 subkey，返回 `(category, subkey)`。
///
/// # 边界与前提
/// - 类别大类名口径不变（心跳序列化 / 既有基线对比兼容）；碎片桶
///   （`compile: other` / `vm: other` / `other`）不再携带消息尾巴
///   （全文已在 FailRecord.message）。
/// - subkey 仅 `not callable`（调用点，提取失败归 `(none)`）与
///   `not defined`（标识符）两类非空，其余恒空串；
///   `IC_GET_PROP on non-object` 独立成桶且 subkey 恒空（靠目录分组区分）。
pub(crate) fn categorize_fail(msg: &str) -> (String, String) {
    if msg.contains("compile error:") {
        let reason = msg.trim_start_matches("compile error: ").trim();
        if reason.contains("already been declared") {
            ("compile: already declared".into(), String::new())
        } else if reason.contains("not yet implemented") {
            ("compile: not yet implemented".into(), String::new())
        } else if reason.contains("not yet supported") {
            ("compile: not yet supported".into(), String::new())
        } else if reason.contains("unsupported") {
            ("compile: unsupported".into(), String::new())
        } else if reason.contains("is not defined") {
            ("compile: not defined".into(), parse_undefined_ident(msg).unwrap_or("").to_string())
        } else {
            ("compile: other".into(), String::new())
        }
    } else if msg.contains("parse error:") {
        ("parse error".into(), String::new())
    } else if msg.contains("vm error:") {
        let reason = msg.trim_start_matches("vm error: ").trim();
        if reason.contains("CALL_NATIVE target") {
            ("vm: CALL_NATIVE no target".into(), String::new())
        } else if reason.contains("not callable") {
            ("vm: not callable".into(), extract_not_callable_subkey(msg))
        } else if reason.contains("IC_GET_PROP on non-object") {
            ("vm: IC_GET_PROP on non-object".into(), String::new())
        } else if reason.contains("not yet implemented") {
            ("vm: not yet implemented".into(), String::new())
        } else if reason.contains("step limit") {
            ("vm: step limit".into(), String::new())
        } else if reason.contains("memory limit") {
            ("vm: memory limit".into(), String::new())
        } else if reason.contains("not defined") {
            ("vm: not defined".into(), parse_undefined_ident(msg).unwrap_or("").to_string())
        } else if reason.contains("unsupported") {
            ("vm: unsupported".into(), String::new())
        } else {
            ("vm: other".into(), String::new())
        }
    } else if msg.contains("engine panic") {
        ("engine panic".into(), String::new())
    } else if msg.contains("out-of-scope harness:") {
        ("harness: blacklisted".into(), String::new())
    } else if msg.contains("unknown harness:") {
        ("harness: unknown".into(), String::new())
    } else if msg.contains("harness compile error:") {
        ("harness: compile error".into(), String::new())
    } else if msg.contains("harness runtime error:") {
        ("harness: runtime error".into(), String::new())
    } else if msg.contains("expected runtime error") {
        ("expected runtime error".into(), String::new())
    } else if let Some(ident) = parse_undefined_ident(msg) {
        // 无前缀形态的未绑定标识符错误兜底（生产路径均带 vm error: 前缀走 vm 臂）：
        // 形态门——parse 出标识符才入 not-defined 桶；negative mismatch 等
        // 解析不出的含 `is not defined` 形态回落 other，不污染 not-defined 计数。
        ("vm: not defined".into(), ident.to_string())
    } else {
        ("other".into(), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷构造：按预期判定构造 Negative 元数据。
    fn neg(error_type: &str) -> Negative {
        Negative {
            phase: "runtime".into(),
            error_type: error_type.into(),
        }
    }

    /// 断言 classify_vm_error 对给定错误消息返回指定结果变体。
    fn assert_outcome(e: &str, neg: Option<&Negative>, no_skip: bool, want: &TestOutcome) {
        let got = classify_vm_error(e, neg, no_skip);
        let same = matches!(
            (want, &got),
            (TestOutcome::Pass(_), TestOutcome::Pass(_))
                | (TestOutcome::Fail(_), TestOutcome::Fail(_))
                | (TestOutcome::Skip(_), TestOutcome::Skip(_))
        );
        assert!(same, "error `{e}` (neg={neg:?}, no_skip={no_skip}) 期望 {want:?}，实际 {got:?}");
    }

    /// receiver 校验类错误：引擎已实现 receiver 检查并正确抛错，非 negative 测试
    /// 抛此错即真实失败，不得计入 skip。
    #[test]
    fn receiver_validation_errors_are_real_failures() {
        for e in [
            "TypeError: Array.prototype.map method called on null",
            "TypeError: method called on incompatible receiver",
            "TypeError: called on non-Set object",
            "TypeError: called on non-Map object",
            "TypeError: called on non-ArrayBuffer object",
            "TypeError: called on non-TypedArray object",
        ] {
            assert_outcome(e, None, false, &TestOutcome::Fail("".into()));
        }
    }

    /// 真实 bug 形态（栈溢出 / 不可调用 / ToPrimitive / 属性写入缺口）：语义与
    /// 测试期望不符，移出 skip 列表后按真实失败计入。
    #[test]
    fn engine_bug_shape_errors_are_real_failures() {
        for e in [
            "RangeError: Maximum call stack size exceeded",
            "TypeError: x is not callable",
            "TypeError: Cannot convert object to primitive value",
            "TypeError: Cannot create property on non-object",
            "TypeError: Property description must be an object",
            "IC_GET_PROP on non-object",
        ] {
            assert_outcome(e, None, false, &TestOutcome::Fail("".into()));
        }
    }

    /// 分配上限超限（与步数上限同款运行期限制）：默认 skip，`--no-skip` 下 fail。
    #[test]
    fn memory_limit_errors_skip_by_default() {
        let e = "vm error: VM memory limit 536870912 exceeded (used 536871936) at pc=14";
        assert_outcome(e, None, false, &TestOutcome::Skip("".into()));
        assert_outcome(e, None, true, &TestOutcome::Fail("".into()));
    }

    /// 能力未实现形态保留 skip；`--no-skip` 下转 fail。
    #[test]
    fn unimplemented_shapes_stay_skipped() {
        for e in [
            "not yet implemented: Proxy",
            "feature is not supported",
            "unsupported syntax",
            "vm error: NEW_EXPRESSION not supported",
            "GET_PROP_DYNAMIC on non-object",
            "SET_PROP_DYNAMIC on non-object",
            "private field brand check",
            "CALL_NATIVE target is null",
            "is not implemented",
            "unexpected tail call",
            "__proto__ must be an object",
        ] {
            assert_outcome(e, None, false, &TestOutcome::Skip("".into()));
            assert_outcome(e, None, true, &TestOutcome::Fail("".into()));
        }
    }

    /// 非 negative 且不含能力缺失标记的错误 → 真实失败。
    #[test]
    fn unrelated_runtime_errors_are_failures() {
        assert_outcome("TypeError: value out of range", None, false, &TestOutcome::Fail("".into()));
        assert_outcome("ReferenceError: unexpected token", None, false, &TestOutcome::Fail("".into()));
    }

    /// negative 期望匹配 → Pass；不匹配 → Fail。
    #[test]
    fn negative_meta_matching_wins_over_skip() {
        assert_outcome("TypeError: boom", Some(&neg("TypeError")), false, &TestOutcome::Pass("".into()));
        assert_outcome("ReferenceError: boom", Some(&neg("ReferenceError")), false, &TestOutcome::Pass("".into()));
        assert_outcome("TypeError: boom", Some(&neg("RangeError")), false, &TestOutcome::Fail("".into()));
    }

    /// `is not defined` 标识符解析：运行期 `uncaught` 前缀（含 `vm error: `
    /// 生产前缀）、编译期 `Identifier '..'` 两种形态都能取出标识符名；非该形态返回 None。
    #[test]
    fn parse_undefined_ident_extracts_name() {
        assert_eq!(parse_undefined_ident("uncaught ReferenceError: $262 is not defined"), Some("$262"));
        assert_eq!(
            parse_undefined_ident("vm error: uncaught ReferenceError: foo is not defined"),
            Some("foo")
        );
        assert_eq!(parse_undefined_ident("ReferenceError: foo is not defined"), Some("foo"));
        assert_eq!(
            parse_undefined_ident("compile error: Identifier 'structuredClone' is not defined"),
            Some("structuredClone")
        );
        assert_eq!(
            parse_undefined_ident("Identifier 'queueMicrotask' is not defined"),
            Some("queueMicrotask")
        );
        assert_eq!(parse_undefined_ident("TypeError: x is not callable"), None);
        assert_eq!(parse_undefined_ident("uncaught ReferenceError: boom"), None);
    }

    /// `is not defined` 白名单化：白名单内标识符（缺失全局）→ skip；白名单外 →
    /// 真实失败（保证修复后应转 PASS 的测试可见）。
    #[test]
    fn undefined_identifier_whitelisted_skip() {
        for e in [
            "uncaught ReferenceError: $262 is not defined",
            "compile error: Identifier '$262' is not defined",
            "uncaught ReferenceError: structuredClone is not defined",
            "compile error: Identifier 'queueMicrotask' is not defined",
        ] {
            assert_outcome(e, None, false, &TestOutcome::Skip("".into()));
            assert_outcome(e, None, true, &TestOutcome::Fail("".into()));
        }
    }

    #[test]
    fn undefined_identifier_outside_whitelist_is_failure() {
        for e in [
            "uncaught ReferenceError: foo is not defined",
            "compile error: Identifier 'whatever' is not defined",
        ] {
            assert_outcome(e, None, false, &TestOutcome::Fail("".into()));
        }
    }

    /// categorize_fail 双返回：类别大类名口径不变（碎片桶去消息尾巴），
    /// subkey 仅 not callable / not defined 三类非空，IC_GET_PROP 独立成桶。
    /// not defined 覆盖 `vm error: ` 生产形态（subkey 非空）与无前缀裸形态
    /// （形态门：parse 出标识符才入桶）；negative mismatch 形态归 other。
    #[test]
    fn categorize_fail_returns_category_and_subkey() {
        let cases = [
            ("vm error: TypeError: CALL target is not callable", ("vm: not callable", "CALL target")),
            ("vm error: TypeError: accessor is not callable", ("vm: not callable", "accessor")),
            ("vm error: TypeError: x is not callable", ("vm: not callable", "x")),
            ("vm error: uncaught ReferenceError: foo is not defined", ("vm: not defined", "foo")),
            ("uncaught ReferenceError: foo is not defined", ("vm: not defined", "foo")),
            ("compile error: Identifier 'x' is not defined", ("compile: not defined", "x")),
            ("vm error: IC_GET_PROP on non-object", ("vm: IC_GET_PROP on non-object", "")),
            (
                "vm error: VM memory limit 536870912 exceeded (used 536871936) at pc=14",
                ("vm: memory limit", ""),
            ),
            ("vm error: TypeError: method called on incompatible receiver", ("vm: other", "")),
            ("engine panic: boom", ("engine panic", "")),
            (
                "expected TypeError error, got: uncaught ReferenceError: foo is not defined",
                ("other", ""),
            ),
            ("random string", ("other", "")),
        ];
        for (msg, (want_cat, want_sub)) in cases {
            assert_eq!(categorize_fail(msg), (want_cat.to_string(), want_sub.to_string()), "消息 {msg}");
        }
    }

    /// panic payload 三类形态：&str / String / 其它类型占位文本。
    #[test]
    fn panic_payload_str_extracts_string_and_str() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_payload_str(&payload), "engine panic: boom");
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom".to_string());
        assert_eq!(panic_payload_str(&payload), "engine panic: boom");
        let payload: Box<dyn std::any::Any + Send> = Box::new(42u32);
        assert_eq!(panic_payload_str(&payload), "engine panic (non-string payload)");
    }
}
