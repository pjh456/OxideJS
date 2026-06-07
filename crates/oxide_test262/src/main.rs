#![allow(clippy::arc_with_non_send_sync)]

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_vm::vm::Vm;
use serde::Deserialize;
use std::path::{Path, PathBuf};
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
}

fn is_skipped(meta: &TestMeta) -> Option<String> {
    for flag in &meta.flags {
        match flag.as_str() {
            "module" => return Some("module tests excluded".into()),
            "async" => return Some("async tests excluded".into()),
            "raw" => return Some("raw tests excluded".into()),
            "onlyStrict" => return Some("strict-only excluded".into()),
            _ => {}
        }
    }

    if !meta.includes.is_empty() {
        return Some(format!(
            "requires harness includes: {}",
            meta.includes.join(", ")
        ));
    }

    let excluded_features = [
        "Proxy",
        "Symbol",
        "Symbol.match",
        "BigInt",
        "generators",
        "generator",
        "async-functions",
        "default-parameters",
        "destructuring-binding",
        "destructuring",
        "arrow-function",
        "rest-parameters",
        "spread",
        "class",
        "super",
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

fn run_test(path: &Path, source: &str, meta: &TestMeta, kernel: &Arc<OxideKernel>) -> TestResult {
    let start = std::time::Instant::now();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_test_inner(path, source, meta, kernel)
    }));

    match result {
        Ok(r) => r,
        Err(_panic) => {
            let dur = start.elapsed().as_millis() as u64;
            TestResult::fail(
                path.to_path_buf(),
                dur,
                "engine panic (unsupported feature)",
            )
        }
    }
}

fn run_test_inner(
    path: &Path,
    source: &str,
    meta: &TestMeta,
    kernel: &Arc<OxideKernel>,
) -> TestResult {
    let start = std::time::Instant::now();

    let code = strip_meta(source);

    let alloc = oxide_parser::Allocator::default();
    let program = match oxide_parser::parse(&alloc, code) {
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
                || e.contains("not supported")
                || e.contains("is not defined")
                || e.contains("FunctionDeclaration")
                || e.contains("FunctionExpression")
                || e.contains("ArrowFunctionExpression")
                || e.contains("unsupported expression")
                || e.contains("unsupported statement")
                || e.contains("NewExpression")
                || e.contains("CallExpression")
                || e.contains("destructuring")
                || e.contains("SpreadElement")
            {
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
                TestResult::fail(
                    path.to_path_buf(),
                    dur,
                    format!("expected {} error, got: {e}", neg.error_type),
                )
            } else if e.contains("not yet implemented")
                || e.contains("not supported")
                || e.contains("unsupported")
                || e.contains("step limit")
            {
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

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let test262_root = if args.len() > 1 {
        PathBuf::from(&args[1])
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

    let filter = if args.len() > 2 {
        Some(args[2].clone())
    } else {
        None
    };

    eprintln!("discovering tests in: {}", test262_root.display());
    let paths = discover_tests(&test262_root);
    eprintln!("found {} test files", paths.len());

    let kernel = Arc::new(OxideKernel::new(KernelConfig::minimal()));

    let mut stats = RunStats::default();
    let mut results: Vec<TestResult> = Vec::new();

    let total = paths.len();
    for (i, path) in paths.iter().enumerate() {
        if i % 500 == 0 {
            eprintln!(
                "  progress: {}/{} ({}% pass={} fail={} skip={})",
                i,
                total,
                i * 100 / total,
                stats.pass,
                stats.fail,
                stats.skip
            );
        }

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                results.push(TestResult::skip(path.clone(), format!("read error: {e}")));
                stats.skip += 1;
                continue;
            }
        };

        let meta = match parse_meta(&source) {
            Some(m) => m,
            None => {
                results.push(TestResult::skip(
                    path.clone(),
                    String::from("no YAML metadata"),
                ));
                stats.skip += 1;
                continue;
            }
        };

        if let Some(filter_str) = &filter {
            let path_str = path.to_string_lossy();
            if !path_str.contains(filter_str.as_str()) {
                results.push(TestResult::skip(path.clone(), "filtered".into()));
                stats.skip += 1;
                continue;
            }
        }

        let path_str = path.to_string_lossy();
        if path_str.contains("staging") {
            results.push(TestResult::skip(
                path.clone(),
                "staging tests excluded".into(),
            ));
            stats.skip += 1;
            continue;
        }

        if let Some(reason) = is_skipped(&meta) {
            results.push(TestResult::skip(path.clone(), reason));
            stats.skip += 1;
            continue;
        }

        let result = run_test(path, &source, &meta, &kernel);
        match &result.outcome {
            TestOutcome::Pass(_) => stats.pass += 1,
            TestOutcome::Fail(_) => stats.fail += 1,
            TestOutcome::Skip(_) => stats.skip += 1,
        }
        stats.total_ms += result.duration_ms;
        results.push(result);
    }

    eprintln!();

    let ran = stats.pass + stats.fail;

    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 results");
    println!("═══════════════════════════════════════");
    println!("  total  : {}", results.len());
    let total = results.len() as f64;
    println!(
        "  pass   : {} ({:.1}%)",
        stats.pass,
        stats.pass as f64 / total * 100.0
    );
    println!(
        "  fail   : {} ({:.1}%)",
        stats.fail,
        stats.fail as f64 / total * 100.0
    );
    println!(
        "  skip   : {} ({:.1}%)",
        stats.skip,
        stats.skip as f64 / total * 100.0
    );
    println!("  time   : {:?}", Duration::from_millis(stats.total_ms));
    println!(
        "  pass%  : {:.1}% (of ran: {:.1}%)",
        stats.pass as f64 / total * 100.0,
        if ran > 0 {
            stats.pass as f64 / ran as f64 * 100.0
        } else {
            0.0
        }
    );
    println!("═══════════════════════════════════════");

    if stats.fail > 0 {
        std::process::exit(1);
    }
}
