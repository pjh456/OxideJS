//! server 进程主体：accept 循环、控制请求直接处理、执行请求路由与优雅退出。
//!
//! 关键约定：
//! - 启动顺序契约（与身份注册一致）：先认领 sidecar，再绑定 socket；
//!   反向会让客户端在 sidecar 缺位时误判无 server 并启动第二实例。
//! - accept 循环为非阻塞 accept 加 10 毫秒轮询加原子关闭标志：唤醒路径
//!   确定、无锁、无唤醒丢失。
//! - 并发模型：主线程只跑 accept 循环；每连接一个标准库线程只做 I/O
//!   （帧读写）；五类控制请求由连接线程直接应答、不占 worker；执行请求
//!   经 mpsc 路由到固定 N 个常驻 worker 执行（各持自己线程上的 VM 池）。
//! - 优雅退出顺序：关闭请求或 SIGINT/SIGTERM 信号置位关闭标志 → 在途归零
//!   → 清空发送端 → join worker → drop 内核 → 删 sidecar 与 socket 文件。
//! - 池本体不是 Send/Sync（Vm 不跨线程），上下文只持池的原子状态句柄；
//!   状态响应读全体 worker 池的共享聚合计数器。

use std::fs;
use std::io;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm_pool::PoolCounters;

use super::protocol::{self, FrameReader, ProtocolError, ServerRequest, ServerResponse};
use super::sidecar::{self, ClaimResult};
use super::workers::{self, WorkerRouter};

/// server 启动与运行错误：绑定失败、认领被拒、接受连接失败、读取身份失败、
/// 信号处理器注册失败。
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
    /// worker 线程异常退出（panic 载荷文本）。
    WorkerPanic(String),
    /// 信号处理器注册失败（系统级错误，非"已注册"的幂等情形）。
    SignalInstallFailed(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::ClaimRefused(result) => write!(f, "sidecar 认领被拒：{result:?}"),
            ServerError::BindFailed(msg) => write!(f, "socket 绑定失败：{msg}"),
            ServerError::AcceptFailed(msg) => write!(f, "接受连接失败：{msg}"),
            ServerError::IdentityReadFailed(msg) => write!(f, "读取身份失败：{msg}"),
            ServerError::WorkerPanic(msg) => write!(f, "worker 线程异常退出：{msg}"),
            ServerError::SignalInstallFailed(msg) => write!(f, "信号处理器注册失败：{msg}"),
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
    /// 常驻 worker 线程数；缺省取宿主核数，下限钳制为 1。
    pub worker_count: usize,
}

impl ServerConfig {
    /// 生产默认配置：socket 与 sidecar 取 well-known 位置，worker 数取宿主核数。
    pub fn well_known() -> Self {
        ServerConfig {
            socket_path: sidecar::well_known_socket_path(),
            sidecar_path: sidecar::well_known_sidecar_path(),
            worker_count: default_worker_count(),
        }
    }
}

/// 缺省 worker 数：宿主核数，下限钳制为 1。
fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// server 运行上下文：共享内核、启动时刻、关闭标志、在途计数、池聚合计数器。
///
/// 内核跨线程共享（`Arc<KernelCore>` 是 Send/Sync）；池本体不进上下文
/// （Vm 不跨线程，池归各 worker 线程所有），状态响应读共享聚合计数器。
struct ServerContext {
    // 只持不读：余留连接线程（在途排水超时后仍存活）持本上下文，内核句柄
    // 随上下文存活，不悬垂。
    #[allow(dead_code)]
    kernel: Arc<KernelCore>,
    started_at: Instant,
    shutdown: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    pool_counters: Arc<PoolCounters>,
    socket_path: String,
}

/// server 进程主体：认领 sidecar → 注册信号处理器 → 绑定 socket → 建内核 →
/// 派生 worker → accept 循环 → 在途归零 → 清空发送端 → join worker →
/// drop 内核 → 删 sidecar 与 socket 文件。
///
/// # 步骤
/// 1. 认领 sidecar（启动顺序契约：先写 sidecar 再 bind socket）。
/// 2. 注册信号处理器（绑定 socket 之前；失败回滚 sidecar，此时无存活线程）。
/// 3. 绑定监听器；失败且路径存在时删 stale socket 文件重试一次。
/// 4. 建内核（标准配置）与共享聚合计数器。
/// 5. 派生 N 个常驻 worker（各在自己线程上建自有池）。
/// 6. 进入 accept 循环；关闭标志置位后按关闭序列退出。
///
/// # 边界与前提
/// - 认领被拒（AlreadyRunning / RefusedAmbiguous）立即返回错误，不双注册。
/// - 信号处理器注册失败（系统级错误）回滚 sidecar 并以错误返回。
/// - stale socket 文件删除重试后再失败时回滚删除 sidecar 并以错误返回。
///
/// # 副作用
/// - 创建 sidecar 与 socket 文件；正常退出时删除两者。
/// - 注册进程级信号处理器（SIGINT、SIGTERM，termination 特性下含 SIGHUP）。
/// - 派生 N 个常驻 worker 线程与每连接一个 I/O 线程。
///
/// # 注意事项
/// - 在调用线程上运行（后续 CLI 骨架任务在主线程分派到它）。
/// - 关闭标志由关闭请求或 SIGINT/SIGTERM 信号置位。
/// - 关闭序列固定：清空发送端 → join 全部 worker → drop 内核（内核 drop
///   的调试断言要求存活 VM 数为零，worker 池随线程退出先释放，前提满足）。
pub fn run_server(config: &ServerConfig) -> Result<(), ServerError> {
    // 认领 sidecar：已有存活 server 或存活证据不足即返回错误。
    match sidecar::claim_sidecar(&config.sidecar_path, &config.socket_path) {
        ClaimResult::Registered => {}
        other => return Err(ServerError::ClaimRefused(other)),
    }

    // 关闭标志提前创建并注册信号处理器：在绑定 socket 之前，socket 文件
    // 出现即蕴含处理器已就位（集成测试的信号投递无竞态）；失败回滚
    // sidecar，此时进程内没有存活线程。
    let shutdown = Arc::new(AtomicBool::new(false));
    if let Err(err) = install_signal_handler(&shutdown) {
        let _ = fs::remove_file(&config.sidecar_path);
        return Err(err);
    }

    // 绑定监听器：stale socket 文件删一次重试，失败回滚 sidecar。
    let listener = bind_listener(config)?;

    // 建内核（标准配置）与共享聚合计数器（全体 worker 池的增减累加到同一聚合）。
    let kernel = KernelCore::new(KernelConfig::standard());
    let counters = PoolCounters::shared();

    // 派生 N 个常驻 worker：各持自己线程上建的池（Vm 不跨线程）。
    let (router, worker_handles) = workers::spawn_workers(
        Arc::clone(&kernel),
        Arc::clone(&counters),
        kernel.config.min_pool_size,
        kernel.config.max_pool_size,
        config.worker_count,
    );
    let router = Arc::new(router);

    let ctx = Arc::new(ServerContext {
        kernel: Arc::clone(&kernel),
        started_at: Instant::now(),
        shutdown: Arc::clone(&shutdown),
        in_flight: Arc::new(AtomicUsize::new(0)),
        pool_counters: counters,
        socket_path: config.socket_path.to_string_lossy().into_owned(),
    });

    // accept 循环：非阻塞 accept 加 10 毫秒轮询加原子关闭标志。
    let accept_result = accept_loop(&listener, &ctx, &router);

    // 在途归零：轮询至零，5 秒安全超时后记警告继续（余留连接线程持内核
    // 与路由器的共享句柄，不悬垂；其后续投递经清空的路由立即失败）。
    wait_in_flight_drained(&ctx.in_flight, Duration::from_secs(5));

    // 关闭序列：清空发送端 → worker 的 recv 断开退出 → 逐一 join。
    router.close();
    for handle in worker_handles {
        if let Err(payload) = handle.join() {
            return Err(ServerError::WorkerPanic(panic_payload_str(&payload)));
        }
    }

    // 按序释放：上下文（持内核句柄）→ 内核（worker 池已随线程退出释放）。
    drop(ctx);
    drop(kernel);

    // 删 sidecar 与 socket 文件（尽力而为，文件已不存在不视为错误）。
    let _ = fs::remove_file(&config.sidecar_path);
    let _ = fs::remove_file(&config.socket_path);

    accept_result
}

/// 信号处理器体：置位关闭标志。
///
/// # 边界与前提
/// - 只做一次原子写，不分配、不取锁、不做 I/O；即使按最严格的信号上下文
///   标准衡量也是安全的。
fn signal_shutdown(shutdown: &Arc<AtomicBool>) {
    shutdown.store(true, Ordering::SeqCst);
}

/// 注册 SIGINT/SIGTERM 信号处理器：信号到达时置位关闭标志。
///
/// # 步骤
/// 1. 闭包捕获关闭标志的共享句柄，转调 `signal_shutdown`。
/// 2. `ctrlc::set_handler` 注册；"已注册"的幂等情形视为成功，其余系统
///    错误映射为 `SignalInstallFailed`。
///
/// # 边界与前提
/// - 处理器是进程级全局的（同一进程只允许一次注册）；生产路径一个进程
///   只有一个 server，注册一次即成功。
///
/// # 副作用
/// - 注册进程级信号处理器（SIGINT、SIGTERM，termination 特性下含 SIGHUP）。
///
/// # 注意事项
/// - 须在绑定 socket 之前调用：socket 文件出现即蕴含处理器已就位，
///   集成测试的信号投递无竞态。
/// - 测试进程内多 server 并存时，仅首个注册者接上信号，其余返回"已注册"
///   幂等成功；单测不发真实信号，无干扰。
fn install_signal_handler(shutdown: &Arc<AtomicBool>) -> Result<(), ServerError> {
    let flag = Arc::clone(shutdown);
    match ctrlc::set_handler(move || signal_shutdown(&flag)) {
        Ok(()) => Ok(()),
        // 进程级处理器已注册（测试进程内多 server 并存的幂等情形）。
        Err(ctrlc::Error::MultipleHandlers) => Ok(()),
        Err(e) => Err(ServerError::SignalInstallFailed(e.to_string())),
    }
}

/// 从 `catch_unwind` 的 panic payload 提取可读文本：`&str` 与 `String` 两种
/// 常见形态，其余返回占位文本。
fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "non-string payload".into()
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
/// - 每连接派生一个标准库线程（只做 I/O），线程结束时减一在途计数。
fn accept_loop(
    listener: &UnixListener,
    ctx: &Arc<ServerContext>,
    router: &Arc<WorkerRouter>,
) -> Result<(), ServerError> {
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
                let router = Arc::clone(router);
                std::thread::spawn(move || {
                    handle_connection(stream, &ctx, &router);
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
/// 3. 畸形帧以 `Error` 帧答复后继续；执行请求路由到 worker 并阻塞等回复；
///    控制请求直接应答并写回。
/// 4. 关闭确认帧写回即退出循环。
///
/// # 边界与前提
/// - 半开连接（对端静默不关）由 30 秒读超时有界化。
/// - 本线程只做 I/O 与分派，不建池（池归 worker 线程所有）。
///
/// # 副作用
/// - 关闭请求会置位关闭标志。
fn handle_connection(stream: UnixStream, ctx: &ServerContext, router: &WorkerRouter) {
    // 克隆写端：原流交给帧读取器，写端独立用于响应写回。
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };

    // 30 秒读超时：半开连接不永久占住线程。
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));

    let mut reader = FrameReader::new(io::BufReader::new(stream));
    // 读帧：EOF、读超时、非法 UTF-8 均退出。
    while let Ok(Some(frame)) = reader.read_frame() {
        // 分派：帧长超限按协议约定关连接；畸形帧以 Error 帧答复后继续；
        // 执行请求走 worker 路由，控制请求直接应答。
        let response = match protocol::parse_request(&frame) {
            Ok(ServerRequest::Eval { code, max_steps }) => dispatch_eval(&code, max_steps, router),
            Ok(request) => dispatch_control(&request, ctx),
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

/// 执行请求分派：路由到 worker 执行，阻塞在一次性回复通道上取回响应帧。
///
/// # 步骤
/// 1. 建一次性回复通道，发送端随任务进 worker。
/// 2. 轮询投递到 worker；发送端已清空（关闭序列进行中）时立即以错误帧答复。
/// 3. 阻塞 recv 至回复；worker 已退出（通道断开）时以错误帧答复，不挂起。
fn dispatch_eval(code: &str, max_steps: Option<u64>, router: &WorkerRouter) -> ServerResponse {
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    let task = workers::WorkerTask::Eval {
        code: code.to_string(),
        max_steps,
        reply: reply_tx,
    };
    match router.route(task) {
        Ok(()) => reply_rx.recv().unwrap_or_else(|_| ServerResponse::eval_err("worker 已退出，执行请求未处理")),
        Err(_) => ServerResponse::eval_err("server 正在关闭，执行请求未受理"),
    }
}

/// 控制请求分派：五类控制请求直接应答，不占 worker。
///
/// Status 响应读全体 worker 池的共享聚合计数器。
fn dispatch_control(request: &ServerRequest, ctx: &ServerContext) -> ServerResponse {
    match request {
        ServerRequest::Version => ServerResponse::Version {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        ServerRequest::Status => ServerResponse::Status {
            pool_available: ctx.pool_counters.available(),
            pool_total: ctx.pool_counters.total(),
            pool_peak: ctx.pool_counters.peak(),
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
        // 防御性兜底：正常路径 eval 在调用本函数前已被拦截走 worker 路由；
        // 若到达此臂说明上游分流缺失，以错误帧答复而非静默执行。
        ServerRequest::Eval { .. } => ServerResponse::eval_err("执行请求应经 worker 路由，不应到达控制分派"),
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
    /// worker 数取小值 2：测试在宿主核数上跑，大值徒增预热成本。
    fn unique_paths(test_name: &str) -> ServerConfig {
        let dir = std::env::temp_dir().join(format!("oxide_server_test_{}_{}", std::process::id(), test_name));
        fs::create_dir_all(&dir).expect("测试目录创建应成功");
        ServerConfig {
            socket_path: dir.join("server.sock"),
            sidecar_path: dir.join("sidecar.json"),
            worker_count: 2,
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

    /// 信号处理器体：置位关闭标志。
    #[test]
    fn signal_shutdown_sets_flag() {
        let shutdown = Arc::new(AtomicBool::new(false));
        signal_shutdown(&shutdown);
        assert!(shutdown.load(Ordering::SeqCst), "标志应被置位");
    }

    /// 注册信号处理器：返回成功（"已注册"的幂等情形同样视为成功）。
    #[test]
    fn install_signal_handler_registers() {
        let shutdown = Arc::new(AtomicBool::new(false));
        install_signal_handler(&shutdown).expect("信号处理器注册应成功");
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

    /// 状态请求得 Status 帧：空闲数不超过总数、总数不小于 1（标准配置每 worker 预热 1 个）、运行时长合理。
    #[test]
    fn status_response() {
        let config = unique_paths("status");
        let handle = start_server(config.clone());
        let mut stream = connect(&config);

        // worker 池异步预热：轮询至聚合总数不小于 1（预热完成）。
        let deadline = Instant::now() + Duration::from_secs(5);
        let response = loop {
            let response = send_and_recv(&mut stream, &ServerRequest::Status);
            if let ServerResponse::Status { pool_total, .. } = &response {
                if *pool_total >= 1 {
                    break response;
                }
            }
            assert!(Instant::now() < deadline, "worker 池预热应完成");
            std::thread::sleep(Duration::from_millis(10));
        };

        match response {
            ServerResponse::Status {
                pool_available,
                pool_total,
                pool_peak,
                uptime_ms,
            } => {
                assert!(pool_available <= pool_total, "空闲数应不超过总数：{pool_available}/{pool_total}");
                assert!(pool_total >= 1, "总数应不小于 1（标准配置预热 1 个）：{pool_total}");
                assert!(pool_peak <= pool_total, "峰值应不超过总数：{pool_peak}/{pool_total}");
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

    /// 端到端并发：两条连接各发 3 个 eval，6 个响应帧全部正确且逐连接顺序与请求一致。
    #[test]
    fn concurrent_evals_across_connections() {
        let config = unique_paths("concurrent_evals");
        let handle = start_server(config.clone());
        let mut stream_a = connect(&config);
        let mut stream_b = connect(&config);

        // 两条连接各发 3 个 eval，响应逐帧按请求顺序送达。
        for code in ["1 + 1", "2 * 3", "10 - 4"] {
            let response = send_and_recv(&mut stream_a, &ServerRequest::Eval { code: code.into(), max_steps: None });
            assert!(
                matches!(response, ServerResponse::EvalResult { ref value, .. } if value.is_some()),
                "连接 A 应得完成值：{response:?}"
            );
        }
        for code in ["21 / 7", "2 + 30", "100 % 9"] {
            let response = send_and_recv(&mut stream_b, &ServerRequest::Eval { code: code.into(), max_steps: None });
            assert!(
                matches!(response, ServerResponse::EvalResult { ref value, .. } if value.is_some()),
                "连接 B 应得完成值：{response:?}"
            );
        }

        // 完成值逐条核对（顺序与请求一致）。
        let expected_a = ["2", "6", "6"];
        let expected_b = ["3", "32", "1"];
        for (code, expected) in [
            ("1 + 1", expected_a[0]),
            ("2 * 3", expected_a[1]),
            ("10 - 4", expected_a[2]),
        ] {
            let response = send_and_recv(&mut stream_a, &ServerRequest::Eval { code: code.into(), max_steps: None });
            match response {
                ServerResponse::EvalResult { ref value, .. } => assert_eq!(value.as_deref(), Some(expected), "{code} 应得 {expected}"),
                other => panic!("应得 EvalResult 帧：{other:?}"),
            }
        }
        for (code, expected) in [
            ("21 / 7", expected_b[0]),
            ("2 + 30", expected_b[1]),
            ("100 % 9", expected_b[2]),
        ] {
            let response = send_and_recv(&mut stream_b, &ServerRequest::Eval { code: code.into(), max_steps: None });
            match response {
                ServerResponse::EvalResult { ref value, .. } => assert_eq!(value.as_deref(), Some(expected), "{code} 应得 {expected}"),
                other => panic!("应得 EvalResult 帧：{other:?}"),
            }
        }

        drop(stream_a);
        stop_server(&mut stream_b, handle);
    }

    /// 控制请求不占 worker：一条连接发长 eval 占住 worker 期间，
    /// 另一连接发 Status 立即返回（不受 eval 阻塞）。
    #[test]
    fn control_request_not_blocked_by_eval() {
        let config = unique_paths("control_not_blocked");
        let handle = start_server(config.clone());
        let mut stream_a = connect(&config);
        let mut stream_b = connect(&config);

        // 连接 A 发长 eval（步数上限使执行有界，占住 worker 数百毫秒）。
        stream_a
            .write_all(
                protocol::encode_request(&ServerRequest::Eval {
                    code: "for(;;){}".into(),
                    max_steps: Some(20_000_000),
                })
                .as_bytes(),
            )
            .expect("写长 eval 应成功");

        // 连接 B 发 Status：控制请求直接应答，须在有界时间内返回。
        let started = Instant::now();
        let response = send_and_recv(&mut stream_b, &ServerRequest::Status);
        assert!(started.elapsed() < Duration::from_secs(3), "Status 不应被 eval 阻塞");
        assert!(matches!(response, ServerResponse::Status { .. }), "应得 Status 帧：{response:?}");

        // 连接 A 的长 eval 最终得步数超限错误帧。
        let mut reader = FrameReader::new(io::BufReader::new(&mut stream_a));
        let frame = reader.read_frame().expect("读响应应成功").expect("应读到响应帧");
        let response = protocol::parse_response(&frame).expect("响应帧解析应成功");
        match response {
            ServerResponse::EvalResult { ref error, .. } => {
                let err = error.as_deref().expect("长 eval 应得步数超限错误");
                assert!(err.contains("step limit"), "错误应含 step limit：{err}");
            }
            other => panic!("应得 EvalResult 帧：{other:?}"),
        }

        drop(stream_a);
        stop_server(&mut stream_b, handle);
    }

    /// 关闭序列：在有在途 eval 时发关闭请求，server 线程正常退出，
    /// sidecar 与 socket 文件均被删除。
    #[test]
    fn shutdown_with_in_flight_eval() {
        let config = unique_paths("shutdown_inflight");
        let handle = start_server(config.clone());
        let mut stream_a = connect(&config);
        let mut stream_b = connect(&config);

        // 连接 A 发长 eval（在途）；连接 B 发关闭请求。
        stream_a
            .write_all(
                protocol::encode_request(&ServerRequest::Eval {
                    code: "for(;;){}".into(),
                    max_steps: Some(20_000_000),
                })
                .as_bytes(),
            )
            .expect("写长 eval 应成功");
        let response = send_and_recv(&mut stream_b, &ServerRequest::Shutdown);
        assert!(matches!(response, ServerResponse::Shutdown), "应得关闭确认帧：{response:?}");

        // 关闭连接 A：在途连接线程写回响应后退出，在途归零。
        drop(stream_a);
        drop(stream_b);

        let result = handle.join().expect("server 线程应正常退出");
        assert!(result.is_ok(), "server 应正常退出：{result:?}");
        assert!(!config.sidecar_path.exists(), "sidecar 文件应被删除");
        assert!(!config.socket_path.exists(), "socket 文件应被删除");
    }
}
