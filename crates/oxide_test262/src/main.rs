#![allow(clippy::arc_with_non_send_sync)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

mod config;
mod discovery;
mod harness;
mod heartbeat;
mod judge;
mod meta;
mod report;
mod runner;
mod stats;
mod test262_log;
use config::RunConfig;
use discovery::discover_tests;
use harness::{HarnessPrefixCache, HarnessSources, HARNESS};
use heartbeat::{merge_heartbeat, read_heartbeat, write_heartbeat};
use judge::{categorize_fail, TestOutcome};
use oxide_log::{Level, LogConfig, Output, SUBSYSTEM_COUNT};
use report::{
    append_fail_log, first_line, format_fail_categories, format_fail_groupings, format_fail_list, parse_fail_log,
};
use runner::{build_runner_kernel, process_path, CURRENT_TEST_PATH};
use stats::RunStats;

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

/// 把 supervise 旁路失败行文件并入统计：逐行重建 FailRecord（类别经本表 id
/// 重映射 + push_fail_record 的 cap 逻辑）。
///
/// # 边界与前提
/// - 失败计数（fail_categories）已由对应心跳合并覆盖，此处只补 fail_records，
///   不得再累计计数，避免双计。worker 循环先写心跳后追加旁路，故"无心跳退出"
///   分支旁路必为空（防御性 no-op）。
/// - 旁路文件跨子进程重启追加保留，本函数只解析自上次消费以来的新增段
///   （`consumed` 由调用方按窗口持有）；文件缩至已消费偏移之下（删除/重建）
///   回绕到文件头重读，避免丢记录。
fn merge_fail_log(stats: &mut RunStats, hb_path: &Path, consumed: &mut usize) {
    let content = match std::fs::read_to_string(hb_path.with_extension("fails")) {
        Ok(c) => c,
        Err(_) => return,
    };
    if content.len() < *consumed {
        *consumed = 0;
    }
    let fresh = &content[*consumed..];
    *consumed = content.len();
    for (index, cat, subkey, message) in parse_fail_log(fresh) {
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
    // 旁路失败行已消费字节偏移：跨重启只并入新增段（见 merge_fail_log）。
    let mut fails_consumed: usize = 0;
    let hb_path = std::env::temp_dir().join(format!("oxide_t262_hb_{}_{}.txt", std::process::id(), window_id));

    let describe = |idx: usize| -> String {
        paths
            .get(idx)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("#{idx}"))
    };

    while cur < wend {
        // 心跳文件每子进程重开；旁路失败行跨重启保留（追加而非清空）：
        // 超时/崩溃重启若删旁路会丢掉此前子进程的全部失败记录，收尾差分
        // 只剩末段。merge_fail_log 按已消费字节偏移只并入新增段（每行恰
        // 并入一次）；重跑测试至多重录一条（心跳先写、旁路后追加的写序），
        // 新行是真实新事件而非重复并入。
        let _ = std::fs::remove_file(&hb_path);
        let _ = std::fs::remove_file(format!("{}.tmp", hb_path.display()));
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
                            merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
                            cur = wend;
                        }
                        Some(hb) => {
                            merge_heartbeat(&mut stats, &hb);
                            let culprit = hb.index + 1;
                            merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
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
                            merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
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
                    merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
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
                merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
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
    // 旁路失败行归档而非删除：统计已由心跳合并入父进程，此文件是收尾逐文件
    // 差分的终态依据（删除即永久丢记录）。
    let _ = std::fs::rename(hb_path.with_extension("fails"), hb_path.with_extension("fails.done"));
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
    let groupings = format_fail_groupings(&stats, paths);
    if !groupings.is_empty() {
        print!("{groupings}");
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
                                    // 旁路行携带真 subkey，父进程并入 fail_records 后直接生效 subkey 分组。
                                    let (cat, subkey) = categorize_fail(msg);
                                    if let Err(e) = append_fail_log(&hb.with_extension("fails"), i, &cat, &subkey, msg)
                                    {
                                        stats.hb_write_errors += 1;
                                        eprintln!("  [warn] fail log append failed at #{i}: {e}");
                                    }
                                }
                            }
                            if verbose {
                                match &result.outcome {
                                    TestOutcome::Pass(_) => println!("PASS {}", paths_ref[i].display()),
                                    TestOutcome::Fail(msg) => {
                                        let (cat, _) = categorize_fail(msg);
                                        println!("FAIL {} [{}] {}", paths_ref[i].display(), cat, first_line(msg));
                                    }
                                    TestOutcome::Skip(_) => println!("SKIP {}", paths_ref[i].display()),
                                }
                            }
                            tests_since_kernel_reset += 1;

                            let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
                            if tests_since_kernel_reset >= kernel_batch {
                                // 重建即全新 forge（结构性必清）：旧核连同其表整体丢弃，
                                // 先 sweep 是对 doomed 核白做一次 O(50k) 清理。
                                // 重建边界契约：旧核上无存活 VM（VM 每测试局部，此处作用域外）。
                                debug_assert!(kernel.active_vms() == 0, "kernel rebuild requires no live VMs");
                                kernel = build_runner_kernel();
                                tests_since_kernel_reset = 0;
                            } else if done % 500 == 0 {
                                kernel.sweep_runner_forges(); // 批内 50k 兜底（数据依赖）
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
    // 分组是聚合汇总段，恒打印（不受 --no-fail-list 控制）；无 fail 数据时静默。
    let groupings = format_fail_groupings(&stats, &paths);
    if !groupings.is_empty() {
        print!("{groupings}");
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
        let mut fails_consumed = 0usize;
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 2);
        // 无新增内容的二次合并不产生重复记录（已消费偏移）。
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
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

    /// 跨重启合并序列：旁路文件两次"子进程死亡"间追加保留，第二次合并只并入
    /// 新增段（不重复并入前段行）；重跑测试的新行是真实新事件；文件重建得比
    /// 已消费偏移更短时回绕重读。
    #[test]
    fn merge_fail_log_restart_sequence_no_reread_duplicates() {
        let dir = std::env::temp_dir();
        let hb_path = dir.join(format!("oxide_t262_hb_test_rst_{}.txt", std::process::id()));
        let sidecar = hb_path.with_extension("fails");
        // 段 1：首个子进程死亡前的失败行。
        append_fail_log(&sidecar, 10, "vm: not callable", "", "a").expect("追加失败");
        append_fail_log(&sidecar, 11, "vm: not callable", "", "b").expect("追加失败");
        let mut stats = RunStats::default();
        let mut fails_consumed = 0usize;
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 2);
        // 段 2：第二个子进程追加（重跑的测试 11 至多重录一条 + 新测试 12）。
        append_fail_log(&sidecar, 11, "vm: not callable", "", "b again").expect("追加失败");
        append_fail_log(&sidecar, 12, "compile: unsupported", "", "c").expect("追加失败");
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 4, "只应并入新增段: {:?}", stats.fail_records);
        assert_eq!(stats.fail_records.iter().map(|r| r.index).collect::<Vec<_>>(), vec![10, 11, 11, 12]);
        // 段 3：无新增内容的合并幂等。
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 4);
        // 文件重建得比已消费偏移短：回绕到文件头重读。
        std::fs::write(&sidecar, "20\tvm: not callable\t\td\n").expect("重写失败");
        merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
        assert_eq!(stats.fail_records.len(), 5);
        assert_eq!(stats.fail_records[4].index, 20);
        let _ = std::fs::remove_file(&sidecar);
        let _ = std::fs::remove_file(&hb_path);
    }
}
