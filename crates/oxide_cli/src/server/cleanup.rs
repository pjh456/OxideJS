//! 人工兜底清理命令：扫 sidecar、探活、杀 PID、删孤儿文件。
//!
//! 处理两个既有路径留出的残留场景：认领的保守拒绝（进程活但 socket 死，
//! 证据不足以裁决）与交接超时（旧 server 挂起不响应 yield，文件原样保留）。
//!
//! 关键约定：
//! - 裁决表是纯函数 `decide`：五个探活信号（sidecar 可解析性、socket 存活、
//!   socket 文件存在、PID 存活、启动时刻刻度匹配）映射到四态决策（无操作 /
//!   只删文件 / 先杀后删 / 拒绝），执行器只做信号采集与动作落地。
//! - 杀 PID 的唯一前置是启动时刻刻度匹配：`/proc/<pid>/stat` 的刻度与 sidecar
//!   记录值一致才证明「这个进程就是写过 sidecar 的 server」；失配即 PID 已被
//!   无关进程复用，只删文件不杀进程。
//! - 删除定序：socket 先、sidecar 后（与交接退出序列一致，新 server 以
//!   sidecar 消失为接管信号，届时 socket 文件必已删除）。
//! - 两条保守拒绝底线：socket 存活但 PID 无法可靠确定、杀进程超时，均不碰
//!   任何文件（拒绝清理不会双注册、不会误删存活 server 的文件）。

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::sidecar::{self, ServerIdentity};

/// 缺省杀进程等待：10 秒，长于 server 的在途排水安全超时 5 秒，正常优雅
/// 退出序列（退出 accept 循环、排水、join worker、删文件）必在窗口内完成。
pub const DEFAULT_KILL_WAIT: Duration = Duration::from_secs(10);

/// 清理结果：四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// 无残留：sidecar 与 socket 文件都不存在。
    Clean,
    /// 残留已清理：孤儿文件已删（可能含杀进程）。
    Removed,
    /// 存活 server 存在且 PID 无法可靠确定，未触碰文件，需人工检查。
    RefusedLiveServer,
    /// SIGTERM 已发但进程在等待窗口内未退出，未触碰文件。
    KillFailed,
}

/// 裁决表决策：四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// 无残留，无操作。
    None,
    /// 只删文件（无存活监听者，无需杀进程）。
    DeleteOnly,
    /// 先杀进程后删文件（PID 活且启动时刻刻度匹配）。
    Kill,
    /// 拒绝触碰文件（存活 server 存在且 PID 无法可靠确定）。
    Refuse,
}

/// 裁决表：五个探活信号映射到四态决策。
///
/// # 步骤
/// 1. socket 存活：存在存活 server。sidecar 可解析且 PID 活且刻度匹配即
///    本进程是真实 server，先杀后删；其余情形 PID 无法可靠确定，拒绝。
/// 2. socket 死：无存活监听者。sidecar 缺失或损坏时 socket 文件存在即删
///    孤儿文件、否则无残留；sidecar 可解析时 PID 活且刻度匹配（socket 被
///    外部删除）先杀后删，PID 死或刻度失配（复用）只删文件。
///
/// # 边界与前提
/// - `socket_alive` 蕴含 `socket_exists`，调用方负责采集一致的信号。
/// - 损坏 sidecar 与缺失同归 `None`；刻度匹配位仅在 sidecar 可解析且
///   PID 活时有意义。
///
/// # 注意事项
/// - 纯函数，无 I/O，全部分支可单测。
fn decide(
    sidecar: Option<&ServerIdentity>, socket_exists: bool, socket_alive: bool, pid_alive: bool, starttime_match: bool,
) -> Decision {
    // socket 存活：只有 sidecar 可解析、PID 活且刻度匹配才证明本进程是
    // 真实 server，其余情形真实 server 的 PID 未知，拒绝触碰文件。
    if socket_alive {
        if sidecar.is_some() && pid_alive && starttime_match {
            return Decision::Kill;
        }
        return Decision::Refuse;
    }

    // socket 死：无存活监听者，按 sidecar 可读性分支。
    match sidecar {
        None => {
            // sidecar 缺失或损坏：socket 文件存在即孤儿，删；否则无残留。
            if socket_exists {
                Decision::DeleteOnly
            } else {
                Decision::None
            }
        }
        Some(_) => {
            // PID 活且刻度匹配：socket 被外部删除，server 仍存活，先杀后删；
            // PID 死或刻度失配（复用）：崩溃残留，只删文件。
            if pid_alive && starttime_match {
                Decision::Kill
            } else {
                Decision::DeleteOnly
            }
        }
    }
}

/// 清理入口：扫 sidecar、探活、杀 PID、删孤儿文件。
///
/// # 步骤
/// 1. 采集五个探活信号（sidecar 可解析性、socket 存活、socket 文件存在、
///    PID 存活、刻度匹配）。
/// 2. 调 `decide` 得决策。
/// 3. 按决策落地（无操作 / 只删文件 / 先杀后删 / 拒绝）。
///
/// # 边界与前提
/// - 杀进程等待取缺省 10 秒（长于 server 的在途排水安全超时 5 秒）。
///
/// # 副作用
/// - 可能向 sidecar 记录的进程发 SIGTERM；可能删除 socket 与 sidecar 文件。
///
/// # 注意事项
/// - 两条保守拒绝底线：socket 存活但 PID 无法可靠确定、杀进程超时，均不
///   触碰文件。
pub fn cleanup(sidecar_path: &Path, socket_path: &Path) -> CleanupOutcome {
    cleanup_with_wait(sidecar_path, socket_path, DEFAULT_KILL_WAIT)
}

/// 清理入口（可注入杀进程等待，测试注入短超时）。
///
/// # 步骤
/// 1. 采集信号：`read_identity`（缺失与损坏同归 `None`）、socket 文件存在、
///    `is_server_alive`（仅文件存在时探活）、`pid_alive`、`process_starttime`
///    与 sidecar 记录值比对（PID 死时匹配位恒假）。
/// 2. 调 `decide` 得决策。
/// 3. 按决策落地：无操作返回 `Clean`；只删文件返回 `Removed`；先杀后删在
///    信号发送失败或等待超时时返回 `KillFailed`（文件不碰）、成功返回
///    `Removed`；拒绝返回 `RefusedLiveServer`（文件不碰）。
///
/// # 边界与前提
/// - `kill_wait` 是 SIGTERM 送达后等进程退出的窗口；server 走信号处理的
///   优雅退出序列，正常必在窗口内完成。
///
/// # 副作用
/// - 可能向 sidecar 记录的进程发 SIGTERM；可能删除 socket 与 sidecar 文件。
pub fn cleanup_with_wait(sidecar_path: &Path, socket_path: &Path, kill_wait: Duration) -> CleanupOutcome {
    // 采集信号：sidecar 可解析性（缺失与损坏同归 None）、socket 存活、
    // PID 存活与刻度匹配（PID 死时匹配位恒假）。
    let identity = sidecar::read_identity(sidecar_path);
    let socket_exists = socket_path.exists();
    let socket_alive = socket_exists && sidecar::is_server_alive(socket_path);
    let (pid_alive, starttime_match) = match &identity {
        Some(id) => {
            let alive = sidecar::pid_alive(id.pid);
            (alive, alive && sidecar::process_starttime(id.pid) == Some(id.starttime))
        }
        None => (false, false),
    };

    // 裁决：五个信号映射到四态决策。
    let decision = decide(identity.as_ref(), socket_exists, socket_alive, pid_alive, starttime_match);

    // 按决策落地。
    match decision {
        Decision::None => CleanupOutcome::Clean,
        Decision::DeleteOnly => {
            remove_files(sidecar_path, socket_path);
            CleanupOutcome::Removed
        }
        Decision::Kill => {
            // Kill 决策必带可解析 sidecar（裁决表的两行 Kill 都在可解析分支），
            // 否则 pid 不可得，退回拒绝。
            let Some(id) = identity else {
                return CleanupOutcome::RefusedLiveServer;
            };
            // 信号未确认送达或等待超时均判杀进程失败，文件不碰。
            if !send_sigterm(id.pid) || !wait_pid_exit(id.pid, kill_wait) {
                return CleanupOutcome::KillFailed;
            }
            // server 自身退出序列已删文件时删除报错被忽略（尽力而为的残留兜底）。
            remove_files(sidecar_path, socket_path);
            CleanupOutcome::Removed
        }
        Decision::Refuse => CleanupOutcome::RefusedLiveServer,
    }
}

/// well-known 路径清理入口：解析 well-known sidecar 与 socket 路径后调
/// `cleanup`。
///
/// # 注意事项
/// - CLI 分派臂与启动 liveness 扫描的稳定入口。
pub fn well_known_cleanup() -> CleanupOutcome {
    cleanup(&sidecar::well_known_sidecar_path(), &sidecar::well_known_socket_path())
}

/// 发送 SIGTERM：`kill -TERM`，与 `pid_alive` 的 `kill -0` 同一 `Command` 路径。
///
/// # 边界与前提
/// - `kill` 二进制缺失或进程不存在时返回 `false`（信号未确认送达，调用方
///   保守判杀进程失败）。
fn send_sigterm(pid: u32) -> bool {
    Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 轮询进程退出：100 毫秒间隔 `pid_alive` 检查，退出返回真，超时返回假。
fn wait_pid_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !sidecar::pid_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 删除残留文件：socket 先、sidecar 后（与交接退出序列同一定序：新 server
/// 以 sidecar 消失为接管信号，届时 socket 文件必已删除）。
///
/// # 副作用
/// - 可能删除 socket 文件与 sidecar 文件；文件已不存在不视为错误（server
///   自身退出序列可能已删过）。
fn remove_files(sidecar_path: &Path, socket_path: &Path) {
    let _ = fs::remove_file(socket_path);
    let _ = fs::remove_file(sidecar_path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// temp_dir 下的唯一临时目录，退出时自动删除。
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("oxide_cleanup_test_{}_{}", std::process::id(), ns));
            fs::create_dir_all(&dir).expect("测试目录创建失败");
            TestDir(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 保证不存在的 pid：pid_max 之上永远不会被分配。
    fn dead_pid() -> u32 {
        let pid_max = fs::read_to_string("/proc/sys/kernel/pid_max")
            .expect("读取 pid_max 失败")
            .trim()
            .parse::<u32>()
            .expect("解析 pid_max 失败");
        pid_max + 1
    }

    /// 测试身份：pid 与刻度可控，其余字段为占位值。
    fn test_identity(pid: u32, starttime: u64, socket_path: &Path) -> ServerIdentity {
        ServerIdentity {
            pid,
            socket_path: socket_path.to_str().unwrap().to_string(),
            version: "0.0.0".into(),
            started_at: 0,
            starttime,
        }
    }

    /// 裁决表一行：五个探活信号与期望决策。
    struct DecideCase {
        sidecar: Option<&'static ServerIdentity>,
        socket_exists: bool,
        socket_alive: bool,
        pid_alive: bool,
        tick_match: bool,
        expected: Decision,
    }

    /// 按五信号顺序构造裁决表一行。
    fn row(
        sidecar: Option<&'static ServerIdentity>, socket_exists: bool, socket_alive: bool, pid_alive: bool,
        tick_match: bool, expected: Decision,
    ) -> DecideCase {
        DecideCase {
            sidecar,
            socket_exists,
            socket_alive,
            pid_alive,
            tick_match,
            expected,
        }
    }

    /// 裁决表：八行加未覆盖分支（可解析加存活加 PID 死归拒绝），五信号逐行注入。
    #[test]
    fn decide_table() {
        let identity: &'static ServerIdentity = Box::leak(Box::new(test_identity(1, 0, Path::new("/tmp/decide.sock"))));
        let cases = [
            // (sidecar, socket 存在, socket 存活, PID 活, 刻度匹配, 决策)
            row(None, false, false, false, false, Decision::None),
            row(None, true, false, false, false, Decision::DeleteOnly),
            row(None, true, true, false, false, Decision::Refuse),
            row(Some(identity), true, true, true, true, Decision::Kill),
            row(Some(identity), true, true, true, false, Decision::Refuse),
            row(Some(identity), false, false, false, false, Decision::DeleteOnly),
            row(Some(identity), false, false, true, true, Decision::Kill),
            row(Some(identity), false, false, true, false, Decision::DeleteOnly),
            row(Some(identity), true, true, false, false, Decision::Refuse),
        ];
        for case in &cases {
            let got = decide(case.sidecar, case.socket_exists, case.socket_alive, case.pid_alive, case.tick_match);
            assert_eq!(got, case.expected, "裁决表行决策不符");
        }
    }

    /// 无残留：两文件均不存在，判 Clean。
    #[test]
    fn cleanup_no_files_clean() {
        let dir = TestDir::new();
        let outcome = cleanup(&dir.path("sidecar.json"), &dir.path("server.sock"));
        assert_eq!(outcome, CleanupOutcome::Clean, "无文件应判无残留");
    }

    /// 孤儿 socket 文件：无 sidecar、文件存在但无监听者，判 Removed，socket 文件被删。
    #[test]
    fn cleanup_orphan_socket_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        fs::write(&socket, b"stale").expect("写陈旧 socket 文件应成功");
        let outcome = cleanup(&sidecar, &socket);
        assert_eq!(outcome, CleanupOutcome::Removed, "孤儿 socket 文件应判已清理");
        assert!(!socket.exists(), "socket 文件应被删除");
        assert!(!sidecar.exists(), "sidecar 不应被创建");
    }

    /// 崩溃残留：sidecar 记死 PID、无 socket，判 Removed，sidecar 被删。
    #[test]
    fn cleanup_dead_pid_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        sidecar::write_exclusive(&test_identity(dead_pid(), 0, &socket), &sidecar).expect("写 sidecar 应成功");
        let outcome = cleanup(&sidecar, &socket);
        assert_eq!(outcome, CleanupOutcome::Removed, "死 PID 残留应判已清理");
        assert!(!sidecar.exists(), "sidecar 应被删除");
    }

    /// 损坏 sidecar 加陈旧 socket 文件：判 Removed，两文件被删。
    #[test]
    fn cleanup_corrupted_dead_socket_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        fs::write(&sidecar, "not json at all").expect("写损坏文件应成功");
        fs::write(&socket, b"stale").expect("写陈旧 socket 文件应成功");
        let outcome = cleanup(&sidecar, &socket);
        assert_eq!(outcome, CleanupOutcome::Removed, "损坏 sidecar 加陈旧 socket 应判已清理");
        assert!(!sidecar.exists(), "sidecar 应被删除");
        assert!(!socket.exists(), "socket 文件应被删除");
    }

    /// 损坏 sidecar 加存活 socket：PID 未知，拒绝，两文件原样。
    #[test]
    fn cleanup_corrupted_live_socket_refused() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        fs::write(&sidecar, "not json at all").expect("写损坏文件应成功");
        let outcome = cleanup(&sidecar, &socket);
        assert_eq!(outcome, CleanupOutcome::RefusedLiveServer, "损坏 sidecar 加存活 socket 应判拒绝");
        assert!(sidecar.exists(), "sidecar 应原样");
        assert!(socket.exists(), "socket 文件应原样");
        drop(listener);
    }

    /// PID 复用：sidecar 记本进程 PID 加占位刻度（失配）、无 socket，判 Removed，
    /// 文件被删，本进程不受影响（无杀进程调用）。
    #[test]
    fn cleanup_reused_pid_files_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        sidecar::write_exclusive(&test_identity(std::process::id(), 0, &socket), &sidecar).expect("写 sidecar 应成功");
        let outcome = cleanup(&sidecar, &socket);
        assert_eq!(outcome, CleanupOutcome::Removed, "刻度失配残留应判已清理");
        assert!(!sidecar.exists(), "sidecar 应被删除");
        assert!(sidecar::pid_alive(std::process::id()), "本进程应不受影响");
    }

    /// 存活 socket 加刻度失配：存活 server 存在且 PID 无法可靠确定，拒绝，
    /// 文件原样（本进程 PID 加占位刻度，无杀进程调用）。
    #[test]
    fn cleanup_live_socket_reused_pid_refused() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        sidecar::write_exclusive(&test_identity(std::process::id(), 0, &socket), &sidecar).expect("写 sidecar 应成功");
        let outcome = cleanup(&sidecar, &socket);
        assert_eq!(outcome, CleanupOutcome::RefusedLiveServer, "存活 socket 加刻度失配应判拒绝");
        assert!(sidecar.exists(), "sidecar 应原样");
        assert!(socket.exists(), "socket 文件应原样");
        drop(listener);
    }

    /// 先杀后删：起真实子进程（sleep 30），以其实测 PID 与刻度写 sidecar、无
    /// socket，注入 3 秒超时，判 Removed，子进程退出、文件被删。
    #[test]
    fn cleanup_kills_matching_pid() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let mut child = Command::new("sleep").arg("30").spawn().expect("子进程启动应成功");
        let pid = child.id();
        // 测试进程是父进程：被杀的子进程在回收前是僵尸，kill -0 恒返回真，
        // 需回收线程并发收尸，探活才会在窗口内转假。
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });
        let id = test_identity(pid, sidecar::process_starttime(pid).expect("子进程启动刻度应可读"), &socket);
        sidecar::write_exclusive(&id, &sidecar).expect("写 sidecar 应成功");
        let outcome = cleanup_with_wait(&sidecar, &socket, Duration::from_secs(3));
        assert_eq!(outcome, CleanupOutcome::Removed, "刻度匹配的存活进程应被杀且文件被删");
        // 有界等待回收线程：子进程被杀后收尸即退出，不无限阻塞。
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reaper.is_finished() {
            assert!(Instant::now() < deadline, "回收线程应有界退出");
            std::thread::sleep(Duration::from_millis(10));
        }
        reaper.join().expect("回收线程应正常退出");
        assert!(!sidecar::pid_alive(pid), "子进程应已退出");
        assert!(!sidecar.exists(), "sidecar 应被删除");
    }

    /// 杀进程失败：子进程忽略 SIGTERM（sh trap），300 毫秒超时判 KillFailed，
    /// 文件原样，子进程存活（测试收尾清理子进程）。
    #[test]
    fn cleanup_kill_failed_when_pid_ignores_term() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; sleep 30"])
            .spawn()
            .expect("子进程启动应成功");
        let pid = child.id();
        let id = test_identity(pid, sidecar::process_starttime(pid).expect("子进程启动刻度应可读"), &socket);
        sidecar::write_exclusive(&id, &sidecar).expect("写 sidecar 应成功");
        let outcome = cleanup_with_wait(&sidecar, &socket, Duration::from_millis(300));
        assert_eq!(outcome, CleanupOutcome::KillFailed, "忽略 SIGTERM 应判杀进程失败");
        assert!(sidecar.exists(), "sidecar 应原样");
        assert!(!socket.exists(), "socket 文件应保持不存在");
        assert!(sidecar::pid_alive(pid), "子进程应存活");
        let _ = child.kill();
        let _ = child.wait();
    }
}
