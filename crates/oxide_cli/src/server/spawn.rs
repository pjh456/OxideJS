//! spawn 与就绪探测：拉起脱离 server 与就绪探测的唯一共享入口。
//!
//! start、restart、watchdog 三条路径共用本模块的同一对函数，不另起第二套
//! spawn 逻辑。`current_exe()` 在 lib 上下文与二进制目标返回同一路径（lib
//! 链接进同一个 oxide 二进制），spawn 行为逐字节不变。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::client::send_control_request;
use super::protocol::{ServerRequest, ServerResponse};

/// spawn 脱离的前台 server 子进程（`server start --foreground`）：标准流全空，不等待。
///
/// # 步骤
/// 1. 取当前可执行文件路径。
/// 2. 构造命令 `server start --foreground` 加可选 `--workers N`。
/// 3. 三个标准流置空并 spawn。
///
/// # 边界与前提
/// - 必须带 `--foreground`：守护形态 spawn 的是前台 server 入口，不带该旗标会
///   再次进入守护形态形成递归 spawn。
///
/// # 副作用
/// - 创建脱离的子进程，调用方退出后由 init 收领继续运行。
pub fn spawn_detached_server(workers: Option<u32>) -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(["server", "start", "--foreground"]);
    if let Some(n) = workers {
        cmd.args(["--workers", &n.to_string()]);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// 就绪探测：轮询健康请求直至得 healthy。
///
/// # 步骤
/// 1. 循环发健康请求，得 healthy 即返回成功。
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
    let end = Instant::now() + deadline;
    loop {
        let ready =
            matches!(send_control_request(&ServerRequest::Health), Ok(ServerResponse::Health { healthy: true }));
        if ready {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!("server 未在 {} 秒内就绪", deadline.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
