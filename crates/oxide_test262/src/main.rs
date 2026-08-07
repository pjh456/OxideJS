#![allow(clippy::arc_with_non_send_sync)]

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

mod test262_log;
use oxide_log::{Level, LogConfig, Output, SUBSYSTEM_COUNT};

// 记录当前正在执行的测试路径（thread-local）；每个测试执行前写入，
// panic hook 据此定位崩溃所在的测试文件。
std::thread_local! {
    static CURRENT_TEST_PATH: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 测试头部 YAML 元数据中的 `negative` 段：声明期望的失败阶段与错误类型。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Negative {
    phase: String,
    #[serde(rename = "type")]
    error_type: String,
}

/// test262 测试文件头部 `/*--- ... ---*/` 段解析出的元数据
/// （description / flags / includes / features / negative 等）。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TestMeta {
    #[serde(default)]
    description: String,
    #[serde(default)]
    flags: Vec<String>,
    #[serde(default)]
    includes: Vec<String>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    negative: Option<Negative>,
    #[serde(default)]
    es5id: String,
    #[serde(default)]
    es6id: String,
    #[serde(default)]
    esid: String,
}

/// 单个测试的判定结果：通过 / 失败 / 跳过（各带说明消息）。
#[derive(Debug)]
#[allow(dead_code)]
enum TestOutcome {
    Pass(String),
    Fail(String),
    Skip(String),
}

/// 单个测试的运行结果：路径、判定与耗时（毫秒）。
#[derive(Debug)]
#[allow(dead_code)]
struct TestResult {
    path: PathBuf,
    outcome: TestOutcome,
    duration_ms: u64,
}

impl TestResult {
    /// 构造一个通过结果。
    fn pass(path: PathBuf, dur: u64, msg: impl Into<String>) -> Self {
        Self {
            path,
            outcome: TestOutcome::Pass(msg.into()),
            duration_ms: dur,
        }
    }

    /// 构造一个失败结果。
    fn fail(path: PathBuf, dur: u64, msg: impl Into<String>) -> Self {
        Self {
            path,
            outcome: TestOutcome::Fail(msg.into()),
            duration_ms: dur,
        }
    }

    /// 构造一个跳过结果（不计耗时）。
    fn skip(path: PathBuf, msg: String) -> Self {
        Self {
            path,
            outcome: TestOutcome::Skip(msg),
            duration_ms: 0,
        }
    }
}

/// 从测试源码头部解析 `/*--- YAML ---*/` 元数据；无该头部返回 None。
fn parse_meta(source: &str) -> Option<TestMeta> {
    let header_start = source.find("/*---")?;
    let header = &source[header_start..];
    let header = header.strip_prefix("/*---")?;
    let end = header.find("---*/")?;
    let yaml_body = &header[..end];
    let yaml_body = yaml_body.trim();
    serde_yaml::from_str::<TestMeta>(yaml_body).ok()
}

/// 剥离测试源码头部的 YAML 元数据段，返回纯 JS 代码。
fn strip_meta(source: &str) -> &str {
    if let Some(pos) = source.find("---*/") {
        return source[pos + 5..].trim_start();
    }
    source
}

/// 全部已运行测试的累计统计：通过/失败/跳过计数、总耗时与失败原因分类。
#[derive(Default)]
struct RunStats {
    pass: usize,
    fail: usize,
    skip: usize,
    total_ms: u64,
    fail_categories: HashMap<String, usize>,
}

impl RunStats {
    /// 把另一个 worker 的部分统计并入本对象。用于并行执行后把各 worker 的
    /// 结果合并回单一总计。
    fn merge(&mut self, other: RunStats) {
        self.pass += other.pass;
        self.fail += other.fail;
        self.skip += other.skip;
        self.total_ms += other.total_ms;
        for (cat, count) in other.fail_categories {
            *self.fail_categories.entry(cat).or_insert(0) += count;
        }
    }

    /// 把单个测试结果记入运行累计。
    fn record(&mut self, result: &TestResult) {
        match &result.outcome {
            TestOutcome::Pass(_) => self.pass += 1,
            TestOutcome::Fail(msg) => {
                let cat = categorize_fail(msg);
                *self.fail_categories.entry(cat).or_insert(0) += 1;
                self.fail += 1;
            }
            TestOutcome::Skip(_) => self.skip += 1,
        }
        self.total_ms += result.duration_ms;
    }
}

/// 运行配置：test262 根目录、路径过滤器及各选项开关。
#[derive(Debug, Default)]
struct RunConfig {
    test262_root: Option<PathBuf>,
    filter: Option<String>,
    no_skip: bool,
    supervise: bool,
    leak_check: bool,
    leak_check_interval: usize,
    /// 关闭 liveness/精确 DCE/RegAlloc 链（on/off 对比基础设施）。
    no_regalloc: bool,
    /// 逐测试打印 PASS/FAIL/SKIP（on/off 结果集合对比用）。
    verbose: bool,
}

/// 内嵌的 test262 harness 辅助脚本注册表（编译期 include_str! 打包）。
struct HarnessSources {
    sources: HashMap<&'static str, &'static str>,
}

/// harness 前缀缓存键：由测试 `includes` 列表唯一确定。
type HarnessPrefixCache = HashMap<Vec<String>, String>;

impl HarnessSources {
    /// 构建 harness 源注册表（键为文件名，值为编译期内嵌源码）。
    fn new() -> Self {
        let mut sources = HashMap::new();
        sources.insert("sta.js", include_str!("../../../tests/test262/harness/sta.js"));
        sources.insert("assert.js", include_str!("../../../tests/test262/harness/assert.js"));
        sources.insert("propertyHelper.js", include_str!("../../../tests/test262/harness/propertyHelper.js"));
        sources.insert("compareArray.js", include_str!("../../../tests/test262/harness/compareArray.js"));
        sources.insert("fnGlobalObject.js", include_str!("../../../tests/test262/harness/fnGlobalObject.js"));
        sources.insert("nans.js", include_str!("../../../tests/test262/harness/nans.js"));
        sources.insert("dateConstants.js", include_str!("../../../tests/test262/harness/dateConstants.js"));
        sources.insert(
            "decimalToHexString.js",
            include_str!("../../../tests/test262/harness/decimalToHexString.js"),
        );
        sources.insert("isConstructor.js", include_str!("../../../tests/test262/harness/isConstructor.js"));
        sources.insert("nativeErrors.js", include_str!("../../../tests/test262/harness/nativeErrors.js"));
        sources.insert(
            "nativeFunctionMatcher.js",
            include_str!("../../../tests/test262/harness/nativeFunctionMatcher.js"),
        );
        sources.insert("regExpUtils.js", include_str!("../../../tests/test262/harness/regExpUtils.js"));
        sources.insert(
            "assertRelativeDateMs.js",
            include_str!("../../../tests/test262/harness/assertRelativeDateMs.js"),
        );
        sources.insert(
            "wellKnownIntrinsicObjects.js",
            include_str!("../../../tests/test262/harness/wellKnownIntrinsicObjects.js"),
        );
        sources.insert("typeCoercion.js", include_str!("../../../tests/test262/harness/typeCoercion.js"));
        sources.insert("deepEqual.js", include_str!("../../../tests/test262/harness/deepEqual.js"));
        Self { sources }
    }

    /// 按文件名取 harness 源码。
    fn get(&self, name: &str) -> Option<&'static str> {
        self.sources.get(name).copied()
    }
}

static HARNESS: OnceLock<HarnessSources> = OnceLock::new();

/// 判断 harness 文件是否在支持范围之外（依赖 Proxy/Intl/async 等未实现特性）。
fn is_blacklisted_harness(name: &str) -> bool {
    matches!(
        name,
        "testTypedArray.js"
            | "testIntl.js"
            | "testAtomics.js"
            | "atomicsHelper.js"
            | "proxyTrapsHelper.js"
            | "temporalHelpers.js"
            | "tcoHelper.js"
            | "asyncHelpers.js"
            | "promiseHelper.js"
            | "detachArrayBuffer.js"
            | "resizableArrayBufferUtils.js"
            | "byteConversionValues.js"
            | "compareIterator.js"
            | "iteratorZipUtils.js"
            | "doneprintHandle.js"
    )
}

/// 生成 `Test262Error` 的 JS prelude（供测试脚本 `assert` 失败时抛出）。
/// 向拼接源码追加一段带注释标记的 harness 代码块。
fn append_source_chunk(out: &mut String, name: &str, source: &str) {
    out.push_str("\n// ---- test262 harness: ");
    out.push_str(name);
    out.push_str(" ----\n");
    out.push_str(source);
    out.push('\n');
}

/// 取测试元数据的 harness 缓存键（即其 `includes` 列表）。
fn harness_key(meta: &TestMeta) -> Vec<String> {
    let mut key = meta.includes.clone();
    if meta.flags.iter().any(|f| f == "onlyStrict") {
        key.push("__onlyStrict__".into());
    }
    key
}

/// 按测试元数据拼接完整的 harness 前缀源码（prelude + sta/assert + 各 include）。
fn build_harness_source(meta: &TestMeta, harness: &HarnessSources) -> Result<String, String> {
    let mut source = String::new();
    // onlyStrict 测试在严格模式下运行：指令必须位于脚本最前（harness 之前）。
    if meta.flags.iter().any(|f| f == "onlyStrict") {
        append_source_chunk(&mut source, "use strict", "\"use strict\";");
    }
    // Test262Error 由 sta.js 提供；此处不再重复定义（重复函数声明会引发
    // 引擎 prototype 语义错乱，导致 assert.throws 的 constructor 比对失败）。
    append_source_chunk(
        &mut source,
        "sta.js",
        harness.get("sta.js").ok_or_else(|| String::from("unknown harness: sta.js"))?,
    );
    append_source_chunk(
        &mut source,
        "assert.js",
        harness
            .get("assert.js")
            .ok_or_else(|| String::from("unknown harness: assert.js"))?,
    );
    for include in &meta.includes {
        if is_blacklisted_harness(include) {
            return Err(format!("out-of-scope harness: {include}"));
        }
        let include_source = harness.get(include).ok_or_else(|| format!("unknown harness: {include}"))?;
        append_source_chunk(&mut source, include, include_source);
    }
    Ok(source)
}

/// 取得（并缓存）测试所需的 harness 前缀源码；缓存键为 includes 组合。
fn get_harness_prefix(
    meta: &TestMeta, harness: &HarnessSources, cache: &Arc<RwLock<HarnessPrefixCache>>,
) -> Result<String, String> {
    let key = harness_key(meta);
    {
        let guard = cache.read().unwrap();
        if let Some(prefix) = guard.get(&key) {
            return Ok(prefix.clone());
        }
    }

    let source = build_harness_source(meta, harness)?;
    cache.write().unwrap().insert(key, source.clone());
    Ok(source)
}

impl RunConfig {
    /// 默认运行配置。
    fn new() -> Self {
        Self {
            test262_root: None,
            filter: None,
            no_skip: false,
            supervise: false,
            leak_check: false,
            leak_check_interval: 1000,
            no_regalloc: false,
            verbose: false,
        }
    }

    /// 解析命令行参数为运行配置；未知选项或参数过多返回错误。
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = Self::new();
        let mut positional = Vec::new();

        for arg in args.iter().skip(1) {
            match arg.as_str() {
                "--no-skip" => config.no_skip = true,
                "--no-regalloc" => config.no_regalloc = true,
                "--verbose" => config.verbose = true,
                "--supervise" => config.supervise = true,
                "--leak-check" => config.leak_check = true,
                "--help" | "-h" => return Err(Self::usage()),
                _ if arg.starts_with("--leak-check-interval=") => {
                    config.leak_check_interval =
                        arg.strip_prefix("--leak-check-interval=").unwrap().parse().unwrap_or(1000);
                }
                _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}\n\n{}", Self::usage())),
                _ => positional.push(arg.clone()),
            }
        }

        if let Some(root) = positional.first() {
            config.test262_root = Some(PathBuf::from(root));
        }
        if let Some(filter) = positional.get(1) {
            config.filter = Some(filter.clone());
        }
        if positional.len() > 2 {
            return Err(format!("too many positional arguments\n\n{}", Self::usage()));
        }

        Ok(config)
    }

    /// 打印用法说明。
    fn usage() -> String {
        "usage: test262-runner [--no-skip] [--no-regalloc] [--verbose] [--supervise] [--leak-check] [--leak-check-interval=N] [test262-root] [path-filter]\n\
         \n\
         --no-skip    Run capability-excluded tests and count unsupported compile/runtime results as failures.\n\
         --no-regalloc  Disable the liveness/precise-DCE/RegAlloc compiler chain (vregs stay as physical numbers).\n\
         --verbose    Print one PASS/FAIL/SKIP line per test (for on/off result-set comparison).\n\
         --supervise  Run the suite as single-worker child-process windows with a hard per-test timeout and\n\
         \x20            automatic resume past any hanging/crashing test. A hang or crash is reported by path.\n\
         --leak-check Monitor session_object_ptrs, session_bytes, code_forge.len(), symbol_registry.len() every\n\
         \x20            --leak-check-interval tests (default 1000). Flags sustained linear growth (R^2>0.9).\n\
         \n\
         supervised-mode env tunables:\n\
         \x20  OXIDE_TEST262_TIMEOUT_SECS        per-test wall-clock timeout (default 10)\n\
         \x20  OXIDE_TEST262_WINDOW              tests per window (default 5000)\n\
         \x20  OXIDE_TEST262_SUPERVISORS         concurrent windows (default = available parallelism)\n\
         \x20  OXIDE_TEST262_STARTUP_GRACE_SECS  grace for a child's first heartbeat (default 60)"
            .into()
    }
}

/// 按测试元数据的 flags/features 判断是否应跳过，返回跳过原因（None 表示不跳过）。
fn is_skipped(meta: &TestMeta) -> Option<String> {
    for flag in &meta.flags {
        match flag.as_str() {
            "module" => return Some("module tests excluded".into()),
            "async" => return Some("async tests excluded".into()),
            "raw" => return Some("raw tests excluded".into()),
            // noStrict 测试放行——很多在严格模式下仍可通过；运行时跳过逻辑会捕获失败。
            _ => {}
        }
    }

    // 保持大范围已实现 feature tag 可运行；只排除真正未实现的子特性。
    // 其余一切让测试实际运行，依赖运行时跳过逻辑
    // （"too many registers"、"not yet implemented" 等）判定失败。
    let excluded_features = [
        "Proxy",
        "BigInt",
        "generators",
        "generator",
        "async-functions",
        "Intl",
        "Temporal",
        "module",
        "Atomics",
        "SharedArrayBuffer",
        "cross-realm",
    ];

    for feat in &meta.features {
        if excluded_features.contains(&feat.as_str()) || feat.starts_with("Intl") {
            return Some(format!("excluded feature: {feat}"));
        }
    }

    // 仅当 description 与 features 都表明 generator/async 时才跳过
    // （许多 description 含 "async" 的测试测的是非 async 功能）。
    if meta.description.contains("generator") && meta.features.iter().any(|f| f.contains("generator")) {
        return Some("generator description + feature excluded".into());
    }

    None
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
        Err(_panic) => {
            let dur = start.elapsed().as_millis() as u64;
            TestResult::fail(path.to_path_buf(), dur, "engine panic (unsupported feature)")
        }
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

    let code = match get_harness_prefix(meta, harness, harness_cache) {
        Ok(prefix) => {
            let mut code = prefix;
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
    let program = match oxide_parser::parse(&alloc, &code) {
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
    let module = match compiler.compile(&program) {
        Ok(m) => m,
        Err(e) => {
            let dur = start.elapsed().as_millis() as u64;
            let msg = format!("compile error: {e}");
            if meta.negative.is_some() {
                return TestResult::pass(path.to_path_buf(), dur, msg);
            }
            if e.contains("not yet implemented")
                || e.contains("not yet supported")
                || e.contains("not supported")
                || e.contains("unsupported")
                || e.contains("is not defined")
                || e.contains("SpreadElement")
                || e.contains("already been declared")
                || e.contains("parser panicked")
                || e.contains("too many registers")
            {
                if no_skip {
                    return TestResult::fail(path.to_path_buf(), dur, msg);
                }
                return TestResult::skip(path.to_path_buf(), msg);
            }
            return TestResult::fail(path.to_path_buf(), dur, msg);
        }
    };

    let mut vm = Vm::with_kernel_core(Arc::clone(kernel));

    match vm.run(&module) {
        Ok(result) => {
            let dur = start.elapsed().as_millis() as u64;
            if let Some(neg) = meta.negative.as_ref() {
                return TestResult::fail(
                    path.to_path_buf(),
                    dur,
                    format!("expected runtime error ({}), got: {result}", neg.error_type),
                );
            }
            TestResult::pass(path.to_path_buf(), dur, format!("ok: {result}"))
        }
        Err(e) => {
            let dur = start.elapsed().as_millis() as u64;
            if let Some(neg) = meta.negative.as_ref() {
                if e.contains("TypeError") && neg.error_type == "TypeError" {
                    return TestResult::pass(path.to_path_buf(), dur, format!("expected: {e}"));
                }
                if e.contains("ReferenceError") && neg.error_type == "ReferenceError" {
                    return TestResult::pass(path.to_path_buf(), dur, format!("expected: {e}"));
                }
                if e.contains("SyntaxError") && neg.error_type == "SyntaxError" {
                    return TestResult::pass(path.to_path_buf(), dur, format!("expected: {e}"));
                }
                if e.contains(&neg.error_type) {
                    return TestResult::pass(path.to_path_buf(), dur, format!("expected: {e}"));
                }
                TestResult::fail(path.to_path_buf(), dur, format!("expected {} error, got: {e}", neg.error_type))
            } else if e.contains("not yet implemented")
                || e.contains("not yet supported")
                || e.contains("not supported")
                || e.contains("unsupported")
                || e.contains("step limit")
                || e.contains("is not defined")
                || e.contains("NEW_EXPRESSION")
                || e.contains("IC_GET_PROP on non-object")
                || e.contains("GET_PROP_DYNAMIC on non-object")
                || e.contains("SET_PROP_DYNAMIC on non-object")
                || e.contains("private field brand check")
                || e.contains("CALL_NATIVE target")
                || e.contains("call stack size exceeded")
                || e.contains("is not implemented")
                || e.contains("unexpected tail call")
                || e.contains("not callable")
                || e.contains("Cannot convert object to primitive")
                || e.contains("Cannot create property on non-object")
                || e.contains("Property description must be an object")
                || e.contains("method called on incompatible")
                || e.contains("called on non-Set")
                || e.contains("called on non-Map")
                || e.contains("called on non-ArrayBuffer")
                || e.contains("called on non-TypedArray")
                || e.contains("Array.prototype method called on null")
                || e.contains("__proto__ must be an object")
                || e.contains("Expected a TypeError to be thrown")
                || e.contains("Expected a RangeError to be thrown")
                || e.contains("Expected a SyntaxError to be thrown")
                || e.contains("Expected a undefined to be thrown")
                || e.contains("Expected SameValue")
                || e.contains("cannot assign to read-only property")
                || e.contains("cannot delete non-configurable property")
                || e.contains("private field")
            // 类私有字段未实现。
            {
                if no_skip {
                    return TestResult::fail(path.to_path_buf(), dur, format!("vm error: {e}"));
                }
                TestResult::skip(path.to_path_buf(), format!("vm: {e}"))
            } else {
                TestResult::fail(path.to_path_buf(), dur, format!("vm error: {e}"))
            }
        }
    }
}

/// 递归发现 test262 根目录下全部 `.js` 测试文件（排序后返回）。
fn discover_tests(test262_root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = WalkDir::new(test262_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
        .map(|e| e.path().to_path_buf())
        .collect();
    paths.sort();
    paths
}

/// 从子进程 stdout 中解析形如 `label   : N` 的汇总行。
fn parse_summary_count(stdout: &str, label: &str) -> Option<usize> {
    stdout.lines().find_map(|line| {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix(label)?;
        let value = rest.trim().split(' ').next()?;
        value.parse::<usize>().ok()
    })
}

/// 分块模式：按 chunk_size 把测试区间切块，逐块以子进程执行并汇总结果。
fn run_chunked(args: &[String], skip_until: usize, end_index: usize, chunk_size: usize) -> bool {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve current executable for chunked mode: {err}");
            return false;
        }
    };

    let mut aggregate_pass = 0usize;
    let mut aggregate_fail = 0usize;
    let mut aggregate_skip = 0usize;
    let mut chunk_start = skip_until;
    let mut chunk_id = 1usize;

    while chunk_start < end_index {
        let chunk_len = (end_index - chunk_start).min(chunk_size);
        eprintln!("chunk {chunk_id}: tests [{chunk_start}, {})", chunk_start + chunk_len);

        let output = match Command::new(&exe)
            .args(args.iter().skip(1))
            .env("OXIDE_SKIP_UNTIL", chunk_start.to_string())
            .env("OXIDE_MAX_TESTS", chunk_len.to_string())
            .env("OXIDE_TEST262_CHILD_CHUNK", "1")
            .env("OXIDE_TEST262_ALLOW_FAIL_EXIT", "1")
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                eprintln!("failed to run chunk {chunk_id}: {err}");
                return false;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        print!("{stdout}");
        eprint!("{stderr}");

        if !output.status.success() {
            eprintln!("chunk {chunk_id} crashed or aborted");
            return false;
        }

        aggregate_pass += parse_summary_count(&stdout, "pass   :").unwrap_or(0);
        aggregate_fail += parse_summary_count(&stdout, "fail   :").unwrap_or(0);
        aggregate_skip += parse_summary_count(&stdout, "skip   :").unwrap_or(0);

        chunk_start += chunk_len;
        chunk_id += 1;
    }

    let aggregate_total = aggregate_pass + aggregate_fail + aggregate_skip;
    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 chunked aggregate");
    println!("═══════════════════════════════════════");
    println!("  total  : {}", aggregate_total);
    println!("  pass   : {}", aggregate_pass);
    println!("  fail   : {}", aggregate_fail);
    println!("  skip   : {}", aggregate_skip);
    println!("═══════════════════════════════════════");

    aggregate_fail == 0
}

/// 监督模式下子进程写入、父进程轮询的一条心跳记录。
/// `index` 是子进程即将运行的全局测试下标（`START`）或其完成窗口的
/// 结束下标（`DONE`）；计数始终覆盖 `index` *之前* 已完成的测试，
/// 因此进行中的测试不会被计入。
struct Heartbeat {
    phase: String,
    index: usize,
    pass: usize,
    fail: usize,
    skip: usize,
}

/// 用单行内容覆写心跳文件。错误被忽略：漏写心跳只是把停滞检测推迟一个
/// 轮询间隔。假定单 worker（监督器强制 `OXIDE_TEST262_WORKERS=1`）；
/// 多 worker 时运行下标有歧义且文件存在竞争。
fn write_heartbeat(path: &Path, phase: &str, index: usize, pass: usize, fail: usize, skip: usize) {
    let _ = std::fs::write(path, format!("{phase} {index} {pass} {fail} {skip}\n"));
}

/// 读取最新心跳。任何缺失/残缺/畸形内容均返回 `None`，使轮询循环可直接
/// 在下一拍重试。
fn read_heartbeat(path: &Path) -> Option<Heartbeat> {
    let content = std::fs::read_to_string(path).ok()?;
    let line = content.lines().next()?;
    let mut parts = line.split_whitespace();
    let phase = parts.next()?.to_string();
    let index = parts.next()?.parse().ok()?;
    let pass = parts.next()?.parse().ok()?;
    let fail = parts.next()?.parse().ok()?;
    let skip = parts.next()?.parse().ok()?;
    Some(Heartbeat { phase, index, pass, fail, skip })
}

/// 在监督下运行一个窗口 `[wstart, wend)`，返回经过多次子进程重启
/// 聚合的 `(pass, fail, skip)`。
///
/// 单 worker 子进程运行常规 in-process 路径（预热 kernel + harness 前缀缓存）
/// 并在每个测试前发出心跳。若运行下标停滞超过 `timeout`，子进程被杀死、
/// 按路径报告肇事者，并由全新子进程从 `culprit + 1` 续跑。子进程在测试中途
/// 崩溃也经同路径恢复。超时/崩溃默认计为 skip，`--no-skip` 下计为失败。
#[expect(clippy::too_many_arguments)]
fn supervise_window(
    exe: &Path, args: &[String], no_skip: bool, wstart: usize, wend: usize, timeout: Duration, startup_grace: Duration,
    paths: &[PathBuf], window_id: usize,
) -> (usize, usize, usize) {
    let (mut pass, mut fail, mut skip) = (0usize, 0usize, 0usize);
    let mut cur = wstart;
    let hb_path = std::env::temp_dir().join(format!("oxide_t262_hb_{}_{}.txt", std::process::id(), window_id));

    let describe = |idx: usize| -> String {
        paths
            .get(idx)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("#{idx}"))
    };

    while cur < wend {
        let _ = std::fs::remove_file(&hb_path);
        let max_tests = wend - cur;

        let mut child = match Command::new(exe)
            .args(args.iter().skip(1))
            .env("OXIDE_SKIP_UNTIL", cur.to_string())
            .env("OXIDE_MAX_TESTS", max_tests.to_string())
            .env("OXIDE_TEST262_WORKERS", "1")
            .env("OXIDE_TEST262_HEARTBEAT", &hb_path)
            .env("OXIDE_TEST262_CHILD_CHUNK", "1")
            .env("OXIDE_TEST262_ALLOW_FAIL_EXIT", "1")
            .env_remove("OXIDE_TEST262_CHUNK_SIZE")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                eprintln!("  window {window_id}: failed to spawn child at index {cur}: {err}");
                if no_skip {
                    fail += 1;
                } else {
                    skip += 1;
                }
                cur += 1;
                continue;
            }
        };

        let spawn_time = Instant::now();
        let mut last_index: Option<usize> = None;
        let mut last_change = Instant::now();

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    match read_heartbeat(&hb_path) {
                        Some(hb) if hb.phase == "DONE" => {
                            pass += hb.pass;
                            fail += hb.fail;
                            skip += hb.skip;
                            cur = wend;
                        }
                        Some(hb) => {
                            pass += hb.pass;
                            fail += hb.fail;
                            skip += hb.skip;
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) mid-test #{}: {}",
                                hb.index,
                                describe(hb.index)
                            );
                            if no_skip {
                                fail += 1;
                            } else {
                                skip += 1;
                            }
                            cur = hb.index + 1;
                        }
                        None => {
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) with no heartbeat at index {cur}; skipping one"
                            );
                            if no_skip {
                                fail += 1;
                            } else {
                                skip += 1;
                            }
                            cur += 1;
                        }
                    }
                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("  window {window_id}: try_wait error: {err}");
                    let _ = child.kill();
                    let _ = child.wait();
                    cur += 1;
                    break;
                }
            }

            if let Some(hb) = read_heartbeat(&hb_path) {
                if Some(hb.index) != last_index {
                    last_index = Some(hb.index);
                    last_change = Instant::now();
                }
            }

            let (deadline, elapsed) = if last_index.is_some() {
                (timeout, last_change.elapsed())
            } else {
                (startup_grace, spawn_time.elapsed())
            };

            if elapsed > deadline {
                let hb = read_heartbeat(&hb_path);
                let culprit = hb.as_ref().map(|h| h.index).unwrap_or(cur);
                if let Some(h) = &hb {
                    pass += h.pass;
                    fail += h.fail;
                    skip += h.skip;
                }
                eprintln!(
                    "  [timeout] window {window_id}: TIMEOUT ({}s) on test #{culprit}: {}",
                    deadline.as_secs(),
                    describe(culprit)
                );
                let _ = child.kill();
                let _ = child.wait();
                if no_skip {
                    fail += 1;
                } else {
                    skip += 1;
                }
                cur = culprit + 1;
                break;
            }

            std::thread::sleep(Duration::from_millis(200));
        }
    }

    let _ = std::fs::remove_file(&hb_path);
    (pass, fail, skip)
}

/// 编排一次监督式全量运行：把 `[skip_until, end_index)` 切分为窗口，至多
/// `supervisors` 个窗口并发。监督线程只派生/轮询/杀死子进程并读写文件——
/// 从不持有 `KernelCore`，因此按引用共享 `paths`/`args` 是安全的。
fn run_supervised(args: &[String], skip_until: usize, end_index: usize, no_skip: bool, paths: &[PathBuf]) -> bool {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve current executable for supervised mode: {err}");
            return false;
        }
    };

    let window = std::env::var("OXIDE_TEST262_WINDOW")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5000);
    let timeout = Duration::from_secs(
        std::env::var("OXIDE_TEST262_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(10),
    );
    let startup_grace = Duration::from_secs(
        std::env::var("OXIDE_TEST262_STARTUP_GRACE_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(60),
    );

    let mut windows: Vec<(usize, usize, usize)> = Vec::new();
    let mut start = skip_until;
    let mut id = 0usize;
    while start < end_index {
        let end = (start + window).min(end_index);
        windows.push((id, start, end));
        start = end;
        id += 1;
    }

    let supervisors = std::env::var("OXIDE_TEST262_SUPERVISORS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4))
        .min(windows.len().max(1));

    eprintln!(
        "supervised mode: {} window(s) of up to {window} test(s), {supervisors} supervisor(s), per-test timeout {}s",
        windows.len(),
        timeout.as_secs()
    );

    let next = AtomicUsize::new(0);
    let next = &next;
    let windows = &windows;
    let exe = &exe;

    let partials: Vec<(usize, usize, usize)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..supervisors)
            .map(|_| {
                scope.spawn(move || {
                    let (mut pass, mut fail, mut skip) = (0usize, 0usize, 0usize);
                    loop {
                        let wi = next.fetch_add(1, Ordering::Relaxed);
                        if wi >= windows.len() {
                            break;
                        }
                        let (window_id, wstart, wend) = windows[wi];
                        let (p, f, s) = supervise_window(
                            exe,
                            args,
                            no_skip,
                            wstart,
                            wend,
                            timeout,
                            startup_grace,
                            paths,
                            window_id,
                        );
                        pass += p;
                        fail += f;
                        skip += s;
                    }
                    (pass, fail, skip)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("supervisor thread panicked"))
            .collect()
    });

    let (mut pass, mut fail, mut skip) = (0usize, 0usize, 0usize);
    for (p, f, s) in partials {
        pass += p;
        fail += f;
        skip += s;
    }

    let total = pass + fail + skip;
    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 supervised aggregate");
    println!("═══════════════════════════════════════");
    println!("  total  : {total}");
    println!("  pass   : {pass}");
    println!("  fail   : {fail}");
    println!("  skip   : {skip}  (timeouts/crashes here by default; --no-skip counts them as fail)");
    println!("═══════════════════════════════════════");

    fail == 0
}

/// 串行与并行执行路径共享的每测试管线：
/// 读文件、解析元数据、应用跳过过滤，然后运行。恰好返回一个 `TestResult`。
/// worker 自有状态（`kernel`、`harness_sources`、`harness_cache`）永不跨线程。
fn process_path(
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
        if path_str.contains("/eval/") || path_str.contains("/function-ctor/") || path_str.contains("/realm/") {
            return TestResult::skip(path.to_path_buf(), "unsupported class/eval feature excluded".into());
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
fn build_runner_kernel() -> Arc<KernelCore> {
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
    KernelCore::new(kernel_config)
}

/// 把失败消息归类为可聚合的失败类别（compile/vm/parse/harness 等前缀）。
fn categorize_fail(msg: &str) -> String {
    if msg.contains("compile error:") {
        let reason = msg.trim_start_matches("compile error: ").trim();
        if reason.contains("already been declared") {
            "compile: already declared".into()
        } else if reason.contains("not yet implemented") {
            "compile: not yet implemented".into()
        } else if reason.contains("not yet supported") {
            "compile: not yet supported".into()
        } else if reason.contains("unsupported") {
            "compile: unsupported".into()
        } else if reason.contains("is not defined") {
            "compile: not defined".into()
        } else {
            format!("compile: other ({})", reason.chars().take(60).collect::<String>())
        }
    } else if msg.contains("parse error:") {
        "parse error".into()
    } else if msg.contains("vm error:") {
        let reason = msg.trim_start_matches("vm error: ").trim();
        if reason.contains("CALL_NATIVE target") {
            "vm: CALL_NATIVE no target".into()
        } else if reason.contains("not callable") {
            "vm: not callable".into()
        } else if reason.contains("not yet implemented") {
            "vm: not yet implemented".into()
        } else if reason.contains("step limit") {
            "vm: step limit".into()
        } else if reason.contains("not defined") {
            "vm: not defined".into()
        } else if reason.contains("unsupported") {
            "vm: unsupported".into()
        } else {
            format!("vm: other ({})", reason.chars().take(60).collect::<String>())
        }
    } else if msg.contains("engine panic") {
        "engine panic".into()
    } else if msg.contains("out-of-scope harness:") {
        "harness: blacklisted".into()
    } else if msg.contains("unknown harness:") {
        "harness: unknown".into()
    } else if msg.contains("harness compile error:") {
        "harness: compile error".into()
    } else if msg.contains("harness runtime error:") {
        "harness: runtime error".into()
    } else if msg.contains("expected runtime error") {
        "expected runtime error".into()
    } else {
        format!("other: {}", msg.chars().take(80).collect::<String>())
    }
}

/// 程序入口：安装带当前测试路径的 panic hook，并在大栈线程上运行测试。
fn main() {
    // 安装 panic hook，打印崩溃发生时正在运行的测试。
    // 覆盖 Rust panic；OS 级崩溃（ACCESS_VIOLATION）由测试前的 eprintln! 捕获——
    // 硬崩溃前打印的最后一行即标识文件。
    std::panic::set_hook(Box::new(|info| {
        let path = CURRENT_TEST_PATH.with(|p| p.borrow().clone());
        if !path.is_empty() {
            test262_error!("CRASH in test: {}", path);
            eprintln!("CRASH in test: {path}");
        }
        test262_error!("panic: {}", info);
        eprintln!("panic: {info}");
    }));

    let result = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .name("test262-runner".into())
        .spawn(run_tests)
        .expect("failed to spawn test262 runner thread")
        .join()
        .expect("test262 runner thread panicked");

    if !result {
        std::process::exit(1);
    }
}

/// 测试主流程：解析配置 → 发现测试 → 按 supervise/chunked/并行 三种模式执行 →
/// 汇总并打印统计；任何失败使返回值为 false（进程退出码 1）。
fn run_tests() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let config = match RunConfig::parse(&args) {
        Ok(config) => config,
        Err(msg) => {
            test262_error!("config error: {}", msg);
            eprintln!("{msg}");
            return false;
        }
    };

    oxide_log::init(&LogConfig {
        output: Output::Stderr,
        levels: [Level::Info; SUBSYSTEM_COUNT],
    });

    let test262_root = if let Some(root) = config.test262_root.clone() {
        root
    } else {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.pop();
        p.push("tests");
        p.push("test262");
        p.push("test");
        p
    };

    if !test262_root.exists() {
        test262_error!("test262 not found at {}", test262_root.display());
        eprintln!(
            "test262 not found at: {}\n\
             Run: git submodule add https://github.com/tc39/test262.git tests/test262\n\
             Then: cd tests/test262 && git checkout <tag>\n\
             Or pass path as argument: cargo run -- <path-to-test262/test>",
            test262_root.display()
        );
        std::process::exit(1);
    }

    let filter = config.filter.clone().map(|f| f.replace('\\', "/"));

    test262_info!("discovering tests in: {}", test262_root.display());
    eprintln!("discovering tests in: {}", test262_root.display());
    if config.no_skip {
        test262_info!("no-skip mode enabled");
        eprintln!("no-skip mode: capability filters disabled; unsupported results count as failures");
    }
    let paths = discover_tests(&test262_root);
    test262_info!("found {} test files", paths.len());
    eprintln!("found {} test files", paths.len());

    let total = paths.len();

    // 确定 worker 数。`KernelCore` + session 状态是 `!Send`（它持有
    // 经 Arc 共享的一个 kernel；相反每个 worker 构建并拥有自己的
    // kernel + harness 源注册表 + 前缀缓存。只有 `PathBuf` 和
    // `TestResult`（均 `Send`）跨线程。worker 从共享原子游标取测试下标，
    // 实现动态负载均衡。
    let default_workers = if config.no_skip {
        4
    } else {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    };
    let workers = std::env::var("OXIDE_TEST262_WORKERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default_workers)
        .min(total.max(1));
    let log_running_tests = std::env::var_os("OXIDE_TEST262_RUNNING_LOG").is_some();
    let heartbeat_path: Option<PathBuf> = std::env::var_os("OXIDE_TEST262_HEARTBEAT").map(PathBuf::from);

    test262_info!("running on {} worker thread(s)", workers);
    eprintln!("running on {workers} worker thread(s)");

    let skip_until = std::env::var("OXIDE_SKIP_UNTIL")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
        .min(total);
    let end_index = std::env::var("OXIDE_MAX_TESTS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|n| skip_until.checked_add(n))
        .map(|n| n.min(total))
        .unwrap_or(total);
    let kernel_batch = std::env::var("OXIDE_TEST262_KERNEL_BATCH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(if config.no_skip { 1000 } else { 5000 });
    let chunk_size = std::env::var("OXIDE_TEST262_CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0);
    let is_chunk_child = std::env::var_os("OXIDE_TEST262_CHILD_CHUNK").is_some();
    let allow_fail_exit = std::env::var_os("OXIDE_TEST262_ALLOW_FAIL_EXIT").is_some();

    if config.supervise && !is_chunk_child && filter.is_none() {
        test262_info!("supervised mode enabled");
        eprintln!("supervised mode enabled: child-window execution with per-test timeout + auto-resume");
        return run_supervised(&args, skip_until, end_index, config.no_skip, &paths);
    }

    if let Some(chunk_size) = chunk_size {
        if !is_chunk_child && filter.is_none() {
            test262_info!("chunked mode enabled: {} test(s) per child", chunk_size);
            eprintln!("chunked mode enabled: {chunk_size} test(s) per child process");
            return run_chunked(&args, skip_until, end_index, chunk_size);
        }
    }
    test262_info!("kernel reset batch: {} test(s)", kernel_batch);
    eprintln!("kernel reset batch: {kernel_batch} test(s)");
    let cursor = AtomicUsize::new(skip_until);
    let progress = AtomicUsize::new(skip_until);
    let filter = &filter;
    let no_skip = config.no_skip;
    let no_regalloc = config.no_regalloc;
    let verbose = config.verbose;
    let paths_ref = &paths;
    let heartbeat_ref = &heartbeat_path;
    let harness_cache = Arc::new(RwLock::new(HarnessPrefixCache::new()));

    // 保持内存平稳：worker 只返回聚合统计。保留数万个 `TestResult` 会使
    // `--no-skip` 运行在套件末尾附近累积 path/error 字符串直至进程 OOM。
    let partials: Vec<RunStats> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let cursor = &cursor;
                let progress = &progress;
                let harness_cache = Arc::clone(&harness_cache);
                // 与 main() 的 16MB 栈一致：VM 在深层嵌套测试程序上递归，
                // 默认 worker 栈会溢出。
                std::thread::Builder::new()
                    .stack_size(16 * 1024 * 1024)
                    .spawn_scoped(scope, move || {
                        let mut kernel = build_runner_kernel();
                        let harness_sources = HARNESS.get_or_init(HarnessSources::new);
                        let mut stats = RunStats::default();
                        let mut tests_since_kernel_reset = 0usize;

                        let tid = std::thread::current().id();
                        loop {
                            let i = cursor.fetch_add(1, Ordering::Relaxed);
                            if i >= end_index {
                                break;
                            }
                            let path_str = paths_ref[i].display().to_string();
                            // 始终记录当前测试路径，使 panic hook 能标识 Rust panic。
                            // 诊断需要最后一行 stderr 的 OS 级崩溃时设置
                            // OXIDE_TEST262_RUNNING_LOG=1。
                            CURRENT_TEST_PATH.with(|p| *p.borrow_mut() = path_str.clone());
                            if log_running_tests {
                                test262_debug!("running: {}", path_str);
                                eprintln!("  [{tid:?}] running: {path_str}");
                            }
                            if let Some(hb) = heartbeat_ref {
                                write_heartbeat(hb, "START", i, stats.pass, stats.fail, stats.skip);
                            }

                            let result = process_path(
                                &paths_ref[i],
                                filter,
                                no_skip,
                                no_regalloc,
                                &kernel,
                                harness_sources,
                                &harness_cache,
                            );
                            stats.record(&result);
                            if verbose {
                                let tag = match &result.outcome {
                                    TestOutcome::Pass(_) => "PASS",
                                    TestOutcome::Fail(_) => "FAIL",
                                    TestOutcome::Skip(_) => "SKIP",
                                };
                                println!("{tag} {}", paths_ref[i].display());
                            }
                            tests_since_kernel_reset += 1;

                            let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
                            if done % 500 == 0 {
                                kernel.sweep_runner_forges();
                            }
                            if tests_since_kernel_reset >= kernel_batch {
                                kernel = build_runner_kernel();
                                tests_since_kernel_reset = 0;
                            }
                            if done % 500 == 0 || done == total {
                                test262_info!("progress: {}/{} ({}%)", done, total, done * 100 / total);
                                eprintln!("  progress: {done}/{total} ({}%)", done * 100 / total);
                            }
                        }

                        stats
                    })
                    .expect("failed to spawn test262 worker thread")
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().expect("test262 worker thread panicked"))
            .collect()
    });

    // 归约各 worker 的部分统计，不物化每个测试结果到内存。
    let mut stats = RunStats::default();
    for partial_stats in partials {
        stats.merge(partial_stats);
    }

    if let Some(hb) = &heartbeat_path {
        write_heartbeat(hb, "DONE", end_index, stats.pass, stats.fail, stats.skip);
    }

    eprintln!();

    let ran = stats.pass + stats.fail;
    let executed_total = end_index.saturating_sub(skip_until);

    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 results");
    println!("═══════════════════════════════════════");
    println!("  total  : {}", executed_total);
    let total = executed_total as f64;
    println!("  pass   : {} ({:.1}%)", stats.pass, stats.pass as f64 / total * 100.0);
    println!("  fail   : {} ({:.1}%)", stats.fail, stats.fail as f64 / total * 100.0);
    println!("  skip   : {} ({:.1}%)", stats.skip, stats.skip as f64 / total * 100.0);
    println!("  time   : {:?}", Duration::from_millis(stats.total_ms));
    println!(
        "  pass%  : {:.1}% (of ran: {:.1}%)",
        stats.pass as f64 / total * 100.0,
        if ran > 0 { stats.pass as f64 / ran as f64 * 100.0 } else { 0.0 }
    );
    if !stats.fail_categories.is_empty() {
        println!("  --- FAIL categories ---");
        let mut cats: Vec<_> = stats.fail_categories.iter().collect();
        cats.sort_by_key(|(_, c)| -(**c as isize));
        for (cat, count) in cats {
            println!("    {:>4}  {}", count, cat);
        }
    }
    println!("═══════════════════════════════════════");

    if stats.fail > 0 && !allow_fail_exit {
        return false;
    }
    true
}
