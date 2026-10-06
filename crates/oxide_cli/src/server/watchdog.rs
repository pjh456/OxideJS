//! watchdog：持久 server 的前台监控进程，崩溃后自动重启。
//!
//! 关键约定：
//! - release 模式 `panic=abort` 使 `catch_unwind` 失效，server 崩溃后进程直接
//!   消亡、文件残留，watchdog 是崩溃后恢复的唯一自动手段。
//! - 裁决信号取自既有工件加子进程收领：子进程是监控对象时，其退出状态是
//!   死亡的权威信号（被杀的子进程是僵尸，PID 探活对僵尸误判活）；子进程在
//!   认领阶段被拒时监控对象是既有 server，按 sidecar 存在性、PID 与启动时
//!   刻度（liveness 原语，排 PID 复用误判）三信号裁决；日志文件（跨重启的
//!   诊断工件）供崩溃诊断打印。
//! - watchdog 自身不注册身份、不写任何文件；watchdog 消亡后无人自动恢复
//!   （纯标准库轮询进程，不建内核、不建池、不执行引擎代码，崩溃风险极低；
//!   server 是脱离进程，watchdog 退出或消亡都不影响 server 运行），人工重跑
//!   `oxide server watchdog` 即接管（就绪探测看到存活 server 即接管监控）。
//! - SIGINT/SIGTERM 时先尽力向 server 发关闭请求（优雅退出、删文件）再退 0
//!   ——停 watchdog 的语义是「停掉整个监控栈」，不留下无人监控的 server。

use std::path::Path;
use std::process::{ExitCode, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::client::send_control_request;
use super::log as server_log;
use super::protocol::ServerRequest;
use super::sidecar::{self, ServerIdentity};
use super::spawn::{spawn_detached_server, wait_server_ready};

/// 监控循环轮询间隔：500 毫秒（与日志跟踪的轮询间隔同值）。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 崩溃重启退避时长：1 秒。
const CRASH_BACKOFF: Duration = Duration::from_secs(1);

/// 崩溃预算窗口：60 秒。
const CRASH_WINDOW: Duration = Duration::from_secs(60);

/// 崩溃预算上限：窗口内 5 次崩溃。
const CRASH_LIMIT: usize = 5;

/// 崩溃诊断打印的日志尾部行数：20 行。
const CRASH_LOG_LINES: usize = 20;

/// 就绪探测截止：10 秒（与 start / restart 路径同值）。
const READY_DEADLINE: Duration = Duration::from_secs(10);

/// server 状态三态：存活、崩溃、已退出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchState {
    /// 存活：PID 活且启动时刻刻度与 sidecar 记录匹配。
    Alive,
    /// 崩溃：sidecar 残留且 PID 死，或 PID 活但刻度失配（PID 已被复用）。
    Crash,
    /// 已退出：sidecar 消失（优雅退出序列的最后删除项，或外部清理）。
    Exited,
}

/// 裁决表：sidecar 可读性、PID 存活、刻度匹配三信号映射三态。
///
/// # 边界与前提
/// - 纯函数无 I/O，信号由调用方采集（与 cleanup 的 decide 同型）。
pub fn decide(identity: Option<&ServerIdentity>, pid_alive: bool, starttime_match: bool) -> WatchState {
    match identity {
        None => WatchState::Exited,
        Some(_) if !pid_alive => WatchState::Crash,
        Some(_) if starttime_match => WatchState::Alive,
        Some(_) => WatchState::Crash,
    }
}

/// 崩溃预算：`window` 滑动窗口内至多 `limit` 次崩溃，超出即放弃自动重启。
///
/// 窗口语义：窗口内崩溃次数超过上限即判 server 不稳定，继续重启只会空转
/// （典型场景：二进制损坏导致每次启动即崩溃，但每次都通过了就绪探测）。
#[derive(Debug)]
pub struct CrashBudget {
    window: Duration,
    limit: usize,
    times: Vec<Instant>,
}

impl CrashBudget {
    /// 默认预算：60 秒窗口、5 次崩溃。
    pub fn new() -> Self {
        Self::with(CRASH_WINDOW, CRASH_LIMIT)
    }

    /// 指定窗口与上限构造预算（生产取默认值，测试注入短窗口）。
    pub fn with(window: Duration, limit: usize) -> Self {
        CrashBudget {
            window,
            limit,
            times: Vec::new(),
        }
    }

    /// 记录一次崩溃：清掉窗口外条目后追加，返回预算是否耗尽。
    ///
    /// # 边界与前提
    /// - 耗尽是幂等的：耗尽后继续记录仍返回耗尽。
    pub fn record(&mut self, now: Instant) -> bool {
        self.times.retain(|t| now.duration_since(*t) <= self.window);
        self.times.push(now);
        self.times.len() > self.limit
    }
}

impl Default for CrashBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// watchdog 主循环：拉起 server、就绪探测、监控循环、崩溃重启、信号退出。
///
/// # 步骤
/// 1. 注册 ctrlc 处理器置位停止标志（SIGINT/SIGTERM，termination 特性）。
///    须在初始 spawn 之前注册：sidecar 出现早于 socket 绑定，外部信号可能
///    在就绪探测期间到达，处理器未就位时走默认终止行为。
/// 2. 初始 spawn（共享入口）加就绪探测（10 秒），失败打印退 1；成功打印
///    监控开始标记行（外部崩溃模拟与信号在此之后才安全）。
/// 3. 监控循环（500 毫秒间隔）：停止标志置位即转优雅关闭；收领子进程
///    （被杀的子进程是僵尸，PID 探活对僵尸误判活，子进程退出状态是死亡的
///    权威信号）；子进程即监控对象时按其存活判 Alive、已退出按 sidecar
///    残留判 Crash；子进程在认领阶段被拒时监控对象是既有 server，按 decide
///    三信号裁决；sidecar 连续两次（1 秒）判缺失即 server 已正常退出、退 0。
/// 4. 崩溃处理：打印崩溃提示（含进程号）、读日志文件最后 20 行打印、记预算
///    （耗尽则打印放弃提示退 1）、退避 1 秒、经同一 spawn 入口重启加就绪
///    探测（失败打印退 1）、继续循环。
/// 5. 优雅关闭：尽力发关闭请求（忽略错误，server 可能已死），打印退 0。
///
/// # 边界与前提
/// - sidecar 缺失须连续两次轮询（1 秒）确认才判 Exited：新 server 的认领
///   路径先删旧 sidecar 再重写，删除与重写之间是微秒级窗口，单次轮询落入
///   窗口会误判「server 已正常退出」而静默退出、留下无人监控的 server；
///   两次确认把该窗口压到实际不可达。
/// - 已有存活 server 时再跑 watchdog：spawn 的子进程在认领阶段被拒（同版本
///   判 `AlreadyRunning`，退 1、不触碰文件），就绪探测看到第一个 server 即
///   通过，watchdog 监控的是第一个 server（与双启动幂等同一语义，天然安全）。
/// - 新 server 启动即崩溃（绑定前消亡、sidecar 从未出现）：就绪探测 10 秒
///   超时后 watchdog 退 1（自限，不形成无限循环），消息带 `oxide server
///   cleanup` 人工检查提示。
///
/// # 副作用
/// - spawn 脱离的 server 子进程；崩溃后经同一入口重启。
/// - SIGINT/SIGTERM 时尽力向 server 发关闭请求。
pub fn run_watchdog(workers: Option<u32>) -> ExitCode {
    // 先注册信号处理器：信号到达时置位停止标志（处理器体只做一次原子写）。
    // 须在初始 spawn 之前注册：sidecar 出现早于 socket 绑定，外部信号可能
    // 在就绪探测期间到达，处理器未就位时 SIGINT 走默认终止行为。
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    if let Err(err) = ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    }) {
        eprintln!("注册信号处理器失败：{err}");
        return ExitCode::FAILURE;
    }

    // 初始 spawn 与就绪探测：失败即退 1，不进入监控循环。
    let mut child = match spawn_detached_server(workers) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("spawn server 进程失败：{err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(msg) = wait_server_ready(READY_DEADLINE) {
        eprintln!("{msg}（可用 `oxide server cleanup` 人工检查）");
        return ExitCode::FAILURE;
    }
    // 初始就绪探测通过：外部信号（崩溃模拟、SIGINT）在此之后才安全，
    // 该行同时是监控循环开始的标记。
    println!("server 已就绪，watchdog 开始监控");

    let mut budget = CrashBudget::new();
    let sidecar_path = sidecar::well_known_sidecar_path();
    let log_path = sidecar::well_known_log_path();
    let mut missing_streak = 0usize;
    // 子进程退出状态：收领后只记一次（try_wait 收领后不再重复出状态）。
    let mut child_exited: Option<ExitStatus> = None;

    loop {
        // 停止标志置位：转优雅关闭（尽力停 server，退 0）。
        if stop.load(Ordering::SeqCst) {
            return graceful_stop();
        }
        std::thread::sleep(POLL_INTERVAL);

        // 收领子进程：子进程是监控对象时，其退出状态是死亡的权威信号
        // （被杀的子进程是僵尸，kill -0 对僵尸返回活，PID 探活会误判）。
        if child_exited.is_none() {
            if let Some(status) = child.try_wait().ok().flatten() {
                child_exited = Some(status);
            }
        }

        // 读 sidecar 采集信号：缺失要连续两次确认（认领路径的删旧写新窗口
        // 是微秒级，单次轮询落入窗口会误判正常退出）。
        let identity = sidecar::read_identity(&sidecar_path);
        let state = match &identity {
            None => {
                missing_streak += 1;
                if missing_streak >= 2 {
                    println!("server 已正常退出，watchdog 退出");
                    return ExitCode::SUCCESS;
                }
                continue;
            }
            Some(id) => {
                missing_streak = 0;
                if id.pid == child.id() {
                    // 子进程即监控对象：存活判 Alive，已退出按 sidecar 残留
                    // 区分崩溃与正常退出。
                    match child_exited {
                        Some(_) => WatchState::Crash,
                        None => WatchState::Alive,
                    }
                } else {
                    // 子进程在认领阶段被拒（已有存活 server）：监控对象是
                    // 既有 server，按三信号裁决。
                    let pid_alive = sidecar::pid_alive(id.pid);
                    let starttime_match = sidecar::process_starttime(id.pid) == Some(id.starttime);
                    decide(Some(id), pid_alive, starttime_match)
                }
            }
        };

        match state {
            WatchState::Alive => {}
            WatchState::Crash => {
                // 崩溃处理：诊断打印、记预算、退避、经同一入口重启。
                // 崩溃态下 sidecar 必可读（Crash 只从 Some 分支产生），进程号
                // 取 sidecar 记录；读不到时以 0 占位（防御性兜底，不 panic）。
                let pid = identity.as_ref().map(|id| id.pid).unwrap_or(0);
                println!("server 已崩溃（进程号 {pid}），准备重启");
                print_crash_log_tail(&log_path);
                if budget.record(Instant::now()) {
                    eprintln!(
                        "崩溃预算耗尽（{} 秒窗口内 {} 次崩溃），放弃自动重启，可用 `oxide server cleanup` 检查",
                        CRASH_WINDOW.as_secs(),
                        CRASH_LIMIT
                    );
                    return ExitCode::FAILURE;
                }
                std::thread::sleep(CRASH_BACKOFF);
                match spawn_detached_server(workers) {
                    Ok(new_child) => {
                        child = new_child;
                        child_exited = None;
                    }
                    Err(err) => {
                        eprintln!("重启 server 失败：{err}");
                        return ExitCode::FAILURE;
                    }
                }
                if let Err(msg) = wait_server_ready(READY_DEADLINE) {
                    eprintln!("新 {msg}（可用 `oxide server cleanup` 人工检查）");
                    return ExitCode::FAILURE;
                }
                // 重启就绪探测通过：新 server 已可服务，外部操作（关闭请求）
                // 在此之后才安全；该行与初始标记同文，是监控循环恢复的标记。
                println!("server 已就绪，watchdog 开始监控");
            }
            WatchState::Exited => {
                // 防御性兜底：sidecar 缺失分支已两次确认后返回，到达此处即
                // 信号采集与裁决不一致。
                println!("server 已正常退出，watchdog 退出");
                return ExitCode::SUCCESS;
            }
        }
    }
}

/// 打印日志文件最后 20 行（崩溃诊断）；文件缺失或为空不视为错误。
///
/// # 边界与前提
/// - server 可能在写日志前就崩了，空日志与缺失文件都只跳过打印。
fn print_crash_log_tail(log_path: &Path) {
    let lines = server_log::read_log(log_path, None, Some(CRASH_LOG_LINES)).unwrap_or_default();
    if lines.is_empty() {
        return;
    }
    println!("日志尾部（最后 {CRASH_LOG_LINES} 行）：");
    for line in lines {
        println!("{line}");
    }
}

/// 优雅关闭：尽力向 server 发关闭请求（忽略错误，server 可能已死），打印退 0。
fn graceful_stop() -> ExitCode {
    let _ = send_control_request(&ServerRequest::Shutdown);
    println!("server 已停止，watchdog 退出");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试身份：字段为占位值（decide 只看 Option 与两个布尔信号）。
    fn test_identity() -> ServerIdentity {
        ServerIdentity {
            pid: 1,
            socket_path: "/tmp/oxide-test.sock".into(),
            version: "0.0.0".into(),
            started_at: 0,
            starttime: 0,
        }
    }

    /// decide 四行裁决表：sidecar 缺失判 Exited、PID 死判 Crash、PID 活刻度
    /// 匹配判 Alive、PID 活刻度失配判 Crash。
    #[test]
    fn decide_four_row_table() {
        let id = test_identity();
        assert_eq!(decide(None, false, false), WatchState::Exited, "sidecar 缺失应判 Exited");
        assert_eq!(decide(Some(&id), false, false), WatchState::Crash, "PID 死应判 Crash");
        assert_eq!(decide(Some(&id), true, true), WatchState::Alive, "PID 活刻度匹配应判 Alive");
        assert_eq!(decide(Some(&id), true, false), WatchState::Crash, "PID 活刻度失配应判 Crash");
    }

    /// 预算窗口语义：上限内未耗尽，窗口外条目被清后不再算入。
    #[test]
    fn budget_window_expiry() {
        let mut budget = CrashBudget::with(Duration::from_millis(100), 2);
        let t0 = Instant::now();
        assert!(!budget.record(t0), "首次记录不应耗尽");
        assert!(!budget.record(t0), "窗口内未超上限不应耗尽");
        // 窗口过期后旧条目被清：新记录不再与旧条目叠加。
        std::thread::sleep(Duration::from_millis(150));
        let t1 = t0 + Duration::from_millis(150);
        assert!(!budget.record(t1), "窗口外条目应被清，不应耗尽");
    }

    /// 第 6 次崩溃判耗尽（默认窗口 5 次上限）。
    #[test]
    fn budget_exhausted_on_sixth() {
        let mut budget = CrashBudget::new();
        let start = Instant::now();
        for _ in 0..CRASH_LIMIT {
            assert!(!budget.record(start), "上限内不应耗尽");
        }
        assert!(budget.record(start), "第 6 次应判耗尽");
    }

    /// 预算耗尽后的 record 仍返回耗尽（幂等）。
    #[test]
    fn budget_exhausted_is_idempotent() {
        let mut budget = CrashBudget::with(Duration::from_secs(60), 2);
        let start = Instant::now();
        budget.record(start);
        budget.record(start);
        assert!(budget.record(start), "第 3 次应判耗尽");
        assert!(budget.record(start), "耗尽后 record 应仍返回耗尽");
    }

    /// 记录时刻单调推进时窗口滑动：仅窗口内条目计入。
    #[test]
    fn budget_sliding_window_counts_only_in_window() {
        let mut budget = CrashBudget::with(Duration::from_millis(100), 3);
        let t0 = Instant::now();
        budget.record(t0);
        budget.record(t0);
        std::thread::sleep(Duration::from_millis(150));
        let t1 = t0 + Duration::from_millis(150);
        // 窗口外旧条目被清：窗口内 3 条恰达上限不耗尽，第 4 条耗尽。
        for _ in 0..3 {
            assert!(!budget.record(t1), "窗口内未超上限不应耗尽");
        }
        assert!(budget.record(t1), "窗口内第 4 条应判耗尽");
    }
}
