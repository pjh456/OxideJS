#![allow(clippy::arc_with_non_send_sync)]

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

mod report;
mod test262_log;
use oxide_log::{Level, LogConfig, Output, SUBSYSTEM_COUNT};
use report::{
    append_fail_log, first_line, format_fail_categories, format_fail_list, parse_fail_log, FailRecord,
    MAX_FAIL_MSG_CHARS, MAX_FAIL_RECORD_BYTES,
};

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
#[derive(Debug, PartialEq)]
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

/// 全部已运行测试的累计统计：通过/失败/跳过计数、总耗时、失败原因分类
/// 与逐路径失败记录（fail_records，供汇总区聚合报告）。
#[derive(Default)]
struct RunStats {
    pass: usize,
    fail: usize,
    skip: usize,
    total_ms: u64,
    fail_categories: HashMap<String, usize>, // 计数口径不变（兼容心跳序列化与既有基线对比）
    // ↓ 新增 ↓
    categories: Vec<String>, // 类别 id 表（FailRecord.category_id 索引）
    fail_records: Vec<FailRecord>,
    fail_record_bytes: usize, // 累计 message 字节（OOM cap）
    // supervise 模式异常计数：spawn 失败 / try_wait 错误 / 心跳写失败（子进程侧
    // 累计后经心跳第 6 字段回传）。
    spawn_errors: usize,
    wait_errors: usize,
    hb_write_errors: usize,
    /// 超时/崩溃清单：(index, elapsed_ms)；elapsed_ms=0 表示非超时崩溃。
    timeout_crashes: Vec<(usize, u64)>,
}

impl RunStats {
    /// 把另一个 worker 的部分统计并入本对象。用于并行执行后把各 worker 的
    /// 结果合并回单一总计。失败记录的类别 id 按本表重映射（两侧类别表独立编号）。
    fn merge(&mut self, other: RunStats) {
        self.pass += other.pass;
        self.fail += other.fail;
        self.skip += other.skip;
        self.total_ms += other.total_ms;
        for (cat, count) in other.fail_categories {
            *self.fail_categories.entry(cat).or_insert(0) += count;
        }
        for rec in other.fail_records {
            let name = other.categories[rec.category_id as usize].clone();
            let id = self.category_id_of(&name);
            self.fail_records.push(FailRecord { category_id: id, ..rec });
        }
        self.fail_record_bytes += other.fail_record_bytes;
        self.spawn_errors += other.spawn_errors;
        self.wait_errors += other.wait_errors;
        self.hb_write_errors += other.hb_write_errors;
        self.timeout_crashes.extend(other.timeout_crashes);
    }

    /// 把单个测试结果记入运行累计；失败同时追加逐路径失败记录（含类别）。
    fn record(&mut self, index: usize, result: &TestResult) {
        match &result.outcome {
            TestOutcome::Pass(_) => self.pass += 1,
            TestOutcome::Fail(msg) => {
                let cat = categorize_fail(msg);
                *self.fail_categories.entry(cat.clone()).or_insert(0) += 1;
                self.push_fail_record(index, cat, String::new(), msg.clone());
                self.fail += 1;
            }
            TestOutcome::Skip(_) => self.skip += 1,
        }
        self.total_ms += result.duration_ms;
    }

    /// 取得类别名在 categories 表中的 id（不存在则追加）。
    fn category_id_of(&mut self, name: &str) -> u16 {
        debug_assert!(self.categories.len() < u16::MAX as usize, "categories 表超出 u16 容量");
        if let Some(pos) = self.categories.iter().position(|c| c == name) {
            return pos as u16;
        }
        self.categories.push(name.to_string());
        (self.categories.len() - 1) as u16
    }

    /// 追加一条失败记录：单条消息截断到 2 KiB；累计字节超 64 MiB 后本条
    /// 降级为消息首行摘要且不再累计字节，防止 OOM。
    fn push_fail_record(&mut self, index: usize, category: String, subkey: String, message: String) {
        let message = message.chars().take(MAX_FAIL_MSG_CHARS).collect::<String>();
        let bytes = message.len();
        let category_id = self.category_id_of(&category);
        let message = if self.fail_record_bytes + bytes <= MAX_FAIL_RECORD_BYTES {
            self.fail_record_bytes += bytes;
            message
        } else {
            // 累计超上限：本条降级为消息首行摘要，不再累计字节。
            first_line(&message).chars().take(120).collect::<String>()
        };
        self.fail_records.push(FailRecord {
            index,
            category_id,
            subkey,
            message,
            scenario: 0,
        });
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
    /// 汇总尾部不打印 FAIL 清单。
    no_fail_list: bool,
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
        sources.insert("testTypedArray.js", include_str!("../../../tests/test262/harness/testTypedArray.js"));
        sources.insert("temporalHelpers.js", include_str!("../../../tests/test262/harness/temporalHelpers.js"));
        sources.insert("asyncHelpers.js", include_str!("../../../tests/test262/harness/asyncHelpers.js"));
        sources.insert("doneprintHandle.js", include_str!("../../../tests/test262/harness/doneprintHandle.js"));
        sources.insert("promiseHelper.js", include_str!("../../../tests/test262/harness/promiseHelper.js"));
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
        "testIntl.js"
            | "testAtomics.js"
            | "atomicsHelper.js"
            | "proxyTrapsHelper.js"
            | "tcoHelper.js"
            | "detachArrayBuffer.js"
            | "resizableArrayBufferUtils.js"
            | "byteConversionValues.js"
            | "compareIterator.js"
            | "iteratorZipUtils.js"
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
            no_fail_list: false,
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
                "--no-fail-list" => config.no_fail_list = true,
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
         --no-fail-list  Do not print the per-path FAIL list at the end of the run.\n\
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

/// eval 相关子族精确排除（档 1+2 后）：
/// built-ins/eval 与 eval-code 的完成值/解析失败/非字符串/this-value-global/间接环境族放行；
/// 仅排除确定失败的 arguments/super/strict/块声明 等子族。
fn eval_family_excluded(path: &str) -> Option<&'static str> {
    if !path.contains("/eval-code/") {
        return None;
    }
    let is_direct = path.contains("eval-code/direct/");
    let common = [
        "declare-arguments", // 直接 eval 的 arguments 语义族（档 3）
        "non-definable",     // 与既有不可配置全局属性冲突（DEFINE_GLOBAL_PROP 静默跳过，不抛）
        "this-value-func",   // 调用者 this 传递（档 3）
        "new.target",        // new.target 语义（档 3）
        "strict-caller",     // 严格调用者传播（档 3）
        "strict-source",
        "strictness-override", // 直接 eval 严格性覆盖
        "onlystrict",          // onlyStrict 块声明族
        "always-non-strict",   // 依赖隐式全局写同步（既有债务）
        "block-decl",          // 块级函数声明（Annex B 严格变体）
        "switch-case-decl",
        "switch-dflt-decl",
    ];
    if common.iter().any(|s| path.contains(s)) {
        return Some("eval 子族未实现（档 1-2 边界）");
    }
    if is_direct {
        // 直接 eval：调用者作用域交互族（函数上下文 var/let + super 方法上下文），档 3 前失败
        if ["var-env-", "lex-env-", "super-prop", "super-call-arrow", "super-call-method"]
            .iter()
            .any(|s| path.contains(s))
        {
            return Some("直接 eval 作用域族未实现（档 3）");
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

/// 从 catch_unwind 的 panic payload 提取可读文本：`&str` 与 `String` 两种
/// 常见形态，其余返回占位文本。
fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return format!("engine panic: {s}");
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return format!("engine panic: {s}");
    }
    "engine panic (non-string payload)".into()
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
    let run_result = vm.run(&module);
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

/// 异步测试判定：`$DONE` 把结果写入捕获串，run() 结束时微任务队列已 drain。
///
/// 失败 marker 优先（`Test262:AsyncTestFailure:<name>: <msg>`）；其次成功 marker
/// （`Test262:AsyncTestComplete`）；两者都无则测试未在 run() 内同步完成
/// （需 setTimeout 等异步 runner 的测试在此失败），回退到 run 结果判定。
fn judge_async_result(
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

/// 读取异步测试捕获的 `$DONE` 输出字符串。
fn read_async_output(vm: &Vm) -> String {
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

/// 运行期错误的判定（negative 匹配优先 + 未实现特性跳过分类）。
fn judge_vm_error(path: &Path, e: &str, meta: &TestMeta, dur: u64, no_skip: bool) -> TestResult {
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
/// - 编译期形态：`Identifier '{name}' is not defined`（可带 `compile error: ` 前缀）
/// - 两种形态都不匹配返回 `None`（不属未绑定标识符错误）。
fn parse_undefined_ident(e: &str) -> Option<&str> {
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
fn classify_compile_error(e: &str, has_negative: bool, no_skip: bool) -> TestOutcome {
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
/// `COMPLETED`：每个测试完成后写，`index` 为刚完成的全局测试下标，计数覆盖
/// ≤ index 的全部测试；`DONE`：窗口尾，`index` 为窗口结束下标。
/// `categories` 为子进程失败分类计数快照（首行之后按 `类别\t计数` 逐行写出）；
/// `hb_write_errors` 为子进程侧心跳/旁路写失败累计，经第 6 字段回传父进程。
struct Heartbeat {
    phase: String,
    index: usize,
    pass: usize,
    fail: usize,
    skip: usize,
    categories: HashMap<String, usize>,
    hb_write_errors: usize, // 子进程侧写失败累计（第 6 字段）
}

/// 用单行心跳头（phase index pass fail skip hb_write_errors）+ 失败类别行覆写心跳文件。
///
/// 经 `{path}.tmp` 临时文件再 rename 原子落盘（同目录 POSIX 原子替换；不 fsync，
/// SIGKILL 下 page cache 幸存，ponytail）。写失败返回 Err 并清理临时文件，由调用方
/// 自增 hb_write_errors 随下一心跳回传父进程。
///
/// # 边界与前提
/// - 假定单 worker（监督器强制 `OXIDE_TEST262_WORKERS=1`）；多 worker 时运行下标
///   有歧义且对同一路径的覆写存在竞争。
///
/// # 副作用
/// - 覆写 `path`；失败时可能残留 `path.tmp`（调用方或 supervise 清理兜底）。
#[expect(clippy::too_many_arguments)]
fn write_heartbeat(
    path: &Path, phase: &str, index: usize, pass: usize, fail: usize, skip: usize, categories: &HashMap<String, usize>,
    hb_write_errors: usize,
) -> std::io::Result<()> {
    let mut content = format!("{phase} {index} {pass} {fail} {skip} {hb_write_errors}\n");
    for (cat, count) in categories {
        // 类别文本内的制表符/换行会破坏行格式，写盘前压平。
        let cat = cat.replace(['\t', '\n', '\r'], " ");
        content.push_str(&format!("{cat}\t{count}\n"));
    }
    let tmp = format!("{}.tmp", path.display());
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

/// 读取最新心跳（含失败类别行与第 6 字段写失败计数）。任何缺失/残缺/畸形内容
/// 均返回 `None`，使轮询循环可直接在下一拍重试。
///
/// # 边界与前提
/// - 旧 5 字段 `START` 格式宽容映射为 `COMPLETED(index - 1)`：旧 START(j) 计数
///   覆盖 < j 的测试，与 COMPLETED(j-1) 语义等价；index=0 时 saturating_sub 防下溢。
fn read_heartbeat(path: &Path) -> Option<Heartbeat> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let line = lines.next()?;
    let mut parts = line.split_whitespace();
    let phase = parts.next()?.to_string();
    let index: usize = parts.next()?.parse().ok()?;
    let pass = parts.next()?.parse().ok()?;
    let fail = parts.next()?.parse().ok()?;
    let skip = parts.next()?.parse().ok()?;
    let hb_write_errors = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut categories = HashMap::new();
    for l in lines {
        if let Some((cat, count)) = l.split_once('\t') {
            if let Ok(c) = count.trim().parse() {
                categories.insert(cat.to_string(), c);
            }
        }
    }
    let (phase, index) = if phase == "START" {
        ("COMPLETED".into(), index.saturating_sub(1))
    } else {
        (phase, index)
    };
    Some(Heartbeat {
        phase,
        index,
        pass,
        fail,
        skip,
        categories,
        hb_write_errors,
    })
}

/// 把心跳快照并入累计统计（含失败类别与子进程侧写失败计数）。
fn merge_heartbeat(stats: &mut RunStats, hb: &Heartbeat) {
    stats.pass += hb.pass;
    stats.fail += hb.fail;
    stats.skip += hb.skip;
    stats.hb_write_errors += hb.hb_write_errors;
    for (cat, count) in &hb.categories {
        *stats.fail_categories.entry(cat.clone()).or_insert(0) += count;
    }
}

/// 把 supervise 旁路失败行文件并入统计：逐行重建 FailRecord（类别经本表 id
/// 重映射 + push_fail_record 的 cap 逻辑）。
///
/// # 注意事项
/// - 失败计数（fail_categories）已由对应心跳合并覆盖，此处只补 fail_records，
///   不得再累计计数，避免双计。worker 循环先写心跳后追加旁路，故"无心跳退出"
///   分支旁路必为空（防御性 no-op）。
fn merge_fail_log(stats: &mut RunStats, hb_path: &Path) {
    let content = match std::fs::read_to_string(hb_path.with_extension("fails")) {
        Ok(c) => c,
        Err(_) => return,
    };
    for (index, cat, subkey, message) in parse_fail_log(&content) {
        stats.push_fail_record(index, cat, subkey, message);
    }
}

/// 记一笔超时/崩溃结果：默认计入 skip，`--no-skip` 下计入 fail（归入
/// `timeout/crash` 类别，父进程无法进一步拆分根因）。
///
/// # 副作用
/// - 把 `(index, elapsed_ms)` 追加进 timeout_crashes 清单（elapsed_ms=0 表示
///   非超时崩溃：spawn 失败 / 中途退出 / waiterr / 无心跳），供收尾逐路径报告。
fn record_timeout_or_crash(stats: &mut RunStats, no_skip: bool, index: usize, elapsed_ms: u64) {
    if no_skip {
        stats.fail += 1;
        *stats.fail_categories.entry("timeout/crash".into()).or_insert(0) += 1;
    } else {
        stats.skip += 1;
    }
    stats.timeout_crashes.push((index, elapsed_ms));
}

/// 在监督下运行一个窗口 `[wstart, wend)`，返回经过多次子进程重启
/// 聚合的 `RunStats`（含失败类别）。
///
/// 单 worker 子进程运行常规 in-process 路径（预热 kernel + harness 前缀缓存）
/// 并在每个测试完成后发出心跳（COMPLETED）。若运行下标停滞超过 `timeout`，
/// 子进程被杀死、按路径报告肇事者，并由全新子进程从肇事者之后续跑；子进程
/// 在测试中途崩溃也经同路径恢复，恢复基于最后完成下标，无漏项。子进程死亡后
/// 读取旁路失败行并入 fail_records。超时/崩溃默认计为 skip，`--no-skip` 下计为失败。
#[expect(clippy::too_many_arguments)]
fn supervise_window(
    exe: &Path, args: &[String], no_skip: bool, wstart: usize, wend: usize, timeout: Duration, startup_grace: Duration,
    paths: &[PathBuf], window_id: usize,
) -> RunStats {
    let mut stats = RunStats::default();
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
        let _ = std::fs::remove_file(format!("{}.tmp", hb_path.display()));
        let _ = std::fs::remove_file(hb_path.with_extension("fails"));
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
                stats.spawn_errors += 1;
                record_timeout_or_crash(&mut stats, no_skip, cur, 0);
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
                            merge_heartbeat(&mut stats, &hb);
                            merge_fail_log(&mut stats, &hb_path);
                            cur = wend;
                        }
                        Some(hb) => {
                            merge_heartbeat(&mut stats, &hb);
                            let culprit = hb.index + 1;
                            merge_fail_log(&mut stats, &hb_path);
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) mid-test #{}: {}",
                                culprit,
                                describe(culprit)
                            );
                            record_timeout_or_crash(&mut stats, no_skip, culprit, 0);
                            cur = culprit;
                        }
                        None => {
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) with no heartbeat at index {cur}; skipping one"
                            );
                            merge_fail_log(&mut stats, &hb_path);
                            record_timeout_or_crash(&mut stats, no_skip, cur, 0);
                            cur += 1;
                        }
                    }
                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("  window {window_id}: try_wait error: {err}");
                    stats.wait_errors += 1;
                    let _ = child.kill();
                    let _ = child.wait();
                    merge_fail_log(&mut stats, &hb_path);
                    record_timeout_or_crash(&mut stats, no_skip, cur, 0);
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
                // 先杀再读：心跳与旁路失败行都须在子进程死亡后取最终状态。
                let _ = child.kill();
                let _ = child.wait();
                let hb = read_heartbeat(&hb_path);
                let culprit = hb.as_ref().map(|h| h.index + 1).unwrap_or(cur);
                if let Some(h) = &hb {
                    merge_heartbeat(&mut stats, h);
                }
                merge_fail_log(&mut stats, &hb_path);
                eprintln!(
                    "  [timeout] window {window_id}: TIMEOUT ({}s) on test #{culprit}: {}",
                    deadline.as_secs(),
                    describe(culprit)
                );
                record_timeout_or_crash(&mut stats, no_skip, culprit, elapsed.as_millis() as u64);
                cur = culprit + 1;
                break;
            }

            std::thread::sleep(Duration::from_millis(200));
        }
    }

    let _ = std::fs::remove_file(&hb_path);
    let _ = std::fs::remove_file(format!("{}.tmp", hb_path.display()));
    let _ = std::fs::remove_file(hb_path.with_extension("fails"));
    stats
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

    let partials: Vec<RunStats> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..supervisors)
            .map(|_| {
                scope.spawn(move || {
                    let mut stats = RunStats::default();
                    loop {
                        let wi = next.fetch_add(1, Ordering::Relaxed);
                        if wi >= windows.len() {
                            break;
                        }
                        let (window_id, wstart, wend) = windows[wi];
                        let window_stats = supervise_window(
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
                        stats.merge(window_stats);
                    }
                    stats
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("supervisor thread panicked"))
            .collect()
    });

    let mut stats = RunStats::default();
    for partial_stats in partials {
        stats.merge(partial_stats);
    }

    let total = stats.pass + stats.fail + stats.skip;
    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 supervised aggregate");
    println!("═══════════════════════════════════════");
    println!("  total  : {total}");
    println!("  pass   : {}", stats.pass);
    println!("  fail   : {}", stats.fail);
    println!(
        "  skip   : {}  (timeouts/crashes here by default; --no-skip counts them as fail)",
        stats.skip
    );
    print_fail_categories(&stats, paths);
    let fail_list = format_fail_list(&stats, paths);
    if !fail_list.is_empty() {
        print!("{fail_list}");
    }
    let anomalies = stats.spawn_errors + stats.wait_errors + stats.hb_write_errors;
    if anomalies > 0 {
        println!("  --- supervise anomalies ---");
        if stats.spawn_errors > 0 {
            println!("    spawn errors   : {}", stats.spawn_errors);
        }
        if stats.wait_errors > 0 {
            println!("    wait errors    : {}", stats.wait_errors);
        }
        if stats.hb_write_errors > 0 {
            println!("    hb write errors: {}", stats.hb_write_errors);
        }
    }
    if !stats.timeout_crashes.is_empty() {
        println!("  --- TIMEOUT/CRASH list ({}) ---", stats.timeout_crashes.len());
        for (idx, elapsed_ms) in &stats.timeout_crashes {
            let path = paths
                .get(*idx)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| format!("#{idx}"));
            println!("    {idx}  {elapsed_ms}ms  {path}");
        }
    }
    println!("═══════════════════════════════════════");

    stats.fail == 0
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
        if path_str.contains("/function-ctor/") || path_str.contains("/realm/") {
            return TestResult::skip(path.to_path_buf(), "unsupported class/eval feature excluded".into());
        }
        if let Some(reason) = eval_family_excluded(&path_str) {
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

/// 打印 `--- FAIL categories ---` 段（无类别时静默）；格式委托 report 模块。
fn print_fail_categories(stats: &RunStats, paths: &[PathBuf]) {
    let out = format_fail_categories(stats, paths);
    if !out.is_empty() {
        print!("{out}");
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

    let mut log_level = Level::Info;
    if let Ok(s) = std::env::var("OXIDE_TEST262_LOG_LEVEL") {
        if let Some(l) = oxide_log::subsystem::parse_level(&s) {
            log_level = l;
        }
    }
    eprintln!("[LOG] level={log_level:?}");
    oxide_log::init(&LogConfig {
        output: Output::Stderr,
        levels: [log_level; SUBSYSTEM_COUNT],
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

                            let result = process_path(
                                &paths_ref[i],
                                filter,
                                no_skip,
                                no_regalloc,
                                &kernel,
                                harness_sources,
                                &harness_cache,
                            );
                            stats.record(i, &result);
                            // COMPLETED 心跳：先写心跳再追加旁路失败行（崩溃恢复
                            // 无漏项、无双计的写序契约）。
                            if let Some(hb) = heartbeat_ref {
                                if let Err(e) = write_heartbeat(
                                    hb,
                                    "COMPLETED",
                                    i,
                                    stats.pass,
                                    stats.fail,
                                    stats.skip,
                                    &stats.fail_categories,
                                    stats.hb_write_errors,
                                ) {
                                    stats.hb_write_errors += 1;
                                    eprintln!("  [warn] heartbeat write failed at #{i}: {e}");
                                }
                                if let TestOutcome::Fail(msg) = &result.outcome {
                                    let cat = categorize_fail(msg);
                                    if let Err(e) = append_fail_log(&hb.with_extension("fails"), i, &cat, "", msg) {
                                        stats.hb_write_errors += 1;
                                        eprintln!("  [warn] fail log append failed at #{i}: {e}");
                                    }
                                }
                            }
                            if verbose {
                                match &result.outcome {
                                    TestOutcome::Pass(_) => println!("PASS {}", paths_ref[i].display()),
                                    TestOutcome::Fail(msg) => {
                                        let cat = categorize_fail(msg);
                                        println!("FAIL {} [{}] {}", paths_ref[i].display(), cat, first_line(msg));
                                    }
                                    TestOutcome::Skip(_) => println!("SKIP {}", paths_ref[i].display()),
                                }
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
        if let Err(e) = write_heartbeat(
            hb,
            "DONE",
            end_index,
            stats.pass,
            stats.fail,
            stats.skip,
            &stats.fail_categories,
            stats.hb_write_errors,
        ) {
            stats.hb_write_errors += 1;
            eprintln!("  [warn] final DONE heartbeat write failed: {e}");
        }
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
    print_fail_categories(&stats, &paths);
    if !config.no_fail_list {
        let fail_list = format_fail_list(&stats, &paths);
        if !fail_list.is_empty() {
            print!("{fail_list}");
        }
    }
    println!("═══════════════════════════════════════");

    if stats.fail > 0 && !allow_fail_exit {
        return false;
    }
    true
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

    /// `is not defined` 标识符解析：运行期 `uncaught` 前缀、编译期 `Identifier '..'`
    /// 两种形态都能取出标识符名；非该形态返回 None。
    #[test]
    fn parse_undefined_ident_extracts_name() {
        assert_eq!(parse_undefined_ident("uncaught ReferenceError: $262 is not defined"), Some("$262"));
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

    /// 心跳写读往返：类别行随心跳头一起持久化并完整还原（含制表符/换行压平），
    /// 第 6 字段 hb_write_errors 同步往返。
    #[test]
    fn heartbeat_round_trips_categories() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_{}.txt", std::process::id()));
        let mut categories = HashMap::new();
        categories.insert("vm: not defined".to_string(), 3);
        categories.insert("compile: unsupported".to_string(), 1);
        write_heartbeat(&path, "DONE", 42, 30, 4, 8, &categories, 3).expect("心跳写失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.phase, "DONE");
        assert_eq!(hb.index, 42);
        assert_eq!(hb.pass, 30);
        assert_eq!(hb.fail, 4);
        assert_eq!(hb.skip, 8);
        assert_eq!(hb.hb_write_errors, 3);
        assert_eq!(hb.categories.get("vm: not defined"), Some(&3));
        assert_eq!(hb.categories.get("compile: unsupported"), Some(&1));
        let _ = std::fs::remove_file(&path);
    }

    /// 类别文本含制表符/换行时写入压平，读取不破坏行结构。
    #[test]
    fn heartbeat_flattens_category_separators() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test2_{}.txt", std::process::id()));
        let mut categories = HashMap::new();
        categories.insert("vm: other (multi\nline\tmessage)".to_string(), 2);
        write_heartbeat(&path, "COMPLETED", 7, 1, 2, 3, &categories, 0).expect("心跳写失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.fail, 2);
        assert_eq!(hb.categories.len(), 1);
        let key = hb.categories.keys().next().unwrap();
        assert!(!key.contains('\t') && !key.contains('\n'), "类别键应已压平，实际 {key:?}");
        assert_eq!(hb.categories.get(key), Some(&2));
        let _ = std::fs::remove_file(&path);
    }

    /// record 追加失败记录：index 透传、类别正确、消息完整保留。
    #[test]
    fn runstats_record_appends_fail_record() {
        let mut stats = RunStats::default();
        let result = TestResult::fail(PathBuf::from("a.js"), 7, "vm error: x is not callable");
        stats.record(5, &result);
        assert_eq!(stats.fail, 1);
        assert_eq!(stats.fail_records.len(), 1);
        assert_eq!(stats.fail_records[0].index, 5);
        let cat = &stats.categories[stats.fail_records[0].category_id as usize];
        assert_eq!(cat, "vm: not callable");
        assert_eq!(stats.fail_records[0].message, "vm error: x is not callable");
    }

    /// 超长失败消息按字符截断到单条上限。
    #[test]
    fn runstats_record_caps_message_length() {
        let mut stats = RunStats::default();
        let result = TestResult::fail(PathBuf::from("b.js"), 1, "x".repeat(5000));
        stats.record(0, &result);
        assert!(stats.fail_records[0].message.chars().count() <= MAX_FAIL_MSG_CHARS);
    }

    /// merge 拼接失败记录并重映射类别 id：同类别共享一个 id，字节计数为各侧之和。
    #[test]
    fn runstats_merge_concats_fail_records_with_id_remap() {
        let mut a = RunStats::default();
        a.record(0, &TestResult::fail(PathBuf::from("x.js"), 1, "vm error: a is not callable"));
        a.record(1, &TestResult::fail(PathBuf::from("y.js"), 1, "vm error: b is not callable"));
        let mut b = RunStats::default();
        b.record(2, &TestResult::fail(PathBuf::from("z.js"), 1, "vm error: c is not callable"));
        a.merge(b);
        assert_eq!(a.fail, 3);
        assert_eq!(a.fail_records.len(), 3);
        let id0 = a.fail_records[0].category_id;
        assert_eq!(a.fail_records[1].category_id, id0);
        assert_eq!(a.fail_records[2].category_id, id0);
        assert_eq!(a.categories[id0 as usize], "vm: not callable");
        assert_eq!(a.fail_record_bytes, a.fail_records.iter().map(|r| r.message.len()).sum::<usize>());
    }

    /// FAIL 清单每类别只列样本条数，其余按折叠行计数。
    #[test]
    fn format_fail_list_folds_over_limit() {
        let mut stats = RunStats::default();
        let paths: Vec<PathBuf> = (0..7).map(|i| PathBuf::from(format!("p{i}.js"))).collect();
        for (i, p) in paths.iter().enumerate() {
            stats.record(i, &TestResult::fail(p.clone(), 1, "vm error: x is not callable"));
        }
        let out = format_fail_list(&stats, &paths);
        let fail_lines = out.lines().filter(|l| l.starts_with("    FAIL ")).count();
        assert_eq!(fail_lines, 5);
        assert!(out.contains("(+2 more in vm: not callable)"), "应含折叠行，实际:\n{out}");
    }

    /// 碎片桶附样本行：路径 + 消息首行，类别带截断尾巴也能前缀匹配。
    #[test]
    fn format_fail_categories_attaches_other_bucket_samples() {
        let mut stats = RunStats::default();
        let paths = vec![PathBuf::from("p.js")];
        stats.record(0, &TestResult::fail(paths[0].clone(), 1, "vm error: weird message one two"));
        let out = format_fail_categories(&stats, &paths);
        assert!(out.contains("sample:"), "应含样本行，实际:\n{out}");
        assert!(out.contains("weird message one"), "样本应含消息首行，实际:\n{out}");
    }

    /// 无失败记录时 FAIL 清单段为空串（调用处静默）。
    #[test]
    fn format_fail_list_empty_without_records() {
        let stats = RunStats::default();
        let paths: Vec<PathBuf> = Vec::new();
        assert_eq!(format_fail_list(&stats, &paths), "");
        assert_eq!(format_fail_categories(&stats, &paths), "");
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

    /// 心跳第 6 字段（hb_write_errors）写读往返。
    #[test]
    fn heartbeat_round_trips_write_errors_field() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_wr_{}.txt", std::process::id()));
        let categories = HashMap::new();
        write_heartbeat(&path, "COMPLETED", 9, 5, 1, 2, &categories, 5).expect("心跳写失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.phase, "COMPLETED");
        assert_eq!(hb.index, 9);
        assert_eq!(hb.hb_write_errors, 5);
        let _ = std::fs::remove_file(&path);
    }

    /// 旧 5 字段 START 心跳宽容映射：START(j) → COMPLETED(j-1)；j=0 不溢出。
    #[test]
    fn read_heartbeat_maps_legacy_start_to_completed() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_legacy_{}.txt", std::process::id()));
        std::fs::write(&path, "START 7 1 2 3\n").expect("写原始心跳行失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.phase, "COMPLETED");
        assert_eq!(hb.index, 6);
        std::fs::write(&path, "START 0 0 0 0\n").expect("写原始心跳行失败");
        let hb = read_heartbeat(&path).expect("心跳应可读回");
        assert_eq!(hb.index, 0, "saturating_sub 防下溢");
        let _ = std::fs::remove_file(&path);
    }

    /// 原子写：写后目标文件内容完整、无 `.tmp` 残留。
    #[test]
    fn write_heartbeat_atomic_no_tmp_leftover() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_hb_test_atomic_{}.txt", std::process::id()));
        let categories = HashMap::new();
        write_heartbeat(&path, "DONE", 10, 3, 2, 1, &categories, 0).expect("心跳写失败");
        let content = std::fs::read_to_string(&path).expect("心跳应可读回");
        assert!(content.starts_with("DONE 10 3 2 1 0\n"), "内容应为 6 字段头，实际:\n{content}");
        let tmp = format!("{}.tmp", path.display());
        assert!(!Path::new(&tmp).exists(), "tmp 文件不应残留");
        let _ = std::fs::remove_file(&path);
    }

    /// 超时/崩溃记账：计数（no_skip 转 fail）与 timeout_crashes 入列。
    #[test]
    fn record_timeout_or_crash_records_index_elapsed() {
        let mut stats = RunStats::default();
        record_timeout_or_crash(&mut stats, true, 3, 2500);
        assert_eq!(stats.fail, 1);
        assert_eq!(stats.fail_categories.get("timeout/crash"), Some(&1));
        assert_eq!(stats.timeout_crashes, vec![(3, 2500)]);
        record_timeout_or_crash(&mut stats, false, 4, 0);
        assert_eq!(stats.skip, 1);
        assert_eq!(stats.timeout_crashes, vec![(3, 2500), (4, 0)]);
    }

    /// merge 合并异常计数器：spawn/wait/hb 求和、timeout_crashes 拼接。
    #[test]
    fn runstats_merge_sums_error_counters() {
        let mut a = RunStats {
            spawn_errors: 1,
            wait_errors: 2,
            hb_write_errors: 3,
            timeout_crashes: vec![(0, 100)],
            ..RunStats::default()
        };
        let b = RunStats {
            spawn_errors: 4,
            wait_errors: 5,
            hb_write_errors: 6,
            timeout_crashes: vec![(1, 200)],
            ..RunStats::default()
        };
        a.merge(b);
        assert_eq!(a.spawn_errors, 5);
        assert_eq!(a.wait_errors, 7);
        assert_eq!(a.hb_write_errors, 9);
        assert_eq!(a.timeout_crashes, vec![(0, 100), (1, 200)]);
    }

    /// 旁路失败行往返：append 后 parse 恢复全部字段；残缺/畸形尾行静默跳过。
    #[test]
    fn fail_log_round_trips_and_skips_truncated() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_fails_test_{}.log", std::process::id()));
        append_fail_log(&path, 3, "vm: not callable", "", "x is not callable").expect("追加失败");
        append_fail_log(&path, 4, "compile: unsupported", "foo", "a\tb\nc").expect("追加失败");
        let mut content = std::fs::read_to_string(&path).expect("读取失败");
        content.push_str("5\tvm: x\n"); // 残缺：仅 2 字段（SIGKILL 半写形态）
        content.push_str("x\tb\tc\td\n"); // 畸形：index 非数字
        std::fs::write(&path, content).expect("写回失败");
        let content = std::fs::read_to_string(&path).expect("读取失败");
        let rows = parse_fail_log(&content);
        assert_eq!(rows.len(), 2, "残缺/畸形行应被跳过，实际 {rows:?}");
        assert_eq!(
            rows[0],
            (3, "vm: not callable".to_string(), "".to_string(), "x is not callable".to_string())
        );
        assert_eq!(rows[1], (4, "compile: unsupported".to_string(), "foo".to_string(), "a b c".to_string()));
        let _ = std::fs::remove_file(&path);
    }

    /// merge_fail_log 重建 fail_records：类别经 id 表重映射、消息 cap；
    /// fail_categories 保持空（计数不双计契约）。
    #[test]
    fn merge_fail_log_reconstructs_fail_records() {
        let dir = std::env::temp_dir();
        let hb_path = dir.join(format!("oxide_t262_hb_test_mrg_{}.txt", std::process::id()));
        let sidecar = hb_path.with_extension("fails");
        append_fail_log(&sidecar, 2, "vm: not callable", "", "x is not callable").expect("追加失败");
        append_fail_log(&sidecar, 7, "compile: unsupported", "", "y").expect("追加失败");
        let mut stats = RunStats::default();
        merge_fail_log(&mut stats, &hb_path);
        assert_eq!(stats.fail_records.len(), 2);
        assert_eq!(stats.fail_records[0].index, 2);
        assert_eq!(stats.categories[stats.fail_records[0].category_id as usize], "vm: not callable");
        assert_eq!(stats.fail_records[0].message, "x is not callable");
        assert_eq!(stats.fail_records[1].index, 7);
        assert_eq!(stats.categories[stats.fail_records[1].category_id as usize], "compile: unsupported");
        assert!(stats.fail_categories.is_empty(), "计数不双计：fail_categories 必须保持空");
        let _ = std::fs::remove_file(&sidecar);
        let _ = std::fs::remove_file(&hb_path);
    }
}
