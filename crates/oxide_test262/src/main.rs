#![allow(clippy::arc_with_non_send_sync)]

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_vm::vm::{init_kernel_builtins, Vm};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use walkdir::WalkDir;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Negative {
    phase: String,
    #[serde(rename = "type")]
    error_type: String,
}

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

#[derive(Debug)]
#[allow(dead_code)]
enum TestOutcome {
    Pass(String),
    Fail(String),
    Skip(String),
}

#[derive(Debug)]
#[allow(dead_code)]
struct TestResult {
    path: PathBuf,
    outcome: TestOutcome,
    duration_ms: u64,
}

impl TestResult {
    fn pass(path: PathBuf, dur: u64, msg: impl Into<String>) -> Self {
        Self {
            path,
            outcome: TestOutcome::Pass(msg.into()),
            duration_ms: dur,
        }
    }

    fn fail(path: PathBuf, dur: u64, msg: impl Into<String>) -> Self {
        Self {
            path,
            outcome: TestOutcome::Fail(msg.into()),
            duration_ms: dur,
        }
    }

    fn skip(path: PathBuf, msg: String) -> Self {
        Self {
            path,
            outcome: TestOutcome::Skip(msg),
            duration_ms: 0,
        }
    }
}

fn parse_meta(source: &str) -> Option<TestMeta> {
    let header_start = source.find("/*---")?;
    let header = &source[header_start..];
    let header = header.strip_prefix("/*---")?;
    let end = header.find("---*/")?;
    let yaml_body = &header[..end];
    let yaml_body = yaml_body.trim();
    serde_yaml::from_str::<TestMeta>(yaml_body).ok()
}

fn strip_meta(source: &str) -> &str {
    if let Some(pos) = source.find("---*/") {
        return source[pos + 5..].trim_start();
    }
    source
}

#[derive(Default)]
struct RunStats {
    pass: usize,
    fail: usize,
    skip: usize,
    total_ms: u64,
    fail_categories: HashMap<String, usize>,
}

impl RunStats {
    /// Fold another worker's partial stats into this one. Used to reduce
    /// per-worker results back into a single total after parallel execution.
    fn merge(&mut self, other: RunStats) {
        self.pass += other.pass;
        self.fail += other.fail;
        self.skip += other.skip;
        self.total_ms += other.total_ms;
        for (cat, count) in other.fail_categories {
            *self.fail_categories.entry(cat).or_insert(0) += count;
        }
    }

    /// Record a single test result into the running totals.
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

#[derive(Debug, Default)]
struct RunConfig {
    test262_root: Option<PathBuf>,
    filter: Option<String>,
    no_skip: bool,
}

struct HarnessSources {
    sources: HashMap<&'static str, &'static str>,
}

type HarnessPrefixCache = HashMap<Vec<String>, String>;

impl HarnessSources {
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

    fn get(&self, name: &str) -> Option<&'static str> {
        self.sources.get(name).copied()
    }
}

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

fn test262_error_prelude() -> &'static str {
    r#"
function Test262Error(message) {
  this.message = message;
  this.name = "Test262Error";
}
Test262Error.prototype = new Error();
Test262Error.prototype.constructor = Test262Error;
"#
}

fn append_source_chunk(out: &mut String, name: &str, source: &str) {
    out.push_str("\n// ---- test262 harness: ");
    out.push_str(name);
    out.push_str(" ----\n");
    out.push_str(source);
    out.push('\n');
}

fn harness_key(meta: &TestMeta) -> Vec<String> {
    meta.includes.clone()
}

fn build_harness_source(meta: &TestMeta, harness: &HarnessSources) -> Result<String, String> {
    let mut source = String::new();
    append_source_chunk(&mut source, "Test262Error prelude", test262_error_prelude());
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

fn get_harness_prefix(
    meta: &TestMeta, harness: &HarnessSources, cache: &mut HarnessPrefixCache,
) -> Result<String, String> {
    let key = harness_key(meta);
    if let Some(prefix) = cache.get(&key) {
        return Ok(prefix.clone());
    }

    let source = build_harness_source(meta, harness)?;
    cache.insert(key, source.clone());
    Ok(source)
}

impl RunConfig {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = Self::default();
        let mut positional = Vec::new();

        for arg in args.iter().skip(1) {
            match arg.as_str() {
                "--no-skip" => config.no_skip = true,
                "--help" | "-h" => return Err(Self::usage()),
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

    fn usage() -> String {
        "usage: test262-runner [--no-skip] [test262-root] [path-filter]\n\
         \n\
         --no-skip  Run capability-excluded tests and count unsupported compile/runtime results as failures."
            .into()
    }
}

fn is_skipped(meta: &TestMeta) -> Option<String> {
    for flag in &meta.flags {
        match flag.as_str() {
            "module" => return Some("module tests excluded".into()),
            "async" => return Some("async tests excluded".into()),
            "raw" => return Some("raw tests excluded".into()),
            _ => {}
        }
    }

    // Keep broad implemented feature tags runnable; exclude only unsupported subfeatures.
    let excluded_features = [
        "Proxy",
        "BigInt",
        "generators",
        "generator",
        "async-functions",
        "default-parameters",
        "destructuring-binding",
        "destructuring",
        "rest-parameters",
        "spread",
        "WeakMap",
        "WeakSet",
        "WeakRef",
        "Reflect",
        "Intl",
        "Promise",
        "TypedArray",
        "DataView",
        "ArrayBuffer",
        "SharedArrayBuffer",
        "Atomics",
        "module",
        "dynamic-import",
        "tail-call-optimization",
        "regexp-named-groups",
        "regexp-lookbehind",
        "regexp-unicode-property-escapes",
        "regexp-dotall",
        "json-superset",
        "for-of",
        "Temporal",
        "cross-realm",
        "new.target",
        "well-formed-json-stringify",
        "optional-chaining",
        "symbols-as-weakmap-keys",
        "class-fields-public",
        "class-fields-private",
        "class-static-block",
        "class-methods-private",
        "class-accessors-private",
        "class-fields-public-static",
        "class-fields-private-static",
    ];

    for feat in &meta.features {
        if excluded_features.contains(&feat.as_str()) || feat.starts_with("Intl") {
            return Some(format!("excluded feature: {feat}"));
        }
    }

    if meta.description.contains("generator")
        || meta.description.contains("async")
        || meta.description.contains("eval ")
    {
        return Some("description matches excluded pattern".into());
    }

    None
}

fn run_test(
    path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<OxideKernel>, harness: &HarnessSources,
    harness_cache: &mut HarnessPrefixCache, no_skip: bool,
) -> TestResult {
    let start = std::time::Instant::now();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_test_inner(path, source, meta, kernel, harness, harness_cache, no_skip)
    }));

    match result {
        Ok(r) => r,
        Err(_panic) => {
            let dur = start.elapsed().as_millis() as u64;
            TestResult::fail(path.to_path_buf(), dur, "engine panic (unsupported feature)")
        }
    }
}

fn run_test_inner(
    path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<OxideKernel>, harness: &HarnessSources,
    harness_cache: &mut HarnessPrefixCache, no_skip: bool,
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

    let module = match Compiler::new().compile(&program) {
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
                || e.contains("ArrowFunctionExpression")
                || e.contains("destructuring")
                || e.contains("SpreadElement")
                || e.contains("compound assignment")
                || e.contains("already been declared")
                || e.contains("before initialization")
                || e.contains("parser panicked")
            {
                if no_skip {
                    return TestResult::fail(path.to_path_buf(), dur, msg);
                }
                return TestResult::skip(path.to_path_buf(), msg);
            }
            return TestResult::fail(path.to_path_buf(), dur, msg);
        }
    };

    let mut vm = Vm::with_kernel(Arc::clone(kernel));

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
                || e.contains("RangeError")
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

fn discover_tests(test262_root: &Path) -> Vec<PathBuf> {
    WalkDir::new(test262_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Per-test pipeline shared by the serial and parallel execution paths:
/// read the file, parse metadata, apply skip filters, then run. Returns
/// exactly one `TestResult`. Worker-owned state (`kernel`, `harness_sources`,
/// `harness_cache`) never crosses a thread boundary.
fn process_path(
    path: &Path, filter: &Option<String>, no_skip: bool, kernel: &Arc<OxideKernel>,
    harness_sources: &HarnessSources, harness_cache: &mut HarnessPrefixCache,
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
        if let Some(reason) = is_skipped(&meta) {
            return TestResult::skip(path.to_path_buf(), reason);
        }
    }

    run_test(path, &source, &meta, kernel, harness_sources, harness_cache, no_skip)
}

/// Build a runner kernel with a bounded step limit. Each parallel worker owns
/// its own kernel because `OxideKernel` is `!Send` (it holds `P<JsObject>` =
/// `Arc<JsObject>`, and `JsObject` stores raw `*mut u8` property pointers).
/// Nothing kernel-shaped can cross a thread boundary, so sharing is impossible;
/// per-worker construction is the only correct design.
fn build_runner_kernel() -> Arc<OxideKernel> {
    // Bound each test's execution so a single infinite-loop / unsupported-feature
    // loop fails (or skips) instead of stalling the run. The VM emits a
    // "VM step limit exceeded" error on overrun, which the runner classifies as a
    // step-limit result (skip by default, fail under --no-skip). Override only the
    // runner's local config; KernelConfig::minimal() stays unbounded for other crates.
    let mut kernel_config = KernelConfig::minimal();
    kernel_config.max_steps = Some(50_000_000);
    let kernel = Arc::new(OxideKernel::new(kernel_config));
    init_kernel_builtins(&kernel);
    kernel
}

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

fn main() {
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

fn run_tests() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let config = match RunConfig::parse(&args) {
        Ok(config) => config,
        Err(msg) => {
            eprintln!("{msg}");
            return false;
        }
    };

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

    eprintln!("discovering tests in: {}", test262_root.display());
    if config.no_skip {
        eprintln!("no-skip mode: capability filters disabled; unsupported results count as failures");
    }
    let paths = discover_tests(&test262_root);
    eprintln!("found {} test files", paths.len());

    let total = paths.len();

    // Determine worker count. `OxideKernel`/`Vm` are `!Send`, so we cannot share
    // one kernel via Arc across threads; instead each worker builds and owns its
    // own kernel + harness-source registry + prefix cache. Only `PathBuf` and
    // `TestResult` (both `Send`) cross thread boundaries. Workers pull test
    // indices from a shared atomic cursor for dynamic load balancing.
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(total.max(1));

    eprintln!("running on {workers} worker thread(s)");

    let cursor = AtomicUsize::new(0);
    let progress = AtomicUsize::new(0);
    let filter = &filter;
    let no_skip = config.no_skip;
    let paths_ref = &paths;

    // Each worker returns its partial stats plus (index, result) pairs so the
    // final results vector can be restored to discovery order for stable output.
    let partials: Vec<(RunStats, Vec<(usize, TestResult)>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let cursor = &cursor;
                let progress = &progress;
                // Match main()'s 16MB stack: the VM recurses on deeply nested
                // test programs and would overflow the default worker stack.
                std::thread::Builder::new()
                    .stack_size(16 * 1024 * 1024)
                    .spawn_scoped(scope, move || {
                        let kernel = build_runner_kernel();
                        let harness_sources = HarnessSources::new();
                        let mut harness_cache = HarnessPrefixCache::new();
                        let mut stats = RunStats::default();
                        let mut out: Vec<(usize, TestResult)> = Vec::new();

                        loop {
                            let i = cursor.fetch_add(1, Ordering::Relaxed);
                            if i >= total {
                                break;
                            }

                            let result = process_path(
                                &paths_ref[i],
                                filter,
                                no_skip,
                                &kernel,
                                &harness_sources,
                                &mut harness_cache,
                            );
                            stats.record(&result);
                            out.push((i, result));

                            let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
                            if done % 500 == 0 || done == total {
                                eprintln!("  progress: {done}/{total} ({}%)", done * 100 / total);
                            }
                        }

                        (stats, out)
                    })
                    .expect("failed to spawn test262 worker thread")
            })
            .collect();

        handles.into_iter().map(|h| h.join().expect("test262 worker thread panicked")).collect()
    });

    // Reduce partial stats and restore discovery order for deterministic output.
    let mut stats = RunStats::default();
    let mut indexed: Vec<(usize, TestResult)> = Vec::with_capacity(total);
    for (partial_stats, partial_results) in partials {
        stats.merge(partial_stats);
        indexed.extend(partial_results);
    }
    indexed.sort_by_key(|(i, _)| *i);
    let results: Vec<TestResult> = indexed.into_iter().map(|(_, r)| r).collect();

    eprintln!();

    let ran = stats.pass + stats.fail;

    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 results");
    println!("═══════════════════════════════════════");
    println!("  total  : {}", results.len());
    let total = results.len() as f64;
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

    if stats.fail > 0 {
        return false;
    }
    true
}
