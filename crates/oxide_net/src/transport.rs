//! 传输枚举层：`Conn` 与 `Listener` 各包装 Unix 与 TCP 两臂，统一六个常用操作，
//! `Endpoint` 描述监听目标（Unix 路径或 TCP 地址）。
//!
//! 关键约定：
//! - 两臂映射到 std 同名同语义方法，无行为分叉。
//! - `Listener::accept` 返回 `Conn` 并丢弃对端地址，现有 accept 循环不消费地址。
//! - 构造面（bind / connect）不含在本模块，调用方先用 std 原语构造句柄再包进枚举。

use std::io;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 双向连接：Unix 域套接字或 TCP 连接。
///
/// 两臂共享同一操作面，调用方无需感知具体传输。
#[derive(Debug)]
pub enum Conn {
    /// Unix 域套接字臂。
    Unix(UnixStream),
    /// TCP 连接臂。
    Tcp(TcpStream),
}

/// 监听套接字：Unix 域套接字监听者或 TCP 监听者。
#[derive(Debug)]
pub enum Listener {
    /// Unix 域套接字监听者臂。
    Unix(UnixListener),
    /// TCP 监听者臂。
    Tcp(TcpListener),
}

/// 监听端点：Unix 套接字路径或 TCP 地址。
///
/// 是 server 配置中监听目标的单一事实源；Unix 臂是文件，TCP 臂不是。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Unix 套接字路径臂。
    Unix(PathBuf),
    /// TCP 地址臂。
    Tcp(SocketAddr),
}

impl Endpoint {
    /// 返回套接字文件路径：仅 Unix 臂有文件，TCP 臂返回 `None`。
    pub fn socket_path(&self) -> Option<&Path> {
        match self {
            Endpoint::Unix(path) => Some(path),
            Endpoint::Tcp(_) => None,
        }
    }

    /// 返回端点文本形态：Unix 臂为路径字符串，TCP 臂为 host:port 字符串。
    pub fn describe(&self) -> String {
        match self {
            Endpoint::Unix(path) => path.to_string_lossy().into_owned(),
            Endpoint::Tcp(addr) => addr.to_string(),
        }
    }
}

impl std::fmt::Display for Endpoint {
    /// 端点文本形态：Unix 臂为路径字符串，TCP 臂为 host:port 字符串。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

impl Listener {
    /// 设置监听者非阻塞模式。
    ///
    /// # 注意事项
    /// - 置非阻塞后，无待接受连接时 `accept` 返回 `io::ErrorKind::WouldBlock`。
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Listener::Unix(l) => l.set_nonblocking(nonblocking),
            Listener::Tcp(l) => l.set_nonblocking(nonblocking),
        }
    }

    /// 接受一个连接并包装为 `Conn`，丢弃对端地址。
    ///
    /// # 边界与前提
    /// - Unix 与 TCP 臂的对端地址类型不同，本层不暴露；需要来源地址的调用方须扩展签名。
    /// - 非阻塞模式下无待接受连接时返回 `WouldBlock`。
    pub fn accept(&self) -> io::Result<Conn> {
        match self {
            // 对端地址按约定丢弃，只取连接句柄。
            Listener::Unix(l) => Ok(Conn::Unix(l.accept()?.0)),
            Listener::Tcp(l) => Ok(Conn::Tcp(l.accept()?.0)),
        }
    }
}

impl Conn {
    /// 克隆连接，返回共享同一底层套接字的新 `Conn`。
    ///
    /// # 副作用
    /// - 新句柄与原句柄共享同一套接字：写侧共享，读侧相互独立（std `try_clone` 语义）。
    pub fn try_clone(&self) -> io::Result<Conn> {
        match self {
            Conn::Unix(s) => Ok(Conn::Unix(s.try_clone()?)),
            Conn::Tcp(s) => Ok(Conn::Tcp(s.try_clone()?)),
        }
    }

    /// 设置读超时。
    ///
    /// # 边界与前提
    /// - `None` 表示不限时（可无限阻塞）。
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Conn::Unix(s) => s.set_read_timeout(timeout),
            Conn::Tcp(s) => s.set_read_timeout(timeout),
        }
    }

    /// 设置写超时。
    ///
    /// # 边界与前提
    /// - `None` 表示不限时（可无限阻塞）。
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Conn::Unix(s) => s.set_write_timeout(timeout),
            Conn::Tcp(s) => s.set_write_timeout(timeout),
        }
    }

    /// 顺序写入全部字节。
    ///
    /// # 边界与前提
    /// - 空切片为无操作，恒成功。
    /// - 已设超时且单次写入超时时返回 `Elapsed`（std 语义）。
    pub fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Conn::Unix(s) => s.write_all(buf),
            Conn::Tcp(s) => s.write_all(buf),
        }
    }
}

impl Read for Conn {
    /// 从连接读取字节，委托内臂。
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Unix(s) => s.read(buf),
            Conn::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    /// 向连接写入字节，委托内臂。
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Unix(s) => s.write(buf),
            Conn::Tcp(s) => s.write(buf),
        }
    }

    /// 刷新连接，委托内臂。
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Unix(s) => s.flush(),
            Conn::Tcp(s) => s.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Unix 回环：临时目录 bind Unix 套接字，accept 后写读往返，六方法逐一可用。
    #[test]
    fn unix_loopback_roundtrip() {
        let dir = std::env::temp_dir().join(format!("oxide_net_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("unix.sock");
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path).unwrap();
        let mut client = UnixStream::connect(&path).unwrap();

        // 六方法逐一过一遍：非阻塞、accept、克隆、读写超时、write_all。
        let listener = Listener::Unix(listener);
        listener.set_nonblocking(false).unwrap();
        let mut conn = listener.accept().unwrap();
        let _peer = conn.try_clone().unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        conn.set_write_timeout(Some(Duration::from_secs(5))).unwrap();

        conn.write_all(b"hello-oxide").unwrap();
        let mut buf = [0u8; 11];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello-oxide");

        let _ = std::fs::remove_file(&path);
    }

    /// TCP 回环：bind `127.0.0.1:0`（端口 0 由操作系统分配空闲端口），
    /// 经 `local_addr` 取端口，connect 后写读往返。
    #[test]
    fn tcp_loopback_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(addr).unwrap();

        let listener = Listener::Tcp(listener);
        listener.set_nonblocking(false).unwrap();
        let mut conn = listener.accept().unwrap();
        let _peer = conn.try_clone().unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        conn.set_write_timeout(Some(Duration::from_secs(5))).unwrap();

        conn.write_all(b"hello-oxide").unwrap();
        let mut buf = [0u8; 11];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello-oxide");
    }

    /// 非阻塞 accept 空轮：置非阻塞后无连接时 accept 应返回 `WouldBlock`（两臂各验一次）。
    #[test]
    fn nonblocking_accept_would_block() {
        let dir = std::env::temp_dir().join(format!("oxide_net_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("unix_nb.sock");
        let _ = std::fs::remove_file(&path);

        let unix = Listener::Unix(UnixListener::bind(&path).unwrap());
        unix.set_nonblocking(true).unwrap();
        let err = unix.accept().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);

        let tcp = Listener::Tcp(TcpListener::bind("127.0.0.1:0").unwrap());
        tcp.set_nonblocking(true).unwrap();
        let err = tcp.accept().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);

        let _ = std::fs::remove_file(&path);
    }
}
