//! 信号处理集成测试（SIGTERM）：真实信号路径触发优雅退出。
//!
//! 独立进程、进程内唯一 server 与唯一信号处理器：`ctrlc` 的处理器注册是
//! 进程级一次性的，信号投递无歧义，测试确定。SIGINT 路径见
//! `server_signal_int.rs`（同型，独立进程）。

use std::process::Command;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use oxide_cli::server::server::{run_server, ServerConfig, ServerError};

/// 唯一临时路径（进程号加测试名）：单进程内唯一，进程间以进程号隔离。
/// worker 数取小值 2：与既有单测同口径，大值徒增预热成本。
fn unique_paths(test_name: &str) -> ServerConfig {
    let dir = std::env::temp_dir().join(format!(
        "oxide_server_signal_term_{}_{}",
        std::process::id(),
        test_name
    ));
    std::fs::create_dir_all(&dir).expect("测试目录创建应成功");
    ServerConfig {
        socket_path: dir.join("server.sock"),
        sidecar_path: dir.join("sidecar.json"),
        worker_count: 2,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// 后台线程起真实 run_server，等 socket 文件出现（bind 成功即就绪，
/// 且蕴含信号处理器已注册——注册在绑定之前）。
fn start_server(config: ServerConfig) -> JoinHandle<Result<(), ServerError>> {
    let server_config = config.clone();
    let handle = std::thread::spawn(move || run_server(&server_config));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !config.socket_path.exists() {
        assert!(Instant::now() < deadline, "socket 文件未出现");
        std::thread::sleep(Duration::from_millis(10));
    }
    handle
}

/// 给本测试进程发 SIGTERM（`kill` 二进制与 sidecar 的 `pid_alive` 同一口径）。
fn send_sigterm() {
    let status = Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .expect("kill 应可执行");
    assert!(status.success(), "kill -TERM 应成功：{status:?}");
}

/// 等 server 线程退出，断言正常退出与 sidecar、socket 文件均被删除。
fn wait_shutdown(config: &ServerConfig, handle: JoinHandle<Result<(), ServerError>>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "server 线程应在有界时间内退出");
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = handle.join().expect("server 线程应正常退出");
    assert!(result.is_ok(), "server 应正常退出：{result:?}");
    assert!(!config.sidecar_path.exists(), "sidecar 文件应被删除");
    assert!(!config.socket_path.exists(), "socket 文件应被删除");
}

/// SIGTERM 触发优雅退出：在途归零、删 sidecar 与 socket 文件。
#[test]
fn sigterm_graceful_shutdown() {
    let config = unique_paths("sigterm");
    let handle = start_server(config.clone());
    send_sigterm();
    wait_shutdown(&config, handle);
}
