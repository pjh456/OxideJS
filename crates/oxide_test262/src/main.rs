#![allow(clippy::arc_with_non_send_sync)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

mod chunked;
mod config;
mod discovery;
mod harness;
mod heartbeat;
mod judge;
mod meta;
mod report;
mod runner;
mod stats;
mod supervise;
mod test262_log;
use chunked::run_chunked;
use config::RunConfig;
use discovery::discover_tests;
use harness::{HarnessPrefixCache, HarnessSources, HARNESS};
use heartbeat::write_heartbeat;
use judge::{categorize_fail, TestOutcome};
use oxide_log::{Level, LogConfig, Output, SUBSYSTEM_COUNT};
use report::{append_fail_log, first_line, format_fail_categories, format_fail_groupings, format_fail_list};
use runner::{build_runner_kernel, process_path, CURRENT_TEST_PATH};
use stats::RunStats;
use supervise::run_supervised;

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
