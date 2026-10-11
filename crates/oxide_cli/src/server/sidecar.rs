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

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use oxide_net::Endpoint;
use serde::{Deserialize, Serialize};

/// server 身份快照：单行 JSON sidecar 文件，启动时写入一次。
///
/// 字段：
/// - `pid`：server 进程标识符，清理命令杀进程用。
/// - `socket_path`：监听端点的文本形态（Unix 臂为路径、TCP 臂为 host:port），
///   控制客户端连接用。
/// - `version`：构建期版本，版本交接时比对用。
/// - `started_at`：Unix 秒，人读展示用。
/// - `starttime`：`/proc/<pid>/stat` 的启动时刻刻度（自开机单调递增），
///   PID 复用判定用。
/// - `port`：TCP 监听端口（机器可读形态），Unix 臂为 0；旧 sidecar 无该
///   字段时读回为 0（serde 缺省），探活裁决仍走 `socket_path` 文本。
/// - `listen_addr`：监听地址（网络身份——server 可达的网络接口，安全边界
///   关注点：loopback-only 与 all-interfaces 的区分）；旧 sidecar 无该字段
///   时读回为空串（serde 缺省）。
/// - `token`：共享 token（十六进制），控制客户端认证帧用；旧 sidecar 无该
///   字段时读回为空串（serde 缺省）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub pid: u32,
    pub socket_path: String,
    pub version: String,
    pub started_at: u64,
    pub starttime: u64,
    /// TCP 监听端口（机器可读形态）；Unix 臂为 0。旧 sidecar 无该字段时
    /// 读回为 0（serde 缺省），探活裁决走 socket_path 文本，行为不变。
    #[serde(default)]
    pub port: u16,
    /// 监听地址（网络身份：server 可达的网络接口，loopback-only 与
    /// all-interfaces 的区分）；旧 sidecar 无该字段时读回为空串（serde
    /// 缺省），探活裁决仍走 socket_path 文本，行为不变。
    #[serde(default)]
    pub listen_addr: String,
    /// 共享 token（十六进制）：控制客户端连接后首帧认证用；旧 sidecar
    /// 无该字段时读回为空串（serde 缺省）。
    #[serde(default)]
    pub token: String,
}

impl ServerIdentity {
    /// 构造当前进程的身份（pid、版本、启动时间、启动时刻刻度、监听端口）。
    ///
    /// # 边界与前提
    /// - 版本由调用方注入（生产默认取构建期版本，测试注入可区分值），
    ///   本函数不读取构建期版本。
    /// - `socket_path` 字段记监听端点的文本形态（`endpoint.describe()`），
    ///   字段名与 JSON 模式不变，旧 sidecar 可读。
    /// - `port` 取端点的 TCP 端口（Unix 臂无端口，记 0）。
    /// - `listen_addr` 取端点的文本形态（`endpoint.describe()`），与
    ///   `socket_path` 同源同时机（认领时、bind 之前），记配置监听地址。
    /// - `token` 由调用方注入（server 启动时生成），本函数不生成。
    /// - 非 Linux 宿主（`/proc` 缺位）时 `starttime` 填 0，交叉核对退化，
    ///   liveness 由 endpoint 探活裁决。
    pub fn new(endpoint: &Endpoint, version: &str, token: &str) -> Self {
        let pid = std::process::id();
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let port = match endpoint {
            Endpoint::Tcp(addr) => addr.port(),
            Endpoint::Unix(_) => 0,
        };
        let socket_path = endpoint.describe();
        ServerIdentity {
            pid,
            socket_path: socket_path.clone(),
            version: version.to_string(),
            started_at,
            starttime: process_starttime(pid).unwrap_or(0),
            port,
            listen_addr: socket_path,
            token: token.to_string(),
        }
    }
}

/// 生成共享 token：读 `/dev/urandom` 32 字节转十六进制（64 位小写）。
///
/// # 边界与前提
/// - 非 Linux 宿主（`/dev/urandom` 缺位）或读取失败时回落时间加 PID 熵
///   混合（本宿主恒为 Linux，回落臂仅保编译）。
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    let ok = fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok();
    if !ok {
        bytes = fallback_entropy();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 非 Linux 熵回落：纳秒时间加 PID 经 xorshift 混合出 32 字节。
fn fallback_entropy() -> [u8; 32] {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    let pid = std::process::id() as u64;
    let mut seed = ns ^ (pid << 32);
    let mut out = [0u8; 32];
    for byte in out.iter_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = (seed & 0xff) as u8;
    }
    out
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

/// well-known TCP 端口：持久 server 缺省端口。
pub const WELL_KNOWN_PORT: u16 = 4242;

/// well-known TCP 绑定地址：缺省 loopback（持久 server 缺省只接受本机连接）。
pub const WELL_KNOWN_BIND_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// well-known sidecar 路径：`$TMPDIR/oxide-<uid>.json`。
pub fn well_known_sidecar_path() -> PathBuf {
    std::env::temp_dir().join(format!("oxide-{}.json", current_uid()))
}

/// well-known 持久 server socket 路径：`$TMPDIR/oxide-<uid>.sock`。
///
/// 保留给 `--rm` 独立模式（进程唯一路径）与旧残留清理；持久 server 缺省
/// 端点已切到 well-known TCP 端口。
pub fn well_known_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("oxide-{}.sock", current_uid()))
}

/// well-known TCP 端点：bind_addr:WELL_KNOWN_PORT（缺省取 loopback）。
pub fn well_known_endpoint(bind_addr: IpAddr) -> Endpoint {
    Endpoint::Tcp(SocketAddr::new(bind_addr, WELL_KNOWN_PORT))
}

/// 解析端点文本形态：先试 `SocketAddr`（host:port）得 TCP 臂，失败回
/// Unix 臂（路径形态）。sidecar 读写的单一入口，解析放这里。
pub fn parse_endpoint(s: &str) -> Endpoint {
    match s.parse::<SocketAddr>() {
        Ok(addr) => Endpoint::Tcp(addr),
        Err(_) => Endpoint::Unix(PathBuf::from(s)),
    }
}

/// well-known 日志文件路径：`$TMPDIR/oxide-<uid>.log`（与 sidecar 同主名）。
pub fn well_known_log_path() -> PathBuf {
    well_known_sidecar_path().with_extension("log")
}

/// well-known 全局路径排他锁文件：`$TMPDIR/oxide-<uid>.lock`（与 sidecar 同主名）。
pub fn well_known_lock_path() -> PathBuf {
    well_known_sidecar_path().with_extension("lock")
}

/// well-known 全局路径排他锁：fcntl 文件锁，跨进程互斥。
///
/// 持锁期间保护 well-known 全局路径（sidecar、socket、日志）的读写；
/// 锁随文件描述符关闭（Drop）自动释放，进程消亡时内核一并释放。
///
/// # 边界与前提
/// - fcntl 锁以进程为单位：同进程多线程并发取锁不互斥，进程内串行化
///   由调用方自己的进程内互斥负责。
///
/// # 副作用
/// - 创建锁文件（不存在时）。
///
/// # 注意事项
/// - 锁文件不随释放删除：删除会破坏他进程已打开描述符上的锁，互斥失效。
pub struct WellKnownLock {
    /// 持有文件描述符：锁随描述符关闭（Drop）释放，字段存在即保活。
    #[allow(dead_code)]
    file: File,
}

impl WellKnownLock {
    /// 取 well-known 全局路径排他锁（阻塞直至持有）。
    ///
    /// # 步骤
    /// 1. 打开锁文件（不存在则创建）。
    /// 2. fcntl 阻塞取排他锁（F_SETLKW）。
    pub fn acquire() -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(well_known_lock_path())?;
        // SAFETY: 文件描述符在 file 生命周期内有效，锁结构体按值传给内核只读。
        let lock = libc::flock {
            l_type: libc::F_WRLCK as _,
            l_whence: libc::SEEK_SET as _,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLKW, &lock) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(WellKnownLock { file })
    }
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
/// # 副作用
/// - Unix 臂写入后显式置 0600（sidecar 含共享 token，仅属主可读写）；
///   非 Unix 平台该位语义为空操作。
///
/// # 注意事项
/// - 写入不同步磁盘：崩溃残留的半写文件按损坏处理，由 liveness 检查兜底。
pub fn write_exclusive(identity: &ServerIdentity, path: &Path) -> io::Result<()> {
    let json = serde_json::to_string(identity).expect("身份序列化不会失败");
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(json.as_bytes())?;
    // sidecar 含共享 token：显式 0600，仅属主可读写。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
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

/// endpoint 探活：连接成功即存在存活 server，两臂按形态分支。
///
/// 内核只把连接完成给存活监听者，该信号不依赖 PID、不受 PID 复用影响，
/// 是注册判定的唯一裁决。
pub fn is_server_alive(endpoint: &Endpoint) -> bool {
    match endpoint {
        Endpoint::Unix(path) => UnixStream::connect(path).is_ok(),
        Endpoint::Tcp(addr) => TcpStream::connect(*addr).is_ok(),
    }
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
/// 2. sidecar 已存在：先读回，探活 sidecar 记录的 endpoint（不可读时探
///    调用方 endpoint 兜底），连接成功即有存活 server，读回旧 sidecar
///    与本进程版本比对：匹配判 `AlreadyRunning`，不匹配或不可读判
///    `VersionMismatch`。
/// 3. endpoint 死：按 sidecar 可读性分支。
///    - 可解析且 PID 死：陈旧残留，删 sidecar 与 socket 文件（Unix 臂）后重建。
///    - 可解析且 PID 活：交叉核对启动时刻刻度，匹配则保守拒绝（server
///      可能正在启动或 endpoint 被外部释放），失配则 PID 已复用，删除重建。
///    - 不可解析（损坏）：删 sidecar 与 socket 文件（Unix 臂）后重建。
///
/// # 边界与前提
/// - 启动顺序契约是「先写 sidecar 再 bind endpoint」，存活 server 必有 sidecar。
/// - 探活必须探 sidecar 记录的 endpoint 而非调用方自己的 endpoint：端口
///   不同时探自己会把存活 server 误判为死并破坏其 sidecar（双注册场景）。
/// - 版本比对是精确字符串相等（同一性判定，不是新旧次序判定）；旧 sidecar
///   不可读（损坏、缺 version 字段）按不匹配处理，方向安全（强制交接失败
///   不触碰旧 server 的文件，旧 server 继续运行）。
///
/// # 副作用
/// - 可能创建、删除、重写 sidecar 与 socket 文件（Unix 臂）。
///
/// # 注意事项
/// - 意外 io 错误（权限等）保守判 `RefusedAmbiguous`；方向安全（拒绝注册
///   不会双注册）。
pub fn claim_sidecar(sidecar_path: &Path, endpoint: &Endpoint, new_version: &str, token: &str) -> ClaimResult {
    // 先尝试 O_EXCL 创建：sidecar 不存在即成功。
    match write_exclusive(&ServerIdentity::new(endpoint, new_version, token), sidecar_path) {
        Ok(()) => return ClaimResult::Registered,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return ClaimResult::RefusedAmbiguous,
    }

    // sidecar 已存在：先读回。探活目标是 sidecar 记录的 endpoint（正确性
    // 红线：探调用方自己的端口会在端口不同时把存活 server 误判为死）；
    // 不可读时探调用方 endpoint（损坏文件防御性兜底，探自己）。
    let recorded = read_identity(sidecar_path);
    let probe = match &recorded {
        Some(id) => parse_endpoint(&id.socket_path),
        None => endpoint.clone(),
    };
    if is_server_alive(&probe) {
        // 读回旧 sidecar 与本进程版本比对：匹配拒绝启动，不匹配或不可读
        // 交调用方强制交接。
        match &recorded {
            Some(id) if id.version == new_version => return ClaimResult::AlreadyRunning,
            _ => return ClaimResult::VersionMismatch,
        }
    }

    // endpoint 死：按 sidecar 可读性分支。
    match recorded {
        Some(id) => claim_dead_socket(&id, sidecar_path, endpoint, new_version, token),
        None => {
            // 损坏文件：删 sidecar 与 socket 文件（Unix 臂）后重建（endpoint
            // 探活已失败，文件无监听者，不会误删存活 server 的 socket）。
            remove_stale(sidecar_path, endpoint);
            re_register(sidecar_path, endpoint, new_version, token)
        }
    }
}

/// endpoint 死且 sidecar 可解析：按 PID 与启动时刻刻度裁决陈旧性。
fn claim_dead_socket(
    id: &ServerIdentity, sidecar_path: &Path, endpoint: &Endpoint, new_version: &str, token: &str,
) -> ClaimResult {
    // PID 死：陈旧残留，删 sidecar 与 socket 文件（Unix 臂）后重建。
    if !pid_alive(id.pid) {
        remove_stale(sidecar_path, endpoint);
        return re_register(sidecar_path, endpoint, new_version, token);
    }

    // PID 活：交叉核对启动时刻刻度裁决 PID 复用。
    if process_starttime(id.pid) == Some(id.starttime) {
        // 进程活且刻度匹配：server 可能正在启动或 endpoint 被外部释放，保守拒绝。
        return ClaimResult::RefusedAmbiguous;
    }

    // 刻度失配：PID 已被复用，sidecar 是陈旧残留。
    remove_stale(sidecar_path, endpoint);
    re_register(sidecar_path, endpoint, new_version, token)
}

/// 删除 sidecar 与 socket 文件（Unix 臂；endpoint 探活已失败，文件无监听者，
/// 不会误删存活 server 的 socket）。TCP 臂无文件可删，只删 sidecar。
fn remove_stale(sidecar_path: &Path, endpoint: &Endpoint) {
    let _ = fs::remove_file(sidecar_path);
    if let Some(path) = endpoint.socket_path() {
        let _ = fs::remove_file(path);
    }
}

/// 清理后重建 sidecar，重建失败保守拒绝。
fn re_register(sidecar_path: &Path, endpoint: &Endpoint, new_version: &str, token: &str) -> ClaimResult {
    match write_exclusive(&ServerIdentity::new(endpoint, new_version, token), sidecar_path) {
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
            port: 0,
            listen_addr: String::new(),
            token: String::new(),
        }
    }

    /// 写读回环：写入文件后读回五字段逐字段一致。
    #[test]
    fn identity_roundtrip() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        let id = ServerIdentity::new(&Endpoint::Unix(PathBuf::from("/tmp/oxide-test.sock")), "0.0.0", "test-token");
        write_exclusive(&id, &path).expect("写 sidecar 应成功");
        let read = read_identity(&path).expect("读回 sidecar 应成功");
        assert_eq!(read, id, "字段应逐字段一致");
    }

    /// 端口字段：TCP 端点记端点端口（机器可读形态），Unix 臂记 0。
    #[test]
    fn identity_port_per_arm() {
        let tcp = ServerIdentity::new(
            &Endpoint::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4242)),
            "0.0.0",
            "test-token",
        );
        assert_eq!(tcp.port, 4242, "TCP 臂应记端点端口");
        let unix = ServerIdentity::new(&Endpoint::Unix(PathBuf::from("/tmp/oxide-test.sock")), "0.0.0", "test-token");
        assert_eq!(unix.port, 0, "Unix 臂应记 0");
    }

    /// 写读回环（TCP 臂）：sidecar 含机器可读 port 字段，读回逐字段一致。
    #[test]
    fn identity_roundtrip_records_tcp_port() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        let endpoint = Endpoint::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4242));
        let id = ServerIdentity::new(&endpoint, "0.0.0", "test-token");
        write_exclusive(&id, &path).expect("写 sidecar 应成功");
        let read = read_identity(&path).expect("读回 sidecar 应成功");
        assert_eq!(read, id, "字段应逐字段一致");
        assert_eq!(read.port, 4242, "port 字段应记端点端口");
        assert_eq!(read.listen_addr, "127.0.0.1:4242", "listen_addr 字段应记端点地址");
    }

    /// 旧 sidecar（无 port 字段）读回：serde 缺省补 0，读回成功不判损坏。
    #[test]
    fn legacy_sidecar_without_port_reads_back() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        fs::write(
            &path,
            r#"{"pid":1,"socket_path":"127.0.0.1:4242","version":"0.0.0","started_at":0,"starttime":0}"#,
        )
        .expect("写旧 sidecar 应成功");
        let id = read_identity(&path).expect("旧 sidecar 应可读回");
        assert_eq!(id.port, 0, "无 port 字段应读回 0");
        assert_eq!(id.listen_addr, "", "无 listen_addr 字段应读回空串");
        assert_eq!(id.token, "", "无 token 字段应读回空串");
    }

    /// token 生成：64 位小写十六进制，两次调用取值不同。
    #[test]
    fn generate_token_is_64_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64, "token 应为 64 位十六进制");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "token 应全为小写十六进制"
        );
        assert_ne!(a, b, "两次生成的 token 应不同");
    }

    /// sidecar 文件权限：写入后显式 0600（仅属主可读写）。
    #[test]
    fn write_exclusive_sets_0600() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        let id = ServerIdentity::new(&Endpoint::Unix(PathBuf::from("/tmp/oxide-test.sock")), "0.0.0", "test-token");
        write_exclusive(&id, &path).expect("写 sidecar 应成功");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("读元数据应成功").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "sidecar 文件权限应为 0600");
        }
    }

    /// O_EXCL 排他：同路径二次 create_new 必得 AlreadyExists。
    #[test]
    fn write_exclusive_second_create_fails() {
        let dir = TestDir::new();
        let path = dir.path("sidecar.json");
        let id = ServerIdentity::new(&Endpoint::Unix(PathBuf::from("/tmp/oxide-test.sock")), "0.0.0", "test-token");
        write_exclusive(&id, &path).expect("首次写入应成功");
        let err = write_exclusive(&id, &path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "二次 create_new 必得 AlreadyExists");
    }

    /// 端点文本解析：host:port 得 TCP 臂，路径形态回 Unix 臂。
    #[test]
    fn parse_endpoint_two_forms() {
        let tcp = parse_endpoint("127.0.0.1:4242");
        assert_eq!(
            tcp,
            Endpoint::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4242)),
            "host:port 应得 TCP 臂"
        );
        let unix = parse_endpoint("/tmp/oxide-test.sock");
        assert_eq!(unix, Endpoint::Unix(PathBuf::from("/tmp/oxide-test.sock")), "路径形态应得 Unix 臂");
    }

    /// 认领分支：无 sidecar 时注册成功，sidecar 是本进程身份。
    #[test]
    fn claim_no_sidecar_registers() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::Registered
        );
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
        // 存活 server 必有 sidecar（启动顺序契约：先写 sidecar 再 bind endpoint）。
        let listener = UnixListener::bind(&socket).expect("绑定监听者应成功");
        write_exclusive(&ServerIdentity::new(&Endpoint::Unix(socket.clone()), "0.0.0", "test-token"), &sidecar)
            .expect("写 sidecar 应成功");
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::AlreadyRunning
        );
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
        write_exclusive(&ServerIdentity::new(&Endpoint::Unix(socket.clone()), "0.0.0", "test-token"), &sidecar)
            .expect("写 sidecar 应成功");
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.1", "test-token"),
            ClaimResult::VersionMismatch
        );
        drop(listener);
    }

    /// 认领分支：死 PID 无 socket 判陈旧，sidecar 重建为本进程身份。
    #[test]
    fn claim_dead_pid_stale_rebuilt() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        write_exclusive(&stale_identity(dead_pid(), &socket), &sidecar).expect("写陈旧 sidecar 应成功");
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::Registered
        );
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
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::Registered
        );
    }

    /// 认领分支：存活 PID 加匹配启动时刻保守拒绝。
    #[test]
    fn claim_alive_pid_matching_starttime_refused() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // PID 与启动时刻刻度都是本进程的：进程活且刻度匹配。
        let id = ServerIdentity::new(&Endpoint::Unix(socket.clone()), "0.0.0", "test-token");
        write_exclusive(&id, &sidecar).expect("写 sidecar 应成功");
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::RefusedAmbiguous
        );
    }

    /// 认领分支：损坏文件加死 socket 判陈旧，sidecar 重建。
    #[test]
    fn claim_corrupted_sidecar_stale_rebuilt() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        // 损坏文件：半写 JSON。
        fs::write(&sidecar, r#"{"pid":1,"socket_path":""#).expect("写损坏文件应成功");
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::Registered
        );
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
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::Registered
        );
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
        assert_eq!(
            claim_sidecar(&sidecar, &Endpoint::Unix(socket.clone()), "0.0.0", "test-token"),
            ClaimResult::VersionMismatch
        );
        drop(listener);
    }

    /// 认领分支（TCP 臂）：无 sidecar 时注册成功，sidecar 记 host:port 文本形态。
    #[test]
    fn claim_tcp_no_sidecar_registers() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let endpoint = Endpoint::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0));
        assert_eq!(claim_sidecar(&sidecar, &endpoint, "0.0.0", "test-token"), ClaimResult::Registered);
        let id = read_identity(&sidecar).expect("sidecar 应已写入");
        assert_eq!(id.pid, std::process::id());
        assert_eq!(id.socket_path, endpoint.describe(), "sidecar 应记 host:port 文本形态");
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

    /// 锁路径：`.lock` 扩展名，与 sidecar 路径同主名同目录。
    #[test]
    fn well_known_lock_path_derives_from_sidecar() {
        let lock = well_known_lock_path();
        let sidecar = well_known_sidecar_path();
        assert_eq!(lock.extension().and_then(|e| e.to_str()), Some("lock"), "锁路径应以 .lock 结尾");
        assert_eq!(lock.file_stem(), sidecar.file_stem(), "锁与 sidecar 应同主名");
        assert_eq!(lock.parent(), sidecar.parent(), "锁与 sidecar 应同目录");
    }

    /// 全局路径锁：取锁与释放后重取均成功（fcntl 锁以进程为单位，
    /// 同进程重取不自我阻塞）。
    #[test]
    fn well_known_lock_acquire_release_reacquire() {
        let lock = WellKnownLock::acquire().expect("取锁应成功");
        drop(lock);
        let _again = WellKnownLock::acquire().expect("释放后重取应成功");
    }
}
