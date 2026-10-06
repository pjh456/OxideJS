//! server CLI 集成测试：真实二进制的分派面（help 列子命令、占位臂退出码、
//! cleanup 幂等、start 端到端）。
//!
//! start 端到端与 cleanup 走 well-known 全局路径（每用户单例）：测试先探活，
//! socket 存活即 panic 不抢占存活 server；两枚触碰全局路径的测试经同一把
//! 锁串行，避免相互干扰。

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use oxide_cli::server::protocol::{self, FrameReader, ServerRequest, ServerResponse};
use oxide_cli::server::sidecar;

/// well-known 全局路径互斥锁：同测试二进制内触碰全局路径的测试串行执行。
static WELL_KNOWN_LOCK: Mutex<()> = Mutex::new(());

/// 运行 oxide 二进制并返回输出。
fn oxide(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(args)
        .output()
        .expect("failed to run oxide")
}

/// 发送请求帧并读回一帧响应。
fn send_and_recv(stream: &mut UnixStream, request: &ServerRequest) -> ServerResponse {
    stream
        .write_all(protocol::encode_request(request).as_bytes())
        .expect("写请求应成功");
    let mut reader = FrameReader::new(BufReader::new(stream));
    let frame = reader.read_frame().expect("读响应应成功").expect("应读到响应帧");
    protocol::parse_response(&frame).expect("响应帧解析应成功")
}

/// help 列全部十个子命令。
#[test]
fn server_help_lists_subcommands() {
    let output = oxide(&["server", "--help"]);
    assert!(output.status.success(), "server --help 应退 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in [
        "start", "stop", "status", "health", "info", "version", "cleanup", "restart", "log", "forge",
    ] {
        assert!(stdout.contains(name), "help 应列出 {name}：{stdout}");
    }
}

/// 三个占位臂各退 2，stderr 含 not yet implemented。
///
/// version/status/health/info/stop 五臂已由控制客户端接管（无 server 时退 1），
/// 不在此列；restart/log/forge 仍是占位。
#[test]
fn server_stub_subcommands_exit_2() {
    for sub in ["restart", "log", "forge"] {
        let output = oxide(&["server", sub]);
        assert_eq!(output.status.code(), Some(2), "{sub} 应退 2");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not yet implemented"), "{sub} 应打印未实现提示：{stderr}");
    }
}

/// 五个控制臂无 server 时各退 1，stderr 含「无已注册的 server」提示。
///
/// 走 well-known 全局路径：先探活，存活 server 占用时 panic 不抢占。
#[test]
fn server_control_arms_no_server_exit_1() {
    let _guard = WELL_KNOWN_LOCK.lock().expect("全局路径锁不应中毒");
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_socket_path()),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    for sub in ["stop", "status", "health", "info", "version"] {
        let output = oxide(&["server", sub]);
        assert_eq!(output.status.code(), Some(1), "{sub} 应退 1");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("无已注册的 server"),
            "{sub} 应打印无 server 提示：{stderr}"
        );
    }
}

/// cleanup 退 0（幂等：有残留则清理、无残留则报无残留），事后全局路径无文件。
#[test]
fn server_cleanup_exit_0() {
    let _guard = WELL_KNOWN_LOCK.lock().expect("全局路径锁不应中毒");
    assert!(
        !sidecar::is_server_alive(&sidecar::well_known_socket_path()),
        "存活 server 占用 well-known 路径，测试不抢占"
    );

    let output = oxide(&["server", "cleanup"]);
    assert_eq!(output.status.code(), Some(0), "cleanup 应退 0");
    assert!(!sidecar::well_known_socket_path().exists(), "socket 文件应不存在");
    assert!(!sidecar::well_known_sidecar_path().exists(), "sidecar 文件应不存在");
}

/// start 端到端：起真实子进程，well-known 路径连接，版本帧断言构建期版本，
/// 关闭帧后子进程退 0，socket 与 sidecar 文件被删。
#[test]
fn server_start_e2e() {
    let _guard = WELL_KNOWN_LOCK.lock().expect("全局路径锁不应中毒");
    let socket = sidecar::well_known_socket_path();
    let sidecar_path = sidecar::well_known_sidecar_path();

    // 探活先行：存活 server 占用全局路径时 panic，不抢占。
    assert!(!sidecar::is_server_alive(&socket), "存活 server 占用 well-known 路径，测试不抢占");

    // worker 数压到 2：避免按宿主核数预热的成本。
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxide"))
        .args(["server", "start", "--workers", "2"])
        .spawn()
        .expect("oxide server start 应可启动");

    // 轮询 socket 文件出现（10 秒截止）判就绪。
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "socket 文件未出现");
        std::thread::sleep(Duration::from_millis(10));
    }

    // 版本请求帧：版本应等于构建期版本。
    let mut stream = UnixStream::connect(&socket).expect("连接 server 应成功");
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

    // 等子进程以退出码 0 退出（30 秒截止）。
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert_eq!(status.code(), Some(0), "server 应以退出码 0 退出：{status:?}");
                break;
            }
            Ok(None) => {
                assert!(Instant::now() < deadline, "server 应在有界时间内退出");
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => panic!("等待子进程失败：{err}"),
        }
    }

    assert!(!socket.exists(), "socket 文件应被删除");
    assert!(!sidecar_path.exists(), "sidecar 文件应被删除");
}
