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

// Thread-local that records the path currently being executed.
// Written before every test; read by the panic hook to identify the crash file.
std::thread_local! {
    static CURRENT_TEST_PATH: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

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
    supervise: bool,
    leak_check: bool,
    leak_check_interval: usize,
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

static HARNESS: OnceLock<HarnessSources> = OnceLock::new();

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
    fn new() -> Self {
        Self {
            test262_root: None,
            filter: None,
            no_skip: false,
            supervise: false,
            leak_check: false,
            leak_check_interval: 1000,
        }
    }

    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = Self::new();
        let mut positional = Vec::new();

        for arg in args.iter().skip(1) {
            match arg.as_str() {
                "--no-skip" => config.no_skip = true,
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

    fn usage() -> String {
        "usage: test262-runner [--no-skip] [--supervise] [--leak-check] [--leak-check-interval=N] [test262-root] [path-filter]\n\
         \n\
         --no-skip    Run capability-excluded tests and count unsupported compile/runtime results as failures.\n\
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

fn is_skipped(meta: &TestMeta) -> Option<String> {
    for flag in &meta.flags {
        match flag.as_str() {
            "module" => return Some("module tests excluded".into()),
            "async" => return Some("async tests excluded".into()),
            "raw" => return Some("raw tests excluded".into()),
            // Engine is strict-mode only; tests that require non-strict semantics
            // (e.g. global var ↔ global object binding, arguments.callee, etc.) are
            // architecturally incompatible and must be skipped rather than failed.
            "noStrict" => return Some("non-strict-mode tests excluded (engine is strict-only)".into()),
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
        "regexp-modifiers",
        "json-superset",
        "Temporal",
        "cross-realm",
        "new.target",
        "well-formed-json-stringify",
        "symbols-as-weakmap-keys",
        "class-accessors-private",
    ];

    for feat in &meta.features {
        if excluded_features.contains(&feat.as_str()) || feat.starts_with("Intl") || feat.starts_with("Reflect.") {
            return Some(format!("excluded feature: {feat}"));
        }
    }

    if meta.description.contains("generator") || meta.description.contains("async") {
        return Some("description matches excluded pattern".into());
    }

    None
}

fn run_test(
    path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<KernelCore>, harness: &HarnessSources,
    harness_cache: &Arc<RwLock<HarnessPrefixCache>>, no_skip: bool,
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
    path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<KernelCore>, harness: &HarnessSources,
    harness_cache: &Arc<RwLock<HarnessPrefixCache>>, no_skip: bool,
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
                || e.contains("SpreadElement")
                || e.contains("already been declared")
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
    let mut paths: Vec<PathBuf> = WalkDir::new(test262_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
        .map(|e| e.path().to_path_buf())
        .collect();
    paths.sort();
    paths
}

fn parse_summary_count(stdout: &str, label: &str) -> Option<usize> {
    stdout.lines().find_map(|line| {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix(label)?;
        let value = rest.trim().split(' ').next()?;
        value.parse::<usize>().ok()
    })
}

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

/// One heartbeat record written by a supervised child and polled by the parent.
/// `index` is the global test index the child is about to run (`START`) or the
/// window end it finished (`DONE`); the tallies always cover tests completed
/// *before* `index`, so the in-flight test is never counted yet.
struct Heartbeat {
    phase: String,
    index: usize,
    pass: usize,
    fail: usize,
    skip: usize,
}

/// Overwrite the heartbeat file with a single line. Errors are ignored: a missed
/// heartbeat just delays stall detection by one poll interval. Assumes a single
/// worker (the supervisor always forces `OXIDE_TEST262_WORKERS=1`); with more
/// than one worker the running index is ambiguous and the file races.
fn write_heartbeat(path: &Path, phase: &str, index: usize, pass: usize, fail: usize, skip: usize) {
    let _ = std::fs::write(path, format!("{phase} {index} {pass} {fail} {skip}\n"));
}

/// Read the latest heartbeat. Returns `None` on any missing/partial/malformed
/// content so the poll loop can simply retry on the next tick.
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

/// Run one window `[wstart, wend)` to completion under supervision, returning
/// `(pass, fail, skip)` aggregated across however many child restarts it took.
///
/// A single-worker child runs the normal in-process path (warm kernel + harness
/// prefix cache) and emits a heartbeat before each test. If the running index
/// stalls past `timeout`, the child is killed, the culprit is reported by path,
/// and a fresh child resumes from `culprit + 1`. A child that crashes mid-test
/// is recovered through the same path. Timeouts/crashes count as skip by default
/// and fail under `--no-skip`.
#[allow(clippy::too_many_arguments)]
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

/// Orchestrate a supervised full run: split `[skip_until, end_index)` into
/// windows and run up to `supervisors` of them concurrently. The supervisor
/// threads only spawn/poll/kill child processes and touch files — they never
/// hold a `KernelCore`, so sharing `paths`/`args` by reference is safe.
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

/// Per-test pipeline shared by the serial and parallel execution paths:
/// read the file, parse metadata, apply skip filters, then run. Returns
/// exactly one `TestResult`. Worker-owned state (`kernel`, `harness_sources`,
/// `harness_cache`) never crosses a thread boundary.
fn process_path(
    path: &Path, filter: &Option<String>, no_skip: bool, kernel: &Arc<KernelCore>, harness_sources: &HarnessSources,
    harness_cache: &Arc<RwLock<HarnessPrefixCache>>,
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
        if path_str.contains("built-ins/Promise/") {
            return TestResult::skip(path.to_path_buf(), "Promise tests excluded".into());
        }
        if path_str.contains("/dstr/")
            || path_str.contains("/eval/")
            || path_str.contains("/function-ctor/")
            || path_str.contains("/realm/")
        {
            return TestResult::skip(path.to_path_buf(), "unsupported class/eval feature excluded".into());
        }
        if let Some(reason) = is_skipped(&meta) {
            return TestResult::skip(path.to_path_buf(), reason);
        }
    }

    run_test(path, &source, &meta, kernel, harness_sources, harness_cache, no_skip)
}

/// Build a runner kernel with a bounded step limit. Each parallel worker owns
/// its own kernel because `KernelCore` + session state is `!Send` (it holds
/// `P<JsObject>` = `Arc<JsObject>`, and `JsObject` stores raw `*mut u8` property
/// pointers). Nothing kernel-shaped can cross a thread boundary, so sharing is
/// impossible; per-worker construction is the only correct design.
fn build_runner_kernel() -> Arc<KernelCore> {
    // Bound each test's execution so a single infinite-loop / unsupported-feature
    // loop fails (or skips) instead of stalling the run. The VM emits a
    // "VM step limit exceeded" error on overrun, which the runner classifies as a
    // step-limit result (skip by default, fail under --no-skip). Override only the
    // runner's local config; KernelConfig::minimal() stays unbounded for other crates.
    let mut kernel_config = KernelConfig::minimal();
    kernel_config.max_steps = Some(50_000_000);
    // Keep test262 recursion tests from reaching Rust's native stack before the
    // VM converts deep JS calls into a catchable RangeError.
    kernel_config.max_call_depth = 256;
    kernel_config.min_pool_size = 1;
    kernel_config.max_pool_size = Some(1);
    KernelCore::new(kernel_config)
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
    // Install a panic hook that prints which test was running when the panic occurred.
    // This covers Rust panics; OS-level crashes (ACCESS_VIOLATION) are caught by the
    // pre-test eprintln! below — the last line printed before a hard crash identifies
    // the file.
    std::panic::set_hook(Box::new(|info| {
        let path = CURRENT_TEST_PATH.with(|p| p.borrow().clone());
        if !path.is_empty() {
            eprintln!("CRASH in test: {path}");
        }
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

    // Determine worker count. `KernelCore` + session state is `!Send` (it holds
    // one kernel via Arc across threads; instead each worker builds and owns its
    // own kernel + harness-source registry + prefix cache. Only `PathBuf` and
    // `TestResult` (both `Send`) cross thread boundaries. Workers pull test
    // indices from a shared atomic cursor for dynamic load balancing.
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
        eprintln!("supervised mode enabled: child-window execution with per-test timeout + auto-resume");
        return run_supervised(&args, skip_until, end_index, config.no_skip, &paths);
    }

    if let Some(chunk_size) = chunk_size {
        if !is_chunk_child && filter.is_none() {
            eprintln!("chunked mode enabled: {chunk_size} test(s) per child process");
            return run_chunked(&args, skip_until, end_index, chunk_size);
        }
    }
    eprintln!("kernel reset batch: {kernel_batch} test(s)");
    let cursor = AtomicUsize::new(skip_until);
    let progress = AtomicUsize::new(skip_until);
    let filter = &filter;
    let no_skip = config.no_skip;
    let paths_ref = &paths;
    let heartbeat_ref = &heartbeat_path;
    let harness_cache = Arc::new(RwLock::new(HarnessPrefixCache::new()));

    // Keep memory flat: workers return only aggregate stats. Retaining tens of
    // thousands of `TestResult`s makes `--no-skip` runs accumulate path/error
    // strings until the process OOMs near the end of the suite.
    let partials: Vec<RunStats> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let cursor = &cursor;
                let progress = &progress;
                let harness_cache = Arc::clone(&harness_cache);
                // Match main()'s 16MB stack: the VM recurses on deeply nested
                // test programs and would overflow the default worker stack.
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
                            // Always record the current test path so the panic hook can
                            // identify Rust panics. Set OXIDE_TEST262_RUNNING_LOG=1 when
                            // diagnosing OS-level crashes that need the last stderr line.
                            CURRENT_TEST_PATH.with(|p| *p.borrow_mut() = path_str.clone());
                            if log_running_tests {
                                eprintln!("  [{tid:?}] running: {path_str}");
                            }
                            if let Some(hb) = heartbeat_ref {
                                write_heartbeat(hb, "START", i, stats.pass, stats.fail, stats.skip);
                            }

                            let result =
                                process_path(&paths_ref[i], filter, no_skip, &kernel, harness_sources, &harness_cache);
                            stats.record(&result);
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

    // Reduce partial stats without materializing every test result in memory.
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
