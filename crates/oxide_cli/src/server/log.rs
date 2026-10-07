//! 日志读取：行级别解析、阈值加最后 N 行过滤、整文件读取、阻塞式跟踪。
//!
//! 日志文件由 server 进程写入（sidecar 同主名的 `.log` 文件），本模块只读。
//! 日志行格式固定为 `epoch秒.毫秒 级别 target: 消息`（tracing 紧凑格式加
//! 毫秒时间戳），级别是第二个空白分隔字段。

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use oxide_log::Level;

/// 跟踪循环轮询间隔：500 毫秒。
const FOLLOW_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 行级别解析：取第二个空白分隔字段，映射到 [`Level`]。
///
/// # 边界与前提
/// - `ERROR` / `WARN` / `INFO` / `DEBUG` / `TRACE` 映射到对应级别；其余
///   （含非日志行）返回 `None`。
pub fn line_level(line: &str) -> Option<Level> {
    let field = line.split_whitespace().nth(1)?;
    match field {
        "ERROR" => Some(Level::Error),
        "WARN" => Some(Level::Warn),
        "INFO" => Some(Level::Info),
        "DEBUG" => Some(Level::Debug),
        "TRACE" => Some(Level::Trace),
        _ => None,
    }
}

/// 级别阈值加最后 N 行过滤。
///
/// # 步骤
/// 1. 按级别阈值过滤：`None` 不过滤；过滤生效时保留不低于该级别（严重度
///    不低于）的行，行级别为 `None` 的行丢弃。
/// 2. 取最后 N 行：`None` 取全部，`Some(0)` 得空集。
///
/// # 边界与前提
/// - 严重度次序为 `Error` > `Warn` > `Info` > `Debug` > `Trace`（与级别
///   枚举值次序相反）：阈值 `error` 只留 `ERROR` 行，阈值 `info` 留
///   `ERROR`/`WARN`/`INFO` 行。
pub fn filter_tail(lines: &[String], min_level: Option<Level>, last_n: Option<usize>) -> Vec<String> {
    let mut filtered: Vec<String> = match min_level {
        Some(min) => lines
            .iter()
            .filter(|line| line_level(line).is_some_and(|level| level <= min))
            .cloned()
            .collect(),
        None => lines.to_vec(),
    };
    match last_n {
        Some(n) if n < filtered.len() => filtered.split_off(filtered.len() - n),
        _ => filtered,
    }
}

/// 读整文件、按行切分、调 [`filter_tail`] 返回过滤后的行。
pub fn read_log(path: &Path, min_level: Option<Level>, last_n: Option<usize>) -> io::Result<Vec<String>> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    Ok(filter_tail(&lines, min_level, last_n))
}

/// 阻塞式跟踪到标准输出：CLI 入口。
///
/// 标准输入中断即进程默认行为退出，无需额外信号处理。
pub fn follow(path: &Path, min_level: Option<Level>) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    follow_to(path, min_level, &mut lock);
}

/// 阻塞式跟踪：从文件末尾起追加读新行，写入给定 writer。
///
/// # 步骤
/// 1. 打开文件并定位到末尾。
/// 2. 每 500 毫秒轮询一次文件长度；长度小于已读偏移时重置偏移为 0
///    （文件被截断，如人工清空或未来轮转）。
/// 3. 读到新字节后只写完整行（末尾无换行的残行留待下轮），按级别阈值
///    过滤后逐行写入。
///
/// # 边界与前提
/// - 单文件描述符、无轮询外副作用；进程退出即关闭，无泄漏。
///
/// # 副作用
/// - 向 writer 写行。
fn follow_to<W: Write>(path: &Path, min_level: Option<Level>, writer: &mut W) {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .expect("日志文件应存在（调用方已检查）");
    let mut offset = file.seek(SeekFrom::End(0)).expect("定位日志文件末尾应成功");
    let mut buf = [0u8; 8192];
    let mut pending = String::new();

    loop {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < offset {
            // 文件被截断（人工清空或轮转）：重置偏移从 0 重读。
            offset = 0;
            pending.clear();
            file.seek(SeekFrom::Start(0)).expect("定位日志文件开头应成功");
        }

        if len > offset {
            let mut to_read = len - offset;
            let mut chunk = Vec::new();
            while to_read > 0 {
                let n = file.read(&mut buf).expect("读日志文件应成功");
                if n == 0 {
                    break;
                }
                chunk.extend_from_slice(&buf[..n]);
                to_read -= n as u64;
            }
            offset += chunk.len() as u64;
            pending.push_str(&String::from_utf8_lossy(&chunk));

            // 只写完整行：末尾无换行的残行留待下轮。
            if let Some(pos) = pending.rfind('\n') {
                let complete: String = pending.drain(..=pos).collect();
                for line in complete.lines() {
                    if level_allowed(line, min_level) {
                        let _ = writeln!(writer, "{line}");
                    }
                }
            }
        }

        thread::sleep(FOLLOW_POLL_INTERVAL);
    }
}

/// 行是否通过级别阈值（`None` 不过滤）：保留严重度不低于该级别的行。
fn level_allowed(line: &str, min_level: Option<Level>) -> bool {
    match min_level {
        Some(min) => line_level(line).is_some_and(|level| level <= min),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// temp_dir 下的唯一临时文件（进程号加纳秒时间戳），与 sidecar 测试惯例一致。
    fn temp_file(name: &str) -> PathBuf {
        let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("oxide_log_test_{}_{}_{}", std::process::id(), name, ns))
    }

    /// 行级别解析：五级映射与非日志行返回 `None`。
    #[test]
    fn line_level_parses_five_levels_and_rejects_other() {
        assert_eq!(line_level("1700000000.123 INFO oxide::kernel: ready"), Some(Level::Info));
        assert_eq!(line_level("1700000000.123 ERROR oxide::vm: boom"), Some(Level::Error));
        assert_eq!(line_level("1700000000.123 WARN oxide::ic: miss"), Some(Level::Warn));
        assert_eq!(line_level("1700000000.123 DEBUG oxide::vm: dbg"), Some(Level::Debug));
        assert_eq!(line_level("1700000000.123 TRACE oxide::vm: trace"), Some(Level::Trace));
        assert_eq!(line_level("not a log line at all"), None);
        assert_eq!(line_level(""), None);
    }

    /// filter_tail 阈值语义（严重度不低于该级别的行：error 只留 ERROR、
    /// info 留 INFO 与更严重、debug 留 DEBUG 与更严重、trace 全留、None
    /// 不过滤）与最后 N 行语义（N 大于行数取全部、N 为 0 空集）。
    #[test]
    fn filter_tail_threshold_and_tail() {
        let lines = vec![
            "1700000000.001 INFO a: one".into(),
            "1700000000.002 ERROR a: two".into(),
            "1700000000.003 DEBUG a: three".into(),
            "1700000000.004 WARN a: four".into(),
        ];
        // error 只留 ERROR 行。
        assert_eq!(filter_tail(&lines, Some(Level::Error), None), vec!["1700000000.002 ERROR a: two"]);
        // info 留 ERROR/WARN/INFO 行，丢 DEBUG 行。
        assert_eq!(
            filter_tail(&lines, Some(Level::Info), None),
            vec!["1700000000.001 INFO a: one", "1700000000.002 ERROR a: two", "1700000000.004 WARN a: four",]
        );
        // debug 留 DEBUG 与更严重行（全留）；trace 全留。
        assert_eq!(filter_tail(&lines, Some(Level::Debug), None), lines);
        assert_eq!(filter_tail(&lines, Some(Level::Trace), None), lines);
        assert_eq!(filter_tail(&lines, None, None), lines);
        assert_eq!(
            filter_tail(&lines, None, Some(2)),
            vec!["1700000000.003 DEBUG a: three", "1700000000.004 WARN a: four"]
        );
        assert_eq!(filter_tail(&lines, None, Some(10)), lines);
        assert_eq!(filter_tail(&lines, None, Some(0)), Vec::<String>::new());
    }

    /// read_log 读临时文件并按阈值过滤（info 丢 DEBUG 行）。
    #[test]
    fn read_log_reads_and_filters() {
        let path = temp_file("read");
        fs::write(&path, "1700000000.001 INFO a: one\n1700000000.002 DEBUG a: two\n").expect("写临时文件应成功");
        let lines = read_log(&path, Some(Level::Info), None).expect("读日志应成功");
        assert_eq!(lines, vec!["1700000000.001 INFO a: one"]);
        let _ = fs::remove_file(&path);
    }

    /// 测试 writer：把每行写入转发到通道。
    struct LineSender(std::sync::mpsc::Sender<String>);

    impl Write for LineSender {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            // follow_to 每次写入一行完整日志（含换行）。
            let text = String::from_utf8_lossy(buf);
            for line in text.lines() {
                self.0.send(line.to_string()).expect("通道不应关闭");
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// follow 跟踪：先写一行，起跟踪线程，再追加一行，经通道断言收到该行
    /// （500 毫秒轮询窗口为上限，2 秒接收窗口避免超时抖动）。
    #[test]
    fn follow_receives_appended_line() {
        let path = temp_file("follow");
        fs::write(&path, "1700000000.001 INFO a: first\n").expect("写临时文件应成功");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let path_clone = path.clone();
        thread::spawn(move || {
            let mut sink = LineSender(tx);
            follow_to(&path_clone, None, &mut sink);
        });

        // 给跟踪线程时间打开文件并定位末尾。
        thread::sleep(Duration::from_millis(200));
        let mut file = OpenOptions::new().append(true).open(&path).expect("追加打开应成功");
        file.write_all(b"1700000000.002 INFO a: second\n").expect("追加应成功");

        let line = rx.recv_timeout(Duration::from_millis(2000)).expect("2 秒内应收到追加行");
        assert_eq!(line, "1700000000.002 INFO a: second");
        let _ = fs::remove_file(&path);
    }
}
