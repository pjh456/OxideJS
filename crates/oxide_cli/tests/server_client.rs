//! server 控制客户端集成测试：真实 run_server 加五类请求响应加关闭确认加
//! 文件删除断言。
//!
//! 只用临时路径（经 `send_control_request_from` 注入），不触碰 well-known
//! 全局单例。

use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use oxide_cli::server::client::send_control_request_from;
use oxide_cli::server::protocol::{ServerRequest, ServerResponse};
use oxide_cli::server::server::{run_server, ServerConfig};
use oxide_net::Endpoint;

/// 临时路径配置：进程号加测试名唯一，worker 数 2，版本取构建期版本。
fn test_config(test_name: &str) -> ServerConfig {
    let dir = std::env::temp_dir().join(format!("oxide_server_client_test_{}_{}", std::process::id(), test_name));
    fs::create_dir_all(&dir).expect("测试目录创建应成功");
    ServerConfig {
        endpoint: Endpoint::Unix(dir.join("server.sock")),
        sidecar_path: dir.join("sidecar.json"),
        worker_count: 2,
        version: env!("CARGO_PKG_VERSION").to_string(),
        bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}

/// 后台线程起真实 run_server，等 socket 文件出现（bind 成功即就绪）。
fn start_server(config: ServerConfig) -> std::thread::JoinHandle<Result<(), oxide_cli::server::server::ServerError>> {
    let server_config = config.clone();
    let handle = std::thread::spawn(move || run_server(&server_config));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !config.endpoint.socket_path().expect("Unix 端点应含 socket 路径").exists() {
        assert!(Instant::now() < deadline, "socket 文件未出现");
        std::thread::sleep(Duration::from_millis(10));
    }
    handle
}

/// 端到端：真实 server 依次应答五类控制请求，关闭确认后线程退出、文件被删。
#[test]
fn control_client_end_to_end() {
    let config = test_config("e2e");
    let handle = start_server(config.clone());

    // 版本请求：版本与构建期版本一致。
    let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Version).expect("版本请求应成功");
    match response {
        ServerResponse::Version { version } => {
            assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
        }
        other => panic!("应得 Version 帧，实得 {other:?}"),
    }

    // 状态请求：worker 池异步预热，轮询至总数不小于 1 后核对字段。
    let deadline = Instant::now() + Duration::from_secs(5);
    let response = loop {
        let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Status).expect("状态请求应成功");
        if let ServerResponse::Status { pool_total, .. } = &response {
            if *pool_total >= 1 {
                break response;
            }
        }
        assert!(Instant::now() < deadline, "worker 池预热应完成");
        std::thread::sleep(Duration::from_millis(10));
    };
    match response {
        ServerResponse::Status { pool_available, pool_total, .. } => {
            assert!(pool_available <= pool_total, "可用数应不超过总数：{pool_available}/{pool_total}");
            assert!(pool_total >= 1, "总数应不小于 1：{pool_total}");
        }
        other => panic!("应得 Status 帧，实得 {other:?}"),
    }

    // 健康请求：池预热异步，轮询至 healthy 为真（预热完成）。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Health).expect("健康请求应成功");
        if matches!(response, ServerResponse::Health { healthy: true }) {
            break;
        }
        assert!(Instant::now() < deadline, "worker 池预热应完成");
        std::thread::sleep(Duration::from_millis(10));
    }

    // 信息请求：socket 路径与配置一致、监听地址与配置一致、进程号为当前进程号。
    let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Info).expect("信息请求应成功");
    match response {
        ServerResponse::Info {
            socket_path, pid, listen_addr, ..
        } => {
            assert_eq!(socket_path, config.endpoint.describe(), "socket 路径应与配置一致");
            assert_eq!(listen_addr, config.endpoint.describe(), "监听地址应与配置一致（Unix 臂）");
            assert_eq!(pid, std::process::id(), "进程号应为当前进程号");
        }
        other => panic!("应得 Info 帧，实得 {other:?}"),
    }

    // 关闭请求：得确认帧，server 线程正常退出，socket 与 sidecar 文件均被删。
    let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Shutdown).expect("关闭请求应成功");
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    let result = handle.join().expect("server 线程应正常退出");
    assert!(result.is_ok(), "server 应正常退出：{result:?}");
    assert!(
        !config.endpoint.socket_path().expect("Unix 端点应含 socket 路径").exists(),
        "socket 文件应被删除"
    );
    assert!(!config.sidecar_path.exists(), "sidecar 文件应被删除");
}

/// TCP 臂端到端：bind 127.0.0.1:0 取空闲端口、drop 监听器、以该端口起 server，
/// 经控制客户端走五类请求加关闭，sidecar 记 host:port 文本形态且被删。
#[test]
fn control_client_tcp_arm_end_to_end() {
    // bind 127.0.0.1:0 取空闲端口，drop 监听器释放端口。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("取空闲端口应成功");
    let addr = listener.local_addr().expect("取端口应成功");
    drop(listener);

    // TCP 配置：临时 sidecar 路径、worker 数 2、版本取构建期版本。
    let dir = std::env::temp_dir().join(format!("oxide_server_client_tcp_{}_{}", std::process::id(), "tcp"));
    fs::create_dir_all(&dir).expect("测试目录创建应成功");
    let config = ServerConfig {
        endpoint: Endpoint::Tcp(addr),
        sidecar_path: dir.join("sidecar.json"),
        worker_count: 2,
        version: env!("CARGO_PKG_VERSION").to_string(),
        bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
    };

    // 后台线程起真实 run_server，等 sidecar 出现（TCP 臂无 socket 文件，
    // sidecar 是就绪信号）。
    let server_config = config.clone();
    let handle = std::thread::spawn(move || run_server(&server_config));
    let deadline = Instant::now() + Duration::from_secs(10);
    while oxide_cli::server::sidecar::read_identity(&config.sidecar_path).is_none() {
        assert!(Instant::now() < deadline, "sidecar 未出现");
        std::thread::sleep(Duration::from_millis(10));
    }

    // 版本请求：版本与构建期版本一致。
    let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Version).expect("版本请求应成功");
    match response {
        ServerResponse::Version { version } => {
            assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
        }
        other => panic!("应得 Version 帧，实得 {other:?}"),
    }

    // 健康请求：池预热异步，轮询至 healthy 为真（预热完成）。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Health).expect("健康请求应成功");
        if matches!(response, ServerResponse::Health { healthy: true }) {
            break;
        }
        assert!(Instant::now() < deadline, "worker 池预热应完成");
        std::thread::sleep(Duration::from_millis(10));
    }

    // 信息请求：endpoint 值与配置一致（host:port 文本形态）、监听地址与配置一致、
    // 进程号为当前进程号。
    let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Info).expect("信息请求应成功");
    match response {
        ServerResponse::Info {
            socket_path, pid, listen_addr, ..
        } => {
            assert_eq!(socket_path, config.endpoint.describe(), "endpoint 值应与配置一致");
            assert_eq!(listen_addr, config.endpoint.describe(), "监听地址应与配置一致（TCP 臂）");
            assert_eq!(pid, std::process::id(), "进程号应为当前进程号");
        }
        other => panic!("应得 Info 帧，实得 {other:?}"),
    }

    // 关闭请求：得确认帧，server 线程正常退出，sidecar 文件被删（TCP 臂无
    // socket 文件）。
    let response = send_control_request_from(&config.sidecar_path, &ServerRequest::Shutdown).expect("关闭请求应成功");
    assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
    let result = handle.join().expect("server 线程应正常退出");
    assert!(result.is_ok(), "server 应正常退出：{result:?}");
    assert!(!config.sidecar_path.exists(), "sidecar 文件应被删除");
}
