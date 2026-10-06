//! 持久 server 子系统：协议数据面、身份注册、执行路径、worker 池与 server 进程主体。
//!
//! 持久 server 是常驻进程，复用预热池、近零 spawn 成本；
//! 通信走 Unix socket + NDJSON（换行分隔 JSON）。
//! 模块分五层：协议数据面（帧格式与 serde 结构体）、身份注册（sidecar 文件
//! 与 liveness 检查原语）、执行请求路径（parse → compile → spawn → run →
//! format → drop 六段纯函数）、worker 池（固定 N 常驻 worker、各持自有 VM
//! 池、mpsc 轮询路由）、server 进程主体（accept 循环、控制请求直接处理、
//! 执行请求路由、优雅退出）。

/// 执行请求路径：parse → compile → spawn → run → format → drop 六段纯函数。
pub mod eval;
/// 协议数据面：NDJSON 帧格式与 serde 结构体。
pub mod protocol;
/// server 进程主体：accept 循环、控制请求分派、执行请求路由与优雅退出。
#[allow(clippy::module_inception)]
pub mod server;
/// 身份注册：sidecar 文件、liveness 检查与陈旧清理。
pub mod sidecar;
/// worker 池：固定 N 常驻 worker、各持自有 VM 池、mpsc 轮询路由。
pub mod workers;
