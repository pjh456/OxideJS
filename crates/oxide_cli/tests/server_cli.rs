//! server CLI 集成测试：真实二进制的分派面（help 列子命令、无 server 时
//! 控制臂与 forge 退 1、cleanup 幂等、start 与 restart 端到端、forge 端到端、
//! watchdog 崩溃自动重启与 SIGINT 优雅停 server）。
//!
//! start / restart / watchdog 端到端与 cleanup 走 well-known 全局路径
//! （每用户单例）：测试先探活，socket 存活即 panic 不抢占存活 server；
//! 触碰全局路径的测试经双重锁（进程内互斥加跨进程文件锁）串行，避免相互
//! 干扰（含与单测二进制的并行干扰）。

use std::io::{BufRead, BufReader};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use oxide_cli::server::client::send_control_request;
use oxide_cli::server::protocol::{self, FrameReader, ServerRequest, ServerResponse};
use oxide_cli::server::sidecar;
use oxide_net::{Conn, Endpoint};

/// well-known 全局路径进程内互斥锁：同测试二进制内触碰全局路径的测试串行
/// 执行（fcntl 锁以进程为单位，不覆盖同进程多线程）。
static WELL_KNOWN_LOCK: Mutex<()> = Mutex::new(());

/// well-known 全局路径双重锁守卫：进程内互斥（同二进制线程串行）加跨进程
/// fcntl 文件锁（nextest 并行独立进程串行）。
struct WellKnownGuard {
    _in_process: MutexGuard<'static, ()>,
    _cross_process: sidecar::WellKnownLock,
}

/// 取 well-known 全局路径双重锁，守卫随 drop 释放。
fn lock_well_known() -> WellKnownGuard {
    let in_process = WELL_KNOWN_LOCK.lock().expect("全局路径锁不应中毒");
    let cross_process = sidecar::WellKnownLock::acquire().expect("全局路径文件锁应可取");
    WellKnownGuard {
        _in_process: in_process,
        _cross_process: cross_process,
    }
}

/// 运行 oxide 二进制并返回输出。
fn oxide(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(args)
        .output()
        .expect("failed to run oxide")
}

/// 向进程发信号（与 sidecar liveness 探活同一 `kill` 二进制路径）。
fn kill_process(pid: u32, signal: &str) {
    Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .expect("发送信号应成功");
}

/// 轮询健康请求直至得 healthy（100 毫秒间隔、给定截止）。
///
/// sidecar 出现早于 socket 绑定与池预热，不能直接作为就绪信号；watchdog
/// 的初始就绪探测与测试的崩溃模拟都以健康请求通过为界。
fn wait_for_healthy(deadline: Duration) {
    let end = Instant::now() + deadline;
    loop {
        let ready =
            matches!(send_control_request(&ServerRequest::Health), Ok(ServerResponse::Health { healthy: true }));
        if ready {
            return;
        }
        assert!(Instant::now() < end, "server 应在有界时间内就绪");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 轮询判 server 进程退出（50 毫秒间隔、给定截止）。
///
/// 删 sidecar 文件早于进程实际退出，二者间隙随构建环境变化（带内存
/// sanitizer 的构建 atexit 收尾更久），立即断言会撞上未退出窗口，故有界轮询。
fn wait_pid_dead(pid: u32, deadline: Duration) {
    let end = Instant::now() + deadline;
    while sidecar::pid_alive(pid) {
        assert!(Instant::now() < end, "server 进程应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 发送请求帧并读回一帧响应。
fn send_and_recv(stream: &mut Conn, request: &ServerRequest) -> ServerResponse {
    stream
        .write_all(protocol::encode_request(request).as_bytes())
        .expect("写请求应成功");
    let mut reader = FrameReader::new(BufReader::new(stream));
    let frame = reader.read_frame().expect("读响应应成功").expect("应读到响应帧");
    protocol::parse_response(&frame).expect("响应帧解析应成功")
}

/// 读 well-known sidecar 取 endpoint 并连接 server（两臂按形态分支）。
fn connect_well_known() -> Conn {
    let id = sidecar::read_identity(&sidecar::well_known_sidecar_path()).expect("读 sidecar 应成功");
    match sidecar::parse_endpoint(&id.socket_path) {
        Endpoint::Unix(path) => Conn::Unix(UnixStream::connect(&path).expect("连接 server 应成功")),
        Endpoint::Tcp(addr) => Conn::Tcp(TcpStream::connect(addr).expect("连接 server 应成功")),
    }
}

/// 探测空闲端口：临时 `TcpListener` 绑 0 端口取操作系统分配的端口后释放。
///
/// 释放与 server 绑定间的微小竞态窗口可接受（与既有测试对固定端口的容忍
/// 一致）：发生则 server 绑定失败、测试红，不加重试。
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("临时监听者绑定应成功");
    let port = listener.local_addr().expect("取端口应成功").port();
    drop(listener);
    port
}

/// 按端口直连 server：TCP 回环连接包成 `Conn`。
fn connect_tcp(port: u16) -> Conn {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    Conn::Tcp(TcpStream::connect(addr).expect("连接 server 应成功"))
}

/// help 列全部十一个子命令。
#[test]
fn server_help_lists_subcommands() {
    let output = oxide(&["server", "--help"]);
    assert!(output.status.success(), "server --help 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in [
        "start", "stop", "status", "health", "info", "version", "cleanup", "restart", "log", "forge", "watchdog",
    ] {
        assert!(stdout.contains(name), "help 应列出 {name}：{stdout}");
    }
}

/// log 子命令无日志文件时退 1，stderr 含「日志文件不存在」。
///
/// 走 well-known 全局路径：先探活，存活 server 占用时 panic 不抢占；删除
/// well-known 日志文件（无存活 server 时删除安全）。
#[test]
fn server_log_no_file_exit_1() {
    let _guard = lock_well_known();
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    let log_path = sidecar::well_known_log_path();
    let _ = std::fs::remove_file(&log_path);

    let output = oxide(&["server", "log"]);
    assert_eq!(output.status.code(), Some(1), "log 应退 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("日志文件不存在"), "log 应打印无日志文件提示：{stderr}");
}

/// 五个控制臂无 server 时各退 1，stderr 含「无已注册的 server」提示。
///
/// 走 well-known 全局路径：先探活，存活 server 占用时 panic 不抢占。
#[test]
fn server_control_arms_no_server_exit_1() {
    let _guard = lock_well_known();
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    for sub in ["stop", "status", "health", "info", "version"] {
        let output = oxide(&["server", sub]);
        assert_eq!(output.status.code(), Some(1), "{sub} 应退 1");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("无已注册的 server"), "{sub} 应打印无 server 提示：{stderr}");
    }
}

/// forge 子命令无 server 时退 1，stderr 含「无已注册的 server」提示。
///
/// 走 well-known 全局路径：先探活，存活 server 占用时 panic 不抢占。
#[test]
fn server_forge_no_server_exit_1() {
    let _guard = lock_well_known();
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    let output = oxide(&["server", "forge", "code"]);
    assert_eq!(output.status.code(), Some(1), "forge 应退 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("无已注册的 server"), "forge 应打印无 server 提示：{stderr}");
}

/// cleanup 退 0（幂等：有残留则清理、无残留则报无残留），事后全局路径无文件。
#[test]
fn server_cleanup_exit_0() {
    let _guard = lock_well_known();
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    let output = oxide(&["server", "cleanup"]);
    assert_eq!(output.status.code(), Some(0), "cleanup 应退 0");
    assert!(!sidecar::well_known_socket_path().exists(), "socket 文件应不存在");
    assert!(!sidecar::well_known_sidecar_path().exists(), "sidecar 文件应不存在");
}

/// start 端到端：CLI 即刻返回（server 是脱离的孙进程），断言退 0 且打印
/// 「已启动」，well-known 路径连接，版本帧断言构建期版本，关闭帧后 server
/// 进程退出，socket 与 sidecar 文件被删。
#[test]
fn server_start_e2e() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // start 为守护形态：CLI 即刻返回，server 是脱离的孙进程；worker 数压到 2
    // 避免按宿主核数预热的成本。
    let output = oxide(&["server", "start", "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "start 应打印「已启动」：{stdout}");

    // 读 sidecar 取 server 进程号。
    let id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");

    // 版本请求帧：版本应等于构建期版本。
    let mut stream = connect_well_known();
    let response = send_and_recv(&mut stream, &ServerRequest::Version);
    match response {
        ServerResponse::Version { version } => {
            assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
        }
        other => panic!("应得 Version 帧，实得 {other:?}"),
    }

    // 关闭请求帧：读回关闭确认帧。
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);

    // 轮询 sidecar 文件消失（30 秒截止、50 毫秒间隔）判 server 进程已退出。
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
    wait_pid_dead(id.pid, Duration::from_secs(30));
}

/// start 双启动幂等：已有存活 server 时再跑 start，断言第二次退 0 且打印
/// 「已启动」（第二个 spawn 的子进程认领被拒退 1 不触碰文件，探测看到第一个
/// server 即通过）。
#[test]
fn server_start_when_running() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 首次 start：CLI 即刻返回，server 是脱离的孙进程。
    let output = oxide(&["server", "start", "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "首次 start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "首次 start 应打印「已启动」：{stdout}");

    // 二次 start：第二个 spawn 的子进程认领被拒，探测看到第一个 server 即通过。
    let output = oxide(&["server", "start", "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "二次 start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "二次 start 应打印「已启动」：{stdout}");

    // 收尾：发关闭请求，等 sidecar 消失。
    let mut stream = connect_well_known();
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
}

/// restart 端到端：起真实子进程 A，跑 `oxide server restart`（发关闭、等退出、
/// 拉起新 server、就绪探测），断言退 0 且打印「已重启」，旧进程退 0，新 sidecar
/// 进程号不同且版本为构建期版本，新 server 健康与版本请求正常，收尾关闭后文件被删。
#[test]
fn server_restart_e2e() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 起子进程 A：worker 数压到 2，轮询 sidecar 出现（10 秒截止）判就绪。
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["server", "start", "--workers", "2"])
        .spawn()
        .expect("oxide server start 应可启动");
    let deadline = Instant::now() + Duration::from_secs(10);
    while sidecar::read_identity(&sidecar_path).is_none() {
        assert!(Instant::now() < deadline, "sidecar 未出现");
        std::thread::sleep(Duration::from_millis(10));
    }

    // 读 sidecar 取旧进程号。
    let old_id = sidecar::read_identity(&sidecar_path).expect("读旧 sidecar 应成功");

    // 跑 restart：断言退 0、stdout 含「已重启」。
    let output = oxide(&["server", "restart"]);
    assert_eq!(output.status.code(), Some(0), "restart 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已重启"), "restart 应打印「已重启」：{stdout}");

    // 断言子进程 A 以退出码 0 退出（30 秒截止，try_wait 轮询）。
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert_eq!(status.code(), Some(0), "旧 server 应以退出码 0 退出：{status:?}");
                break;
            }
            Ok(None) => {
                assert!(Instant::now() < deadline, "旧 server 应在有界时间内退出");
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => panic!("等待子进程失败：{err}"),
        }
    }

    // 断言新 sidecar 存在、进程号与旧进程号不同、版本等于构建期版本。
    let new_id = sidecar::read_identity(&sidecar_path).expect("读新 sidecar 应成功");
    assert_ne!(new_id.pid, old_id.pid, "新 server 进程号应不同于旧 server");
    assert_eq!(new_id.version, env!("CARGO_PKG_VERSION"), "新 sidecar 版本应与构建期版本一致");

    // 连接新 server：健康请求得 healthy，版本请求得构建期版本。
    let mut stream = connect_well_known();
    match send_and_recv(&mut stream, &ServerRequest::Health) {
        ServerResponse::Health { healthy } => assert!(healthy, "新 server 应健康"),
        other => panic!("应得 Health 帧，实得 {other:?}"),
    }
    match send_and_recv(&mut stream, &ServerRequest::Version) {
        ServerResponse::Version { version } => {
            assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
        }
        other => panic!("应得 Version 帧，实得 {other:?}"),
    }

    // 收尾：发关闭请求，等新 server 退出，断言 sidecar 文件被删。
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "新 server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
}

/// log 端到端：守护形态 start 后轮询日志文件出现「KernelCore initialized」
/// 行判就绪（socket 文件出现早于内核构造，不能直接作为日志就绪信号），
/// 断言 `log` 退 0 且含该行、`--lines 1` 恰一行、`--level error` 为空
/// （Info 级无 ERROR 行），关闭后日志文件有意保留。
#[test]
fn server_log_e2e() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();
    let log_path = sidecar::well_known_log_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 删旧日志文件：server 即将全新启动，内容断言不受历史污染。
    let _ = std::fs::remove_file(&log_path);

    // start 为守护形态：CLI 子进程即刻退 0 并打印「已启动」，server 是脱离
    // 的孙进程（其日志初始化在孙进程内确定性生效，内容断言无竞态）。
    let output = oxide(&["server", "start", "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "start 应打印「已启动」：{stdout}");

    // 读 sidecar 取 server 进程号。
    let id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");

    // 轮询日志文件出现「KernelCore initialized」行判就绪（30 秒截止、
    // 50 毫秒间隔）。
    let deadline = Instant::now() + Duration::from_secs(30);
    let content = loop {
        let content = std::fs::read_to_string(&log_path).unwrap_or_default();
        if content.contains("KernelCore initialized") {
            break content;
        }
        assert!(Instant::now() < deadline, "日志文件应在 30 秒内含内核初始化行");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!content.is_empty(), "日志文件内容应非空");

    // log：退 0 且含内核初始化行。
    let output = oxide(&["server", "log"]);
    assert_eq!(output.status.code(), Some(0), "log 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("KernelCore initialized"), "log 应含内核初始化行：{stdout}");

    // --lines 1：恰一行。
    let output = oxide(&["server", "log", "--lines", "1"]);
    assert_eq!(output.status.code(), Some(0), "log --lines 1 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "log --lines 1 应恰一行：{stdout}");

    // --level error：空（Info 级无 ERROR 行）。
    let output = oxide(&["server", "log", "--level", "error"]);
    assert_eq!(output.status.code(), Some(0), "log --level error 应退 0");
    assert!(
        output.stdout.is_empty(),
        "log --level error 应为空：{}",
        String::from_utf8_lossy(&output.stdout)
    );

    // 关闭请求帧：读回关闭确认帧。
    let mut stream = connect_well_known();
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);

    // 轮询 sidecar 文件消失（30 秒截止、50 毫秒间隔）判 server 进程已退出。
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    wait_pid_dead(id.pid, Duration::from_secs(30));

    // 日志文件有意保留（跨重启的诊断工件）。
    assert!(log_path.exists(), "日志文件退出后应保留");

    // 收尾：删除日志文件恢复状态。
    let _ = std::fs::remove_file(&log_path);
}

/// forge 端到端：守护形态 start 后，`forge code` 打印条目数与容量行，
/// `forge string --lookup` 打印 lookup 行，`forge code --gc --clear-cache`
/// 打印 gc 与清缓存行；关闭后 socket 与 sidecar 文件被删。
#[test]
fn server_forge_e2e() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // start 为守护形态：CLI 即刻返回，server 是脱离的孙进程；轮询 sidecar
    // 出现判就绪（worker 数压到 2 避免按宿主核数预热的成本）。
    let output = oxide(&["server", "start", "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "start 应退 0");
    let deadline = Instant::now() + Duration::from_secs(10);
    while sidecar::read_identity(&sidecar_path).is_none() {
        assert!(Instant::now() < deadline, "sidecar 未出现");
        std::thread::sleep(Duration::from_millis(10));
    }

    // forge code：退 0，stdout 含条目数行与容量行。
    let output = oxide(&["server", "forge", "code"]);
    assert_eq!(output.status.code(), Some(0), "forge code 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("code forge:"), "forge code 应打印条目数行：{stdout}");
    assert!(stdout.contains("capacity: 512"), "forge code 应打印容量行：{stdout}");

    // forge string --lookup：退 0，stdout 含 lookup 行。
    let output = oxide(&["server", "forge", "string", "--lookup", "forge-e2e-key"]);
    assert_eq!(output.status.code(), Some(0), "forge string --lookup 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("lookup:"), "forge --lookup 应打印 lookup 行：{stdout}");

    // forge code --gc --clear-cache：退 0，stdout 含 gc 与清缓存行。
    let output = oxide(&["server", "forge", "code", "--gc", "--clear-cache"]);
    assert_eq!(output.status.code(), Some(0), "forge code --gc --clear-cache 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gc: collected"), "forge --gc 应打印 gc 行：{stdout}");
    assert!(stdout.contains("cache cleared"), "forge --clear-cache 应打印清缓存行：{stdout}");

    // 关闭请求帧：读回关闭确认帧。
    let mut stream = connect_well_known();
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);

    // 轮询 sidecar 文件消失（30 秒截止、50 毫秒间隔）判 server 进程已退出。
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
}

/// watchdog 端到端：拉起 watchdog 前台进程（其拉起 server），`kill -9` 模拟
/// 崩溃，断言 watchdog 自动重启（新 sidecar 进程号不同、新 server 健康），
/// 关闭后 watchdog 退 0 且 stdout 含崩溃提示与日志尾部。
#[test]
fn server_watchdog_e2e() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 拉起 watchdog 前台进程（stdout 管道捕获崩溃诊断输出）。
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["server", "watchdog", "--workers", "2"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("oxide server watchdog 应可启动");

    // 后台线程消费 stdout 收集全部行，见到「开始监控」标记行即通知：
    // 该标记行蕴含 watchdog 已越过初始就绪探测，崩溃模拟在此之后进行
    // 才不与初始探测窗口竞争（预热期间杀 server 会让探测超时退 1、不重启）。
    let stdout_pipe = child.stdout.take().expect("stdout 管道应可取");
    let (monitoring_tx, monitoring_rx) = std::sync::mpsc::channel::<()>();
    let out_handle = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout_pipe);
        let mut lines = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            lines.push(line.clone());
            if line.contains("watchdog 开始监控") {
                let _ = monitoring_tx.send(());
            }
        }
        lines
    });

    // 轮询 sidecar 出现（10 秒截止）判 server 已启动。
    let deadline = Instant::now() + Duration::from_secs(10);
    while sidecar::read_identity(&sidecar_path).is_none() {
        assert!(Instant::now() < deadline, "sidecar 未出现");
        std::thread::sleep(Duration::from_millis(50));
    }
    let first_id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");

    // 等 watchdog 越过初始就绪探测进入监控循环（30 秒截止）。
    monitoring_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("watchdog 应在有界时间内进入监控循环");

    // 模拟崩溃：kill -9（文件残留、无退出序列，与 panic=abort 消亡等价）。
    kill_process(first_id.pid, "-9");

    // 轮询新 sidecar 出现且进程号不同（30 秒截止、50 毫秒间隔）判 watchdog
    // 已重启。
    let deadline = Instant::now() + Duration::from_secs(30);
    let second_id = loop {
        if let Some(id) = sidecar::read_identity(&sidecar_path) {
            if id.pid != first_id.pid {
                break id;
            }
        }
        assert!(Instant::now() < deadline, "watchdog 应在有界时间内重启 server");
        std::thread::sleep(Duration::from_millis(50));
    };

    // 等 watchdog 越过重启就绪探测（30 秒截止）：关闭请求发给探测完成前的
    // 新 server 会让它优雅退出、删 sidecar，与 watchdog 的重启探测竞争
    // （探测超时退 1、不重启）。
    monitoring_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("watchdog 应在有界时间内完成重启探测");

    // 连接新 server：健康请求得 healthy。
    let mut stream = connect_well_known();
    match send_and_recv(&mut stream, &ServerRequest::Health) {
        ServerResponse::Health { healthy } => assert!(healthy, "新 server 应健康"),
        other => panic!("应得 Health 帧，实得 {other:?}"),
    }

    // 发关闭请求得确认帧，轮询 sidecar 消失（watchdog 判 Exited 退 0）。
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }

    // 等 watchdog 子进程退出断言退出码 0，收集 stdout 断言含崩溃提示与日志尾部。
    let status = child.wait().expect("等待 watchdog 应成功");
    assert_eq!(status.code(), Some(0), "watchdog 应以退出码 0 退出：{status:?}");
    let lines = out_handle.join().expect("stdout 读取线程应可汇合");
    let stdout = lines.join("");
    assert!(stdout.contains("server 已崩溃"), "watchdog 应打印崩溃提示：{stdout}");
    assert!(stdout.contains("日志尾部"), "watchdog 应打印日志尾部：{stdout}");
    assert_ne!(second_id.pid, first_id.pid, "重启后的 server 进程号应不同");
}

/// watchdog SIGINT：拉起 watchdog 前台进程后向其发 SIGINT，断言 watchdog 退 0、
/// sidecar 消失、server 进程已退出（停 watchdog 即优雅停 server）。
#[test]
fn server_watchdog_sigint_stops_server() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 拉起 watchdog 前台进程。
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["server", "watchdog", "--workers", "2"])
        .spawn()
        .expect("oxide server watchdog 应可启动");

    // 轮询 sidecar 出现（10 秒截止）判 server 已启动。
    let deadline = Instant::now() + Duration::from_secs(10);
    while sidecar::read_identity(&sidecar_path).is_none() {
        assert!(Instant::now() < deadline, "sidecar 未出现");
        std::thread::sleep(Duration::from_millis(50));
    }
    let id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");

    // 等健康请求通过判 server 就绪：watchdog 已越过初始就绪探测进入监控
    // 循环，SIGINT 的优雅关闭请求发给已就绪的 server 即被处理。
    wait_for_healthy(Duration::from_secs(60));

    // 向 watchdog 发 SIGINT。
    kill_process(child.id(), "-INT");

    // 等 watchdog 退出断言退出码 0。
    let status = child.wait().expect("等待 watchdog 应成功");
    assert_eq!(status.code(), Some(0), "watchdog 应以退出码 0 退出：{status:?}");

    // server 被优雅停止：sidecar 消失、进程已退出。
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    wait_pid_dead(id.pid, Duration::from_secs(30));
}

/// start --port 端到端：自定义端口起守护 server，sidecar 记该端口，控制臂四
/// 帧（版本 / 状态 / 健康 / 信息）经 sidecar 端点应答，eval 得完成值，关闭帧后
/// server 进程退出、sidecar 被删。
#[test]
fn server_start_port_e2e() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 探测空闲端口起守护 server（worker 数压到 2 避免按宿主核数预热的成本）。
    let port = free_port();
    let port_arg = port.to_string();
    let output = oxide(&["server", "start", "--port", &port_arg, "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "start 应打印「已启动」：{stdout}");

    // 读 sidecar：endpoint 应为自定义端口，记进程号。
    let id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");
    assert_eq!(id.socket_path, format!("127.0.0.1:{port}"), "sidecar 应记自定义端口");

    // 控制臂四帧经 sidecar 端点应答（send_control_request 读 sidecar 取端口）。
    let response = send_control_request(&ServerRequest::Version).expect("版本臂应成功");
    match response {
        ServerResponse::Version { version } => {
            assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
        }
        other => panic!("应得 Version 帧，实得 {other:?}"),
    }
    let response = send_control_request(&ServerRequest::Status).expect("状态臂应成功");
    assert!(matches!(response, ServerResponse::Status { .. }), "应得 Status 帧：{response:?}");
    let response = send_control_request(&ServerRequest::Health).expect("健康臂应成功");
    match response {
        ServerResponse::Health { healthy } => assert!(healthy, "server 应健康"),
        other => panic!("应得 Health 帧，实得 {other:?}"),
    }
    let response = send_control_request(&ServerRequest::Info).expect("信息臂应成功");
    match response {
        ServerResponse::Info { version, socket_path, pid } => {
            assert_eq!(socket_path, format!("127.0.0.1:{port}"), "Info 端点应为自定义端口");
            assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
            assert_eq!(pid, id.pid, "Info 进程号应与 sidecar 一致");
        }
        other => panic!("应得 Info 帧，实得 {other:?}"),
    }

    // eval：1 + 1 得完成值 2。
    let mut stream = connect_tcp(port);
    let response = send_and_recv(
        &mut stream,
        &ServerRequest::Eval {
            code: "1 + 1".into(),
            max_steps: None,
        },
    );
    assert!(
        matches!(response, ServerResponse::EvalResult { ref value, .. } if value.as_deref() == Some("2")),
        "应得完成值为 2 的 EvalResult 帧：{response:?}"
    );

    // 关闭请求帧：读回关闭确认帧。
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);

    // 轮询 sidecar 文件消失（30 秒截止、50 毫秒间隔）判 server 进程已退出。
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
    wait_pid_dead(id.pid, Duration::from_secs(30));
}

/// start --port 多客户端并发：四线程各建独立连接、各发三枚 eval，逐连接顺序
/// 与精确完成值断言，汇合后关闭、sidecar 消失、进程退出。
#[test]
fn server_start_port_multi_client() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 探测空闲端口起守护 server。
    let port = free_port();
    let port_arg = port.to_string();
    let output = oxide(&["server", "start", "--port", &port_arg, "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "start 应打印「已启动」：{stdout}");

    let id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");

    // 四线程各建独立连接、各发三枚 eval（代码按线程号构造，常量全非零，
    // 避开编译缓存键对交换操作数的既有碰撞面）。
    let handles: Vec<_> = (0u32..4)
        .map(|i| {
            std::thread::spawn(move || {
                let mut stream = connect_tcp(port);
                for j in 0..3u32 {
                    let code = format!("{} * 10 + {}", i + 1, j + 1);
                    let response = send_and_recv(&mut stream, &ServerRequest::Eval { code, max_steps: None });
                    match response {
                        ServerResponse::EvalResult { ref value, ref error } => {
                            assert!(error.is_none(), "eval 不应有错误：{response:?}");
                            let expected = ((i + 1) * 10 + (j + 1)).to_string();
                            assert_eq!(value.as_deref(), Some(expected.as_str()), "完成值应精确匹配：{response:?}");
                        }
                        other => panic!("应得 EvalResult 帧：{other:?}"),
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("客户端线程应可汇合");
    }

    // 收尾：关闭请求帧，轮询 sidecar 消失判 server 进程已退出。
    let mut stream = connect_tcp(port);
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
    wait_pid_dead(id.pid, Duration::from_secs(30));
}

/// start --port 同端口第二实例拒绝：首实例守护形态起 server 后，第二实例用
/// 前台形态起同端口 server，认领判 AlreadyRunning 退 1、stderr 含启动失败与
/// 认领被拒，首 server 不受影响（版本臂仍应答、sidecar 进程号不变），收尾关闭。
#[test]
fn server_start_port_second_instance_refused() {
    let _guard = lock_well_known();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_endpoint(sidecar::WELL_KNOWN_BIND_ADDR)),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    // 首实例：守护形态起 server（CLI 即刻返回、就绪探测通过）。
    let port = free_port();
    let port_arg = port.to_string();
    let output = oxide(&["server", "start", "--port", &port_arg, "--workers", "2"]);
    assert_eq!(output.status.code(), Some(0), "首实例 start 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已启动"), "首实例 start 应打印「已启动」：{stdout}");
    let id = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");

    // 第二实例：前台形态同端口，认领判 AlreadyRunning 退 1。
    let output = oxide(&["server", "start", "--foreground", "--port", &port_arg, "--workers", "2"]);
    assert_eq!(output.status.code(), Some(1), "同端口第二实例（前台形态）应退 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("server 启动失败"), "第二实例应打印启动失败：{stderr}");
    assert!(stderr.contains("认领被拒"), "第二实例应打印认领被拒：{stderr}");

    // 首 server 不受影响：版本臂仍正常应答、sidecar 进程号不变。
    let response = send_control_request(&ServerRequest::Version).expect("首 server 版本臂应正常应答");
    assert!(matches!(response, ServerResponse::Version { .. }), "应得 Version 帧：{response:?}");
    let id_after = sidecar::read_identity(&sidecar_path).expect("读 sidecar 应成功");
    assert_eq!(id_after.pid, id.pid, "首 server 的 sidecar 进程号应不变");

    // 收尾：关闭首实例，轮询 sidecar 消失判进程已退出。
    let mut stream = connect_tcp(port);
    let response = send_and_recv(&mut stream, &ServerRequest::Shutdown);
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(30);
    while sidecar_path.exists() {
        assert!(Instant::now() < deadline, "server 应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
    wait_pid_dead(id.pid, Duration::from_secs(30));
}

/// --rm 动态端口打印：`--rm --port 0` 绑 0 端口后向前台标准输出打印实际端点，
/// 解析得非 0 端口，连接发一枚 eval 得完成值，关闭连接（EOF）后单连接模型
/// 进程干净退出（退 0）。不写 sidecar、不碰 well-known 路径，无需双重锁。
#[test]
fn server_rm_dynamic_port_prints() {
    // 拉起 --rm 前台进程（stdout 管道收集端点打印行）。
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["server", "start", "--rm", "--port", "0", "--workers", "2"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("oxide server start --rm 应可启动");

    // 后台线程消费 stdout：见到端点打印行即把端口发主线程，收集全部行至 EOF。
    // 端口解析取稳健形态：只认 `127.0.0.1:` 前缀与紧随数字，不断言完整句子。
    let stdout_pipe = child.stdout.take().expect("stdout 管道应可取");
    let (port_tx, port_rx) = std::sync::mpsc::channel::<u16>();
    let out_handle = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout_pipe);
        let mut lines = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            lines.push(line.clone());
            if let Some(idx) = line.find("127.0.0.1:") {
                let rest = &line[idx + "127.0.0.1:".len()..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(port) = digits.parse::<u16>() {
                    let _ = port_tx.send(port);
                }
            }
        }
        lines
    });

    // 端口行是就绪信号（打印在空闲窗口之前）：有界时间内应得非 0 端口。
    let port = port_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("--rm 进程应在有界时间内打印实际端口");
    assert_ne!(port, 0, "操作系统应分配非 0 端口");

    // 连接该端口发一枚 eval 得完成值。
    let mut stream = connect_tcp(port);
    let response = send_and_recv(
        &mut stream,
        &ServerRequest::Eval {
            code: "1 + 1".into(),
            max_steps: None,
        },
    );
    assert!(
        matches!(response, ServerResponse::EvalResult { ref value, .. } if value.as_deref() == Some("2")),
        "应得完成值为 2 的 EvalResult 帧：{response:?}"
    );

    // 关闭连接（EOF）：--rm 单连接模型在首个连接处理完毕后干净退出（退 0）。
    drop(stream);
    let status = child.wait().expect("等待 --rm 进程应成功");
    assert_eq!(status.code(), Some(0), "--rm 进程应以退出码 0 退出：{status:?}");

    let lines = out_handle.join().expect("stdout 读取线程应可汇合");
    let stdout = lines.join("");
    assert!(stdout.contains("127.0.0.1:"), "stdout 应含端点打印行：{stdout}");
}
