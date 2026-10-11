//! 分片帧协议：远程分片的 NDJSON 帧格式与 serde 结构。
//!
//! 帧约定：每帧一行 JSON，以换行符（U+000A）终止，UTF-8 编码。
//! 两个方向的帧均由 `type` 字段区分类型（tagged 枚举，snake_case 取值）：
//! `ShardCommand` 是监督者到 worker 方向（assign / kill / auth_ok），
//! `ShardEvent` 是 worker 到监督者方向（auth / heartbeat / fail / scene / done）。
//! 帧长上限 `MAX_FRAME_BYTES`；超限帧读侧提前停止，避免无界内存增长。
//! 畸形帧（非法 JSON、未知类型、载荷不匹配、空帧）统一返回 `ShardProtocolError`，
//! 任何路径不 panic。
//!
//! 写序契约由 worker 侧执行：同一测试的心跳帧先于失败帧，协议模块本身无状态。
//! assign 帧携带窗口路径清单切片：worker 读不到监督者本机临时文件，窗口内容
//! 必须随帧携带。
//!
//! 认证门契约：worker 连接后首帧必须是 `Auth` 事件帧（携带 token），监督者
//! 异或判等匹配后写回 `AuthOk` 命令帧才进入窗口循环；不匹配或首帧非 `Auth`
//! 直接断连、不回帧。读首帧带 `AUTH_READ_TIMEOUT` 读超时，半开连接不永久
//! 占线程。

use std::collections::HashMap;
use std::io::BufRead;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// 帧长上限（字节，1 MiB）：超过即判 `FrameTooLarge`，调用方应关闭连接。
pub(crate) const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 认证门读超时（30 秒）：半开连接不发认证帧不永久占线程或进程。
pub(crate) const AUTH_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 协议错误：畸形帧或未知类型，统一返回、不 panic。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShardProtocolError {
    /// 帧不是合法 JSON，附语法错误描述。
    InvalidJson(String),
    /// 帧是合法 JSON，但 `type` 字段缺失或不是已知协议类型，附原始取值。
    UnknownType(String),
    /// `type` 已知但载荷字段与结构不符（缺字段、类型不匹配），附错误描述。
    MalformedPayload(String),
    /// 帧为空（仅空白字符）。
    EmptyFrame,
    /// 帧超过长度上限（见 `MAX_FRAME_BYTES`）。
    FrameTooLarge,
}

impl std::fmt::Display for ShardProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShardProtocolError::InvalidJson(msg) => write!(f, "帧不是合法 JSON：{msg}"),
            ShardProtocolError::UnknownType(tag) => write!(f, "未知协议类型：{tag}"),
            ShardProtocolError::MalformedPayload(msg) => write!(f, "帧载荷不匹配：{msg}"),
            ShardProtocolError::EmptyFrame => write!(f, "帧为空"),
            ShardProtocolError::FrameTooLarge => write!(f, "帧超过长度上限"),
        }
    }
}

/// 命令帧（监督者到 worker）：窗口指派与 kill。
///
/// 序列化为 tagged 对象，`type` 字段区分类型（snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ShardCommand {
    /// 窗口指派：携带该窗口的路径清单切片与单测试超时。
    /// `paths` 是窗口路径清单切片（worker 读不到监督者本机临时文件，须随帧携带）；
    /// `timeout_secs` 是单测试超时，供 worker 侧看门狗使用。
    Assign {
        wstart: usize,
        wend: usize,
        paths: Vec<String>,
        timeout_secs: u64,
    },
    /// kill：无载荷。
    Kill,
    /// 认证通过确认：无载荷。监督者认证门匹配后写回，worker 据此进入主循环。
    AuthOk,
}

/// 事件帧（worker 到监督者）：心跳、失败记录、现场、窗口结束。
///
/// 序列化为 tagged 对象，`type` 字段区分类型（snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ShardEvent {
    /// 心跳：字段与心跳记录逐一对应；`categories` 是失败分类计数快照，
    /// 序列化为 JSON 对象（键可含冒号与空格）。
    Heartbeat {
        phase: String,
        index: usize,
        pass: usize,
        fail: usize,
        skip: usize,
        categories: HashMap<String, usize>,
        hb_write_errors: usize,
    },
    /// 失败记录：一测试失败一帧，字段与旁路失败行四元组逐一对应。
    Fail {
        index: usize,
        category: String,
        subkey: String,
        message: String,
    },
    /// 现场：last-pc 现场行，加可选的 sidecar 日志尾。
    Scene {
        line: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        log_tail: Option<String>,
    },
    /// 窗口结束：无载荷（窗口结束下标由 assign 帧的 wend 隐含）。
    Done,
    /// 认证：worker 连接后的首帧，携带共享 token（监督者前台打印、
    /// 用户带外注入，不经网络分发）。
    Auth { token: String },
}

/// 命令帧可取的 `type` 值（须与 `ShardCommand` 变体保持同步）。
const COMMAND_TAGS: &[&str] = &["assign", "kill", "auth_ok"];

/// 事件帧可取的 `type` 值（须与 `ShardEvent` 变体保持同步）。
const EVENT_TAGS: &[&str] = &["heartbeat", "fail", "scene", "done", "auth"];

/// 序列化命令为帧字符串（含结尾换行）。
///
/// 命令结构字段皆为基础类型，序列化不会失败；`expect` 是安全网而非错误路径。
pub(crate) fn encode_command(cmd: &ShardCommand) -> String {
    let mut frame = serde_json::to_string(cmd).expect("命令结构序列化不会失败");
    frame.push('\n');
    frame
}

/// 序列化事件为帧字符串（含结尾换行）。
///
/// 事件结构字段皆为基础类型，序列化不会失败；`expect` 是安全网而非错误路径。
pub(crate) fn encode_event(ev: &ShardEvent) -> String {
    let mut frame = serde_json::to_string(ev).expect("事件结构序列化不会失败");
    frame.push('\n');
    frame
}

/// 解析单行命令帧（结尾换行可有可无）。
///
/// # 步骤
/// 1. 去首尾空白；空帧判 `EmptyFrame`，超长判 `FrameTooLarge`。
/// 2. 解析为 JSON；语法错误判 `InvalidJson`。
/// 3. 读取 `type` 字段；缺失或未知判 `UnknownType`（附原始取值）。
/// 4. 反序列化为 `ShardCommand`；载荷不匹配判 `MalformedPayload`。
///
/// # 边界与前提
/// - 输入是单帧（一行）；分行由读侧负责（见 `FrameReader`）。
/// - 未知类型保留原始 `type` 取值，供调用方引用。
pub(crate) fn parse_command(line: &str) -> Result<ShardCommand, ShardProtocolError> {
    parse_frame(line, COMMAND_TAGS)
}

/// 解析单行事件帧（结尾换行可有可无）。
///
/// 步骤与判定同 `parse_command`，目标结构为 `ShardEvent`。
pub(crate) fn parse_event(line: &str) -> Result<ShardEvent, ShardProtocolError> {
    parse_frame(line, EVENT_TAGS)
}

fn parse_frame<T: DeserializeOwned>(line: &str, known_tags: &[&str]) -> Result<T, ShardProtocolError> {
    // 去首尾空白：帧自带的换行与行尾空白不参与解析。
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(ShardProtocolError::EmptyFrame);
    }
    if trimmed.len() > MAX_FRAME_BYTES {
        return Err(ShardProtocolError::FrameTooLarge);
    }

    // 先解析为通用 Value，以便先检查 `type` 字段再定型。
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| ShardProtocolError::InvalidJson(e.to_string()))?;

    // 读取 `type` 字段：缺失或非字符串取值原样上报。
    let tag = match value.get("type") {
        None => "<missing>".to_string(),
        Some(t) => t.as_str().map(str::to_string).unwrap_or_else(|| t.to_string()),
    };
    if !known_tags.contains(&tag.as_str()) {
        return Err(ShardProtocolError::UnknownType(tag));
    }

    // 类型已知：反序列化为目标结构，载荷不匹配判 `MalformedPayload`。
    serde_json::from_value(value).map_err(|e| ShardProtocolError::MalformedPayload(e.to_string()))
}

/// 帧读取器：从行流读取 NDJSON 帧，跳过空行。
///
/// 流可能一次交还超过一行的数据（带缓冲的读侧），读取器内部持有余量缓冲，
/// 调用方不得绕过读取器直接读底层流。
pub(crate) struct FrameReader<R: BufRead> {
    inner: R,
    /// 上次读取后尚未消费成帧的余量数据。
    pending: Vec<u8>,
}

impl<R: BufRead> FrameReader<R> {
    /// 以底层流构造读取器。
    pub(crate) fn new(inner: R) -> Self {
        FrameReader { inner, pending: Vec::new() }
    }

    /// 暴露底层读流（供调用方设读超时或克隆写句柄）。
    ///
    /// # 注意事项
    /// - 调用方不得绕过读取器直接读底层流（余量缓冲会丢数据）；
    ///   只允许经此取句柄做超时设置与写侧克隆。
    pub(crate) fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// 读取一帧（跳过空行）。
    ///
    /// # 步骤
    /// 1. 取一行：优先消费余量缓冲，不足时按 4 KiB 块从流补足到换行或 EOF。
    /// 2. 行长超过上限即提前停止读取，流停在行中间，调用方应判 `FrameTooLarge`
    ///    并关闭连接，不得在同一流上继续读。
    /// 3. 空行（仅空白）跳过；EOF 返回 `Ok(None)`。
    ///
    /// # 边界与前提
    /// - 帧必须是合法 UTF-8，否则返回 `InvalidData` io 错误。
    /// - 超长提前停止后流停在行中间，调用方必须关闭连接。
    ///
    /// # 副作用
    /// - 只从流读取，并维护余量缓冲。
    pub(crate) fn read_frame(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let line = match self.take_line()? {
                None => return Ok(None),
                Some(line) => line,
            };

            // UTF-8 校验：协议帧必须是合法 UTF-8 文本。
            let text = String::from_utf8(line)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "帧不是合法 UTF-8"))?;
            if text.trim().is_empty() {
                continue;
            }
            return Ok(Some(text));
        }
    }

    /// 取一行（含结尾换行），或 EOF 时的残行。
    ///
    /// 余量缓冲已含换行时直接切出首行；缓冲无换行且超上限时提前停止；
    /// 否则从流读一块追加，直到凑齐一行或 EOF。
    fn take_line(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            // 余量缓冲已含换行：切出首行，余下进下一轮。
            if let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
                let line = std::mem::take(&mut self.pending);
                let (first, rest) = line.split_at(pos + 1);
                self.pending = rest.to_vec();
                return Ok(Some(first.to_vec()));
            }

            // 余量缓冲无换行且已超上限：行必然超长，提前停止。
            if self.pending.len() > MAX_FRAME_BYTES {
                return Ok(Some(std::mem::take(&mut self.pending)));
            }

            // 从流读一块追加到余量缓冲。
            // 信号中断（EINTR）不视为错误：重试读，保持读循环不变式。
            let mut chunk = [0u8; 4096];
            let n = match self.inner.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if n == 0 {
                // EOF：余量缓冲的残行原样交付（可能为空）。
                let line = std::mem::take(&mut self.pending);
                return Ok(if line.is_empty() { None } else { Some(line) });
            }
            self.pending.extend_from_slice(&chunk[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 命令帧 round-trip：编码后解析应得回原结构。
    fn roundtrip_command(cmd: &ShardCommand) {
        let frame = encode_command(cmd);
        assert!(frame.ends_with('\n'), "帧必须以换行终止");
        let parsed = parse_command(&frame).expect("命令帧 round-trip 应成功");
        assert_eq!(parsed, *cmd);
    }

    /// 事件帧 round-trip：编码后解析应得回原结构。
    fn roundtrip_event(ev: &ShardEvent) {
        let frame = encode_event(ev);
        assert!(frame.ends_with('\n'), "帧必须以换行终止");
        let parsed = parse_event(&frame).expect("事件帧 round-trip 应成功");
        assert_eq!(parsed, *ev);
    }

    /// 命令 round-trip：assign（带路径清单与超时）与 kill。
    #[test]
    fn command_roundtrip() {
        roundtrip_command(&ShardCommand::Assign {
            wstart: 0,
            wend: 10,
            paths: vec!["a/b.js".into(), "c/d.js".into()],
            timeout_secs: 10,
        });
        roundtrip_command(&ShardCommand::Kill);
        roundtrip_command(&ShardCommand::AuthOk);
    }

    /// 事件 round-trip：心跳（带失败类别）、失败记录、现场（带与不带日志尾）、窗口结束。
    #[test]
    fn event_roundtrip() {
        let mut categories = HashMap::new();
        categories.insert("vm: not callable".to_string(), 2);
        roundtrip_event(&ShardEvent::Heartbeat {
            phase: "COMPLETED".into(),
            index: 3,
            pass: 2,
            fail: 1,
            skip: 0,
            categories,
            hb_write_errors: 1,
        });
        roundtrip_event(&ShardEvent::Fail {
            index: 5,
            category: "vm: not callable".into(),
            subkey: "x".into(),
            message: "x is not callable".into(),
        });
        roundtrip_event(&ShardEvent::Scene {
            line: "pc=1 op=ADD frames=1".into(),
            log_tail: None,
        });
        roundtrip_event(&ShardEvent::Scene {
            line: "pc=1 op=ADD frames=1".into(),
            log_tail: Some("line a\nline b".into()),
        });
        roundtrip_event(&ShardEvent::Done);
        roundtrip_event(&ShardEvent::Auth {
            token: "0123456789abcdef".into(),
        });
    }

    /// 序列化形态：钉住精确 JSON 字符串，含 categories 键带冒号与空格的形态。
    #[test]
    fn serialization_shape() {
        assert_eq!(
            serde_json::to_string(&ShardCommand::Assign {
                wstart: 0,
                wend: 10,
                paths: vec!["a/b.js".into(), "c/d.js".into()],
                timeout_secs: 10,
            })
            .unwrap(),
            r#"{"type":"assign","wstart":0,"wend":10,"paths":["a/b.js","c/d.js"],"timeout_secs":10}"#
        );
        assert_eq!(serde_json::to_string(&ShardCommand::Kill).unwrap(), r#"{"type":"kill"}"#);
        assert_eq!(serde_json::to_string(&ShardCommand::AuthOk).unwrap(), r#"{"type":"auth_ok"}"#);

        let mut categories = HashMap::new();
        categories.insert("vm: not callable".to_string(), 1);
        assert_eq!(
            serde_json::to_string(&ShardEvent::Heartbeat {
                phase: "COMPLETED".into(),
                index: 3,
                pass: 2,
                fail: 1,
                skip: 0,
                categories,
                hb_write_errors: 0,
            })
            .unwrap(),
            r#"{"type":"heartbeat","phase":"COMPLETED","index":3,"pass":2,"fail":1,"skip":0,"categories":{"vm: not callable":1},"hb_write_errors":0}"#
        );
        assert_eq!(
            serde_json::to_string(&ShardEvent::Fail {
                index: 5,
                category: "vm: not callable".into(),
                subkey: "x".into(),
                message: "x is not callable".into(),
            })
            .unwrap(),
            r#"{"type":"fail","index":5,"category":"vm: not callable","subkey":"x","message":"x is not callable"}"#
        );
        assert_eq!(
            serde_json::to_string(&ShardEvent::Scene {
                line: "pc=1 op=ADD frames=1".into(),
                log_tail: None
            })
            .unwrap(),
            r#"{"type":"scene","line":"pc=1 op=ADD frames=1"}"#
        );
        assert_eq!(
            serde_json::to_string(&ShardEvent::Scene {
                line: "pc=1 op=ADD frames=1".into(),
                log_tail: Some("a\nb".into()),
            })
            .unwrap(),
            r#"{"type":"scene","line":"pc=1 op=ADD frames=1","log_tail":"a\nb"}"#
        );
        assert_eq!(serde_json::to_string(&ShardEvent::Done).unwrap(), r#"{"type":"done"}"#);
        assert_eq!(
            serde_json::to_string(&ShardEvent::Auth {
                token: "0123456789abcdef".into()
            })
            .unwrap(),
            r#"{"type":"auth","token":"0123456789abcdef"}"#
        );
    }

    /// 非法 JSON：判 `InvalidJson`，不 panic。
    #[test]
    fn malformed_json() {
        let err = parse_command("not json at all").unwrap_err();
        assert!(matches!(err, ShardProtocolError::InvalidJson(_)), "应判 InvalidJson：{err}");
        let err = parse_event("not json at all").unwrap_err();
        assert!(matches!(err, ShardProtocolError::InvalidJson(_)), "应判 InvalidJson：{err}");
    }

    /// 未知 `type`：判 `UnknownType` 且保留原始取值。
    #[test]
    fn unknown_type() {
        let err = parse_command(r#"{"type":"bogus"}"#).unwrap_err();
        assert_eq!(err, ShardProtocolError::UnknownType("bogus".into()));
        let err = parse_event(r#"{"type":"bogus"}"#).unwrap_err();
        assert_eq!(err, ShardProtocolError::UnknownType("bogus".into()));
    }

    /// 缺失 `type`：判 `UnknownType` 且取值为 `<missing>`。
    #[test]
    fn missing_type() {
        let err = parse_command(r#"{"wstart":0}"#).unwrap_err();
        assert_eq!(err, ShardProtocolError::UnknownType("<missing>".into()));
        let err = parse_event(r#"{"index":1}"#).unwrap_err();
        assert_eq!(err, ShardProtocolError::UnknownType("<missing>".into()));
    }

    /// 载荷不匹配：`type` 已知但字段缺失或类型错，判 `MalformedPayload`。
    #[test]
    fn malformed_payload() {
        let err = parse_command(r#"{"type":"assign"}"#).unwrap_err();
        assert!(matches!(err, ShardProtocolError::MalformedPayload(_)), "应判 MalformedPayload：{err}");
        let err = parse_command(r#"{"type":"assign","wstart":0,"wend":10,"paths":[],"timeout_secs":"x"}"#).unwrap_err();
        assert!(matches!(err, ShardProtocolError::MalformedPayload(_)), "应判 MalformedPayload：{err}");
        let err = parse_event(r#"{"type":"fail"}"#).unwrap_err();
        assert!(matches!(err, ShardProtocolError::MalformedPayload(_)), "应判 MalformedPayload：{err}");
        let err = parse_event(
            r#"{"type":"heartbeat","phase":"COMPLETED","index":1,"pass":0,"fail":0,"skip":0,"categories":{}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ShardProtocolError::MalformedPayload(_)), "应判 MalformedPayload：{err}");
    }

    /// 空帧（空串与纯空白）：判 `EmptyFrame`。
    #[test]
    fn empty_frame() {
        assert_eq!(parse_command("").unwrap_err(), ShardProtocolError::EmptyFrame);
        assert_eq!(parse_command("   \t ").unwrap_err(), ShardProtocolError::EmptyFrame);
        assert_eq!(parse_event("").unwrap_err(), ShardProtocolError::EmptyFrame);
    }

    /// 超长帧：判 `FrameTooLarge`。
    #[test]
    fn oversized_frame() {
        let line = format!("{{\"type\":\"kill\",\"pad\":\"{}\"}}", "a".repeat(MAX_FRAME_BYTES));
        assert_eq!(parse_command(&line).unwrap_err(), ShardProtocolError::FrameTooLarge);
        let line = format!("{{\"type\":\"done\",\"pad\":\"{}\"}}", "a".repeat(MAX_FRAME_BYTES));
        assert_eq!(parse_event(&line).unwrap_err(), ShardProtocolError::FrameTooLarge);
    }

    /// 帧读取：跳过空行，返回非空帧（含结尾换行）；流一次交还多行时不丢数据。
    #[test]
    fn read_frame_skips_blank_lines() {
        let mut reader = FrameReader::new(Cursor::new(b"\n   \n{\"type\":\"kill\"}\n".to_vec()));
        let frame = reader.read_frame().expect("读帧应成功").expect("应读到帧");
        assert_eq!(frame, "{\"type\":\"kill\"}\n");
        assert_eq!(reader.read_frame().expect("EOF 应得 None"), None);
    }

    /// 帧读取：一次读取交还多帧时逐帧交付，余量不丢。
    #[test]
    fn read_frame_multiple_frames_in_one_read() {
        let mut reader = FrameReader::new(Cursor::new(b"{\"type\":\"kill\"}\n{\"type\":\"done\"}\n".to_vec()));
        assert_eq!(reader.read_frame().expect("读帧应成功").expect("应读到帧"), "{\"type\":\"kill\"}\n");
        assert_eq!(reader.read_frame().expect("读帧应成功").expect("应读到帧"), "{\"type\":\"done\"}\n");
        assert_eq!(reader.read_frame().expect("EOF 应得 None"), None);
    }

    /// 帧读取：EOF 前无换行的残行也作为一帧交付。
    #[test]
    fn read_frame_unterminated_line_at_eof() {
        let mut reader = FrameReader::new(Cursor::new(b"{\"type\":\"done\"}".to_vec()));
        let frame = reader.read_frame().expect("读帧应成功").expect("应读到帧");
        assert_eq!(frame, "{\"type\":\"done\"}");
    }

    /// 帧读取：超长行提前停止，返回长度超上限的残行，解析判 `FrameTooLarge`。
    #[test]
    fn read_frame_oversized_stops_early() {
        let payload = vec![b'a'; MAX_FRAME_BYTES + 8192];
        let mut reader = FrameReader::new(Cursor::new(payload));
        let frame = reader.read_frame().expect("读帧应成功").expect("应读到帧");
        assert!(frame.len() > MAX_FRAME_BYTES, "超长行应提前停止并返回残行");
        assert_eq!(parse_command(&frame).unwrap_err(), ShardProtocolError::FrameTooLarge);
    }

    /// 帧读取：非法 UTF-8 判 `InvalidData` io 错误，不 panic。
    #[test]
    fn read_frame_non_utf8() {
        let mut reader = FrameReader::new(Cursor::new(vec![0xff, 0xfe, b'\n']));
        let err = reader.read_frame().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
