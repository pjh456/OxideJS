//! 启动时 liveness 扫描：扫陈旧 sidecar 与 socket，僵尸态自动恢复。
//!
//! 关键约定：
//! - 扫描插在 `run_server` 启动序列第一步（`claim_sidecar` 之前、信号处理器
//!   注册之前）：认领裁决前清掉陈旧残留。僵尸态（sidecar 可解析、PID 活、
//!   启动时刻刻度匹配、socket 死）是 `claim_sidecar` 唯一保守拒绝阻塞再注册
//!   之态，本模块自动解开，不再依赖人工清理命令。
//! - socket 存活时一律无操作：存活 server 的处置归 `claim_sidecar` 裁决（同
//!   版本拒绝启动、异版本走交接）。自动扫描绝不杀存活 server——`decide` 表
//!   的 Kill 行人工清理语义，自动场景杀存活 server 等于抢占单例，违反单例
//!   交接协议。
//! - 只删 `config` 注入的 sidecar 与 socket 两条路径；日志文件不碰（跨重启
//!   保留的诊断工件，不在清理范围）。`--rm` 路径不调用本模块（不碰全局
//!   well-known 路径的不变量）。
//! - 杀进程唯一前置是启动时刻刻度匹配（沿用清理命令裁决）：刻度自开机单调
//!   递增，PID 复用必然失配，无新增误杀风险。
//! - 僵尸分支两段式：先有界等待 sidecar 消失（覆盖旧 server 正在退出序列中
//!   的窗口，排水安全超时 5 秒必在窗内），仍在则发 SIGTERM 等退出后删文件；
//!   杀进程失败不碰任何文件，映射保守拒绝并指向人工清理命令。

use std::path::Path;
use std::time::{Duration, Instant};

use super::cleanup::{self, Decision};
use super::sidecar;

/// 僵尸退出窗口：有界等待旧 server 的 sidecar 消失，10 秒（与 restart 臂等
/// 旧 server 退出的窗口同值），长于旧 server 的排水安全超时 5 秒。
const ZOMBIE_EXIT_WINDOW: Duration = Duration::from_secs(10);

/// 僵尸退出窗口轮询间隔：50 毫秒（与交接等待同值）。
const ZOMBIE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 扫描结果：三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessOutcome {
    /// 无操作：无残留，或 socket 存活（存活 server 归 `claim_sidecar` 裁决）。
    Noop,
    /// 已清理：陈旧文件已删（可能含杀僵尸进程）。
    Cleaned,
    /// 杀进程失败：SIGTERM 已发但进程在窗口内未退出，文件未触碰。
    KillFailed,
}

/// 启动时 liveness 扫描入口：扫 sidecar 与 socket，清理陈旧残留，僵尸态自动恢复。
///
/// # 步骤
/// 1. 采集五个探活信号（sidecar 可解析性、socket 存活、socket 文件存在、
///    PID 存活、刻度匹配）。
/// 2. socket 存活守卫：存活即无操作（自动扫描绝不杀存活 server）。
/// 3. 按 `decide` 裁决表分派：无操作 / 只删文件 / 僵尸分支 / 拒绝。
///
/// # 边界与前提
/// - 只删 `config` 注入的 sidecar 与 socket 两条路径；日志文件不碰。
/// - 杀进程等待取缺省 10 秒（长于 server 的在途排水安全超时 5 秒）。
///
/// # 副作用
/// - 可能向 sidecar 记录的进程发 SIGTERM；可能删除 socket 与 sidecar 文件。
///
/// # 注意事项
/// - 须在 `claim_sidecar` 之前调用（认领裁决前清掉陈旧状态）。
pub fn liveness_scan(sidecar_path: &Path, socket_path: &Path) -> LivenessOutcome {
    liveness_scan_with_waits(sidecar_path, socket_path, ZOMBIE_EXIT_WINDOW, cleanup::DEFAULT_KILL_WAIT)
}

/// 启动时 liveness 扫描（可注入两个超时，测试注入短窗口）。
///
/// # 步骤
/// 1. 采集信号：`read_identity`（缺失与损坏同归 `None`）、socket 文件存在、
///    `is_server_alive`（仅文件存在时探活）、`pid_alive`、`process_starttime`
///    与 sidecar 记录值比对（PID 死时匹配位恒假）。
/// 2. socket 存活守卫：存活即无操作。
/// 3. 按 `decide` 裁决表分派：
///    - `None` 无操作。
///    - `DeleteOnly` 删文件（socket 先、sidecar 后）。
///    - `Kill` 僵尸分支：有界等待 sidecar 消失，仍在则杀进程后删文件。
///    - `Refuse` 无操作（文件不碰，归 `claim_sidecar` 裁决）。
///
/// # 边界与前提
/// - `zombie_exit_window` 是僵尸分支等 sidecar 消失的窗口；`kill_wait` 是
///   SIGTERM 送达后等进程退出的窗口。
/// - 僵尸退出窗口长于旧 server 的排水安全超时（5 秒），退出序列中的旧 server
///   的 sidecar 必在窗口内消失，杀进程不触发。
///
/// # 副作用
/// - 可能向 sidecar 记录的进程发 SIGTERM；可能删除 socket 与 sidecar 文件。
/// - 杀进程决策与失败记入跨重启保留的日志文件（守护形态 stderr 是 null）。
pub fn liveness_scan_with_waits(
    sidecar_path: &Path, socket_path: &Path, zombie_exit_window: Duration, kill_wait: Duration,
) -> LivenessOutcome {
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

    // socket 存活守卫：存活 server 的处置归 `claim_sidecar` 裁决（同版本
    // AlreadyRunning 拒绝、异版本 VersionMismatch 走交接）。自动扫描绝不杀
    // 存活 server：`decide` 表的 Kill 行人工清理语义，杀存活 server 等于抢占，
    // 违反单例交接协议。
    if socket_alive {
        return LivenessOutcome::Noop;
    }

    // 按裁决表分派。
    match cleanup::decide(identity.as_ref(), socket_exists, socket_alive, pid_alive, starttime_match) {
        Decision::None => LivenessOutcome::Noop,
        Decision::DeleteOnly => {
            cleanup::remove_files(sidecar_path, socket_path);
            LivenessOutcome::Cleaned
        }
        Decision::Kill => {
            // Kill 决策必带可解析 sidecar（裁决表的两行 Kill 都在可解析分支），
            // 否则 pid 不可得，退回无操作。
            let Some(id) = identity else {
                return LivenessOutcome::Noop;
            };
            kill_zombie(&id, sidecar_path, socket_path, zombie_exit_window, kill_wait)
        }
        Decision::Refuse => LivenessOutcome::Noop,
    }
}

/// 僵尸分支：有界等待 sidecar 消失，仍在则杀进程后删文件。
///
/// # 步骤
/// 1. 有界等待 sidecar 消失（`zombie_exit_window`、50 毫秒轮询）：覆盖旧
///    server 正在退出序列中的窗口（排水安全超时 5 秒，sidecar 必在窗内消失）。
///    消失即无操作，`claim_sidecar` 的 O_EXCL 创建成功。
/// 2. 窗口后 sidecar 仍在：真僵尸（进程活着且不在退出序列，socket 被外部
///    删除）。发 SIGTERM 并有界等待退出（`kill_wait`）。
/// 3. 进程退出后删文件（尽力而为；旧 server 的退出序列自己删文件，此处兜底）。
///    进程在窗口内未退出（杀进程失败）：不触碰任何文件，返回 `KillFailed`。
///
/// # 边界与前提
/// - 杀进程唯一前置是刻度匹配（由调用方经 `decide` 保证）：PID 复用必失配。
///
/// # 副作用
/// - 可能向 sidecar 记录的进程发 SIGTERM；可能删除 socket 与 sidecar 文件。
/// - 杀进程决策与失败记入跨重启保留的日志文件。
fn kill_zombie(
    id: &sidecar::ServerIdentity, sidecar_path: &Path, socket_path: &Path, zombie_exit_window: Duration,
    kill_wait: Duration,
) -> LivenessOutcome {
    // 有界等待 sidecar 消失：覆盖旧 server 退出序列窗口（排水 5 秒，sidecar
    // 最后删除）。消失即无操作，`claim_sidecar` 的 O_EXCL 创建成功。
    let deadline = Instant::now() + zombie_exit_window;
    while sidecar_path.exists() {
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(ZOMBIE_POLL_INTERVAL);
    }
    if !sidecar_path.exists() {
        return LivenessOutcome::Noop;
    }

    // 窗口后 sidecar 仍在：真僵尸（进程活着且不在退出序列，socket 被外部删除）。
    // 发 SIGTERM 并有界等待退出。
    oxide_log::__log_event!(
        "oxide::kernel",
        oxide_log::tracing::Level::INFO,
        "启动 liveness 扫描：发现僵尸 server（进程号 {}），发送 SIGTERM",
        id.pid
    );
    if !cleanup::send_sigterm(id.pid) || !cleanup::wait_pid_exit(id.pid, kill_wait) {
        // 杀进程失败：不触碰任何文件，保守拒绝并指向人工清理命令。
        oxide_log::__log_event!(
            "oxide::kernel",
            oxide_log::tracing::Level::WARN,
            "启动 liveness 扫描：僵尸 server（进程号 {}）杀进程失败，文件未触碰，可用 `oxide server cleanup` 人工清理",
            id.pid
        );
        return LivenessOutcome::KillFailed;
    }

    // 进程已退出：删文件（尽力而为；旧 server 的退出序列自己删文件，此处兜底）。
    cleanup::remove_files(sidecar_path, socket_path);
    oxide_log::__log_event!(
        "oxide::kernel",
        oxide_log::tracing::Level::INFO,
        "启动 liveness 扫描：僵尸 server（进程号 {}）已清理，文件已删",
        id.pid
    );
    LivenessOutcome::Cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// temp_dir 下的唯一临时目录，退出时自动删除。
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("oxide_liveness_test_{}_{}", std::process::id(), ns));
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
    fn test_identity(pid: u32, starttime: u64, socket_path: &Path) -> sidecar::ServerIdentity {
        sidecar::ServerIdentity {
            pid,
            socket_path: socket_path.to_str().unwrap().to_string(),
            version: "0.0.0".into(),
            started_at: 0,
            starttime,
        }
    }

    /// 无残留：两文件都不存在，判无操作。
    #[test]
    fn no_residue_noop() {
        let dir = TestDir::new();
        let outcome = liveness_scan(&dir.path("sidecar.json"), &dir.path("server.sock"));
        assert_eq!(outcome, LivenessOutcome::Noop, "无文件应判无操作");
    }

    /// 孤儿 socket：无 sidecar、socket 文件存在但无监听者，socket 文件被删。
    #[test]
    fn orphan_socket_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        fs::write(&socket, b"stale").expect("写陈旧 socket 文件应成功");
        let outcome = liveness_scan(&sidecar, &socket);
        assert_eq!(outcome, LivenessOutcome::Cleaned, "孤儿 socket 文件应判已清理");
        assert!(!socket.exists(), "socket 文件应被删除");
        assert!(!sidecar.exists(), "sidecar 不应被创建");
    }

    /// 死 PID：sidecar 记死 PID、无 socket，两文件被删。
    #[test]
    fn dead_pid_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        sidecar::write_exclusive(&test_identity(dead_pid(), 0, &socket), &sidecar).expect("写 sidecar 应成功");
        let outcome = liveness_scan(&sidecar, &socket);
        assert_eq!(outcome, LivenessOutcome::Cleaned, "死 PID 残留应判已清理");
        assert!(!sidecar.exists(), "sidecar 应被删除");
        assert!(!socket.exists(), "socket 文件应保持不存在");
    }

    /// PID 复用：sidecar 记本进程 PID 加占位刻度（失配）、无 socket，文件被删，
    /// 本进程不受影响（无杀进程调用）。
    #[test]
    fn reused_pid_files_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        sidecar::write_exclusive(&test_identity(std::process::id(), 0, &socket), &sidecar).expect("写 sidecar 应成功");
        let outcome = liveness_scan(&sidecar, &socket);
        assert_eq!(outcome, LivenessOutcome::Cleaned, "刻度失配残留应判已清理");
        assert!(!sidecar.exists(), "sidecar 应被删除");
        assert!(sidecar::pid_alive(std::process::id()), "本进程应不受影响");
    }

    /// 损坏 sidecar 加陈旧 socket 文件：两文件被删。
    #[test]
    fn corrupted_sidecar_stale_socket_removed() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        fs::write(&sidecar, "not json at all").expect("写损坏文件应成功");
        fs::write(&socket, b"stale").expect("写陈旧 socket 文件应成功");
        let outcome = liveness_scan(&sidecar, &socket);
        assert_eq!(outcome, LivenessOutcome::Cleaned, "损坏 sidecar 加陈旧 socket 应判已清理");
        assert!(!sidecar.exists(), "sidecar 应被删除");
        assert!(!socket.exists(), "socket 文件应被删除");
    }

    /// socket 存活：绑定监听者加同版本 sidecar，无操作、两文件原样（自动场景
    /// 不杀存活 server）。
    #[test]
    fn live_socket_noop() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        sidecar::write_exclusive(&sidecar::ServerIdentity::new(socket.to_str().unwrap(), "0.0.0"), &sidecar)
            .expect("写 sidecar 应成功");
        let outcome = liveness_scan(&sidecar, &socket);
        assert_eq!(outcome, LivenessOutcome::Noop, "存活 socket 应判无操作");
        assert!(sidecar.exists(), "sidecar 应原样");
        assert!(socket.exists(), "socket 文件应原样");
        drop(listener);
    }

    /// 僵尸退出窗口：sidecar 记本进程 PID 加真实刻度、无 socket，并发线程在
    /// 等待窗口内删 sidecar，判无操作（不触发杀进程）。
    #[test]
    fn zombie_exit_window_noop() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // sidecar 记本进程 PID 加真实刻度（进程活且刻度匹配）、无 socket。
        let id = sidecar::ServerIdentity::new(socket.to_str().unwrap(), "0.0.0");
        sidecar::write_exclusive(&id, &sidecar).expect("写 sidecar 应成功");
        // 扫描线程：sidecar 存在时进入僵尸分支，有界等待 sidecar 消失。
        let sidecar_scan = sidecar.clone();
        let socket_scan = socket.clone();
        let scan_handle = std::thread::spawn(move || {
            liveness_scan_with_waits(&sidecar_scan, &socket_scan, Duration::from_secs(5), Duration::from_secs(1))
        });
        // 主线程在窗口内删 sidecar（模拟旧 server 退出序列），扫描应判无操作。
        std::thread::sleep(Duration::from_millis(200));
        let _ = fs::remove_file(&sidecar);
        let outcome = scan_handle.join().expect("扫描线程应正常退出");
        assert_eq!(outcome, LivenessOutcome::Noop, "sidecar 在窗口内消失应判无操作");
    }

    /// 僵尸杀进程：真实子进程（sleep 30）加其实测刻度写 sidecar、无 socket，
    /// 注入短超时，子进程被杀、文件被删，判已清理。
    #[test]
    fn zombie_kill_process() {
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
        // 注入短僵尸退出窗口（500 毫秒）加短杀进程等待（3 秒）：子进程存活，
        // sidecar 不消失，扫描等窗口后杀进程。
        let outcome = liveness_scan_with_waits(&sidecar, &socket, Duration::from_millis(500), Duration::from_secs(3));
        assert_eq!(outcome, LivenessOutcome::Cleaned, "刻度匹配的存活进程应被杀且文件被删");
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

    /// 杀进程失败：子进程忽略 SIGTERM（sh trap）、注入短超时，判杀进程失败、
    /// 文件原样、子进程存活（测试收尾清理子进程）。
    #[test]
    fn kill_failed_when_pid_ignores_term() {
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
        // 注入短僵尸退出窗口（500 毫秒）加短杀进程等待（300 毫秒）：子进程忽略
        // SIGTERM，扫描判杀进程失败，文件原样。
        let outcome =
            liveness_scan_with_waits(&sidecar, &socket, Duration::from_millis(500), Duration::from_millis(300));
        assert_eq!(outcome, LivenessOutcome::KillFailed, "忽略 SIGTERM 应判杀进程失败");
        assert!(sidecar.exists(), "sidecar 应原样");
        assert!(!socket.exists(), "socket 文件应保持不存在");
        assert!(sidecar::pid_alive(pid), "子进程应存活");
        let _ = child.kill();
        let _ = child.wait();
    }
}
