//! 持久 server 子系统：当前含协议数据面与身份注册。
//!
//! 持久 server 是常驻进程，复用预热池、近零 spawn 成本；
//! 通信走 Unix socket + NDJSON（换行分隔 JSON）。
//! server 骨架、accept 循环与执行路径在后续任务，本模块先立协议数据面
//! （帧格式与 serde 结构体）与身份注册（sidecar 文件与 liveness 检查原语），
//! 供 server 与客户端共用。

/// 协议数据面：NDJSON 帧格式与 serde 结构体。
pub mod protocol;
/// 身份注册：sidecar 文件、liveness 检查与陈旧清理。
pub mod sidecar;
