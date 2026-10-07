//! 持久 server 子系统：协议数据面、身份注册、执行路径、worker 池与 server 进程主体。
//!
//! 持久 server 是常驻进程，复用预热池、近零 spawn 成本；
//! 通信走 Unix socket + NDJSON（换行分隔 JSON）。
//! 模块分十一层：协议数据面（帧格式与 serde 结构体）、身份注册（sidecar 文件
//! 与 liveness 检查原语）、启动时 liveness 扫描（扫陈旧 sidecar 与 socket、
//! 僵尸态自动恢复）、控制客户端（读 sidecar、连接 server、发控制请求
//! 帧、读回响应帧、三分支错误）、执行请求路径（parse → compile → spawn →
//! run → format → drop 六段纯函数）、worker 池（固定 N 常驻 worker、各持
//! 自有 VM 池、mpsc 轮询路由）、server 进程主体（accept 循环、控制请求直接
//! 处理、执行请求路由、优雅退出）、日志读取（行级别解析、阈值加最后 N 行
//! 过滤、阻塞式跟踪）、forge 状态查询（读四张共享 forge 条目数与容量、执行
//! gc / clear-cache / lookup 三旗标）、spawn 与就绪探测（start、restart、
//! watchdog 三条路径共用的唯一共享入口）、watchdog（前台监控进程、三态裁决
//! 加崩溃预算、崩溃后自动重启）。

/// 人工兜底清理命令：扫 sidecar、探活、杀 PID、删孤儿文件。
pub mod cleanup;
/// 控制客户端：读 sidecar、连接 server、发一帧控制请求、读回一帧响应。
pub mod client;
/// 执行请求路径：parse → compile → spawn → run → format → drop 六段纯函数。
pub mod eval;
/// forge 状态查询：读四张共享 forge 条目数与容量，执行 gc / clear-cache /
/// lookup 三旗标。
pub mod forge;
/// 启动时 liveness 扫描：扫陈旧 sidecar 与 socket，僵尸态自动恢复。
pub mod liveness;
/// 日志读取：行级别解析、阈值加最后 N 行过滤、整文件读取、阻塞式跟踪。
pub mod log;
/// 协议数据面：NDJSON 帧格式与 serde 结构体。
pub mod protocol;
/// server 进程主体：accept 循环、控制请求分派、执行请求路由、信号处理
/// （SIGINT/SIGTERM 置位关闭标志）、版本管理（sidecar 记构建版本，启动时
/// 与存活 server 版本比对：匹配不交接、不匹配强制交接）、交接协议（向旧
/// server 发 yield 请求使其排空退出、新 server 接管 socket 路径）、--rm
/// 独立模式（进程唯一 socket 路径、不注册 sidecar、不碰全局路径、单连接、
/// 空闲超时或断开即退出）与优雅退出。
#[allow(clippy::module_inception)]
pub mod server;
/// 身份注册：sidecar 文件、liveness 检查与陈旧清理。
pub mod sidecar;
/// spawn 与就绪探测：start、restart、watchdog 三条路径共用的唯一共享入口。
pub mod spawn;
/// watchdog：前台监控进程，三态裁决加崩溃预算，崩溃后自动重启。
pub mod watchdog;
/// worker 池：固定 N 常驻 worker、各持自有 VM 池、mpsc 轮询路由。
pub mod workers;
