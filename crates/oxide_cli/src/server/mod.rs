//! 持久 server 子系统：协议数据面、身份注册与 server 进程主体。
//!
//! 持久 server 是常驻进程，复用预热池、近零 spawn 成本；
//! 通信走 Unix socket + NDJSON（换行分隔 JSON）。
//! 模块分三层：协议数据面（帧格式与 serde 结构体）、身份注册（sidecar 文件
//! 与 liveness 检查原语）、server 进程主体（accept 循环、控制请求直接处理、
//! 优雅退出）。执行路径与并发模型在后续任务。

/// 协议数据面：NDJSON 帧格式与 serde 结构体。
pub mod protocol;
/// server 进程主体：accept 循环、控制请求分派与优雅退出。
#[allow(clippy::module_inception)]
pub mod server;
/// 身份注册：sidecar 文件、liveness 检查与陈旧清理。
pub mod sidecar;
