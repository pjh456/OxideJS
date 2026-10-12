//! safe 模式：本地远程监督的 Unix socket 变体加自动派生 worker 进程。
//!
//! 并行 worker 从线程变派生进程（崩溃隔离），IPC 走本地 Unix socket，复用分片帧
//! 协议与跨窗口保活传输。监督者先 bind socket（临时目录短路径，防 `Sun_pathname`
//! 上限），再派生 N 个 worker 进程；每个监督线程 accept 一条连接，经
//! `supervise_window` 跑动态窗口（共享原子游标）。worker 超时/崩溃后重新派生并
//! 自肇事者后续跑。缺省 off（unsafe）保现状逐位不变。
//!
//! 连接生命周期（跨窗口保活）：每个监督线程一个传输实例，跨窗口复用——连接跨窗口
//! 保活，worker 常驻摊薄进程冷启动；worker 死亡（超时/崩溃）后 `assign` 重新派生
//! 进程并 accept 新连接。

use crate::print_fail_categories;
use crate::report::{format_fail_groupings, format_fail_list};
use crate::stats::RunStats;
use crate::supervise::{generate_token, supervise_window};
use crate::tcp_window_transport::TcpWindowTransport;
use oxide_net::Listener;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 派生一个 worker 进程：注入 `--worker` 与透传旗标（`--no-skip` /
/// `--no-regalloc`），端点与 token 经环境变量带外注入（不经网络分发）。
///
/// # 边界与前提
/// - stdout/stderr 置空：safe 模式无 sidecar 日志，现场经 scene 帧回传。
fn spawn_worker_process(
    exe: &Path, socket_path: &Path, token: &str, no_skip: bool, no_regalloc: bool,
) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new(exe);
    cmd.arg("--worker");
    if no_skip {
        cmd.arg("--no-skip");
    }
    if no_regalloc {
        cmd.arg("--no-regalloc");
    }
    cmd.env("OXIDE_TEST262_WORKER_ENDPOINT", socket_path)
        .env("OXIDE_TEST262_WORKER_TOKEN", token)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// 编排一次 safe 模式全量运行：bind Unix socket、派生 worker、监督线程并行
/// accept 加跑窗口、归约统计并打印。
pub(crate) fn run_safe(
    skip_until: usize, end_index: usize, no_skip: bool, no_regalloc: bool, paths: &[PathBuf], workers: usize,
) -> bool {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve current executable for safe mode: {err}");
            return false;
        }
    };

    // 先 bind Unix socket（临时目录短路径，防 `Sun_pathname` 108 字节上限），
    // 再派生 worker（worker 侧另有连接重试兜底 bind 完成前的竞态窗口）。
    let socket_path = std::env::temp_dir().join(format!("oxide_t262_{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket_path);
    let unix_listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("failed to bind Unix socket {}: {err}", socket_path.display());
            return false;
        }
    };
    let listener = Arc::new(Listener::Unix(unix_listener));

    // 认证 token：启动时生成，经环境变量注入 worker 进程（带外，不经网络分发）。
    let token = generate_token();

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

    let supervisors = workers.min(windows.len().max(1));

    // 路径清单经 Arc 跨监督线程共享（每窗口克隆一份随 assign 帧携带）。
    let paths_vec: Arc<Vec<String>> = Arc::new(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect());
    let timeout_secs = timeout.as_secs();
    let next = AtomicUsize::new(0);
    let next = &next;
    let windows = &windows;

    eprintln!(
        "safe mode: {} window(s) of up to {window} test(s), {supervisors} worker process(es), per-test timeout {}s",
        windows.len(),
        timeout.as_secs()
    );

    // socket 文件收尾删除用克隆（原值移入 scope 闭包）。
    let socket_for_cleanup = socket_path.clone();

    let partials: Vec<RunStats> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..supervisors)
            .map(|_| {
                scope.spawn({
                    // 每监督线程克隆一份所需值（原值被 scope 闭包持有，多闭包不得
                    // 重复 move）。
                    let paths_clone = (*paths_vec).clone();
                    let socket_clone = socket_path.clone();
                    let token_clone = token.clone();
                    let exe_clone = exe.clone();
                    let listener_clone = Arc::clone(&listener);
                    move || {
                        let mut stats = RunStats::default();
                        // 每监督线程一个传输实例，跨窗口复用（跨窗口保活）：
                        // 连接跨窗口存活，worker 超时/崩溃后重新派生并 accept。
                        let mut transport = TcpWindowTransport::with_worker_spawn(
                            listener_clone,
                            paths_clone,
                            timeout_secs,
                            token_clone.clone(),
                            Box::new(move || {
                                spawn_worker_process(&exe_clone, &socket_clone, &token_clone, no_skip, no_regalloc)
                            }),
                        );
                        loop {
                            let wi = next.fetch_add(1, Ordering::Relaxed);
                            if wi >= windows.len() {
                                break;
                            }
                            let (window_id, wstart, wend) = windows[wi];
                            let window_stats = supervise_window(
                                no_skip,
                                wstart,
                                wend,
                                timeout,
                                startup_grace,
                                paths,
                                window_id,
                                &mut transport,
                            );
                            stats.merge(window_stats);
                        }
                        stats
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("supervisor thread panicked"))
            .collect()
    });

    // 收尾删除 socket 文件，防临时目录残留。
    let _ = std::fs::remove_file(&socket_for_cleanup);

    let mut stats = RunStats::default();
    for partial_stats in partials {
        stats.merge(partial_stats);
    }

    let total = stats.pass + stats.fail + stats.skip;
    println!();
    println!("═══════════════════════════════════════");
    println!("  test262 safe aggregate");
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
        println!("  --- safe anomalies ---");
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
        for (idx, elapsed_ms, scene) in &stats.timeout_crashes {
            let path = paths
                .get(*idx)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| format!("#{idx}"));
            match scene {
                Some(s) => println!("    {idx}  {elapsed_ms}ms  {path}  [{s}]"),
                None => println!("    {idx}  {elapsed_ms}ms  {path}"),
            }
        }
    }
    println!("═══════════════════════════════════════");

    stats.fail == 0
}
