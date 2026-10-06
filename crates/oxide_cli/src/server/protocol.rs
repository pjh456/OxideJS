//! 持久 server 协议数据面：NDJSON 帧格式与 serde 结构体。
//!
//! 帧约定：每帧一行 JSON，以换行符（U+000A）终止，UTF-8 编码。
//! 请求帧与响应帧均由 `type` 字段区分类型（tagged 枚举，snake_case 取值）。
//! 帧长上限 `MAX_FRAME_BYTES`；超限帧读侧提前停止，避免无界内存增长。
//! 畸形帧（非法 JSON、未知类型、载荷不匹配、空帧）统一返回 `ProtocolError`，
//! server 侧以 `Error` 响应帧答复，任何路径不 panic。

use std::io::BufRead;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// 帧长上限（字节，1 MiB）：超过即判 `FrameTooLarge`，调用方应关闭连接。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 协议错误：畸形帧或未知类型，统一返回、不 panic。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
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

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::InvalidJson(msg) => write!(f, "帧不是合法 JSON：{msg}"),
            ProtocolError::UnknownType(tag) => write!(f, "未知协议类型：{tag}"),
            ProtocolError::MalformedPayload(msg) => write!(f, "帧载荷不匹配：{msg}"),
            ProtocolError::EmptyFrame => write!(f, "帧为空"),
            ProtocolError::FrameTooLarge => write!(f, "帧超过长度上限"),
        }
    }
}

impl ProtocolError {
    /// 把协议错误转成响应帧，供 server 答复畸形帧。
    pub fn to_error_frame(&self) -> ServerResponse {
        ServerResponse::Error { message: self.to_string() }
    }
}

/// server 请求帧：eval 执行请求加五类控制请求。
///
/// 序列化为 tagged 对象，`type` 字段区分类型（snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerRequest {
    /// 执行请求：携带源码，可选最大指令数。
    Eval {
        code: String,
        /// 最大指令数；None 表示用引擎默认。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_steps: Option<u64>,
    },
    /// 查询 server 版本。
    Version,
    /// 查询 server 状态（池与运行时长）。
    Status,
    /// 健康检查。
    Health,
    /// 查询 server 信息（版本、socket 路径、进程号）。
    Info,
    /// 优雅关闭。
    Shutdown,
}

/// server 响应帧：每类请求一个响应，外加协议错误帧。
///
/// 序列化为 tagged 对象，`type` 字段区分类型（snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerResponse {
    /// 执行结果：完成值渲染文本与错误消息至多一个非空。
    EvalResult {
        /// 完成值渲染文本；出错时为 None。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        /// 错误消息；成功时为 None。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// 版本响应。
    Version { version: String },
    /// 状态响应：池可用/总数与运行时长。
    Status {
        pool_available: usize,
        pool_total: usize,
        uptime_ms: u64,
    },
    /// 健康检查响应。
    Health { healthy: bool },
    /// 信息响应：版本、socket 路径、进程号。
    Info { version: String, socket_path: String, pid: u32 },
    /// 关闭确认。
    Shutdown,
    /// 协议错误帧：畸形帧或未知类型，server 以该帧答复。
    Error { message: String },
}

impl ServerResponse {
    /// 构造执行成功响应（完成值渲染文本）。
    pub fn eval_ok(value: impl Into<String>) -> Self {
        ServerResponse::EvalResult {
            value: Some(value.into()),
            error: None,
        }
    }

    /// 构造执行失败响应（错误消息）。
    pub fn eval_err(message: impl Into<String>) -> Self {
        ServerResponse::EvalResult {
            value: None,
            error: Some(message.into()),
        }
    }
}

/// 请求帧可取的 `type` 值（须与 `ServerRequest` 变体保持同步）。
const REQUEST_TAGS: &[&str] = &["eval", "version", "status", "health", "info", "shutdown"];

/// 响应帧可取的 `type` 值（须与 `ServerResponse` 变体保持同步）。
const RESPONSE_TAGS: &[&str] = &["eval_result", "version", "status", "health", "info", "shutdown", "error"];

/// 序列化请求为帧字符串（含结尾换行）。
///
/// 请求结构字段皆为基础类型，序列化不会失败；`expect` 是安全网而非错误路径。
pub fn encode_request(req: &ServerRequest) -> String {
    let mut frame = serde_json::to_string(req).expect("请求结构序列化不会失败");
    frame.push('\n');
    frame
}

/// 序列化响应为帧字符串（含结尾换行）。
///
/// 响应结构字段皆为基础类型，序列化不会失败；`expect` 是安全网而非错误路径。
pub fn encode_response(resp: &ServerResponse) -> String {
    let mut frame = serde_json::to_string(resp).expect("响应结构序列化不会失败");
    frame.push('\n');
    frame
}

/// 解析单行请求帧（结尾换行可有可无）。
///
/// # 步骤
/// 1. 去首尾空白；空帧判 `EmptyFrame`，超长判 `FrameTooLarge`。
/// 2. 解析为 JSON；语法错误判 `InvalidJson`。
/// 3. 读取 `type` 字段；缺失或未知判 `UnknownType`（附原始取值）。
/// 4. 反序列化为 `ServerRequest`；载荷不匹配判 `MalformedPayload`。
///
/// # 边界与前提
/// - 输入是单帧（一行）；分行由读侧负责（见 `read_frame`）。
/// - 未知类型保留原始 `type` 取值，供 server 答复 `Error` 帧时引用。
pub fn parse_request(line: &str) -> Result<ServerRequest, ProtocolError> {
    parse_frame(line, REQUEST_TAGS)
}

/// 解析单行响应帧（结尾换行可有可无）。
///
/// 步骤与判定同 `parse_request`，目标结构为 `ServerResponse`。
pub fn parse_response(line: &str) -> Result<ServerResponse, ProtocolError> {
    parse_frame(line, RESPONSE_TAGS)
}

fn parse_frame<T: DeserializeOwned>(line: &str, known_tags: &[&str]) -> Result<T, ProtocolError> {
    // 去首尾空白：帧自带的换行与行尾空白不参与解析。
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(ProtocolError::EmptyFrame);
    }
    if trimmed.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }

    // 先解析为通用 Value，以便先检查 `type` 字段再定型。
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| ProtocolError::InvalidJson(e.to_string()))?;

    // 读取 `type` 字段：缺失或非字符串取值原样上报。
    let tag = match value.get("type") {
        None => "<missing>".to_string(),
        Some(t) => t.as_str().map(str::to_string).unwrap_or_else(|| t.to_string()),
    };
    if !known_tags.contains(&tag.as_str()) {
        return Err(ProtocolError::UnknownType(tag));
    }

    // 类型已知：反序列化为目标结构，载荷不匹配判 `MalformedPayload`。
    serde_json::from_value(value).map_err(|e| ProtocolError::MalformedPayload(e.to_string()))
}

/// 帧读取器：从行流读取 NDJSON 帧，跳过空行。
///
/// 流可能一次交还超过一行的数据（带缓冲的读侧），读取器内部持有余量缓冲，
/// 调用方不得绕过读取器直接读底层流。
pub struct FrameReader<R: BufRead> {
    inner: R,
    /// 上次读取后尚未消费成帧的余量数据。
    pending: Vec<u8>,
}

impl<R: BufRead> FrameReader<R> {
    /// 以底层流构造读取器。
    pub fn new(inner: R) -> Self {
        FrameReader { inner, pending: Vec::new() }
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
    pub fn read_frame(&mut self) -> std::io::Result<Option<String>> {
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
            let mut chunk = [0u8; 4096];
            let n = self.inner.read(&mut chunk)?;
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

    /// 请求帧 round-trip：编码后解析应得回原结构。
    fn roundtrip_request(req: &ServerRequest) {
        let frame = encode_request(req);
        assert!(frame.ends_with('\n'), "帧必须以换行终止");
        let parsed = parse_request(&frame).expect("请求帧 round-trip 应成功");
        assert_eq!(parsed, *req);
    }

    /// 响应帧 round-trip：编码后解析应得回原结构。
    fn roundtrip_response(resp: &ServerResponse) {
        let frame = encode_response(resp);
        assert!(frame.ends_with('\n'), "帧必须以换行终止");
        let parsed = parse_response(&frame).expect("响应帧 round-trip 应成功");
        assert_eq!(parsed, *resp);
    }

    /// eval 请求 round-trip：带与不带 max_steps 两个形态。
    #[test]
    fn eval_request_roundtrip() {
        roundtrip_request(&ServerRequest::Eval {
            code: "1 + 1".into(),
            max_steps: None,
        });
        roundtrip_request(&ServerRequest::Eval {
            code: "x".into(),
            max_steps: Some(1000),
        });
    }

    /// 五类控制请求 round-trip。
    #[test]
    fn control_request_roundtrip() {
        for req in [
            ServerRequest::Version,
            ServerRequest::Status,
            ServerRequest::Health,
            ServerRequest::Info,
            ServerRequest::Shutdown,
        ] {
            roundtrip_request(&req);
        }
    }

    /// 各响应变体 round-trip，含 eval_ok / eval_err 构造器。
    #[test]
    fn response_roundtrip() {
        for resp in [
            ServerResponse::eval_ok("42"),
            ServerResponse::eval_err("runtime error"),
            ServerResponse::Version { version: "0.8.23".into() },
            ServerResponse::Status {
                pool_available: 2,
                pool_total: 4,
                uptime_ms: 1500,
            },
            ServerResponse::Health { healthy: true },
            ServerResponse::Info {
                version: "0.8.23".into(),
                socket_path: "/tmp/oxide-server.sock".into(),
                pid: 1234,
            },
            ServerResponse::Shutdown,
            ServerResponse::Error { message: "boom".into() },
        ] {
            roundtrip_response(&resp);
        }
    }

    /// 序列化形态：eval 请求省略空 max_steps，控制请求无载荷字段。
    #[test]
    fn request_serialization_shape() {
        assert_eq!(
            serde_json::to_string(&ServerRequest::Eval {
                code: "1".into(),
                max_steps: None
            })
            .unwrap(),
            r#"{"type":"eval","code":"1"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerRequest::Eval {
                code: "1".into(),
                max_steps: Some(7)
            })
            .unwrap(),
            r#"{"type":"eval","code":"1","max_steps":7}"#
        );
        assert_eq!(serde_json::to_string(&ServerRequest::Health).unwrap(), r#"{"type":"health"}"#);
    }

    /// 序列化形态：eval_result 省略 None 字段。
    #[test]
    fn response_serialization_shape() {
        assert_eq!(
            serde_json::to_string(&ServerResponse::eval_ok("42")).unwrap(),
            r#"{"type":"eval_result","value":"42"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerResponse::eval_err("boom")).unwrap(),
            r#"{"type":"eval_result","error":"boom"}"#
        );
    }

    /// 非法 JSON：判 `InvalidJson`，不 panic。
    #[test]
    fn malformed_json() {
        let err = parse_request("not json at all").unwrap_err();
        assert!(matches!(err, ProtocolError::InvalidJson(_)), "应判 InvalidJson：{err}");
    }

    /// 未知 `type`：判 `UnknownType` 且保留原始取值。
    #[test]
    fn unknown_type() {
        let err = parse_request(r#"{"type":"bogus"}"#).unwrap_err();
        assert_eq!(err, ProtocolError::UnknownType("bogus".into()));
        let err = parse_response(r#"{"type":"bogus"}"#).unwrap_err();
        assert_eq!(err, ProtocolError::UnknownType("bogus".into()));
    }

    /// 缺失 `type`：判 `UnknownType` 且取值为 `<missing>`。
    #[test]
    fn missing_type() {
        let err = parse_request(r#"{"code":"x"}"#).unwrap_err();
        assert_eq!(err, ProtocolError::UnknownType("<missing>".into()));
    }

    /// 载荷不匹配：`type` 已知但字段缺失或类型错，判 `MalformedPayload`。
    #[test]
    fn malformed_payload() {
        let err = parse_request(r#"{"type":"eval"}"#).unwrap_err();
        assert!(matches!(err, ProtocolError::MalformedPayload(_)), "应判 MalformedPayload：{err}");
        let err = parse_request(r#"{"type":"eval","code":123}"#).unwrap_err();
        assert!(matches!(err, ProtocolError::MalformedPayload(_)), "应判 MalformedPayload：{err}");
    }

    /// 空帧（空串与纯空白）：判 `EmptyFrame`。
    #[test]
    fn empty_frame() {
        assert_eq!(parse_request("").unwrap_err(), ProtocolError::EmptyFrame);
        assert_eq!(parse_request("   \t ").unwrap_err(), ProtocolError::EmptyFrame);
    }

    /// 超长帧：判 `FrameTooLarge`。
    #[test]
    fn oversized_frame() {
        let line = format!("{{\"type\":\"eval\",\"code\":\"{}\"}}", "a".repeat(MAX_FRAME_BYTES));
        assert_eq!(parse_request(&line).unwrap_err(), ProtocolError::FrameTooLarge);
    }

    /// 协议错误转 `Error` 响应帧，round-trip 后消息保留。
    #[test]
    fn error_frame_roundtrip() {
        let err = ProtocolError::UnknownType("bogus".into());
        let frame = encode_response(&err.to_error_frame());
        let parsed = parse_response(&frame).expect("错误帧 round-trip 应成功");
        match parsed {
            ServerResponse::Error { message } => assert_eq!(message, err.to_string()),
            other => panic!("应得 Error 帧，实得 {other:?}"),
        }
    }

    /// 帧读取：跳过空行，返回非空帧（含结尾换行）；流一次交还多行时不丢数据。
    #[test]
    fn read_frame_skips_blank_lines() {
        let mut reader = FrameReader::new(Cursor::new(b"\n   \n{\"type\":\"health\"}\n".to_vec()));
        let frame = reader.read_frame().expect("读帧应成功").expect("应读到帧");
        assert_eq!(frame, "{\"type\":\"health\"}\n");
        assert_eq!(reader.read_frame().expect("EOF 应得 None"), None);
    }

    /// 帧读取：一次读取交还多帧时逐帧交付，余量不丢。
    #[test]
    fn read_frame_multiple_frames_in_one_read() {
        let mut reader = FrameReader::new(Cursor::new(b"{\"type\":\"health\"}\n{\"type\":\"version\"}\n".to_vec()));
        assert_eq!(reader.read_frame().expect("读帧应成功").expect("应读到帧"), "{\"type\":\"health\"}\n");
        assert_eq!(reader.read_frame().expect("读帧应成功").expect("应读到帧"), "{\"type\":\"version\"}\n");
        assert_eq!(reader.read_frame().expect("EOF 应得 None"), None);
    }

    /// 帧读取：EOF 前无换行的残行也作为一帧交付。
    #[test]
    fn read_frame_unterminated_line_at_eof() {
        let mut reader = FrameReader::new(Cursor::new(b"{\"type\":\"health\"}".to_vec()));
        let frame = reader.read_frame().expect("读帧应成功").expect("应读到帧");
        assert_eq!(frame, "{\"type\":\"health\"}");
    }

    /// 帧读取：超长行提前停止，返回长度超上限的残行，解析判 `FrameTooLarge`。
    #[test]
    fn read_frame_oversized_stops_early() {
        let payload = vec![b'a'; MAX_FRAME_BYTES + 8192];
        let mut reader = FrameReader::new(Cursor::new(payload));
        let frame = reader.read_frame().expect("读帧应成功").expect("应读到帧");
        assert!(frame.len() > MAX_FRAME_BYTES, "超长行应提前停止并返回残行");
        assert_eq!(parse_request(&frame).unwrap_err(), ProtocolError::FrameTooLarge);
    }

    /// 帧读取：非法 UTF-8 判 `InvalidData` io 错误，不 panic。
    #[test]
    fn read_frame_non_utf8() {
        let mut reader = FrameReader::new(Cursor::new(vec![0xff, 0xfe, b'\n']));
        let err = reader.read_frame().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
