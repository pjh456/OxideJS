//! spawn 与就绪探测：拉起脱离 server 与就绪探测的唯一共享入口。
//!
//! start、restart、watchdog 三条路径共用本模块的同一组函数，不另起第二套
//! spawn 逻辑。`current_exe()` 在 lib 上下文与二进制目标返回同一路径（lib
//! 链接进同一个 oxide 二进制），spawn 行为逐字节不变。

use std::net::IpAddr;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use oxide_net::Endpoint;

use super::client::{send_control_request, send_control_request_at};
use super::protocol::{ServerRequest, ServerResponse};

/// spawn 脱离的前台 server 子进程（`server start --foreground --host H --port N`）：
/// 标准流全空，不等待。
///
/// # 步骤
/// 1. 取当前可执行文件路径。
/// 2. 构造命令 `server start --foreground --host H --port N` 加可选 `--workers N`。
/// 3. 三个标准流置空并 spawn。
///
/// # 边界与前提
/// - 必须带 `--foreground`：守护形态 spawn 的是前台 server 入口，不带该旗标会
///   再次进入守护形态形成递归 spawn。
/// - `host` 是 TCP 绑定地址（调用方传入，缺省 loopback）；`port` 是 TCP 监听
///   端口，缺省取 well-known 端口（调用方传入）。
///
/// # 副作用
/// - 创建脱离的子进程，调用方退出后由 init 收领继续运行。
pub fn spawn_detached_server(workers: Option<u32>, host: IpAddr, port: u16) -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args([
        "server",
        "start",
        "--foreground",
        "--host",
        &host.to_string(),
        "--port",
        &port.to_string(),
    ]);
    if let Some(n) = workers {
        cmd.args(["--workers", &n.to_string()]);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
}

/// 就绪探测：轮询健康请求直至得 healthy。
///
/// # 步骤
/// 1. 循环发健康请求（well-known sidecar 入口），得 healthy 即返回成功。
/// 2. 超过截止返回含超时秒数的消息。
///
/// # 边界与前提
/// - 控制客户端发请求前先读 sidecar 取 socket 路径，sidecar 缺失时自然走
///   连接失败分支，「sidecar 出现」与「健康请求通过」一个循环全覆盖
///   （socket 文件出现蕴含监听器已绑定，健康请求通过蕴含 accept 循环在运行）。
///
/// # 副作用
/// - 每轮建立并关闭一条 Unix socket 连接。
pub fn wait_server_ready(deadline: Duration) -> Result<(), String> {
    wait_ready_until(
        || matches!(send_control_request(&ServerRequest::Health), Ok(ServerResponse::Health { healthy: true })),
        deadline,
    )
}

/// 远程就绪探测：对着给定 endpoint（带 token）轮询健康请求直至得 healthy。
///
/// # 步骤
/// 1. 循环对 endpoint 发健康请求（token 由调用方注入），得 healthy 即返回成功。
/// 2. 超过截止返回含超时秒数的消息。
///
/// # 边界与前提
/// - endpoint 与 token 均由调用方注入：远程场景的 token 由拉起方传入，
///   截止时长由调用方按场景注入。
/// - 连接与读写超时沿用控制客户端 5 秒口径，轮询间隔 100 毫秒，与
///   well-known 入口同口径。
///
/// # 副作用
/// - 每轮建立并关闭一条连接（Unix socket 或 TCP）。
pub fn wait_server_ready_remote(endpoint: &Endpoint, token: &str, deadline: Duration) -> Result<(), String> {
    wait_ready_until(
        || matches!(
            send_control_request_at(endpoint, token, &ServerRequest::Health),
            Ok(ServerResponse::Health { healthy: true })
        ),
        deadline,
    )
}

/// 共享探测循环：反复调用探测函数，100 毫秒间隔，探测返回真即成功；
/// 超过截止返回含超时秒数的消息。
fn wait_ready_until(probe: impl Fn() -> bool, deadline: Duration) -> Result<(), String> {
    let end = Instant::now() + deadline;
    loop {
        if probe() {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!("server 未在 {} 秒内就绪", deadline.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Write};
    use std::net::{SocketAddr, TcpListener};

    use crate::server::protocol::{self, FrameReader};

    /// 假 TCP server：绑动态端口，接受循环逐连接读认证帧回 AuthOk、
    /// 读请求帧回给定的健康帧。
    fn fake_tcp_health_server(healthy: bool) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定假 server 应成功");
        let addr = listener.local_addr().expect("取端口应成功");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut reader = FrameReader::new(BufReader::new(stream.try_clone().expect("克隆流应成功")));
                let _ = reader.read_frame();
                let auth_ok = protocol::encode_response(&ServerResponse::AuthOk);
                if stream.write_all(auth_ok.as_bytes()).is_err() {
                    break;
                }
                let _ = reader.read_frame();
                let health = protocol::encode_response(&ServerResponse::Health { healthy });
                if stream.write_all(health.as_bytes()).is_err() {
                    break;
                }
            }
        });
        addr
    }

    /// TCP 臂：假 server 回 AuthOk 加 healthy 健康帧，就绪探测返回成功。
    #[test]
    fn remote_ready_tcp_arm() {
        let addr = fake_tcp_health_server(true);
        let result = wait_server_ready_remote(&Endpoint::Tcp(addr), "test-token", Duration::from_secs(5));
        assert!(result.is_ok(), "就绪探测应成功：{result:?}");
    }

    /// TCP 臂超时：假 server 恒回不健康帧，就绪探测在截止后返回超时消息。
    #[test]
    fn remote_ready_timeout_when_unhealthy() {
        let addr = fake_tcp_health_server(false);
        let result = wait_server_ready_remote(&Endpoint::Tcp(addr), "test-token", Duration::from_millis(300));
        let err = result.expect_err("应判超时");
        assert!(err.contains("就绪"), "应得超时消息：{err}");
    }
}
