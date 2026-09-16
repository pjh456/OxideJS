//! 单测试执行管线：harness 前缀拼接 → parse → compile → run → 判定，以及跳过门与每 worker kernel 构建。
//!
//! worker 自有的 kernel / harness 源 / 前缀缓存永不跨线程；CURRENT_TEST_PATH 在每个测试
//! 执行前写入，供 panic hook 定位崩溃测试文件。
#![allow(clippy::arc_with_non_send_sync)]

use crate::discovery::{eval_family_excluded, is_skipped, pre_existing_excluded};
use crate::harness::{append_source_chunk, get_harness_prefix, HarnessPrefixCache, HarnessSources};
use crate::judge::{
    classify_compile_error, judge_async_result, judge_vm_error, panic_payload_str, TestOutcome, TestResult,
};
use crate::meta::{parse_meta, strip_meta, TestMeta};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

// 记录当前正在执行的测试路径（thread-local）；每个测试执行前写入，
// panic hook 据此定位崩溃所在的测试文件。
std::thread_local! {
    pub(crate) static CURRENT_TEST_PATH: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 在 catch_unwind 保护下运行单个测试，把引擎 panic 记为失败。
#[expect(clippy::too_many_arguments)]
fn run_test(
    path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<KernelCore>, harness: &HarnessSources,
    harness_cache: &Arc<RwLock<HarnessPrefixCache>>, no_skip: bool, no_regalloc: bool,
) -> TestResult {
    let start = std::time::Instant::now();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_test_inner(path, source, meta, kernel, harness, harness_cache, no_skip, no_regalloc)
    }));

    match result {
        Ok(r) => r,
        Err(panic) => {
            let dur = start.elapsed().as_millis() as u64;
            TestResult::fail(path.to_path_buf(), dur, panic_payload_str(&panic))
        }
    }
}

/// test262 模块用例的依赖加载器：以测试文件父目录为基准解析相对导入，
/// 读取磁盘上的 fixture 源码；json/text/bytes 经 import attributes 判定。
struct Test262ModuleLoader;

impl oxide_emit::module::ModuleSourceLoader for Test262ModuleLoader {
    fn resolve(
        &mut self, base_dir: &str, specifier: &str, attributes: &[(&str, &str)],
    ) -> Result<oxide_emit::module::ResolvedModule, String> {
        use oxide_emit::module::{ModuleKind, ResolvedModule};
        let kind = attributes
            .iter()
            .find(|(k, _)| *k == "type")
            .map(|(_, v)| match *v {
                "json" => ModuleKind::Json,
                "text" => ModuleKind::Text,
                "bytes" => ModuleKind::Bytes,
                _ => ModuleKind::Js,
            })
            .unwrap_or(ModuleKind::Js);
        let base = Path::new(base_dir);
        let full = if specifier.starts_with('/') {
            PathBuf::from(specifier.trim_start_matches('/'))
        } else {
            base.join(specifier)
        };
        let canonical = full
            .canonicalize()
            .map_err(|e| format!("cannot resolve module {specifier}: {e}"))?;
        let content =
            std::fs::read_to_string(&canonical).map_err(|e| format!("cannot read module {specifier}: {e}"))?;
        Ok(ResolvedModule {
            source: content,
            path: canonical.to_string_lossy().to_string(),
            kind,
        })
    }
}

/// 单测执行主流程：拼 harness 前缀 → parse → compile → run；
/// 依据 `negative` 元数据校验期望错误，未实现特性按 no_skip 选择跳过或失败。
#[expect(clippy::too_many_arguments)]
fn run_test_inner(
    path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<KernelCore>, harness: &HarnessSources,
    harness_cache: &Arc<RwLock<HarnessPrefixCache>>, no_skip: bool, no_regalloc: bool,
) -> TestResult {
    let start = std::time::Instant::now();

    let is_async = meta.flags.iter().any(|f| f == "async");
    let is_module = meta.flags.iter().any(|f| f == "module");

    let code = match get_harness_prefix(meta, harness, harness_cache) {
        Ok(prefix) => {
            let mut code = prefix;
            if is_async {
                // 异步测试：注入 print 捕获 + $DONE（asyncTests 用 $DONE 报告结果）。
                // 结果写入 globalThis 全局字符串，run() 结束后由 runner 读取并按 marker 判定。
                // print 为顶层函数声明，经全局对象反射落 globalThis；此处仅初始化累加器。
                append_source_chunk(
                    &mut code,
                    "async capture",
                    "globalThis.$__test262_async_result = \"\";\n\
                     function print(msg) { globalThis.$__test262_async_result += String(msg) + \"\\n\"; }",
                );
                if let Some(done_src) = harness.get("doneprintHandle.js") {
                    append_source_chunk(&mut code, "doneprintHandle.js", done_src);
                }
            }
            append_source_chunk(&mut code, "test source", strip_meta(source));
            code
        }
        Err(e) => {
            let dur = start.elapsed().as_millis() as u64;
            if no_skip {
                return TestResult::fail(path.to_path_buf(), dur, e);
            }
            return TestResult::skip(path.to_path_buf(), e);
        }
    };

    let alloc = oxide_parser::Allocator::default();
    let program = match if is_module {
        oxide_parser::parse_module(&alloc, &code)
    } else {
        oxide_parser::parse(&alloc, &code)
    } {
        Ok(p) => p,
        Err(errs) => {
            let dur = start.elapsed().as_millis() as u64;
            let msg = format!("parse error: {}", errs[0].message);
            if meta.negative.is_some() {
                return TestResult::pass(path.to_path_buf(), dur, msg);
            }
            return TestResult::fail(path.to_path_buf(), dur, msg);
        }
    };

    let compiler = if no_regalloc { Compiler::new().with_regalloc(false) } else { Compiler::new() };
    let module = match if is_module {
        let mut loader = Test262ModuleLoader;
        compiler.compile_module(&program, path.to_string_lossy().as_ref(), &mut loader)
    } else {
        compiler.compile(&program)
    } {
        Ok(m) => m,
        Err(e) => {
            let dur = start.elapsed().as_millis() as u64;
            let msg = format!("compile error: {e}");
            match classify_compile_error(&msg, meta.negative.is_some(), no_skip) {
                TestOutcome::Pass(m) => return TestResult::pass(path.to_path_buf(), dur, m),
                TestOutcome::Fail(m) => return TestResult::fail(path.to_path_buf(), dur, m),
                TestOutcome::Skip(m) => return TestResult::skip(path.to_path_buf(), m),
            }
        }
    };

    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));
    let run_result = vm.run(&Arc::new(module));
    let dur = start.elapsed().as_millis() as u64;

    if is_async {
        return judge_async_result(path, run_result, &mut vm, meta, dur, no_skip);
    }

    match run_result {
        Ok(result) => {
            if let Some(neg) = meta.negative.as_ref() {
                return TestResult::fail(
                    path.to_path_buf(),
                    dur,
                    format!("expected runtime error ({}), got: {result}", neg.error_type),
                );
            }
            TestResult::pass(path.to_path_buf(), dur, format!("ok: {result}"))
        }
        Err(e) => judge_vm_error(path, &e, meta, dur, no_skip),
    }
}

/// 读取异步测试捕获的 `$DONE` 输出字符串。
pub(crate) fn read_async_output(vm: &Vm) -> String {
    let si = vm.kernel_core().perm_interner().intern("$__test262_async_result").0;
    let global = vm.session().global_object();
    let val = vm
        .kernel_core()
        .shape_forge()
        .lookup_position(global.shape_id(), si)
        .map(|pos| global.get_prop_at(pos))
        .unwrap_or(JsValue::undefined());
    vm.lookup_str(val).unwrap_or_default()
}

/// 串行与并行执行路径共享的每测试管线：
/// 读文件、解析元数据、应用跳过过滤，然后运行。恰好返回一个 `TestResult`。
/// worker 自有状态（`kernel`、`harness_sources`、`harness_cache`）永不跨线程。
pub(crate) fn process_path(
    path: &Path, filter: &Option<String>, no_skip: bool, no_regalloc: bool, kernel: &Arc<KernelCore>,
    harness_sources: &HarnessSources, harness_cache: &Arc<RwLock<HarnessPrefixCache>>,
) -> TestResult {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return TestResult::skip(path.to_path_buf(), format!("read error: {e}")),
    };

    let meta = match parse_meta(&source) {
        Some(m) => m,
        None => return TestResult::skip(path.to_path_buf(), String::from("no YAML metadata")),
    };

    if let Some(filter_str) = filter {
        let path_str = path.to_string_lossy().replace('\\', "/");
        if !path_str.contains(filter_str.as_str()) {
            return TestResult::skip(path.to_path_buf(), "filtered".into());
        }
    }

    if path.to_string_lossy().contains("staging") {
        return TestResult::skip(path.to_path_buf(), "staging tests excluded".into());
    }

    if !no_skip {
        let path_str = path.to_string_lossy().replace('\\', "/");
        if path_str.contains("/function-ctor/") || path_str.contains("/realm/") {
            return TestResult::skip(path.to_path_buf(), "unsupported class/eval feature excluded".into());
        }
        if let Some(reason) = eval_family_excluded(&path_str) {
            return TestResult::skip(path.to_path_buf(), reason.into());
        }
        if let Some(reason) = pre_existing_excluded(&path_str) {
            return TestResult::skip(path.to_path_buf(), reason.into());
        }
        if let Some(reason) = is_skipped(&meta) {
            return TestResult::skip(path.to_path_buf(), reason);
        }
    }

    run_test(path, &source, &meta, kernel, harness_sources, harness_cache, no_skip, no_regalloc)
}

/// 构建带步数上限的 runner kernel。每个并行 worker 拥有自己的 kernel，
/// 因为 `KernelCore` + session 状态是 `!Send`（持有 `P<JsObject>` =
/// `Arc<JsObject>`，而 `JsObject` 存有裸 `*mut u8` 属性指针）。任何 kernel
/// 形态的对象都不能跨线程边界，因此共享不可能；每 worker 自建是唯一正确设计。
pub(crate) fn build_runner_kernel() -> Arc<KernelCore> {
    // 约束每个测试的执行步数，使单个死循环 / 未支持特性循环失败（或跳过）
    // 而非拖垮整个运行。VM 超限时抛 "VM step limit exceeded" 错误，
    // runner 将其归类为 step-limit 结果（默认 skip，--no-skip 下 fail）。
    // 只覆盖 runner 本地配置；KernelConfig::minimal() 对其它 crate 保持无界。
    let mut kernel_config = KernelConfig::minimal();
    kernel_config.max_steps = Some(50_000_000);
    // 防止 test262 递归测试在 VM 把深层 JS 调用转成可捕获的 RangeError 之前
    // 触及 Rust 原生栈。
    kernel_config.max_call_depth = 256;
    kernel_config.min_pool_size = 1;
    kernel_config.max_pool_size = Some(1);
    // 单测试分配上限：死循环类测试触步数上限时持续分配，arena 高水位可达 GB
    // 级；多 worker 并发下进程 RSS 包络被各 worker 当前高水位顶起，全量运行
    // 必然 OOM。真合法峰值实测 259.6MiB（含 RegExp property-escapes/
    // character-class 生成大表），512MiB 上限留 ~252MiB 余量；
    // 主要作用是把失控测试的驻留面封顶到上限本身。
    kernel_config.max_alloc_bytes = Some(512 * 1024 * 1024);
    KernelCore::new(kernel_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分配上限常量与错误串单测的字面值机器耦合：改 cap 本钉必红。
    #[test]
    fn runner_alloc_cap_matches_error_string_tests() {
        assert_eq!(build_runner_kernel().config.max_alloc_bytes, Some(512 * 1024 * 1024));
    }
}
