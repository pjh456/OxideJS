//! 控制客户端：读 sidecar、连接 server、发一帧控制请求、读回一帧响应。
//!
//! 关键约定：
//! - 连接超时 5 秒（安全网）：本地 Unix socket 连接立即完成或立即失败，
//!   超时分支只覆盖 server 挂起等异常情形。
//! - 读超时 5 秒（与交接路径的读超时同值）：server 不回应即判连接失败，
//!   不挂死。
//! - 错误三分支：连接失败、协议错误、server 错误帧，各映射到可读消息，
//!   不 panic。

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use super::protocol::{self, FrameReader, ProtocolError, ServerRequest, ServerResponse};
use super::sidecar;

/// 控制客户端超时：连接与读写各 5 秒（与交接路径的读超时同值）。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 控制客户端错误：三分支。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// 连接失败：sidecar 缺失或不可读、socket 缺失、连接被拒、5 秒超时、
    /// 写读 io 错误、server 未应答即关闭连接。
    Connect(String),
    /// 协议错误：响应帧畸形（协议层的五变体原样包装）。
    Protocol(ProtocolError),
    /// server 错误帧：server 以 Error 帧答复（如旧版本 server 不认新请求类型）。
    Server(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Connect(msg) => write!(f, "{msg}"),
            ClientError::Protocol(err) => {
                write!(f, "响应帧畸形：{err}，server 版本可能不匹配，可用 `oxide server restart` 更新")
            }
            ClientError::Server(msg) => write!(f, "server 拒绝请求：{msg}"),
        }
    }
}

/// 控制客户端入口：读 well-known sidecar，连接 server 并发一帧控制请求。
pub fn send_control_request(request: &ServerRequest) -> Result<ServerResponse, ClientError> {
    send_control_request_from(&sidecar::well_known_sidecar_path(), request)
}

/// 注入 sidecar 路径的入口（测试与复用）：读 sidecar 取 socket 路径，
/// 连接、发请求、读响应，三分支错误映射。
///
/// # 步骤
/// 1. 读 sidecar；缺失或损坏（解析失败）同归连接失败分支。
/// 2. 连接 socket（5 秒超时安全网）。
/// 3. 设读超时与写超时各 5 秒。
/// 4. 写请求帧。
/// 5. 读响应帧；io 错误或 server 未应答即关闭（EOF）判连接失败。
/// 6. 解析响应帧；畸形判协议错误，Error 帧判 server 错误，其余返回。
///
/// # 边界与前提
/// - sidecar 路径由调用方注入，生产入口用 well-known 路径。
/// - 响应帧为 Error 变体时不视为成功，message 原样带出。
///
/// # 副作用
/// - 建立并关闭一条 Unix socket 连接。
pub fn send_control_request_from(sidecar_path: &Path, request: &ServerRequest) -> Result<ServerResponse, ClientError> {
    // 读 sidecar：缺失或损坏都判连接失败，消息提示先启动 server。
    let identity = sidecar::read_identity(sidecar_path).ok_or_else(|| {
        ClientError::Connect(format!(
            "无已注册的 server（sidecar 文件缺失或不可读：{}），请先用 `oxide server start` 启动 server",
            sidecar_path.display()
        ))
    })?;

    // 连接：5 秒超时是安全网，本地 socket 连接立即完成或立即失败。
    let mut stream = connect_with_timeout(Path::new(&identity.socket_path), CONNECT_TIMEOUT)?;

    // 读写超时各 5 秒：server 挂起（不回应）时不无限阻塞。
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));

    // 写请求帧（含结尾换行）。
    stream
        .write_all(protocol::encode_request(request).as_bytes())
        .map_err(|e| ClientError::Connect(format!("写请求帧失败：{e}")))?;

    // 读响应帧：io 错误判连接失败，EOF（server 未应答即关闭）判连接失败。
    let mut reader = FrameReader::new(BufReader::new(stream));
    let frame = reader
        .read_frame()
        .map_err(|e| ClientError::Connect(format!("读响应帧失败：{e}")))?
        .ok_or_else(|| ClientError::Connect("server 未应答即关闭连接".into()))?;

    // 解析响应帧：畸形判协议错误，Error 帧判 server 错误，其余返回。
    let response = protocol::parse_response(&frame).map_err(ClientError::Protocol)?;
    match response {
        ServerResponse::Error { message } => Err(ClientError::Server(message)),
        other => Ok(other),
    }
}

/// 带超时的连接：线程内连接、通道上带超时取结果。
///
/// # 步骤
/// 1. 派生线程连接 socket。
/// 2. 通道上带超时取结果；超时判连接失败。
///
/// # 边界与前提
/// - 超时后连接线程继续存活到连接完成或失败（本地 socket 立即出结果），
///   发送端已被接收端丢弃，线程随即退出，无泄漏。
fn connect_with_timeout(path: &Path, timeout: Duration) -> Result<UnixStream, ClientError> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawn_path = path.to_path_buf();
    let display = path.display().to_string();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(&spawn_path));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(ClientError::Connect(format!(
            "连接 server 失败：{e}（socket 路径 {display}，可用 `oxide server cleanup` 清理陈旧文件）"
        ))),
        Err(RecvTimeoutError::Timeout) => Err(ClientError::Connect(format!(
            "连接 server 超时（{} 秒），socket 路径 {display}",
            timeout.as_secs()
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(ClientError::Connect("连接线程异常退出".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// temp_dir 下的唯一临时目录，退出时自动删除。
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("oxide_client_test_{}_{}", std::process::id(), ns));
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

    /// 假 server 助手：绑定监听器，派生线程接受一个连接并执行给定处理。
    fn fake_server(socket: &Path, respond: impl FnOnce(&mut UnixStream) + Send + 'static) {
        let listener = UnixListener::bind(socket).expect("绑定假 server 应成功");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                respond(&mut stream);
            }
        });
    }

    /// 写一个指向给定 socket 路径的合法 sidecar。
    fn write_sidecar(sidecar: &Path, socket: &Path) {
        let identity = sidecar::ServerIdentity::new(socket.to_str().unwrap(), "0.0.0");
        sidecar::write_exclusive(&identity, sidecar).expect("写 sidecar 应成功");
    }

    /// 无 sidecar：判连接失败分支。
    #[test]
    fn no_sidecar_is_connect_branch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let err = send_control_request_from(&sidecar, &ServerRequest::Version).unwrap_err();
        assert!(matches!(err, ClientError::Connect(_)), "应判连接失败：{err:?}");
    }

    /// 损坏 sidecar（非 JSON）：判连接失败分支。
    #[test]
    fn corrupt_sidecar_is_connect_branch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        fs::write(&sidecar, "not json at all").expect("写损坏文件应成功");
        let err = send_control_request_from(&sidecar, &ServerRequest::Version).unwrap_err();
        assert!(matches!(err, ClientError::Connect(_)), "应判连接失败：{err:?}");
    }

    /// sidecar 合法但 socket 无监听者：连接被拒判连接失败分支。
    #[test]
    fn socket_missing_is_connect_branch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        write_sidecar(&sidecar, &socket);
        let err = send_control_request_from(&sidecar, &ServerRequest::Version).unwrap_err();
        assert!(matches!(err, ClientError::Connect(_)), "应判连接失败：{err:?}");
    }

    /// 假 server 回非 JSON：判协议错误分支。
    #[test]
    fn garbage_response_is_protocol_branch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        write_sidecar(&sidecar, &socket);
        fake_server(&socket, |stream| {
            let mut reader = FrameReader::new(BufReader::new(stream.try_clone().expect("克隆流应成功")));
            let _ = reader.read_frame();
            let _ = stream.write_all(b"not json\n");
        });
        let err = send_control_request_from(&sidecar, &ServerRequest::Version).unwrap_err();
        assert!(matches!(err, ClientError::Protocol(_)), "应判协议错误：{err:?}");
    }

    /// 假 server 回 Error 帧：判 server 错误分支且 message 原样带出。
    #[test]
    fn error_frame_is_server_branch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        write_sidecar(&sidecar, &socket);
        fake_server(&socket, |stream| {
            let mut reader = FrameReader::new(BufReader::new(stream.try_clone().expect("克隆流应成功")));
            let _ = reader.read_frame();
            let frame = protocol::encode_response(&ServerResponse::Error {
                message: "server 内部错误".into(),
            });
            let _ = stream.write_all(frame.as_bytes());
        });
        let err = send_control_request_from(&sidecar, &ServerRequest::Version).unwrap_err();
        assert_eq!(err, ClientError::Server("server 内部错误".into()), "message 应原样带出：{err:?}");
    }

    /// 假 server 接受后不回应：5 秒读超时后判连接失败分支。
    #[test]
    fn read_timeout_is_connect_branch() {
        let dir = TestDir::new();
        let sidecar = dir.path("sidecar.json");
        let socket = dir.path("server.sock");
        write_sidecar(&sidecar, &socket);
        fake_server(&socket, |stream| {
            let mut reader = FrameReader::new(BufReader::new(stream.try_clone().expect("克隆流应成功")));
            let _ = reader.read_frame();
            // 不回应：保持连接打开，客户端应命中 5 秒读超时。
            std::thread::sleep(Duration::from_secs(10));
        });
        let started = std::time::Instant::now();
        let err = send_control_request_from(&sidecar, &ServerRequest::Version).unwrap_err();
        let elapsed = started.elapsed();
        assert!(matches!(err, ClientError::Connect(_)), "应判连接失败：{err:?}");
        assert!(elapsed >= Duration::from_secs(5), "应命中 5 秒读超时：{elapsed:?}");
    }
}
