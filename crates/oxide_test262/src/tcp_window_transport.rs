//! 远程监督窗口传输：`WindowTransport` 的 Tcp 臂。监督者 `accept` 一条 worker
//! 连接，经该连接发 assign 帧、收事件帧；断连（EOF / 真实 io 错误 / 畸形帧）
//! 映射为 `PollResult::Exited`（合成退出码），走与本地子进程死亡完全同构的恢复
//! 路径。
//!
//! 写读侧分离：`writer` 是 `try_clone` 出的写侧句柄，`reader` 包住底层连接读侧
//! （含余量缓冲）。`kill` 后须 drop 两侧句柄真正断连，否则连接半开。
//!
//! 计数合并契约：监督者每窗口只合并一次最终心跳（DONE，带全窗累计计数）；
//! 中间 COMPLETED 心跳只用于超时跟踪（last_index），不合并计数。`Done` 帧把
//! `last_heartbeat` 的 phase 改 DONE 并保留计数；worker 发 Done 前无 Heartbeat
//! 时合成零计数 DONE。
//!
//! 非阻塞读陷阱：`set_read_timeout(0)` 后 `WouldBlock` 必须判「无数据、仍存活」
//! 而非断连，否则正常窗口被误杀重启；仅真实 io 错误与 EOF 判断连。残行留在
//! `FrameReader` 余量缓冲，跨 poll 不丢。
//!
//! 认证门：`accept` 后、发 assign 帧前，读首帧（带读超时，半开连接不永久占
//! 线程），必须是 `Auth` 事件帧且 token 异或判等匹配，才写回 `AuthOk` 帧进
//! 窗口循环；不匹配或首帧非 `Auth` 直接断连、不回帧（不向攻击者泄露差异）。

use std::collections::HashMap;
use std::io::BufReader;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;

use crate::heartbeat::Heartbeat;
use crate::shard_protocol::{encode_command, parse_event, FrameReader, ShardCommand, ShardEvent, AUTH_READ_TIMEOUT};
use crate::stats::RunStats;
use crate::window_transport::{PollResult, SpawnError, WindowTransport};
use oxide_net::{Conn, Listener};

/// 远程监督窗口传输：`WindowTransport` 的 Tcp 臂。`listener` 供跨子进程重启
/// 重新 `accept`；`writer` / `reader` 是当前 worker 连接的写读两侧句柄。
pub(crate) struct TcpWindowTransport {
    listener: Arc<Listener>,
    /// 写侧句柄（`try_clone` 出，与读侧共享底层套接字）。
    writer: Option<Conn>,
    /// 读侧（含余量缓冲，跨 poll 保留残行）。
    reader: Option<FrameReader<BufReader<Conn>>>,
    /// worker 认证 token（监督者启动时生成、前台打印，用户带外注入 worker）。
    token: String,
    /// 认证门读超时（缺省 30 秒，半开连接不永久占线程）。
    auth_read_timeout: Duration,
    /// 最近一次心跳帧（中间 COMPLETED 只跟踪不合并；Done 帧改 phase 为 DONE）。
    last_heartbeat: Option<Heartbeat>,
    /// 待并入的失败记录帧（一测试失败一帧，按序消费，无偏移态，天然无双计）。
    pending_fails: Vec<(usize, String, String, String)>,
    /// 最近一次现场帧（last-pc 现场行加可选日志尾）。
    last_scene: Option<(String, Option<String>)>,
    /// 全量路径文本（assign 时切窗口片随帧携带，worker 读不到监督者本机文件）。
    paths: Vec<String>,
    /// 单测试超时，随 assign 帧携带供 worker 侧看门狗使用。
    timeout_secs: u64,
}

impl TcpWindowTransport {
    /// 构造传输：`listener` 是监督者绑定端点后的监听句柄（跨重启 `accept` 用），
    /// `paths` 是全量路径文本，`timeout_secs` 随 assign 帧携带，`token` 是 worker
    /// 认证 token（认证门异或判等基准）。
    pub(crate) fn new(listener: Arc<Listener>, paths: Vec<String>, timeout_secs: u64, token: String) -> Self {
        Self {
            listener,
            writer: None,
            reader: None,
            last_heartbeat: None,
            pending_fails: Vec::new(),
            last_scene: None,
            paths,
            timeout_secs,
            token,
            auth_read_timeout: AUTH_READ_TIMEOUT,
        }
    }

    /// 写读两侧句柄都 drop，真正断连（防连接半开）。
    fn drop_conn(&mut self) {
        self.writer = None;
        self.reader = None;
    }

    /// Done 帧处理：把 `last_heartbeat` 的 phase 改 DONE 并保留计数（每窗口只
    /// 合并一次的计数契约），随后断连判死亡。
    fn mark_done(&mut self) {
        match self.last_heartbeat.as_mut() {
            Some(hb) => hb.phase = "DONE".to_string(),
            None => {
                // worker 发 Done 前无 Heartbeat：合成零计数 DONE。
                self.last_heartbeat = Some(Heartbeat {
                    phase: "DONE".to_string(),
                    index: 0,
                    pass: 0,
                    fail: 0,
                    skip: 0,
                    categories: HashMap::new(),
                    hb_write_errors: 0,
                });
            }
        }
        self.drop_conn();
    }
}

/// 共享 token 判等：逐字节异或累加，避免逐字节短路带来的时序侧信道。
///
/// # 边界与前提
/// - 长度不等直接判不等（生产 token 长度固定 64 位十六进制）。
fn tokens_match(expected: &str, given: &str) -> bool {
    if expected.len() != given.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in expected.bytes().zip(given.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl WindowTransport for TcpWindowTransport {
    fn assign(&mut self, wstart: usize, wend: usize) -> Result<(), SpawnError> {
        // 无连接时阻塞 `accept` 取一条 worker 连接（io 错误包成 SpawnError）。
        if self.writer.is_none() {
            let base = self.listener.accept().map_err(SpawnError)?;
            // 认证门：读超时内读首帧（半开连接不永久占线程），必须是 Auth 事件
            // 帧且 token 异或判等匹配，才写回 AuthOk；不匹配或首帧非 Auth 直接
            // 断连、不回帧（reader 连同底层连接一起 drop，连接即关）。
            base.set_read_timeout(Some(self.auth_read_timeout)).map_err(SpawnError)?;
            let mut reader = FrameReader::new(BufReader::new(base));
            let first = match reader.read_frame() {
                Ok(Some(frame)) => frame,
                _ => {
                    return Err(SpawnError(std::io::Error::new(
                        ErrorKind::ConnectionAborted,
                        "worker auth failed or timed out",
                    )))
                }
            };
            let auth_ok = match parse_event(&first) {
                Ok(ShardEvent::Auth { token }) => tokens_match(&self.token, &token),
                _ => false,
            };
            if !auth_ok {
                return Err(SpawnError(std::io::Error::new(
                    ErrorKind::ConnectionAborted,
                    "worker auth token mismatch",
                )));
            }
            // 写回 AuthOk：worker 据以继续收发。
            let mut writer = reader.get_mut().get_mut().try_clone().map_err(SpawnError)?;
            let frame = encode_command(&ShardCommand::AuthOk);
            writer.write_all(frame.as_bytes()).map_err(SpawnError)?;
            // 握手完成：清读超时，读侧置非阻塞（主循环 poll 用非阻塞读，
            // std 拒绝 0 时长读超时，改用 `set_nonblocking` 使无数据读返回
            // `WouldBlock`，判「无数据、仍存活」，不误杀正常窗口）。
            reader.get_mut().get_mut().set_read_timeout(None).map_err(SpawnError)?;
            match reader.get_mut().get_mut() {
                Conn::Tcp(s) => s.set_nonblocking(true).map_err(SpawnError)?,
                Conn::Unix(s) => s.set_nonblocking(true).map_err(SpawnError)?,
            }
            self.writer = Some(writer);
            self.reader = Some(reader);
        }

        // 发 assign 帧：窗口路径片随帧携带（worker 读不到监督者本机临时文件）。
        let window_paths = self.paths[wstart..wend].to_vec();
        let cmd = ShardCommand::Assign {
            wstart,
            wend,
            paths: window_paths,
            timeout_secs: self.timeout_secs,
        };
        let frame = encode_command(&cmd);
        let writer = self.writer.as_mut().expect("assign 刚建立写侧句柄");
        writer.write_all(frame.as_bytes()).map_err(SpawnError)?;
        Ok(())
    }

    fn heartbeat(&self) -> Option<Heartbeat> {
        // `Heartbeat` 未派生 `Clone`（心跳模块零改动），逐字段手工克隆。
        self.last_heartbeat.as_ref().map(|hb| Heartbeat {
            phase: hb.phase.clone(),
            index: hb.index,
            pass: hb.pass,
            fail: hb.fail,
            skip: hb.skip,
            categories: hb.categories.clone(),
            hb_write_errors: hb.hb_write_errors,
        })
    }

    fn merge_fail(&mut self, stats: &mut RunStats) {
        // 排空待并入失败帧逐条重建 FailRecord（帧按序消费，无偏移态，无双计）。
        for (index, category, subkey, message) in self.pending_fails.drain(..) {
            stats.push_fail_record(index, category, subkey, message);
        }
    }

    fn scene(&self) -> Option<String> {
        self.last_scene.as_ref().map(|(line, _)| line.clone())
    }

    fn kill(&mut self) {
        // 发 kill 帧后 drop 写读两侧句柄真正断连（否则连接半开）。
        if let Some(writer) = self.writer.as_mut() {
            let frame = encode_command(&ShardCommand::Kill);
            let _ = writer.write_all(frame.as_bytes());
        }
        self.drop_conn();
    }

    fn poll(&mut self) -> PollResult {
        // 先取帧（读侧借用在此结束，帧是拥有值），再改状态，避免借用冲突。
        let frame_result = match self.reader.as_mut() {
            Some(reader) => reader.read_frame(),
            None => return PollResult::Alive,
        };
        match frame_result {
            Ok(Some(frame)) => match parse_event(&frame) {
                Ok(ShardEvent::Heartbeat {
                    phase,
                    index,
                    pass,
                    fail,
                    skip,
                    categories,
                    hb_write_errors,
                }) => {
                    // 中间心跳只跟踪不合并计数（合并契约：每窗口只合并一次 DONE）。
                    self.last_heartbeat = Some(Heartbeat {
                        phase,
                        index,
                        pass,
                        fail,
                        skip,
                        categories,
                        hb_write_errors,
                    });
                }
                Ok(ShardEvent::Fail {
                    index,
                    category,
                    subkey,
                    message,
                }) => {
                    self.pending_fails.push((index, category, subkey, message));
                }
                Ok(ShardEvent::Scene { line, log_tail }) => {
                    self.last_scene = Some((line, log_tail));
                }
                Ok(ShardEvent::Done) => {
                    // 窗口结束：改末次心跳 phase 为 DONE 保留计数，断连判死亡
                    // （与本地子进程退出同构）。
                    self.mark_done();
                    return PollResult::Exited(1);
                }
                Ok(ShardEvent::Auth { .. }) => {
                    // Auth 只应作首帧且在认证门消费；此处到达是协议违例，
                    // 判断连（同畸形帧同构恢复）。
                    self.drop_conn();
                    return PollResult::Exited(1);
                }
                Err(_) => {
                    // 畸形帧：对端状态不定，判断连（同 EOF 同构恢复）。
                    self.drop_conn();
                    return PollResult::Exited(1);
                }
            },
            Ok(None) => {
                // EOF：对端关闭，判死亡。
                self.drop_conn();
                return PollResult::Exited(1);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                // 非阻塞读无数据：仍存活，下一拍重试（不得误判断连）。
                return PollResult::Alive;
            }
            Err(_) => {
                // 真实 io 错误：判断连。
                self.drop_conn();
                return PollResult::Exited(1);
            }
        }
        PollResult::Alive
    }

    fn log_tail(&self) -> Option<String> {
        self.last_scene.as_ref().and_then(|(_, log_tail)| log_tail.clone())
    }

    fn reset_round(&mut self) {
        // 清事件缓冲（心跳 / 待并入失败帧 / 现场归零），跨重启不保留。
        self.last_heartbeat = None;
        self.pending_fails.clear();
        self.last_scene = None;
    }

    fn archive_window(&mut self) {
        // 远程无文件归档：失败记录已并入 stats，无数据丢失；drop 连接即收尾。
        self.drop_conn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use crate::shard_protocol::{encode_event, parse_command};

    const TEST_TOKEN: &str = "test-token-0123456789abcdef";

    /// 回环测试建监听：bind 动态端口，返回监听句柄与本地地址。
    fn bind_test_listener() -> (Arc<Listener>, std::net::SocketAddr) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 失败");
        let addr = listener.local_addr().expect("local_addr 失败");
        (Arc::new(Listener::Tcp(listener)), addr)
    }

    /// 派生客户端线程过认证门：客户端线程连接、发 Auth 帧、阻塞读 AuthOk 帧，
    /// 主线程调用 `assign`（accept 加读 Auth 加写回 AuthOk）使其解除阻塞，
    /// 客户端线程把读侧经通道送回（assign 帧留在余量缓冲，测试无需消费）。
    ///
    /// # 注意事项
    /// - 客户端必须在独立线程：主线程阻塞读 AuthOk 时 `assign` 无人调用即死锁。
    fn spawn_authed_client(addr: std::net::SocketAddr) -> std::sync::mpsc::Receiver<FrameReader<BufReader<TcpStream>>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut client = TcpStream::connect(addr).expect("客户端连接失败");
            client
                .write_all(encode_event(&ShardEvent::Auth { token: TEST_TOKEN.to_string() }).as_bytes())
                .expect("写 Auth 帧失败");
            let mut reader = FrameReader::new(BufReader::new(client));
            let first = reader.read_frame().expect("读帧失败").expect("应读到帧");
            assert!(matches!(parse_command(&first), Ok(ShardCommand::AuthOk)), "首帧应是 AuthOk：{first}");
            tx.send(reader).expect("送回读侧失败");
        });
        rx
    }

    /// 经客户端读侧句柄写事件帧（写侧与读侧共享底层套接字）。
    fn client_write(reader: &mut FrameReader<BufReader<TcpStream>>, event: &ShardEvent) {
        reader
            .get_mut()
            .get_mut()
            .write_all(encode_event(event).as_bytes())
            .expect("写事件帧失败");
    }

    /// TcpWindowTransport 回环：bind 动态端口，客户端连接过认证门后发
    /// Heartbeat / Fail / Scene 帧，断言 heartbeat / scene / log_tail /
    /// merge_fail 读回正确；客户端断连后 poll 返回 Exited（合成码）。
    #[test]
    fn tcp_transport_loopback() {
        let (listener, addr) = bind_test_listener();

        let mut transport = TcpWindowTransport::new(
            Arc::clone(&listener),
            vec!["a/b.js".to_string(), "c/d.js".to_string()],
            10,
            TEST_TOKEN.to_string(),
        );

        // assign：accept 客户端连接、过认证门、发 assign 帧。
        let client_rx = spawn_authed_client(addr);
        transport.assign(0, 2).expect("assign 应成功");
        let mut client = client_rx.recv().expect("收回客户端读侧失败");

        // 发 Heartbeat / Fail / Scene 帧（按序，一测试失败一帧）。
        client_write(
            &mut client,
            &ShardEvent::Heartbeat {
                phase: "COMPLETED".into(),
                index: 0,
                pass: 1,
                fail: 0,
                skip: 0,
                categories: HashMap::new(),
                hb_write_errors: 0,
            },
        );
        client_write(
            &mut client,
            &ShardEvent::Fail {
                index: 0,
                category: "vm: not callable".into(),
                subkey: "x".into(),
                message: "x is not callable".into(),
            },
        );
        client_write(
            &mut client,
            &ShardEvent::Scene {
                line: "pc=1 op=ADD frames=1".into(),
                log_tail: Some("log line".into()),
            },
        );

        // 读帧：poll 逐帧消费，直到 Scene（末帧）读回。
        let deadline = Instant::now() + Duration::from_secs(5);
        while transport.scene().is_none() && Instant::now() < deadline {
            match transport.poll() {
                PollResult::Exited(_) | PollResult::WaitError(_) => break,
                PollResult::Alive => {}
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(transport.scene().is_some(), "现场帧应读回");
        let hb = transport.heartbeat().expect("心跳帧应读回");
        assert_eq!(hb.phase, "COMPLETED");
        assert_eq!(hb.index, 0);
        assert_eq!(hb.pass, 1);
        assert_eq!(transport.scene(), Some("pc=1 op=ADD frames=1".to_string()));
        assert_eq!(transport.log_tail(), Some("log line".to_string()));

        // merge_fail 排空待并入失败帧；二次合并不重复（无偏移态，排空即空）。
        let mut stats = RunStats::default();
        transport.merge_fail(&mut stats);
        assert_eq!(stats.fail_records.len(), 1, "应并入一条失败记录");
        assert_eq!(stats.fail_records[0].index, 0);
        transport.merge_fail(&mut stats);
        assert_eq!(stats.fail_records.len(), 1, "二次合并不应重复");

        // 客户端断连：poll 返回 Exited（合成码）。
        drop(client);
        let mut exited = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !exited && Instant::now() < deadline {
            if matches!(transport.poll(), PollResult::Exited(_)) {
                exited = true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(exited, "断连后 poll 应返回 Exited");
    }

    /// Done 帧：把末次心跳 phase 改 DONE 保留计数，poll 返回 Exited；worker 发
    /// Done 前无 Heartbeat 时合成零计数 DONE。
    #[test]
    fn tcp_transport_done_frame_keeps_counts() {
        let (listener, addr) = bind_test_listener();

        let mut transport =
            TcpWindowTransport::new(Arc::clone(&listener), vec!["a/b.js".to_string()], 10, TEST_TOKEN.to_string());
        let client_rx = spawn_authed_client(addr);
        transport.assign(0, 1).expect("assign 应成功");
        let mut client = client_rx.recv().expect("收回客户端读侧失败");

        // 先发一条带计数的心跳，再发 Done：Done 应把 phase 改 DONE 并保留计数。
        client_write(
            &mut client,
            &ShardEvent::Heartbeat {
                phase: "COMPLETED".into(),
                index: 0,
                pass: 5,
                fail: 2,
                skip: 1,
                categories: HashMap::new(),
                hb_write_errors: 0,
            },
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while transport.heartbeat().is_none() && Instant::now() < deadline {
            let _ = transport.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        client_write(&mut client, &ShardEvent::Done);

        // poll 读到 Done 帧应返回 Exited，且心跳 phase 改 DONE、计数保留。
        let mut exited = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !exited && Instant::now() < deadline {
            if matches!(transport.poll(), PollResult::Exited(_)) {
                exited = true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(exited, "Done 帧后 poll 应返回 Exited");
        let hb = transport.heartbeat().expect("Done 后心跳应可读回");
        assert_eq!(hb.phase, "DONE", "Done 帧应把 phase 改 DONE");
        assert_eq!(hb.pass, 5, "Done 帧应保留末次心跳计数");
        assert_eq!(hb.fail, 2);
        assert_eq!(hb.skip, 1);
    }

    /// Done 帧前无 Heartbeat：合成零计数 DONE（不 panic、计数全零）。
    #[test]
    fn tcp_transport_done_without_heartbeat_synthesizes_zero() {
        let (listener, addr) = bind_test_listener();

        let mut transport =
            TcpWindowTransport::new(Arc::clone(&listener), vec!["a/b.js".to_string()], 10, TEST_TOKEN.to_string());
        let client_rx = spawn_authed_client(addr);
        transport.assign(0, 1).expect("assign 应成功");
        let mut client = client_rx.recv().expect("收回客户端读侧失败");

        client_write(&mut client, &ShardEvent::Done);
        let mut exited = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !exited && Instant::now() < deadline {
            if matches!(transport.poll(), PollResult::Exited(_)) {
                exited = true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(exited, "Done 帧后 poll 应返回 Exited");
        let hb = transport.heartbeat().expect("无心跳 Done 应合成零计数 DONE");
        assert_eq!(hb.phase, "DONE");
        assert_eq!(hb.pass, 0);
        assert_eq!(hb.fail, 0);
        assert_eq!(hb.skip, 0);
        assert!(hb.categories.is_empty());
    }

    /// 认证门：token 不匹配时 assign 报错，客户端读首帧得 EOF（断连不回帧）。
    #[test]
    fn tcp_transport_auth_mismatch_disconnects() {
        let (listener, addr) = bind_test_listener();

        let mut transport =
            TcpWindowTransport::new(Arc::clone(&listener), vec!["a/b.js".to_string()], 10, TEST_TOKEN.to_string());
        let mut client = TcpStream::connect(addr).expect("客户端连接失败");
        client
            .write_all(
                encode_event(&ShardEvent::Auth {
                    token: "wrong-token".to_string(),
                })
                .as_bytes(),
            )
            .expect("写 Auth 帧失败");
        let err = transport.assign(0, 1).expect_err("token 不匹配应报错");
        assert!(err.0.to_string().contains("mismatch"), "错误应说明 token 不匹配：{err:?}");

        // 断连不回帧：客户端读首帧得 EOF。
        let mut reader = FrameReader::new(BufReader::new(client));
        assert_eq!(reader.read_frame().expect("读帧应成功"), None, "断连后应得 EOF");
    }

    /// 认证门：首帧非 Auth（发心跳帧）时 assign 报错，客户端读首帧得 EOF。
    #[test]
    fn tcp_transport_auth_first_frame_not_auth_disconnects() {
        let (listener, addr) = bind_test_listener();

        let mut transport =
            TcpWindowTransport::new(Arc::clone(&listener), vec!["a/b.js".to_string()], 10, TEST_TOKEN.to_string());
        let mut client = TcpStream::connect(addr).expect("客户端连接失败");
        client
            .write_all(encode_event(&ShardEvent::Done).as_bytes())
            .expect("写首帧失败");
        let err = transport.assign(0, 1).expect_err("首帧非 Auth 应报错");
        assert!(err.0.to_string().contains("mismatch"), "错误应说明认证失败：{err:?}");

        let mut reader = FrameReader::new(BufReader::new(client));
        assert_eq!(reader.read_frame().expect("读帧应成功"), None, "断连后应得 EOF");
    }

    /// 认证门：静默连接（不发首帧）在读超时后 assign 报错，客户端读首帧得 EOF。
    #[test]
    fn tcp_transport_auth_silence_times_out() {
        let (listener, addr) = bind_test_listener();

        let mut transport =
            TcpWindowTransport::new(Arc::clone(&listener), vec!["a/b.js".to_string()], 10, TEST_TOKEN.to_string());
        // 测试用短读超时：半开连接 200 ms 内即断，不占满 30 秒缺省值。
        transport.auth_read_timeout = Duration::from_millis(200);
        let client = TcpStream::connect(addr).expect("客户端连接失败");
        let err = transport.assign(0, 1).expect_err("静默连接应读超时");
        assert!(err.0.kind() == ErrorKind::ConnectionAborted, "错误应是连接中止：{err:?}");

        let mut reader = FrameReader::new(BufReader::new(client));
        assert_eq!(reader.read_frame().expect("读帧应成功"), None, "断连后应得 EOF");
    }
}
