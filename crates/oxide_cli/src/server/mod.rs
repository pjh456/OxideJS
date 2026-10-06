//! 持久 server 子系统：当前仅含协议数据面。
//!
//! 持久 server 是常驻进程，复用预热池、近零 spawn 成本；
//! 通信走 Unix socket + NDJSON（换行分隔 JSON）。
//! server 骨架、accept 循环与执行路径在后续任务，本模块先立协议数据面
//! （帧格式与 serde 结构体），供 server 与客户端共用。

/// 协议数据面：NDJSON 帧格式与 serde 结构体。
pub mod protocol;
