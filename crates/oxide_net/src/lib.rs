//! OxideJS 引擎无关传输层。
//!
//! `transport` 提供 `Conn` 与 `Listener` 两枚枚举，各包装 Unix 域套接字与 TCP 连接两臂，
//! 统一六个常用操作：非阻塞、accept、克隆、读超时、写超时、write_all。协议层完全
//! 传输无关，本 crate 只提供传输面。
//!
//! 构造面（bind / connect）不含在本 crate：调用方先用 std 原语构造句柄，再包进枚举。
//! 零外部依赖，纯 `std::net` 与 `std::os::unix::net`。

/// 传输枚举层：`Conn`、`Listener` 与 `Endpoint` 及统一操作。
pub mod transport;

/// 重导出 `transport` 模块的 `Conn`、`Listener` 与 `Endpoint` 枚举。
pub use transport::{Conn, Endpoint, Listener};
