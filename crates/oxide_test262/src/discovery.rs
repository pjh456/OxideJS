//! test262 测试发现与跳过门：`discover_tests` 递归发现与三道跳过门
//! （`is_skipped`/`eval_family_excluded`/`pre_existing_excluded`）。
//! 4 函数放宽 `pub(crate)` 供 crate 内跨模块访问；`process_path` 五道门调用点与顺序居 main.rs。

use crate::meta::TestMeta;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// 递归发现 test262 根目录下全部 `.js` 测试文件（排序后返回）。
pub(crate) fn discover_tests(test262_root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = WalkDir::new(test262_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
        .map(|e| e.path().to_path_buf())
        .collect();
    paths.sort();
    paths
}

/// 按测试元数据的 flags/features 判断是否应跳过，返回跳过原因（None 表示不跳过）。
pub(crate) fn is_skipped(meta: &TestMeta) -> Option<String> {
    for flag in &meta.flags {
        if flag.as_str() == "raw" {
            return Some("raw tests excluded".into());
        }
        // noStrict 测试放行——很多在严格模式下仍可通过；运行时跳过逻辑会捕获失败。
    }

    // 保持大范围已实现 feature tag 可运行；只排除真正未实现的子特性。
    // 其余一切让测试实际运行，依赖运行时跳过逻辑
    // （"too many registers"、"not yet implemented" 等）判定失败。
    let excluded_features = [
        "Proxy",
        "Intl",
        "Atomics",
        "SharedArrayBuffer",
        "cross-realm",
        // await-dictionary（Promise.allKeyed/allSettledKeyed）是 2025 proposal，未实现。
        "await-dictionary",
    ];

    for feat in &meta.features {
        if excluded_features.contains(&feat.as_str()) || feat.starts_with("Intl") {
            return Some(format!("excluded feature: {feat}"));
        }
    }

    None
}

/// eval 相关子族精确排除：
/// built-ins/eval 与 eval-code 的完成值/解析失败/非字符串/this-value-global/间接环境族放行；
/// 仅排除确定失败的 arguments/super/strict/块声明 等子族。
pub(crate) fn eval_family_excluded(path: &str) -> Option<&'static str> {
    if !path.contains("/eval-code/") {
        return None;
    }
    let is_direct = path.contains("eval-code/direct/");
    let common = [
        "declare-arguments", // 直接 eval 的 arguments 语义族
        "this-value-func",   // 调用者 this 传递
        "new.target",        // new.target 语义
        "strict-caller",     // 严格调用者传播
        "strict-source",
        "strictness-override", // 直接 eval 严格性覆盖
        "onlystrict",          // onlyStrict 块声明族
        "always-non-strict",   // 依赖隐式全局写同步，引擎尚未实现
        "block-decl",          // 块级函数声明（Annex B 严格变体）
        "switch-case-decl",
        "switch-dflt-decl",
    ];
    if common.iter().any(|s| path.contains(s)) {
        return Some("eval 子族未实现");
    }
    if is_direct {
        // 直接 eval：调用者作用域交互族（函数上下文 var/let + super 方法上下文）
        if ["var-env-", "lex-env-", "super-prop", "super-call-arrow", "super-call-method"]
            .iter()
            .any(|s| path.contains(s))
        {
            return Some("直接 eval 作用域族未实现");
        }
    } else if [
        "super-",
        "var-env-func-strict",
        "var-env-var-strict",
        "var-env-global-lex",
        "var-env-lower-lex",
    ]
    .iter()
    .any(|s| path.contains(s))
    {
        return Some("间接 eval 严格/词法冲突族未实现");
    }
    None
}

/// 确定性失败白名单：引擎尚未实现该语义的测试集，
/// 修复前归入 skip 以免噪音掩盖真实回归；对应缺口修好后移出本列表。
pub(crate) fn pre_existing_excluded(path: &str) -> Option<&'static str> {
    const LIST: &[(&str, &str)] = &[
        (
            "expressions/tagged-template/template-object-frozen-non-strict.js",
            "sloppy 只读静默写未实现（引擎统一抛 TypeError）",
        ),
        (
            "expressions/tagged-template/cache-eval-inner-function.js",
            "eval 不共享调用方作用域（既有 eval 作用域限制）",
        ),
    ];
    LIST.iter()
        .find(|(suffix, _)| path.ends_with(suffix))
        .map(|(_, reason)| *reason)
}
