//! Worker 模式：远程分片的执行半区。连监督者端点收窗口指派帧，in-process
//! 执行测试，发心跳/失败/现场/DONE 帧。
//!
//! 认证门：连接后首帧发 `Auth` 事件帧（token 经 `OXIDE_TEST262_WORKER_TOKEN`
//! 带外注入），读回 `AuthOk` 命令帧才进主循环；缺失 token、读超时、首帧非
//! `AuthOk` 均报错退出。握手必须在看门狗线程启动前完成，否则看门狗会把
//! `AuthOk` 帧当命令帧消费。
//!
//! 看门狗线程独立读控制帧（kill → 置标志），主线程在测试间隙检查标志后从
//! 主线程上下文发现场帧再退出。`KernelCore` 是 `!Send`，看门狗不得触引擎；
//! 现场帧内容来自主线程上下文（last-pc 采样行）。断连按死亡处理（恢复由
//! 监督者侧同构完成）。

use std::io::BufReader;
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::RunConfig;
use crate::harness::{HarnessModuleCache, HarnessPrefixCache, HarnessSources, HARNESS};
use crate::judge::{categorize_fail, TestOutcome};
use crate::runner::{build_runner_engine, process_path, CURRENT_TEST_PATH};
use crate::shard_protocol::{encode_event, parse_command, FrameReader, ShardCommand, ShardEvent, AUTH_READ_TIMEOUT};
use crate::stats::RunStats;
use oxide_net::Conn;

/// 窗口指派帧载荷：窗口起始下标与路径清单（`wend` 与 `timeout_secs` 由
/// 监督者侧消费，worker 侧按路径清单长度执行，超时由监督者发 kill 帧兜底）。
struct AssignWindow {
    wstart: usize,
    paths: Vec<String>,
}

/// 看门狗到主线程的通道消息：窗口指派或 kill 信号。
/// kill 信号使主线程在测试间隙（或窗口间）发现场帧并退出，
/// 不依赖 `kill_flag` 的轮询（主线程阻塞在通道时也能收到）。
enum ChannelMsg {
    Assign(AssignWindow),
    Kill,
}

/// Worker 模式入口：连监督者端点，过认证门，收窗口指派帧，in-process 执行，发帧。
///
/// 端点经 `OXIDE_TEST262_WORKER_ENDPOINT` 环境变量传入（host:port 形态），
/// token 经 `OXIDE_TEST262_WORKER_TOKEN` 带外注入（监督者前台打印，不经网络
/// 分发）。看门狗线程独立读控制帧（kill → 置标志），主线程在测试间隙检查
/// 标志后发现场帧再退出。断连按死亡处理（恢复由监督者侧完成）。
pub(crate) fn run_worker(config: &RunConfig) -> bool {
    let endpoint = match std::env::var("OXIDE_TEST262_WORKER_ENDPOINT") {
        Ok(ep) => ep,
        Err(_) => {
            eprintln!("worker mode requires OXIDE_TEST262_WORKER_ENDPOINT (host:port)");
            return false;
        }
    };
    // token 带外注入：缺失或为空在连接前报错退出，不占连接。
    let token = match std::env::var("OXIDE_TEST262_WORKER_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            eprintln!(
                "worker mode requires OXIDE_TEST262_WORKER_TOKEN (out-of-band from the supervisor's foreground print)"
            );
            return false;
        }
    };

    let conn = match connect_with_retry(&endpoint) {
        Some(c) => c,
        None => {
            eprintln!("worker: failed to connect to {endpoint} after retries");
            return false;
        }
    };
    let mut conn = conn;

    // 认证门：读超时 30 秒（半开连接不永久占进程），发 Auth 帧后读回首帧，
    // 必须是 AuthOk，否则报错退出。握手必须在看门狗线程启动前完成：看门狗
    // 独立读命令帧，提前启动会消费 AuthOk 帧。
    if conn.set_read_timeout(Some(AUTH_READ_TIMEOUT)).is_err() {
        eprintln!("worker: failed to set read timeout");
        return false;
    }
    let auth_frame = encode_event(&ShardEvent::Auth { token });
    if conn.write_all(auth_frame.as_bytes()).is_err() {
        eprintln!("worker: failed to send auth frame");
        return false;
    }
    let mut reader = FrameReader::new(BufReader::new(conn));
    let first = match reader.read_frame() {
        Ok(Some(frame)) => frame,
        Ok(None) => {
            eprintln!("worker: supervisor closed the connection before auth ok");
            return false;
        }
        Err(e) => {
            eprintln!("worker: failed to read auth ok frame: {e}");
            return false;
        }
    };
    if !matches!(parse_command(&first), Ok(ShardCommand::AuthOk)) {
        eprintln!("worker: supervisor did not confirm auth (first frame is not authok)");
        return false;
    }
    // 握手完成：清读超时（看门狗阻塞读不得超时），克隆写句柄供主线程发
    // 事件帧，读句柄交给看门狗。
    if reader.get_mut().get_mut().set_read_timeout(None).is_err() {
        eprintln!("worker: failed to clear read timeout");
        return false;
    }
    let writer = match reader.get_mut().get_mut().try_clone() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("worker: failed to clone conn for writer: {e}");
            return false;
        }
    };
    let mut conn = writer;

    // 看门狗线程：独立读控制帧（读句柄移入）。指派帧经通道转主线程
    // （主线程持引擎，看门狗不得触引擎）；kill 帧置标志并经通道发 kill
    // 信号（主线程阻塞在通道时也能收到，不依赖标志轮询）。
    let kill_flag = Arc::new(AtomicBool::new(false));
    let (msg_tx, msg_rx) = mpsc::channel::<ChannelMsg>();
    let watchdog_kill = Arc::clone(&kill_flag);
    let watchdog_tx = msg_tx;
    std::thread::spawn(move || {
        let mut reader = reader;
        loop {
            match reader.read_frame() {
                Ok(Some(frame)) => match parse_command(&frame) {
                    Ok(ShardCommand::Assign {
                        wstart,
                        wend: _,
                        paths,
                        timeout_secs: _,
                    }) => {
                        let _ = watchdog_tx.send(ChannelMsg::Assign(AssignWindow { wstart, paths }));
                    }
                    Ok(ShardCommand::AuthOk) => {
                        // AuthOk 只出现一次且已被主线程握手消费；重复到达
                        // 忽略（协议上监督者不会重发）。
                        continue;
                    }
                    Ok(ShardCommand::Kill) => {
                        watchdog_kill.store(true, Ordering::SeqCst);
                        let _ = watchdog_tx.send(ChannelMsg::Kill);
                        break;
                    }
                    Err(_) => continue,
                },
                Ok(None) => break,
                Err(_) => break,
            }
        }
    });

    // 主线程：收窗口指派帧，in-process 执行，发心跳/失败/现场/DONE 帧。
    let mut engine = build_runner_engine();
    let harness_sources = HARNESS.get_or_init(HarnessSources::new);
    let harness_cache = Arc::new(RwLock::new(HarnessPrefixCache::new()));
    let harness_module_cache = Arc::new(RwLock::new(HarnessModuleCache::new()));
    let kernel_batch = std::env::var("OXIDE_TEST262_KERNEL_BATCH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .map(|n| n.min(512))
        .unwrap_or(256);
    let mut tests_since_kernel_reset = 0usize;

    for msg in msg_rx {
        let window = match msg {
            ChannelMsg::Assign(window) => window,
            ChannelMsg::Kill => {
                // 窗口间收到 kill：发现场帧并退出。
                send_scene_frame(&mut conn);
                return true;
            }
        };
        // 每窗口一份统计：DONE 心跳的 pass/fail/skip 是窗口级计数（监督者每窗口
        // 只合并一次最终心跳的契约），跨窗口累计会使后窗口心跳含前窗口计数而双计。
        // 失败记录帧逐测试随发（带全局下标），不依赖本统计跨窗口保留。
        let mut stats = RunStats::default();
        for (offset, path_str) in window.paths.iter().enumerate() {
            let i = window.wstart + offset;
            // 检查 kill 标志（看门狗读 kill 帧后置位）。
            if kill_flag.load(Ordering::SeqCst) {
                send_scene_frame(&mut conn);
                return true;
            }

            let path = PathBuf::from(path_str);
            let path_str = path.display().to_string();
            CURRENT_TEST_PATH.with(|p| *p.borrow_mut() = path_str.clone());

            let result = process_path(
                &path,
                &None,
                config.no_skip,
                config.no_regalloc,
                &engine,
                harness_sources,
                &harness_cache,
                &harness_module_cache,
            );
            stats.record(i, &result);

            // 发心跳帧（COMPLETED 阶段，计数覆盖 ≤ i 的全部测试）。
            stats.hb_write_errors += send_heartbeat_frame(&mut conn, i, &stats);

            // 发失败记录帧（一测试失败一帧）。
            if let TestOutcome::Fail(msg) = &result.outcome {
                let (cat, subkey) = categorize_fail(msg);
                stats.hb_write_errors += send_fail_frame(&mut conn, i, &cat, &subkey, msg);
            }

            tests_since_kernel_reset += 1;
            if tests_since_kernel_reset >= kernel_batch {
                debug_assert!(engine.kernel().active_vms() == 0, "kernel rebuild requires no live VMs");
                engine = build_runner_engine();
                tests_since_kernel_reset = 0;
            } else if tests_since_kernel_reset % 500 == 0 {
                engine.kernel().sweep_runner_forges();
            }
        }

        // 发 DONE 帧（退出前 flush，监督者不致误判死亡重跑窗口）。
        if let Err(e) = send_event_frame(&mut conn, &ShardEvent::Done) {
            eprintln!("worker: failed to send DONE frame: {e}");
        }
    }

    true
}

/// 按端点形态连接监督者：含 `:` 走 TCP（host:port），否则走 Unix socket 路径
/// （safe 模式临时目录短路径，防 `Sun_pathname` 上限）。
///
/// # 步骤
/// 1. 按端点形态选传输臂尝试连接。
/// 2. 失败（含 socket 文件尚未 bind 的竞态）睡 100 ms 重试，至 30 秒上限。
///
/// # 边界与前提
/// - safe 模式监督者先 bind 再派生 worker，正常路径首连即成；重试只兜底
///   进程启动早于 bind 完成的竞态窗口。
///
/// # 副作用
/// - 无（只建连接，不发帧）。
fn connect_with_retry(endpoint: &str) -> Option<Conn> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if endpoint.contains(':') {
            if let Ok(s) = TcpStream::connect(endpoint) {
                return Some(Conn::Tcp(s));
            }
        } else if let Ok(s) = UnixStream::connect(endpoint) {
            return Some(Conn::Unix(s));
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 发送事件帧（write_all 即 flush）。
fn send_event_frame(conn: &mut Conn, event: &ShardEvent) -> std::io::Result<()> {
    let frame = encode_event(event);
    conn.write_all(frame.as_bytes())
}

/// 发心跳帧；返回写失败数（0 或 1）。
fn send_heartbeat_frame(conn: &mut Conn, index: usize, stats: &RunStats) -> usize {
    let event = ShardEvent::Heartbeat {
        phase: "COMPLETED".into(),
        index,
        pass: stats.pass,
        fail: stats.fail,
        skip: stats.skip,
        categories: stats.fail_categories.clone(),
        hb_write_errors: stats.hb_write_errors,
    };
    match send_event_frame(conn, &event) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("worker: failed to send heartbeat frame at #{index}: {e}");
            1
        }
    }
}

/// 发失败记录帧；返回写失败数（0 或 1）。
fn send_fail_frame(conn: &mut Conn, index: usize, category: &str, subkey: &str, message: &str) -> usize {
    let event = ShardEvent::Fail {
        index,
        category: category.to_string(),
        subkey: subkey.to_string(),
        message: message.to_string(),
    };
    match send_event_frame(conn, &event) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("worker: failed to send fail frame at #{index}: {e}");
            1
        }
    }
}

/// 发现场帧：读 last-pc 现场文件末行，作为现场帧发出。
///
/// 须主线程上下文调用（引擎 `!Send`，看门狗不得触引擎）。现场文件缺或
/// 末行不合法时静默返回（监督者侧按无现场处理）。
fn send_scene_frame(conn: &mut Conn) {
    let pc_path = match std::env::var_os("OXIDE_TEST262_PC_WATCH") {
        Some(p) => PathBuf::from(p),
        None => return,
    };
    let content = match std::fs::read_to_string(&pc_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let last = content.lines().last().unwrap_or("").trim();
    if last.is_empty() {
        return;
    }
    let event = ShardEvent::Scene {
        line: last.to_string(),
        log_tail: None,
    };
    if let Err(e) = send_event_frame(conn, &event) {
        eprintln!("worker: failed to send scene frame: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// token 缺失时 worker 在连接前报错退出（不占连接）。
    #[test]
    fn worker_missing_token_exits_before_connect() {
        std::env::set_var("OXIDE_TEST262_WORKER_ENDPOINT", "127.0.0.1:1");
        std::env::remove_var("OXIDE_TEST262_WORKER_TOKEN");
        let config = RunConfig::new();
        assert!(!run_worker(&config), "token 缺失应报错退出");
    }
}
