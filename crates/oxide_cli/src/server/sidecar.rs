//! server 身份注册：sidecar 文件（单行 JSON）与 liveness 检查原语。
//!
//! 关键约定：
//! - sidecar 是启动时写入一次的静态身份快照，不记池状态（动态状态由控制协议
//!   实时查询，写进 sidecar 只会过期）。
//! - 单例性由内核 O_EXCL 创建标志保证（文件已存在时创建失败），不需要锁。
//! - liveness 以 socket 连接探活为唯一裁决（内核只把连接完成给存活监听者），
//!   PID 探活为次信号，PID 复用以启动时刻刻度交叉核对排除。
//! - --rm 模式走独立分支：生成唯一 socket 路径、不写 sidecar、不碰全局
//!   well-known 路径；该不变量由运行路径执行。
//! - 日志文件由 sidecar 路径按扩展名派生（`.log`），与 sidecar、socket 同主名
//!   同目录；有意不随退出删除（跨重启的诊断工件），不在清理命令范围内。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// server 身份快照：单行 JSON sidecar 文件，启动时写入一次。
///
/// 字段：
/// - `pid`：server 进程标识符，清理命令杀进程用。
/// - `socket_path`：well-known socket 的绝对路径，控制客户端连接用。
/// - `version`：构建期版本，版本交接时比对用。
/// - `started_at`：Unix 秒，人读展示用。
/// - `starttime`：`/proc/<pid>/stat` 的启动时刻刻度（自开机单调递增），
///   PID 复用判定用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub pid: u32,
    pub socket_path: String,
    pub version: String,
    pub started_at: u64,
    pub starttime: u64,
}

impl ServerIdentity {
    /// 构造当前进程的身份（pid、版本、启动时间、启动时刻刻度）。
    ///
    /// # 边界与前提
    /// - 版本由调用方注入（生产默认取构建期版本，测试注入可区分值），
    ///   本函数不读取构建期版本。
    /// - 非 Linux 宿主（`/proc` 缺位）时 `starttime` 填 0，交叉核对退化，
    ///   liveness 由 socket 探活裁决。
    pub fn new(socket_path: impl AsRef<Path>, version: &str) -> Self {
        let pid = std::process::id();
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        ServerIdentity {
            pid,
            socket_path: socket_path.as_ref().to_string_lossy().into_owned(),
            version: version.to_string(),
            started_at,
            starttime: process_starttime(pid).unwrap_or(0),
        }
    }
}

/// 读取当前进程 uid（`/proc/self/status` 的 `Uid:` 行首值）。
///
/// # 边界与前提
/// - 非 Linux 宿主或解析失败时返回 0，路径退化为固定名，由 liveness 检查兜底。
pub fn current_uid() -> u32 {
    let status = match fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            if let Some(first) = rest.split_whitespace().next() {
                return first.parse().unwrap_or(0);
            }
        }
    }
    0
}

/// well-known sidecar 路径：`$TMPDIR/oxide-<uid>.json`。
pub fn well_known_sidecar_path() -> PathBuf {
    std::env::temp_dir().join(format!("oxide-{}.json", current_uid()))
}

/// well-known 持久 server socket 路径：`$TMPDIR/oxide-<uid>.sock`。
pub fn well_known_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("oxide-{}.sock", current_uid()))
}

/// well-known 日志文件路径：`$TMPDIR/oxide-<uid>.log`（与 sidecar 同主名）。
pub fn well_known_log_path() -> PathBuf {
    well_known_sidecar_path().with_extension("log")
}

/// O_EXCL 原子创建 sidecar（文件已存在时创建失败）。
///
/// # 步骤
/// 1. 身份序列化为单行紧凑 JSON。
/// 2. `create_new(true)` 打开并写入全部内容。
///
/// # 边界与前提
/// - 文件已存在时返回 `AlreadyExists`，调用方据此转入读回裁决。
///
/// # 注意事项
/// - 写入不同步磁盘：崩溃残留的半写文件按损坏处理，由 liveness 检查兜底。
pub fn write_exclusive(identity: &ServerIdentity, path: &Path) -> io::Result<()> {
    let json = serde_json::to_string(identity).expect("身份序列化不会失败");
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

/// 读回并解析 sidecar。
///
/// # 边界与前提
/// - 文件缺失或解析失败（损坏、半写）返回 `None`，调用方按陈旧残留处理。
pub fn read_identity(path: &Path) -> Option<ServerIdentity> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// socket 探活：连接成功即存在存活 server。
///
/// 内核只把连接完成给存活监听者，该信号不依赖 PID、不受 PID 复用影响，
/// 是注册判定的唯一裁决。
pub fn is_server_alive(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).is_ok()
}

/// PID 探活：`kill -0` 退出码 0 即进程存在。
///
/// # 边界与前提
/// - `kill` 二进制缺失或进程不存在时返回 `false`。
pub fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 读取进程启动时刻刻度（`/proc/<pid>/stat` 第 22 字段，自开机单调递增）。
///
/// # 边界与前提
/// - 进程不存在或 `/proc` 缺位（非 Linux）时返回 `None`。
pub fn process_starttime(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm 字段带括号且可能含空格，从最后一个 `)` 起切分，避免 comm 内的
    // 空格干扰字段计数。
    let rest = text.rsplit(')').next()?;
    // 剩余部分从 state（全文件第 3 字段）起，第 20 个字段即全文件第 22 字段。
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// 认领结果：注册入口的四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimResult {
    /// 新注册成功（sidecar 已写入）。
    Registered,
    /// 已有同版本存活 server，拒绝注册（旧 server 继续运行）。
    AlreadyRunning,
    /// 已有存活 server 且版本与本进程不同（含旧 sidecar 不可读），调用方应强制交接。
    VersionMismatch,
    /// 存活证据不足以裁决（进程存活但 socket 不在），保守拒绝并提示跑清理命令。
    RefusedAmbiguous,
}

/// 注册入口：认领 well-known sidecar，必要时清理陈旧残留。
///
/// # 步骤
/// 1. 尝试 O_EXCL 创建；sidecar 不存在即成功。
/// 2. sidecar 已存在：探活 socket，连接成功即有存活 server，读回旧 sidecar
///    与本进程版本比对：匹配判 `AlreadyRunning`，不匹配或不可读判
///    `VersionMismatch`。
/// 3. socket 死：按 sidecar 可读性分支。
///    - 可解析且 PID 死：陈旧残留，删 sidecar 与 socket 文件后重建。
///    - 可解析且 PID 活：交叉核对启动时刻刻度，匹配则保守拒绝（server
///      可能正在启动或 socket 被外部删除），失配则 PID 已复用，删除重建。
///    - 不可解析（损坏）：删 sidecar 与 socket 文件后重建。
///
/// # 边界与前提
/// - 启动顺序契约是「先写 sidecar 再 bind socket」，存活 server 必有 sidecar。
/// - 版本比对是精确字符串相等（同一性判定，不是新旧次序判定）；旧 sidecar
///   不可读（损坏、缺 version 字段）按不匹配处理，方向安全（强制交接失败
///   不触碰旧 server 的文件，旧 server 继续运行）。
///
/// # 副作用
/// - 可能创建、删除、重写 sidecar 与 socket 文件。
///
/// # 注意事项
/// - 意外 io 错误（权限等）保守判 `RefusedAmbiguous`；方向安全（拒绝注册
///   不会双注册）。
pub fn claim_sidecar(sidecar_path: &Path, socket_path: &Path, new_version: &str) -> ClaimResult {
    // 先尝试 O_EXCL 创建：sidecar 不存在即成功。
    match write_exclusive(&ServerIdentity::new(socket_path, new_version), sidecar_path) {
        Ok(()) => return ClaimResult::Registered,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return ClaimResult::RefusedAmbiguous,
    }

    // socket 探活是存活的唯一事实源：连接成功即有存活 server。
    if is_server_alive(socket_path) {
        // 读回旧 sidecar 与本进程版本比对：匹配拒绝启动，不匹配或不可读
        // 交调用方强制交接。
        match read_identity(sidecar_path) {
            Some(id) if id.version == new_version => return ClaimResult::AlreadyRunning,
            _ => return ClaimResult::VersionMismatch,
        }
    }

    // socket 死：按 sidecar 可读性分支。
    match read_identity(sidecar_path) {
        Some(id) => claim_dead_socket(&id, sidecar_path, socket_path, new_version),
        None => {
            // 损坏文件：删 sidecar 与 socket 文件后重建（socket 探活已失败，
            // 文件无监听者，不会误删存活 server 的 socket）。
            remove_stale(sidecar_path, socket_path);
            re_register(sidecar_path, socket_path, new_version)
        }
    }
}

/// socket 死且 sidecar 可解析：按 PID 与启动时刻刻度裁决陈旧性。
fn claim_dead_socket(id: &ServerIdentity, sidecar_path: &Path, socket_path: &Path, new_version: &str) -> ClaimResult {
    // PID 死：陈旧残留，删 sidecar 与 socket 文件后重建。
    if !pid_alive(id.pid) {
        remove_stale(sidecar_path, socket_path);
        return re_register(sidecar_path, socket_path, new_version);
    }

    // PID 活：交叉核对启动时刻刻度裁决 PID 复用。
    if process_starttime(id.pid) == Some(id.starttime) {
        // 进程活且刻度匹配：server 可能正在启动或 socket 被外部删除，保守拒绝。
        return ClaimResult::RefusedAmbiguous;
    }

    // 刻度失配：PID 已被复用，sidecar 是陈旧残留。
    remove_stale(sidecar_path, socket_path);
    re_register(sidecar_path, socket_path, new_version)
}

/// 删除 sidecar 与 socket 文件（socket 探活已失败，文件无监听者，不会误删
/// 存活 server 的 socket）。
fn remove_stale(sidecar_path: &Path, socket_path: &Path) {
    let _ = fs::remove_file(sidecar_path);
    let _ = fs::remove_file(socket_path);
}

/// 清理后重建 sidecar，重建失败保守拒绝。
fn re_register(sidecar_path: &Path, socket_path: &Path, new_version: &str) -> ClaimResult {
    match write_exclusive(&ServerIdentity::new(socket_path, new_version), sidecar_path) {
        Ok(()) => ClaimResult::Registered,
        Err(_) => ClaimResult::RefusedAmbiguous,
    }
}

/// 生成 --rm 模式唯一 socket 路径：`$TMPDIR/oxide-rm-<pid>-<纳秒时间戳>.sock`。
///
/// # 边界与前提
/// - PID 加纳秒时间戳在单进程内唯一；--rm 是独立单进程，不需要 O_EXCL 竞争。
///
/// # 注意事项
/// - 不变量：--rm 模式从不调用 `claim_sidecar`、不写 sidecar、不碰全局
///   well-known 路径。
pub fn rm_socket_path() -> PathBuf {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    std::env::temp_dir().join(format!("oxide-rm-{}-{ns}.sock", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    /// temp_dir 下的唯一临时目录，退出时自动删除。
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("oxide_sidecar_test_{}_{}", std::process::id(), ns));
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

    /// 陈旧身份：pid 可控，其余字段为占位值。
    fn stale_identity(pid: u32, socket_path: &Path) -> ServerIdentity {
        ServerIdentity {
            pid,
            socket_path: socket_path.to_str().unwrap().to_string(),
            version: "0.0.0".into(),
            started_at: 0,
            starttime: 0,
        }
    }

    /// 写读回环：写入文件后读回五字段逐字段一致。
    #[test]
    fn identity_roundtrip() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        let id = ServerIdentity::new("/tmp/oxide-test.sock", "0.0.0");
        write_exclusive(&id, &path).expect("写 sidecar 应成功");
        let read = read_identity(&path).expect("读回 sidecar 应成功");
        assert_eq!(read, id, "五字段应逐字段一致");
    }

    /// O_EXCL 排他：同路径二次 create_new 必得 AlreadyExists。
    #[test]
    fn write_exclusive_second_create_fails() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        let id = ServerIdentity::new("/tmp/oxide-test.sock", "0.0.0");
        write_exclusive(&id, &path).expect("首次写入应成功");
        let err = write_exclusive(&id, &path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "二次 create_new 必得 AlreadyExists");
    }

    /// 认领分支：无 sidecar 时注册成功，sidecar 是本进程身份。
    #[test]
    fn claim_no_sidecar_registers() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::Registered);
        let id = read_identity(&sidecar).expect("sidecar 应已写入");
        assert_eq!(id.pid, std::process::id());
        assert_eq!(id.socket_path, socket.to_str().unwrap());
    }

    /// 认领分支：存活监听者加同版本 sidecar 判 AlreadyRunning（旧 server 继续
    /// 运行，新 server 拒绝启动）。
    #[test]
    fn claim_live_server_same_version_refused() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // 存活 server 必有 sidecar（启动顺序契约：先写 sidecar 再 bind socket）。
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        write_exclusive(&ServerIdentity::new(socket.to_str().unwrap(), "0.0.0"), &sidecar).expect("写 sidecar 应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::AlreadyRunning);
        drop(listener);
    }

    /// 认领分支：存活监听者加异版本 sidecar 判 VersionMismatch（调用方应强制
    /// 交接）。
    #[test]
    fn claim_live_server_version_mismatch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        // 旧 sidecar 版本与本进程版本不同：比对判不匹配。
        write_exclusive(&ServerIdentity::new(socket.to_str().unwrap(), "0.0.0"), &sidecar).expect("写 sidecar 应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.1"), ClaimResult::VersionMismatch);
        drop(listener);
    }

    /// 认领分支：死 PID 无 socket 判陈旧，sidecar 重建为本进程身份。
    #[test]
    fn claim_dead_pid_stale_rebuilt() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        write_exclusive(&stale_identity(dead_pid(), &socket), &sidecar).expect("写陈旧 sidecar 应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::Registered);
        let id = read_identity(&sidecar).expect("重建 sidecar 应可读");
        assert_eq!(id.pid, std::process::id(), "重建 sidecar 应为本进程身份");
    }

    /// 认领分支：存活 PID 加失配启动时刻判 PID 复用，sidecar 重建。
    #[test]
    fn claim_reused_pid_stale_rebuilt() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // PID 是本进程（存活），启动时刻刻度为占位值：PID 已复用。
        write_exclusive(&stale_identity(std::process::id(), &socket), &sidecar).expect("写陈旧 sidecar 应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::Registered);
    }

    /// 认领分支：存活 PID 加匹配启动时刻保守拒绝。
    #[test]
    fn claim_alive_pid_matching_starttime_refused() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // PID 与启动时刻刻度都是本进程的：进程活且刻度匹配。
        let id = ServerIdentity::new(socket.to_str().unwrap(), "0.0.0");
        write_exclusive(&id, &sidecar).expect("写 sidecar 应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::RefusedAmbiguous);
    }

    /// 认领分支：损坏文件加死 socket 判陈旧，sidecar 重建。
    #[test]
    fn claim_corrupted_sidecar_stale_rebuilt() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // 损坏文件：半写 JSON。
        fs::write(&sidecar, r#"{"pid":1,"socket_path":""#).expect("写损坏文件应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::Registered);
        let id = read_identity(&sidecar).expect("重建 sidecar 应可读");
        assert_eq!(id.pid, std::process::id());
    }

    /// 认领分支：损坏文件加无监听者的陈旧 socket 文件判陈旧，两文件都删后重建。
    #[test]
    fn claim_corrupted_sidecar_stale_socket_rebuilt() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // 损坏文件：非 JSON 内容。
        fs::write(&sidecar, "not json at all").expect("写损坏文件应成功");
        // 陈旧 socket 文件：文件存在但无监听者。
        fs::write(&socket, b"stale").expect("写陈旧 socket 文件应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::Registered);
        let id = read_identity(&sidecar).expect("重建 sidecar 应可读");
        assert_eq!(id.pid, std::process::id(), "重建 sidecar 应为本进程身份");
        assert!(!socket.exists(), "陈旧 socket 文件应被删除");
    }

    /// 认领分支：损坏文件加存活 socket 判 VersionMismatch（不可读按不匹配
    /// 处理，调用方应强制交接，socket 是存活事实源）。
    #[test]
    fn claim_corrupted_sidecar_live_socket_mismatch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        fs::write(&sidecar, "not json at all").expect("写损坏文件应成功");
        assert_eq!(claim_sidecar(&sidecar, &socket, "0.0.0"), ClaimResult::VersionMismatch);
        drop(listener);
    }

    /// --rm 路径：两次调用路径不同，且文件名含本进程 PID 与 oxide-rm- 前缀。
    #[test]
    fn rm_socket_path_unique_and_contains_pid() {
        let a = rm_socket_path();
        std::thread::sleep(Duration::from_nanos(1000));
        let b = rm_socket_path();
        assert_ne!(a, b, "两次调用应得不同路径");
        let file_name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(file_name.contains(&std::process::id().to_string()), "文件名应含本进程 PID");
        assert!(file_name.starts_with("oxide-rm-"), "文件名应以 oxide-rm- 前缀开头");
    }

    /// 日志路径：`.log` 扩展名，与 sidecar 路径同主名同目录。
    #[test]
    fn well_known_log_path_derives_from_sidecar() {
        let log = well_known_log_path();
        let sidecar = well_known_sidecar_path();
        assert_eq!(log.extension().and_then(|e| e.to_str()), Some("log"), "日志路径应以 .log 结尾");
        assert_eq!(log.file_stem(), sidecar.file_stem(), "日志与 sidecar 应同主名");
        assert_eq!(log.parent(), sidecar.parent(), "日志与 sidecar 应同目录");
    }
}
