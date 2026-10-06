//! server 进程主体：accept 循环、控制请求直接处理与优雅退出。
//!
//! 关键约定：
//! - 启动顺序契约（与身份注册一致）：先认领 sidecar，再绑定 socket；
//!   反向会让客户端在 sidecar 缺位时误判无 server 并启动第二实例。
//! - accept 循环为非阻塞 accept 加 10 毫秒轮询加原子关闭标志：唤醒路径
//!   确定、无锁、无唤醒丢失；每连接一个标准库线程是临时模型，并发任务
//!   整体替换为固定 worker 加消息队列路由，`handle_connection` 签名保持稳定。
//! - 五类控制请求全部直接应答，不 spawn 虚拟机；执行请求暂以协议错误帧
//!   答复（执行路径由后续任务补齐）。
//! - 优雅退出顺序：关闭请求 → 在途归零 → drop 池 → drop 内核 →
//!   删 sidecar 与 socket 文件。
//! - 池本体不是 Send/Sync（Vm 不跨线程），上下文只持池的原子状态句柄。

use std::fs;
use std::io;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm_pool::VmPool;

use super::eval;
use super::protocol::{self, FrameReader, ProtocolError, ServerRequest, ServerResponse};
use super::sidecar::{self, ClaimResult};

/// server 启动与运行错误：绑定失败、认领被拒、接受连接失败、读取身份失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    /// sidecar 认领未通过（已有存活 server 或存活证据不足以裁决）。
    ClaimRefused(ClaimResult),
    /// socket 绑定失败（含删除 stale socket 文件重试一次后仍失败）。
    BindFailed(String),
    /// 接受连接失败（accept 返回非 WouldBlock 错误）。
    AcceptFailed(String),
    /// 读取身份失败（sidecar 写后读回解析失败）。
    IdentityReadFailed(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::ClaimRefused(result) => write!(f, "sidecar 认领被拒：{result:?}"),
            ServerError::BindFailed(msg) => write!(f, "socket 绑定失败：{msg}"),
            ServerError::AcceptFailed(msg) => write!(f, "接受连接失败：{msg}"),
            ServerError::IdentityReadFailed(msg) => write!(f, "读取身份失败：{msg}"),
        }
    }
}

/// server 运行配置：socket 与 sidecar 路径可注入。
///
/// 生产默认取 well-known 位置；测试注入临时路径。
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Unix socket 路径。
    pub socket_path: PathBuf,
    /// sidecar 文件路径。
    pub sidecar_path: PathBuf,
}

impl ServerConfig {
    /// 生产默认配置：socket 与 sidecar 取 well-known 位置。
    pub fn well_known() -> Self {
        ServerConfig {
            socket_path: sidecar::well_known_socket_path(),
            sidecar_path: sidecar::well_known_sidecar_path(),
        }
    }
}

/// server 运行上下文：共享内核、启动时刻、关闭标志、在途计数。
///
/// 内核跨线程共享（`Arc<KernelCore>` 是 Send/Sync）；池本体不进上下文
/// （Vm 不跨线程），每连接线程自建池（见 `handle_connection`）。
struct ServerContext {
    kernel: Arc<KernelCore>,
    started_at: Instant,
    shutdown: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    socket_path: String,
}

/// server 进程主体：认领 sidecar → 绑定 socket → 建内核与池 → accept 循环 →
/// 在途归零 → 按序 drop 池与内核 → 删 sidecar 与 socket 文件。
///
/// # 步骤
/// 1. 认领 sidecar（启动顺序契约：先写 sidecar 再 bind socket）。
/// 2. 绑定监听器；失败且路径存在时删 stale socket 文件重试一次。
/// 3. 建内核（标准配置）与池（按 min_pool_size 预热）。
/// 4. 进入 accept 循环；关闭标志置位后在途归零、按序释放、删文件。
///
/// # 边界与前提
/// - 认领被拒（AlreadyRunning / RefusedAmbiguous）立即返回错误，不双注册。
/// - stale socket 文件删除重试后再失败时回滚删除 sidecar 并以错误返回。
///
/// # 副作用
/// - 创建 sidecar 与 socket 文件；正常退出时删除两者。
/// - 每连接派生一个标准库线程（临时模型）。
///
/// # 注意事项
/// - 在调用线程上运行（后续 CLI 骨架任务在主线程分派到它）。
/// - 关闭标志由关闭请求置位；信号处理由后续任务在同一标志上接线。
pub fn run_server(config: &ServerConfig) -> Result<(), ServerError> {
    // 认领 sidecar：已有存活 server 或存活证据不足即返回错误。
    match sidecar::claim_sidecar(&config.sidecar_path, &config.socket_path) {
        ClaimResult::Registered => {}
        other => return Err(ServerError::ClaimRefused(other)),
    }

    // 绑定监听器：stale socket 文件删一次重试，失败回滚 sidecar。
    let listener = bind_listener(config)?;

    // 建内核（标准配置）；池每连接线程自建（Vm 不跨线程，池本体不共享）。
    let kernel = KernelCore::new(KernelConfig::standard());

    let ctx = Arc::new(ServerContext {
        kernel: Arc::clone(&kernel),
        started_at: Instant::now(),
        shutdown: Arc::new(AtomicBool::new(false)),
        in_flight: Arc::new(AtomicUsize::new(0)),
        socket_path: config.socket_path.to_string_lossy().into_owned(),
    });

    // accept 循环：非阻塞 accept 加 10 毫秒轮询加原子关闭标志。
    let accept_result = accept_loop(&listener, &ctx);

    // 在途归零：轮询至零，5 秒安全超时后记警告继续。
    wait_in_flight_drained(&ctx.in_flight, Duration::from_secs(5));

    // 按序释放：上下文（持内核句柄）→ 内核。
    drop(ctx);
    drop(kernel);

    // 删 sidecar 与 socket 文件（尽力而为，文件已不存在不视为错误）。
    let _ = fs::remove_file(&config.sidecar_path);
    let _ = fs::remove_file(&config.socket_path);

    accept_result
}

/// 绑定监听器：失败且路径存在时删 stale socket 文件重试一次。
///
/// # 边界与前提
/// - stale socket 文件是崩溃残留（文件存在但无监听者）。
///
/// # 副作用
/// - 可能删除 socket 文件；重试仍失败时删除 sidecar（回滚）。
fn bind_listener(config: &ServerConfig) -> Result<UnixListener, ServerError> {
    match UnixListener::bind(&config.socket_path) {
        Ok(listener) => Ok(listener),
        Err(_first) if config.socket_path.exists() => {
            // 路径存在：崩溃残留的 stale socket 文件，删一次重试。
            let _ = fs::remove_file(&config.socket_path);
            match UnixListener::bind(&config.socket_path) {
                Ok(listener) => Ok(listener),
                Err(second) => {
                    let _ = fs::remove_file(&config.sidecar_path);
                    Err(ServerError::BindFailed(format!("socket 绑定失败：{second}")))
                }
            }
        }
        Err(first) => {
            let _ = fs::remove_file(&config.sidecar_path);
            Err(ServerError::BindFailed(format!("socket 绑定失败：{first}")))
        }
    }
}

/// accept 循环：非阻塞 accept 加 10 毫秒轮询，关闭标志置位即退出。
///
/// # 步骤
/// 1. 检查关闭标志（原子布尔，序一致排序）。
/// 2. accept：成功则派生连接线程（在途计数加一）；WouldBlock 睡 10 毫秒再查；
///    其余错误以 `AcceptFailed` 返回。
///
/// # 副作用
/// - 每连接派生一个标准库线程，线程结束时减一在途计数。
fn accept_loop(listener: &UnixListener, ctx: &Arc<ServerContext>) -> Result<(), ServerError> {
    listener
        .set_nonblocking(true)
        .map_err(|e| ServerError::AcceptFailed(e.to_string()))?;

    loop {
        // 关闭标志置位（来自任意连接线程）即退出。
        if ctx.shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }

        match listener.accept() {
            Ok((stream, _addr)) => {
                ctx.in_flight.fetch_add(1, Ordering::SeqCst);
                let ctx = Arc::clone(ctx);
                std::thread::spawn(move || {
                    handle_connection(stream, &ctx);
                    ctx.in_flight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(ServerError::AcceptFailed(e.to_string())),
        }
    }
}

/// 每连接处理：帧读取 → 分派 → 响应写回，循环至对端关闭。
///
/// # 步骤
/// 1. 克隆写端，原流交给帧读取器，设 30 秒读超时。
/// 2. 循环读帧：EOF、读超时、非法 UTF-8 退出；帧长超限按协议约定关连接。
/// 3. 畸形帧以 `Error` 帧答复后继续；正常帧分派并写回。
/// 4. 关闭确认帧写回即退出循环。
///
/// # 边界与前提
/// - 半开连接（对端静默不关）由 30 秒读超时有界化。
///
/// # 副作用
/// - 关闭请求会置位关闭标志。
fn handle_connection(stream: UnixStream, ctx: &ServerContext) {
    // 克隆写端：原流交给帧读取器，写端独立用于响应写回。
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };

    // 30 秒读超时：半开连接不永久占住线程。
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));

    // 每连接自建池（Vm 不跨线程，池是每线程资产），按内核配置预热。
    let pool = VmPool::new(Arc::clone(&ctx.kernel), ctx.kernel.config.min_pool_size, ctx.kernel.config.max_pool_size);

    let mut reader = FrameReader::new(io::BufReader::new(stream));
    // 读帧：EOF、读超时、非法 UTF-8 均退出。
    while let Ok(Some(frame)) = reader.read_frame() {
        // 分派：帧长超限按协议约定关连接；畸形帧以 Error 帧答复后继续。
        let response = match protocol::parse_request(&frame) {
            Ok(request) => dispatch_control(&request, ctx, &pool),
            Err(ProtocolError::FrameTooLarge) => break,
            Err(error) => error.to_error_frame(),
        };

        // 写回响应帧；写失败退出。
        let encoded = protocol::encode_response(&response);
        if writer.write_all(encoded.as_bytes()).is_err() {
            break;
        }

        // 关闭确认帧写回即退出循环。
        if matches!(response, ServerResponse::Shutdown) {
            break;
        }
    }
}

/// 控制请求分派：五类控制请求直接应答；执行请求走 `handle_eval`。
///
/// 池是每连接资产（`handle_connection` 创建），Status 响应读本连接池状态。
fn dispatch_control(request: &ServerRequest, ctx: &ServerContext, pool: &Arc<VmPool>) -> ServerResponse {
    match request {
        ServerRequest::Version => ServerResponse::Version {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        ServerRequest::Status => ServerResponse::Status {
            pool_available: pool.available_count(),
            pool_total: pool.total_count(),
            uptime_ms: ctx.started_at.elapsed().as_millis() as u64,
        },
        ServerRequest::Health => ServerResponse::Health { healthy: true },
        ServerRequest::Info => ServerResponse::Info {
            version: env!("CARGO_PKG_VERSION").to_string(),
            socket_path: ctx.socket_path.clone(),
            pid: std::process::id(),
        },
        ServerRequest::Shutdown => {
            // 置关闭标志；主循环至多 10 毫秒内退出 accept 循环。
            ctx.shutdown.store(true, Ordering::SeqCst);
            ServerResponse::Shutdown
        }
        ServerRequest::Eval { code, max_steps } => eval::handle_eval(code, *max_steps, &ctx.kernel, pool),
    }
}

/// 在途归零：轮询在途计数至零，超时记警告继续。
fn wait_in_flight_drained(in_flight: &AtomicUsize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while in_flight.load(Ordering::SeqCst) > 0 {
        if Instant::now() >= deadline {
            eprintln!("[oxide] server 关闭：在途连接未归零，超时继续");
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::JoinHandle;

    /// 唯一临时路径（进程号加测试名）：单进程内唯一，进程间以进程号隔离。
    fn unique_paths(test_name: &str) -> ServerConfig {
        let dir = std::env::temp_dir().join(format!("oxide_server_test_{}_{}", std::process::id(), test_name));
        fs::create_dir_all(&dir).expect("测试目录创建应成功");
        ServerConfig {
            socket_path: dir.join("server.sock"),
            sidecar_path: dir.join("sidecar.json"),
        }
    }

    /// 后台线程起真实 run_server，等 socket 文件出现（bind 成功即就绪）。
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

    /// 连接 server 并返回流。
    fn connect(config: &ServerConfig) -> UnixStream {
        UnixStream::connect(&config.socket_path).expect("连接 server 应成功")
    }

    /// 发送请求帧并读回一帧响应。
    fn send_and_recv(stream: &mut UnixStream, request: &ServerRequest) -> ServerResponse {
        stream
            .write_all(protocol::encode_request(request).as_bytes())
            .expect("写请求应成功");
        let mut reader = FrameReader::new(io::BufReader::new(stream));
        let frame = reader.read_frame().expect("读响应应成功").expect("应读到响应帧");
        protocol::parse_response(&frame).expect("响应帧解析应成功")
    }

    /// 停 server：发关闭请求、等线程退出、断言正常退出。
    fn stop_server(stream: &mut UnixStream, handle: JoinHandle<Result<(), ServerError>>) {
        let response = send_and_recv(stream, &ServerRequest::Shutdown);
        assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");
        let result = handle.join().expect("server 线程应正常退出");
        assert!(result.is_ok(), "server 应正常退出：{result:?}");
    }

    /// 版本请求得 Version 帧，取值与构建期版本一致。
    #[test]
    fn version_response() {
        let config = unique_paths("version");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);
        let response = send_and_recv(&mut stream, &ServerRequest::Version);
        assert!(
            matches!(response, ServerResponse::Version { ref version } if version == env!("CARGO_PKG_VERSION")),
            "版本应与构建期版本一致：{response:?}"
        );
        stop_server(&mut stream, handle);
    }

    /// 状态请求得 Status 帧：空闲数不超过总数、总数不小于 1（标准配置预热 1 个）、运行时长合理。
    #[test]
    fn status_response() {
        let config = unique_paths("status");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);
        let response = send_and_recv(&mut stream, &ServerRequest::Status);
        match response {
            ServerResponse::Status {
                pool_available,
                pool_total,
                uptime_ms,
            } => {
                assert!(pool_available <= pool_total, "空闲数应不超过总数：{pool_available}/{pool_total}");
                assert!(pool_total >= 1, "总数应不小于 1（标准配置预热 1 个）：{pool_total}");
                assert!(uptime_ms < 10_000, "刚启动的 server 运行时长应小于 10 秒：{uptime_ms}");
            }
            other => panic!("应得 Status 帧，实得 {other:?}"),
        }
        stop_server(&mut stream, handle);
    }

    /// 健康请求得 Health 帧（骨架阶段活着即健康）。
    #[test]
    fn health_response() {
        let config = unique_paths("health");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);
        let response = send_and_recv(&mut stream, &ServerRequest::Health);
        assert!(
            matches!(response, ServerResponse::Health { healthy: true }),
            "healthy 应为真：{response:?}"
        );
        stop_server(&mut stream, handle);
    }

    /// 信息请求得 Info 帧：socket 路径与配置一致、进程号为当前进程号。
    #[test]
    fn info_response() {
        let config = unique_paths("info");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);
        let response = send_and_recv(&mut stream, &ServerRequest::Info);
        match response {
            ServerResponse::Info { version, socket_path, pid } => {
                assert_eq!(version, env!("CARGO_PKG_VERSION"), "版本应与构建期版本一致");
                assert_eq!(socket_path, config.socket_path.to_string_lossy().into_owned(), "socket 路径应与配置一致");
                assert_eq!(pid, std::process::id(), "进程号应为当前进程号");
            }
            other => panic!("应得 Info 帧，实得 {other:?}"),
        }
        stop_server(&mut stream, handle);
    }

    /// 关闭请求得 Shutdown 帧，server 线程退出，sidecar 与 socket 文件均被删除。
    #[test]
    fn shutdown_response_and_cleanup() {
        let config = unique_paths("shutdown");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);
        stop_server(&mut stream, handle);
        assert!(!config.sidecar_path.exists(), "sidecar 文件应被删除");
        assert!(!config.socket_path.exists(), "socket 文件应被删除");
    }

    /// 畸形帧得 Error 帧，连接保持可用（后续请求仍正常应答）。
    #[test]
    fn malformed_frame_error_response() {
        let config = unique_paths("malformed");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);

        // 发非 JSON 帧：应得 Error 帧。
        stream.write_all(b"not json at all\n").expect("写畸形帧应成功");
        let mut reader = FrameReader::new(io::BufReader::new(&mut stream));
        let frame = reader.read_frame().expect("读响应应成功").expect("应读到响应帧");
        let response = protocol::parse_response(&frame).expect("响应帧解析应成功");
        assert!(matches!(response, ServerResponse::Error { .. }), "应得 Error 帧：{response:?}");

        // 连接保持可用：后续健康请求正常应答。
        let response = send_and_recv(&mut stream, &ServerRequest::Health);
        assert!(
            matches!(response, ServerResponse::Health { healthy: true }),
            "连接应保持可用：{response:?}"
        );
        stop_server(&mut stream, handle);
    }

    /// 执行请求得 EvalResult 帧："1 + 1" 得完成值 "2"，不 panic。
    #[test]
    fn eval_request_returns_value() {
        let config = unique_paths("eval");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);
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
        stop_server(&mut stream, handle);
    }

    /// 同一连接连续两帧（状态后版本）逐帧应答；同路径第二实例返回认领被拒错误。
    #[test]
    fn multi_frame_same_connection_and_second_instance_refused() {
        let config = unique_paths("multi");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);

        // 连续两帧逐帧应答，顺序保持。
        let first = send_and_recv(&mut stream, &ServerRequest::Status);
        assert!(matches!(first, ServerResponse::Status { .. }), "应得 Status 帧：{first:?}");
        let second = send_and_recv(&mut stream, &ServerRequest::Version);
        assert!(matches!(second, ServerResponse::Version { .. }), "应得 Version 帧：{second:?}");

        // 同路径第二实例：sidecar 已存在且 socket 存活，认领被拒。
        let refused = run_server(&config);
        assert!(
            matches!(refused, Err(ServerError::ClaimRefused(ClaimResult::AlreadyRunning))),
            "第二实例应被拒：{refused:?}"
        );

        stop_server(&mut stream, handle);
    }
}
