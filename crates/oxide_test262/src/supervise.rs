//! 监督模式子进程监督与聚合：把测试区间切窗口，监督线程并行派生/轮询/杀死单 worker
//! 子进程；心跳并入累计统计，旁路失败行按已消费偏移并入；超时/崩溃杀肇事者自其后续跑（默认计 skip，--no-skip 计 fail）。

use crate::heartbeat::{merge_heartbeat, read_heartbeat};
use crate::print_fail_categories;
use crate::report::{format_fail_groupings, format_fail_list, parse_fail_log};
use crate::stats::RunStats;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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

/// 读子进程被杀前留下的 last-pc 现场行：取文件末行，须含 pc/op/frames 三字段
/// 才接受（缺文件 / 空文件 / 残行均返回 None）。
fn read_pc_scene(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let last = content.lines().last()?.trim();
    if last.starts_with("pc=") && last.contains(" op=") && last.contains(" frames=") {
        Some(last.to_string())
    } else {
        None
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
    // last-pc 现场文件：子进程运行期定频追加写，监督者杀子进程后读回末行。
    let pc_path = std::env::temp_dir().join(format!("oxide_t262_pc_{}_{}.txt", std::process::id(), window_id));

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
        let _ = std::fs::remove_file(&pc_path);
        let max_tests = wend - cur;

        let mut child = match Command::new(exe)
            .args(args.iter().skip(1))
            .env("OXIDE_SKIP_UNTIL", cur.to_string())
            .env("OXIDE_MAX_TESTS", max_tests.to_string())
            .env("OXIDE_TEST262_WORKERS", "1")
            .env("OXIDE_TEST262_HEARTBEAT", &hb_path)
            .env("OXIDE_TEST262_PC_WATCH", &pc_path)
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
                record_timeout_or_crash(&mut stats, no_skip, cur, 0, None);
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
                            let scene = read_pc_scene(&pc_path);
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) mid-test #{}: {}",
                                culprit,
                                describe(culprit)
                            );
                            record_timeout_or_crash(&mut stats, no_skip, culprit, 0, scene.as_deref());
                            cur = culprit;
                        }
                        None => {
                            eprintln!(
                                "  [warn] window {window_id}: child exited ({status}) with no heartbeat at index {cur}; skipping one"
                            );
                            merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
                            let scene = read_pc_scene(&pc_path);
                            record_timeout_or_crash(&mut stats, no_skip, cur, 0, scene.as_deref());
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
                    let scene = read_pc_scene(&pc_path);
                    record_timeout_or_crash(&mut stats, no_skip, cur, 0, scene.as_deref());
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
                // 先杀再读：心跳、旁路失败行与 last-pc 现场都须在子进程死亡后取最终状态。
                let _ = child.kill();
                let _ = child.wait();
                let hb = read_heartbeat(&hb_path);
                let culprit = hb.as_ref().map(|h| h.index + 1).unwrap_or(cur);
                if let Some(h) = &hb {
                    merge_heartbeat(&mut stats, h);
                }
                merge_fail_log(&mut stats, &hb_path, &mut fails_consumed);
                let scene = read_pc_scene(&pc_path);
                eprintln!(
                    "  [timeout] window {window_id}: TIMEOUT ({}s) on test #{culprit}: {}{}",
                    deadline.as_secs(),
                    describe(culprit),
                    scene.as_deref().map(|s| format!("  [{s}]")).unwrap_or_default()
                );
                record_timeout_or_crash(&mut stats, no_skip, culprit, elapsed.as_millis() as u64, scene.as_deref());
                cur = culprit + 1;
                break;
            }

            std::thread::sleep(Duration::from_millis(200));
        }
    }

    let _ = std::fs::remove_file(&hb_path);
    let _ = std::fs::remove_file(format!("{}.tmp", hb_path.display()));
    let _ = std::fs::remove_file(&pc_path);
    // 旁路失败行归档而非删除：统计已由心跳合并入父进程，此文件是收尾逐文件
    // 差分的终态依据（删除即永久丢记录）。
    let _ = std::fs::rename(hb_path.with_extension("fails"), hb_path.with_extension("fails.done"));
    stats
}

/// 编排一次监督式全量运行：把 `[skip_until, end_index)` 切分为窗口，至多
/// `supervisors` 个窗口并发。监督线程只派生/轮询/杀死子进程并读写文件——
/// 从不持有 `KernelCore`，因此按引用共享 `paths`/`args` 是安全的。
pub(crate) fn run_supervised(
    args: &[String], skip_until: usize, end_index: usize, no_skip: bool, paths: &[PathBuf],
) -> bool {
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

    // 缺省 16：全量复测验证过的安全并发；未经复测不得抬高。
    let supervisors = std::env::var("OXIDE_TEST262_SUPERVISORS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(16)
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
    use crate::report::append_fail_log;

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

    /// read_pc_scene 防御解析：缺文件 / 空文件 / 残行（缺 frames 字段）均 None，
    /// 末行完整时接受（忽略中间行）。
    #[test]
    fn read_pc_scene_defensive_parse() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxide_t262_pc_test_{}.txt", std::process::id()));
        assert_eq!(read_pc_scene(&path), None, "缺文件返回 None");
        std::fs::write(&path, "").expect("写失败");
        assert_eq!(read_pc_scene(&path), None, "空文件返回 None");
        std::fs::write(&path, "pc=1 op=ADD flat_id=0 frames=1\npc=2 op=ADD flat_id=0 frames=1\n").expect("写失败");
        assert_eq!(read_pc_scene(&path), Some("pc=2 op=ADD flat_id=0 frames=1".to_string()));
        // 残行：末行缺 frames 字段，返回 None。
        std::fs::write(&path, "pc=1 op=ADD flat_id=0 frames=1\npc=2 op=AD").expect("写失败");
        assert_eq!(read_pc_scene(&path), None, "残行返回 None");
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
