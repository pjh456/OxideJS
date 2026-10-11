//! 监督模式子进程监督与聚合：把测试区间切窗口，监督线程并行派生/轮询/杀死单 worker
//! 子进程；心跳并入累计统计，旁路失败行按已消费偏移并入；超时/崩溃杀肇事者自其后续跑（默认计 skip，--no-skip 计 fail）。
//!
//! 文件协议（心跳/旁路失败行/last-pc 现场/sidecar 日志/子进程派生与杀死）收口在
//! window_transport 的 WindowTransport 传输接口，本模块只保留监督策略：超时判定、
//! 重启循环、记账。

use crate::heartbeat::merge_heartbeat;
use crate::print_fail_categories;
use crate::report::{format_fail_groupings, format_fail_list};
use crate::stats::RunStats;
use crate::tcp_window_transport::TcpWindowTransport;
use crate::window_transport::{LocalFileWindowTransport, PollResult, WindowTransport};
use oxide_net::Listener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 超时/崩溃后把 sidecar 日志的末 ~20 行打到父进程 stderr，供现场重建。
fn print_log_tail(window_id: usize, transport: &dyn WindowTransport) {
    if let Some(tail) = transport.log_tail() {
        eprintln!("  [log tail] window {window_id}:\n{tail}");
    }
}

/// 记一笔超时/崩溃结果：默认计入 skip，`--no-skip` 下计入 fail（归入
/// `timeout/crash` 类别，父进程无法进一步拆分根因）。
///
/// # 副作用
/// - 把 `(index, elapsed_ms, scene)` 追加进 timeout_crashes 清单（elapsed_ms=0
///   表示非超时崩溃：spawn 失败 / 中途退出 / waiterr / 无心跳；scene 为挂死点
///   的 last-pc 现场行，缺文件/解析失败为 None），供收尾逐路径报告。
fn record_timeout_or_crash(stats: &mut RunStats, no_skip: bool, index: usize, elapsed_ms: u64, scene: Option<&str>) {
    if no_skip {
        stats.fail += 1;
        *stats.fail_categories.entry("timeout/crash".into()).or_insert(0) += 1;
    } else {
        stats.skip += 1;
    }
    stats.timeout_crashes.push((index, elapsed_ms, scene.map(str::to_string)));
}

/// 在监督下运行一个窗口 `[wstart, wend)`，返回经过多次子进程重启
/// 聚合的 `RunStats`（含失败类别）。
///
/// 单 worker 子进程运行常规 in-process 路径（预热 kernel + harness 前缀缓存）
/// 并在每个测试完成后发出心跳（COMPLETED）。若运行下标停滞超过 `timeout`，
/// 子进程被杀死、按路径报告肇事者，并由全新子进程从肇事者之后续跑；子进程
/// 在测试中途崩溃也经同路径恢复，恢复基于最后完成下标，无漏项。子进程死亡后
/// 读取旁路失败行并入 fail_records。超时/崩溃默认计为 skip，`--no-skip` 下计为失败。
///
/// # 注意事项
/// - `transport` 是窗口级传输实例（跨子进程重启存活），六操作与文件生命周期
///   都经它调用；路径清单文件由传输持有并经 `OXIDE_TEST262_PATHS_FILE` 传给
///   子进程，在窗口内子进程多次重启间只读共享，不得按 spawn 删除。
/// - `paths` 是父进程的全量路径清单（每行一路径，与父扫描同序），仅供肇事者
///   路径描述。
#[expect(clippy::too_many_arguments)]
fn supervise_window(
    no_skip: bool, wstart: usize, wend: usize, timeout: Duration, startup_grace: Duration, paths: &[PathBuf],
    window_id: usize, transport: &mut dyn WindowTransport,
) -> RunStats {
    let mut stats = RunStats::default();
    let mut cur = wstart;

    let describe = |idx: usize| -> String {
        paths
            .get(idx)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("#{idx}"))
    };

    while cur < wend {
        // 心跳文件每子进程重开；旁路失败行与 sidecar 日志跨重启保留（追加而非
        // 清空）：超时/崩溃重启若删旁路会丢掉此前子进程的全部失败记录与输出，
        // 收尾差分只剩末段。merge_fail 按已消费字节偏移只并入新增段（每行恰
        // 并入一次）；重跑测试至多重录一条（心跳先写、旁路后追加的写序），
        // 新行是真实新事件而非重复并入。
        transport.reset_round();

        let mut last_index: Option<usize> = None;
        let mut last_change = Instant::now();

        if let Err(err) = transport.assign(cur, wend) {
            eprintln!("  window {window_id}: failed to spawn child at index {cur}: {err}");
            stats.spawn_errors += 1;
            record_timeout_or_crash(&mut stats, no_skip, cur, 0, None);
            cur += 1;
            continue;
        }
        let spawn_time = Instant::now();

        loop {
            match transport.poll() {
                PollResult::Exited(status) => {
                    match transport.heartbeat() {
                        Some(hb) if hb.phase == "DONE" => {
                            merge_heartbeat(&mut stats, &hb);
                            transport.merge_fail(&mut stats);
                            cur = wend;
                        }
                        Some(hb) => {
                            merge_heartbeat(&mut stats, &hb);
                            let culprit = hb.index + 1;
                            transport.merge_fail(&mut stats);
                            let scene = transport.scene();
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) mid-test #{}: {}",
                                culprit,
                                describe(culprit)
                            );
                            print_log_tail(window_id, transport);
                            record_timeout_or_crash(&mut stats, no_skip, culprit, 0, scene.as_deref());
                            cur = culprit;
                        }
                        None => {
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) with no heartbeat at index {cur}; skipping one"
                            );
                            transport.merge_fail(&mut stats);
                            let scene = transport.scene();
                            print_log_tail(window_id, transport);
                            record_timeout_or_crash(&mut stats, no_skip, cur, 0, scene.as_deref());
                            cur += 1;
                        }
                    }
                    break;
                }
                PollResult::Alive => {}
                PollResult::WaitError(err) => {
                    eprintln!("  window {window_id}: try_wait error: {err}");
                    stats.wait_errors += 1;
                    transport.kill();
                    transport.merge_fail(&mut stats);
                    let scene = transport.scene();
                    print_log_tail(window_id, transport);
                    record_timeout_or_crash(&mut stats, no_skip, cur, 0, scene.as_deref());
                    cur += 1;
                    break;
                }
            }

            if let Some(hb) = transport.heartbeat() {
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
                // 先杀再读：心跳、旁路失败行与 last-pc 现场都须在子进程死亡后取最终状态。
                transport.kill();
                let hb = transport.heartbeat();
                let culprit = hb.as_ref().map(|h| h.index + 1).unwrap_or(cur);
                if let Some(h) = &hb {
                    merge_heartbeat(&mut stats, h);
                }
                transport.merge_fail(&mut stats);
                let scene = transport.scene();
                eprintln!(
                    "  [timeout] window {window_id}: TIMEOUT ({}s) on test #{culprit}: {}{}",
                    deadline.as_secs(),
                    describe(culprit),
                    scene.as_deref().map(|s| format!("  [{s}]")).unwrap_or_default()
                );
                print_log_tail(window_id, transport);
                record_timeout_or_crash(&mut stats, no_skip, culprit, elapsed.as_millis() as u64, scene.as_deref());
                cur = culprit + 1;
                break;
            }

            std::thread::sleep(Duration::from_millis(200));
        }
    }

    transport.archive_window();
    stats
}

/// 编排一次监督式全量运行：把 `[skip_until, end_index)` 切分为窗口，至多
/// `supervisors` 个窗口并发。监督线程只派生/轮询/杀死子进程并读写文件——
/// 从不持有 `KernelCore`，因此按引用共享 `paths`/`args` 是安全的。
pub(crate) fn run_supervised(
    args: &[String], skip_until: usize, end_index: usize, no_skip: bool, paths: &[PathBuf],
    remote_supervise: Option<(String, usize)>,
) -> bool {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve current executable for supervised mode: {err}");
            return false;
        }
    };

    // 路径清单文件：仅本地模式需要（子进程读文件代替全量重扫目录）；远程模式
    // 路径随 assign 帧携带，不写文件。运行结束删除，防临时目录残留。
    let paths_file: Option<PathBuf> = if remote_supervise.is_none() {
        let pf = std::env::temp_dir().join(format!("oxide_t262_paths_{}.txt", std::process::id()));
        let list = paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
        if let Err(err) = std::fs::write(&pf, list) {
            eprintln!("failed to write paths file {}: {err}", pf.display());
            return false;
        }
        Some(pf)
    } else {
        None
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

    // 缺省 16：全量复测验证过的安全并发；未经复测不得抬高。远程模式用旗标 worker 数。
    let (supervisors, endpoint) = match &remote_supervise {
        Some((endpoint, workers)) => ((*workers).min(windows.len().max(1)), Some(endpoint.clone())),
        None => (
            std::env::var("OXIDE_TEST262_SUPERVISORS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(16)
                .min(windows.len().max(1)),
            None,
        ),
    };

    let next = AtomicUsize::new(0);
    let next = &next;
    let windows = &windows;
    let exe = &exe;

    let partials: Vec<RunStats> = match &endpoint {
        Some(endpoint) => {
            // 远程分支：bind 监听端点，每窗口 accept 一条 worker 连接，worker 数控制
            // 并发线程数。路径随 assign 帧携带，不写 paths_file。
            let listener = match std::net::TcpListener::bind(endpoint) {
                Ok(l) => Arc::new(Listener::Tcp(l)),
                Err(err) => {
                    eprintln!("failed to bind remote supervisor endpoint {endpoint}: {err}");
                    return false;
                }
            };
            let bound = listener.local_addr().unwrap_or_else(|_| endpoint.clone());
            eprintln!(
                "remote supervised mode: {} window(s) of up to {window} test(s), {supervisors} worker(s), endpoint {bound}, per-test timeout {}s",
                windows.len(),
                timeout.as_secs()
            );
            // 路径清单经 Arc 跨监督线程共享（每窗口克隆一份随 assign 帧携带）。
            let paths_vec: Arc<Vec<String>> =
                Arc::new(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect());
            let timeout_secs = timeout.as_secs();
            let listener_ref = &listener;
            let paths_vec_ref = &paths_vec;
            std::thread::scope(|scope| {
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
                                // 每窗口一个传输实例（跨该窗口内 worker 重启存活）。
                                let window_stats = supervise_window(
                                    no_skip,
                                    wstart,
                                    wend,
                                    timeout,
                                    startup_grace,
                                    paths,
                                    window_id,
                                    &mut TcpWindowTransport::new(
                                        Arc::clone(listener_ref),
                                        (**paths_vec_ref).clone(),
                                        timeout_secs,
                                    ),
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
            })
        }
        None => {
            // 本地分支：现状不变（写 paths_file、LocalFileWindowTransport、缺省 16）。
            let paths_file = paths_file.as_ref().expect("本地模式已写 paths_file");
            eprintln!(
                "supervised mode: {} window(s) of up to {window} test(s), {supervisors} supervisor(s), per-test timeout {}s",
                windows.len(),
                timeout.as_secs()
            );
            std::thread::scope(|scope| {
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
                                // 每窗口一个传输实例（跨该窗口内子进程重启存活），
                                // 已消费偏移等窗口级状态随之保留。
                                let window_stats = supervise_window(
                                    no_skip,
                                    wstart,
                                    wend,
                                    timeout,
                                    startup_grace,
                                    paths,
                                    window_id,
                                    &mut LocalFileWindowTransport::new(exe, args, paths_file, window_id),
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
            })
        }
    };

    let mut stats = RunStats::default();
    for partial_stats in partials {
        stats.merge(partial_stats);
    }

    // 全部窗口结束、子进程已读完清单，删除路径清单文件防临时目录残留（远程模式无文件）。
    if let Some(pf) = &paths_file {
        let _ = std::fs::remove_file(pf);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 超时/崩溃记账：计数（no_skip 转 fail）与 timeout_crashes 入列（scene 原样
    /// 存串，缺现场为 None）。
    #[test]
    fn record_timeout_or_crash_records_index_elapsed() {
        let mut stats = RunStats::default();
        record_timeout_or_crash(&mut stats, true, 3, 2500, Some("pc=1 op=ADD flat_id=0 frames=1"));
        assert_eq!(stats.fail, 1);
        assert_eq!(stats.fail_categories.get("timeout/crash"), Some(&1));
        assert_eq!(stats.timeout_crashes, vec![(3, 2500, Some("pc=1 op=ADD flat_id=0 frames=1".to_string()))]);
        record_timeout_or_crash(&mut stats, false, 4, 0, None);
        assert_eq!(stats.skip, 1);
        assert_eq!(
            stats.timeout_crashes,
            vec![(3, 2500, Some("pc=1 op=ADD flat_id=0 frames=1".to_string())), (4, 0, None)]
        );
    }
}
